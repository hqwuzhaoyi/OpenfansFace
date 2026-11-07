import { nativeArchiveEnabled, nativeArchive } from '@/lib/archive-native-backend'
import fs from 'node:fs'
import path from 'node:path'
import { createHash, randomUUID } from 'node:crypto'
import { NextRequest, NextResponse } from 'next/server'
import { acquireReviewLock, queuedWithOrigin, loadArchive, saveArchive } from '@/lib/review-store'
import { buildPlan } from '@/lib/review-plan'
import { saveQueue, writeJson } from '@/lib/jsonio'
import { validateVids, ReviewError, type ArchivedItem } from '@/lib/review-logic'
import { BASE, MANIFESTS_D, STAGE } from '@/lib/env'
import { identicalFiles, moveExclusiveAsync } from '@/lib/archive-files'
import { requireMovePermissions } from '@/lib/file-permissions'
import { toWin } from '@/lib/paths'
import { archiveLedger, type ArchiveLedgerRow } from '@/lib/archive-ledger'
import { invalidateIntegrationCache } from '@/lib/integration'
import { startArchiveJob } from '@/lib/runtime-jobs'
import { readArchiveRun, saveArchiveRun, validArchiveId } from '@/lib/archive-run'
import type { ArchiveRun, ArchiveResult, ArchiveFailure } from '@/lib/archive-run-model'

export async function POST(req: NextRequest) {
  try {if(nativeArchiveEnabled())return NextResponse.json(await nativeArchive('archive-apply',await req.json()),{headers:{'Cache-Control':'no-store'}})}
  catch(error:any){return NextResponse.json({error:error.message,code:error.code,path:error.path,step:error.step},{status:error.status||500})}
  let release: ReturnType<typeof acquireReviewLock> | undefined, keepLock=false
  let progress: ReturnType<typeof startArchiveJob> | undefined, run:ArchiveRun|undefined
  let ledgerState='reserved', mf='', operationId=''
  const moves:{src:string;dst:string;restored?:boolean}[]=[],errors:string[]=[],failures:ArchiveFailure[]=[]
  const completed:ArchivedItem[]=[],ledger:ArchiveLedgerRow[]=[]
  const manifest:any={at:new Date().toISOString(),status:'checking',moves,ledger,errors}
  const finish=(result:ArchiveResult)=>{
    if(run){run.result=result;progress?.finish(result.errors.join('；')||undefined);saveArchiveRun(run)}
    invalidateIntegrationCache()
    return NextResponse.json(result)
  }
  const recover=async()=>{
    progress?.update({stage:'恢复文件',current:undefined,done:0,total:moves.length,unit:'个文件'})
    for(const [index,mv] of [...moves].reverse().entries()){
      try{await moveExclusiveAsync(mv.dst,mv.src);mv.restored=true}
      catch(e){errors.push(`恢复失败 ${path.basename(mv.src)}：${(e as Error).message}`);keepLock=true}
      progress?.update({done:index+1})
      writeJson(mf,manifest)
    }
    manifest.status=keepLock?'recovery_needed':'rolled_back'
  }
  try {
    const body=await req.json(),vids=validateVids(body.vids)
    operationId=body.operation_id??randomUUID()
    if(!validArchiveId(operationId))throw new ReviewError('归档操作编号无效')
    const fingerprint=createHash('sha256').update(JSON.stringify({vids:[...vids].sort(),version:body.version})).digest('hex')
    const replay=()=>{
      const previous=readArchiveRun(operationId)
      if(!previous)return null
      if(previous.fingerprint!==fingerprint)throw new ReviewError('操作编号对应的归档范围已变化',409)
      if(previous.result)return NextResponse.json(previous.result)
      throw new ReviewError('本次归档仍在执行或结果待核对，请查看进度，勿重复提交',409)
    }
    const previous=replay();if(previous)return previous
    release=acquireReviewLock()
    const lockedPrevious=replay();if(lockedPrevious)return lockedPrevious
    progress=startArchiveJob(vids.length,operationId,job=>{run={fingerprint,job,...(run?.result?{result:run.result}:{})};saveArchiveRun(run)})
    const plan=buildPlan()
    if(body.version!==plan.version)throw new ReviewError('待归档内容或文件状态已变化，请刷新预览后再执行',409)
    const ids=new Set(vids),selected=plan.rows.filter(r=>ids.has(r.vid))
    if(selected.length!==vids.length||selected.some(r=>r.status!=='ok'))throw new ReviewError('只能归档检查通过的视频，请刷新并处理异常项',409)
    fs.mkdirSync(MANIFESTS_D,{recursive:true});mf=path.join(MANIFESTS_D,`apply-${Date.now()}-${operationId}.json`)
    manifest.operation_id=operationId
    const stageBatch=path.join(STAGE,'archive-duplicates',operationId)
    const planned=selected.map(item=>{
      const actualDst=item.operation==='stage-duplicate'?path.join(stageBatch,randomUUID(),path.basename(item.src!)):item.dst!
      requireMovePermissions(item.src!,actualDst)
      return {item,actualDst,entry:{from:toWin(item.src!),to:toWin(actualDst),creator:item.creator,...(item.operation==='stage-duplicate'?{canonical:toWin(item.dst!)}:{})}}
    })
    writeJson(mf,manifest)
    progress.update({stage:'检查 Peach 账本',done:null,total:null,unit:''})
    await archiveLedger('reserve',operationId,planned.map(p=>p.entry))
    manifest.status='moving';writeJson(mf,manifest)
    for(const [index,{item,actualDst,entry}] of planned.entries()){
      progress.update({stage:item.operation==='stage-duplicate'?'校验重复文件':'移动文件',current:item.name,vid:item.vid,done:index,total:selected.length,unit:'个文件'})
      await new Promise<void>(resolve=>setImmediate(resolve))
      try{
        const src=item.src!,dst=item.dst!,source=fs.statSync(src)
        if(!source.isFile()||source.size!==item.size||source.mtimeMs!==item.mtime)throw new Error('原文件已变化，请重新核对')
        if(item.operation==='stage-duplicate'){
          const target=fs.statSync(dst)
          if(!target.isFile()||target.size!==item.dst_size||target.mtimeMs!==item.dst_mtime)throw new Error('目标文件已变化，请重新核对')
          if(!await identicalFiles(src,dst))throw new Error('完整内容校验不一致，源文件未移动。请改选“保留两份”')
          const checkedSource=fs.statSync(src),checkedTarget=fs.statSync(dst)
          if(checkedSource.size!==item.size||checkedSource.mtimeMs!==item.mtime||checkedTarget.size!==item.dst_size||checkedTarget.mtimeMs!==item.dst_mtime)throw new Error('校验后文件状态已变化，请重新核对')
        }else if(fs.existsSync(dst))throw new Error('目标文件已出现，请核对冲突')
        await moveExclusiveAsync(src,actualDst)
        moves.push({src,dst:actualDst})
        ledger.push(entry)
        completed.push({...item,dst,win_dst:toWin(dst),manifest:mf,...(item.operation==='stage-duplicate'?{staged_path:actualDst}:{})})
      }catch(e){const message=(e as Error).message;errors.push(`${item.name}：${message}`);failures.push({vid:item.vid,name:item.name,message})}
      writeJson(mf,manifest)
      progress.update({done:index+1,failed:failures.length})
    }
    if(completed.length){
      progress.update({stage:'同步 Peach 账本',current:undefined,vid:undefined,done:null,total:null,unit:''})
      manifest.status='updating_ledger';writeJson(mf,manifest)
      ledgerState='unknown'
      try{
        const result=await archiveLedger('apply',operationId,ledger)
        if(result.state!=='committed'||result.updated!==completed.length)throw new Error('账本回执与文件数量不一致')
        ledgerState='committed'
      }catch(e){
        manifest.ledger_error=(e as Error).message
        try{const result=await archiveLedger('status',operationId);ledgerState=result.state==='committed'&&result.updated===completed.length?'committed':result.state==='cancelled'?'cancelled':'unknown'}catch{ledgerState='unknown'}
        if(ledgerState!=='committed')throw new Error(`Peach 账本同步失败：${manifest.ledger_error}`)
      }
      // A committed ledger must never be rolled back merely because the HTTP reply was lost.
      manifest.status='committing';manifest.ledger_remapped=completed.length;writeJson(mf,manifest)
      const history=new Map(loadArchive().map(v=>[v.vid,v]));completed.forEach(v=>history.set(v.vid,v));saveArchive([...history.values()])
      const succeeded=new Set(completed.map(v=>v.vid));saveQueue(queuedWithOrigin().filter(v=>!succeeded.has(v.vid)))
    }else{await archiveLedger('status',operationId);ledgerState='cancelled'}
    manifest.status=errors.length?'partial':'complete';writeJson(mf,manifest)
    return finish({ok:!errors.length,moved:completed.length,staged:completed.filter(v=>v.staged_path).length,ledger:completed.length,errors,failures,manifest:mf,remaining:queuedWithOrigin().length})
  }catch(e){
    errors.push((e as Error).message)
    if(moves.length){
      if(ledgerState==='committed'||ledgerState==='unknown'){
        keepLock=true;manifest.status='recovery_needed';errors.push('文件与账本结果需要核对，已暂停写操作，请勿重复归档')
      }else{try{await recover()}catch(err){keepLock=true;manifest.status='recovery_needed';errors.push(`恢复记录失败：${(err as Error).message}`)}}
    }else{manifest.status='failed'}
    if(mf)writeJson(mf,manifest)
    if(run){
      const response=finish({ok:false,moved:0,remaining:queuedWithOrigin().length,errors,failures,manifest:mf||undefined,recovery_needed:keepLock,rolled_back:moves.length>0&&!keepLock})
      return e instanceof ReviewError?NextResponse.json({...run.result,error:e.message},{status:e.status||400}):response
    }
    return NextResponse.json({error:(e as Error).message},{status:(e as ReviewError).status||500})
  }finally{
    if(keepLock)writeJson(path.join(BASE,'.review-write.lock'),{pid:process.pid,at:new Date().toISOString(),operation:operationId,status:'recovery_needed',manifest:mf})
    release?.(keepLock)
  }
}
