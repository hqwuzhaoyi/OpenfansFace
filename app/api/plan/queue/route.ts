import { nativeArchiveEnabled, nativeArchive } from '@/lib/archive-native-backend'
import { matchesReviewSelection } from '@/lib/processing-model'
import { rememberPerson } from '@/lib/people-catalog'
import { canonicalPersonName } from '@/lib/person-names'
import { isStagedPath } from '@/lib/person-file-paths'
import fs from 'fs'
import { processingState, watermarkRuleChanged, restoreReviewTasks } from '@/lib/processing'
import { loadWatermarkMappings, watermarkFile, WATERMARK_DIR } from '@/lib/watermarks'
import { writeJson } from '@/lib/jsonio'
import { healthScope } from '@/lib/video-health'
import { creatorFolder } from '@/lib/source-resolution'
import { MEDIA2 } from '@/lib/env'
import { readVideoSources, videoSource, isMediaFile } from '@/lib/video-sources'
import { NextRequest, NextResponse } from 'next/server'
import { saveQueue, saveDecisions, loadQueue } from '@/lib/jsonio'
import { acquireReviewLock, queuedWithOrigin, loadDecisions, loadArchive, videoInfo } from '@/lib/review-store'
import { enqueueItems, undoItems, origin, validateCreator, validateVids, ReviewError, type ReviewSource } from '@/lib/review-logic'

export async function POST(req: NextRequest) {
  try {if(nativeArchiveEnabled())return NextResponse.json(await nativeArchive('queue-confirm',await req.json()),{headers:{'Cache-Control':'no-store'}})}
  catch(error:any){return NextResponse.json({error:error.message,code:error.code,path:error.path,step:error.step},{status:error.status||500})}
  let release: (() => void) | undefined
  try {
    const body = await req.json()
    const creator = canonicalPersonName(validateCreator(body.creator)), vids = validateVids(body.vids)
    const source: ReviewSource = ['cluster', 'person', 'approval', 'all', 'watermark', 'review'].includes(body.source) ? body.source : 'legacy'
    const provenance = origin(source, String(body.source_id || ''))
    const sources=readVideoSources()
    const hidden=healthScope()
    if(vids.some(v=>{const p=videoSource(v,sources);return !!p&&isStagedPath(p)}))throw new ReviewError('暂存区中的视频不能重新归名',409)
    if(vids.some(v=>{const p=videoSource(v,sources);return !!p&&hidden(v,p)}))throw new ReviewError('视频体检已连续确认异常，请到回收站查看记录',409)
    if(vids.some(v=>{const p=videoSource(v,sources);return !!p&&creatorFolder(p,MEDIA2)!==null}))throw new ReviewError('creators 内的视频已归类，不再进入待归名；请刷新列表',409)
    if(vids.some(v=>{const p=videoSource(v,sources);return !p||!isMediaFile(p)}))throw new ReviewError('选中视频的源文件无法读取，请刷新列表并核对路径',409)
    release = acquireReviewLock()
    const reviewTasks = source === 'review' ? new Map(processingState().tasks.map(t => [t.vid,t])) : new Map()
    let ruleCandidate: any
    if (source === 'review') {
      for (const vid of vids) {
        const task=reviewTasks.get(vid), expected=body.fingerprints?.[vid], file=videoSource(vid,sources)
        if(!task || (task.stage!=='review'&&!task.manual) || !expected || !file)throw new ReviewError('视频尚未就绪或状态已变化，请刷新',409)
        if(body.review_group && (typeof body.review_group.key!=='string'||typeof body.review_group.deferred!=='boolean'||!matchesReviewSelection(task,body.review_group)))throw new ReviewError('所选视频的确认组或暂缓状态已变化，请重新核对',409)
        const stat=fs.statSync(file,{bigint:true})
        if(expected.path!==file||expected.size!==Number(stat.size)||expected.mtime_ns!==stat.mtimeNs.toString()||task.path!==file||task.size!==Number(stat.size)||task.mtime_ns!==stat.mtimeNs.toString())throw new ReviewError('视频文件已变化，请重新分析后核对',409)
        if(body.watermark_key){
          const mark=task.data.watermark?.candidates.find((c:any)=>c.key===body.watermark_key)
          if(!mark||mark.kind!=='account'||mark.platform==='Telegram')throw new ReviewError('选中视频没有共同的作者账号证据',409)
          ruleCandidate=mark
        }
      }
    }
    const archived = new Set(loadArchive().map(v => v.vid))
    if (vids.some(v => archived.has(v))) throw new ReviewError('部分视频已归档，请刷新列表', 409)
    const current=loadQueue(),selectedPaths=new Set<string>()
    const pendingPaths=new Map(current.map(v=>[videoSource(v.vid,sources),v.vid]))
    for(const vid of vids){const p=videoSource(vid,sources)!;if(selectedPaths.has(p)||(pendingPaths.has(p)&&pendingPaths.get(p)!==vid))throw new ReviewError('同一个源文件已在本次选择或待归档中，请刷新列表',409);selectedPaths.add(p)}
    const result = enqueueItems(current, creator, vids, provenance)
    for(const item of result.added)if(reviewTasks.get(item.vid)?.note)item.note=reviewTasks.get(item.vid)!.note
    rememberPerson(creator,videoInfo().get(vids[0])?.thumb)
    saveQueue(result.queue)
    let rule_warning: string | undefined
    if(ruleCandidate){
      try{
        fs.mkdirSync(WATERMARK_DIR,{recursive:true})
        const mappings=loadWatermarkMappings(),c=ruleCandidate
        mappings[c.key]={key:c.key,text:c.text,token:c.token,platform:c.platform,kind:'person',creator,confirmed_at:new Date().toISOString()}
        writeJson(watermarkFile('mappings.json'),mappings)
        watermarkRuleChanged(c.key)
      }catch{rule_warning='归名已保存，账号规则保存失败，请稍后重新关联'}
    }
    return NextResponse.json({ ok: true, added: result.added.length, queue_len: result.queue.length, vids, rule_warning })
  } catch (e: any) { return NextResponse.json({ error: e.message }, { status: e.status || 500 }) }
  finally { release?.() }
}
export async function DELETE(req: NextRequest) {
  try {if(nativeArchiveEnabled())return NextResponse.json(await nativeArchive('queue-undo',await req.json()),{headers:{'Cache-Control':'no-store'}})}
  catch(error:any){return NextResponse.json({error:error.message,code:error.code,path:error.path,step:error.step},{status:error.status||500})}
  let release: (() => void) | undefined
  try {
    const vids = validateVids((await req.json()).vids)
    release = acquireReviewLock()
    const archived = new Set(loadArchive().map(v => v.vid))
    if (vids.some(v => archived.has(v))) throw new ReviewError('部分视频已实际归档，请刷新后检查完成记录', 409)
    const withOrigin=queuedWithOrigin()
    const result = undoItems(loadQueue(), loadDecisions(), vids)
    saveDecisions(result.decisions)
    saveQueue(result.queue)
    let warning: string | undefined
    try{restoreReviewTasks(result.removed.map(v=>v.vid))}catch{warning='归名已撤销，分析状态将在自动发现时恢复'}
    const removedIds=new Set(result.removed.map(v=>v.vid))
    return NextResponse.json({ ok: true, removed: withOrigin.filter(v=>removedIds.has(v.vid)), queue_len: result.queue.length, warning })
  } catch (e: any) { return NextResponse.json({ error: e.message }, { status: e.status || 500 }) }
  finally { release?.() }
}
