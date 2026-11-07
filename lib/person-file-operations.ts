import fs from 'node:fs'
import path from 'node:path'
import { createHash, randomUUID } from 'node:crypto'
import Database from 'better-sqlite3'
import { BASE, CREATORS, MEDIA2, STAGE } from './env'
import { ReviewError, validateCreator } from './review-logic'
import { acquireReviewLock, loadArchive, videoInfo } from './review-store'
import { writeJson } from './jsonio'
import { readVideoSources, videoSource } from './video-sources'
import { sourceCandidates } from './source-resolution'
import { libraryPeople } from './person-library'
import { invalidateIntegrationCache } from './integration'
import { validateArchiveName, moveExclusiveAsync } from './archive-files'
import { canonicalPersonName, personRenames } from './person-names'
import { fingerprint, guardedPath, personDirectory, safeSubfolder } from './person-file-paths'
import { metadataFiles, remappedPath, transformPersonData, updatedMetadata } from './person-file-metadata'
import { personLedger } from './person-file-ledger'
import { mergedDirectoryPaths } from './person-merge-paths'
import { personVideoExtension } from './person-library-stats'
import { requireMovePermissions, requireWritableDirectory } from './file-permissions'
import type { PersonFileRequest, PersonFilePlan, PersonFileMove, PersonFileResult } from './person-file-model'

const operationsRoot=path.join(BASE,'person-operations')
const hash=(value:unknown)=>createHash('sha256').update(typeof value==='string'?value:JSON.stringify(value)).digest('hex')
const arrays=new Set(['claims-detail.json','clusters.json','integration-queue.json','archive-history.json','people-manual.json'])
function jsonSnapshot(){return metadataFiles.map(name=>{const file=path.join(BASE,name);let raw:string|null=null;try{raw=fs.readFileSync(/*turbopackIgnore: true*/ file,'utf8')}catch(error){if((error as NodeJS.ErrnoException).code!=='ENOENT')throw error}return {name,file,raw,value:raw===null?(arrays.has(name)?[]:{}):JSON.parse(raw)}})}
function openDatabase(){
  const db=new Database(path.join(BASE,'faces.db'),{fileMustExist:true});db.pragma('busy_timeout=15000');db.pragma('foreign_keys=ON')
  const jobs=path.join(BASE,'processing/processing.db')
  if(fs.existsSync(/*turbopackIgnore: true*/ jobs))db.prepare('ATTACH DATABASE ? AS jobs').run(jobs)
  return db
}
function jobRows(db:Database.Database){return (db.prepare('PRAGMA database_list').all() as any[]).some(r=>r.name==='jobs')?db.prepare('SELECT * FROM jobs.tasks ORDER BY vid').all() as any[]:[]}
function databaseState(db:Database.Database,plan:PersonFilePlan){
  const ids=new Set(plan.files.flatMap(f=>f.vids)),paths=new Set(plan.files.map(f=>f.src))
  const affected=(row:{vid:string;path:string})=>ids.has(row.vid)||paths.has(row.path)||remappedPath(row.path,plan)!==row.path
  const identityChange=['rename-person','merge-person','delete-person'].includes(plan.action)
  return {
    videos:(db.prepare('SELECT vid,path FROM videos ORDER BY vid').all() as {vid:string;path:string}[]).filter(affected),
    jobs:jobRows(db).filter(row=>{
      if(affected(row))return true
      if(!identityChange)return false
      // Bind only records this operation can modify. Background inference on
      // unrelated videos must not expire a person's confirmation every second.
      const data=JSON.parse(row.data)
      return JSON.stringify(transformPersonData(data,plan))!==JSON.stringify(data)
    }),
  }
}
function metadataVersion(plan:PersonFilePlan,db?:Database.Database){
  const own=!db;db=db||openDatabase()
  try{return hash({json:jsonSnapshot().map(file=>[file.name,file.raw]),database:databaseState(db,plan)})}finally{if(own)db.close()}
}
function validName(value:unknown){try{const name=validateCreator(value);validateArchiveName(name);if(name.startsWith('.'))throw new Error('名称不能以点开头');return name}catch(e){throw new ReviewError((e as Error).message)}}
const normalized=(value:string)=>value.normalize('NFC').toLowerCase()
function descendants(directory:string):{files:string[];directories:string[]} {
  const files:string[]=[],directories=[directory]
  for(const entry of fs.readdirSync(/*turbopackIgnore: true*/ directory,{withFileTypes:true})){
    const full=path.join(directory,entry.name)
    if(entry.isSymbolicLink())throw new ReviewError('人物目录含符号链接，请先核对后再整体改名',409)
    if(entry.isDirectory()){const nested=descendants(full);files.push(...nested.files);directories.push(...nested.directories)}
    else if(entry.isFile())files.push(full)
    else throw new ReviewError('人物目录含特殊文件，不能整体改名',409)
  }
  return {files:files.sort(),directories:directories.sort()}
}
export async function previewPersonFiles(body:PersonFileRequest):Promise<PersonFilePlan> {
  if(!body||typeof body!=='object')throw new ReviewError('缺少操作请求')
  if(!['rename-file','move-files','delete-files','rename-person','merge-person','delete-person'].includes(body.action))throw new ReviewError('请选择文件操作')
  const id=body.id||randomUUID()
  if(!/^[a-f0-9]{8}-[a-f0-9]{4}-4[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}$/i.test(id))throw new ReviewError('操作编号无效')
  const person=canonicalPersonName(validName(body.person)),sourceRoot=guardedPath(personDirectory(person),CREATORS,true)
  guardedPath(CREATORS,MEDIA2)
  const catalog=await libraryPeople()
  if(!catalog.some(p=>p.name===person))throw new ReviewError('找不到当前人物，请刷新',404)
  const sources=readVideoSources(),idsByPath=new Map<string,string[]>()
  for(const row of loadArchive())if(row.win_dst)sources.set(row.vid,row.win_dst)
  for(const vid of new Set([...sources.keys(),...videoInfo().keys()]))for(const candidate of sourceCandidates(vid,sources.get(vid),MEDIA2))idsByPath.set(candidate,[...(idsByPath.get(candidate)||[]),vid])
  const idsForPath=(file:string)=>[...new Set(idsByPath.get(file)||[])].filter(vid=>videoSource(vid,sources)===file)
  const trackedIdsForPath=(file:string)=>{const ids=idsForPath(file);if(!ids.length&&personVideoExtension.test(file))ids.push(`file:${hash([file,String(fs.statSync(/*turbopackIgnore: true*/ file,{bigint:true}).ino)])}`);return ids}
  const files:PersonFileMove[]=[],directories:string[]=[]
  let target=person,targetRoot:string|undefined
  let directoryTargets:{src:string;dst:string}[]|undefined
  const request:PersonFileRequest={id,action:body.action,person}
  if(body.action==='rename-person'){
    target=validName(body.new_name);request.new_name=target
    if(target===person)throw new ReviewError('新人物名与当前名字相同')
    const canonicalTarget=canonicalPersonName(target)
    if((canonicalTarget!==target&&canonicalTarget!==person)||catalog.some(p=>normalized(p.name)===normalized(target)))throw new ReviewError('该人物名已存在或属于其他人物，不能自动合并',409)
    targetRoot=guardedPath(personDirectory(target),CREATORS,true)
    if(fs.existsSync(/*turbopackIgnore: true*/ targetRoot))throw new ReviewError('目标人物目录已存在，不能覆盖',409)
    if(fs.existsSync(/*turbopackIgnore: true*/ sourceRoot)){
      const tree=descendants(sourceRoot);directories.push(...tree.directories)
      for(const src of tree.files)files.push({src,dst:path.join(targetRoot,path.relative(sourceRoot,src)),vids:trackedIdsForPath(src),size:fs.statSync(/*turbopackIgnore: true*/ src).size,version:fingerprint(src)})
    }
    const overrides=jsonSnapshot().find(s=>s.name==='template-overrides.json')!.value
    if(Object.hasOwn(overrides,target))throw new ReviewError('目标人物已有模板设置，不能覆盖',409)
  }else if(body.action==='delete-person'){
    targetRoot=guardedPath(path.join(STAGE,'person-deletions',id,person),MEDIA2,true)
    if(fs.existsSync(/*turbopackIgnore: true*/ sourceRoot)){
      const tree=descendants(sourceRoot);directories.push(...tree.directories)
      for(const src of tree.files)files.push({src,dst:path.join(targetRoot,path.relative(sourceRoot,src)),vids:trackedIdsForPath(src),size:fs.statSync(/*turbopackIgnore: true*/ src).size,version:fingerprint(src)})
    }
  }else if(body.action==='merge-person'){
    target=canonicalPersonName(validName(body.target_person));request.target_person=target
    if(normalized(target)===normalized(person))throw new ReviewError('来源和目标不能是同一个人物')
    if(!catalog.some(p=>p.name===target))throw new ReviewError('请选择已有的目标人物',404)
    targetRoot=guardedPath(personDirectory(target),CREATORS,true)
    if(fs.existsSync(/*turbopackIgnore: true*/ targetRoot)&&!fs.lstatSync(/*turbopackIgnore: true*/ targetRoot).isDirectory())throw new ReviewError('目标人物路径不是目录',409)
    const tree=mergedDirectoryPaths(sourceRoot,targetRoot);directoryTargets=tree.directories
    directories.push(...tree.directories.map(directory=>directory.src))
    for(const file of tree.files){guardedPath(file.dst,MEDIA2,true);files.push({...file,vids:trackedIdsForPath(file.src),size:fs.statSync(/*turbopackIgnore: true*/ file.src).size,version:fingerprint(file.src)})}
  }else{
    if(!Array.isArray(body.files)||!body.files.length||body.files.length>500)throw new ReviewError('请选择 1–500 个已有视频')
    if(body.action==='rename-file'&&body.files.length!==1)throw new ReviewError('重命名每次只处理一个文件')
    if(body.action==='move-files'){
      target=canonicalPersonName(validName(body.target_person));request.target_person=target
      if(!catalog.some(p=>p.name===target))throw new ReviewError('请选择已有的目标人物',404)
      request.target_folder=safeSubfolder(body.target_folder)
      targetRoot=guardedPath(path.join(/*turbopackIgnore: true*/ personDirectory(target),request.target_folder),CREATORS,true)
    }
    const seen=new Set<string>(),targets=new Set<string>()
    request.files=[]
    for(const item of body.files){
      if(!item||typeof item.path!=='string'||typeof item.version!=='string')throw new ReviewError('缺少文件版本，请刷新列表')
      const src=guardedPath(item.path,sourceRoot)
      if(src===sourceRoot||! /\.(mp4|m4v|mov|mkv|webm|avi|wmv|ts|mts|m2ts)$/i.test(src))throw new ReviewError('只允许操作人物目录中的视频')
      if(seen.has(src))throw new ReviewError('重复选择了同一个文件')
      seen.add(src)
      const version=fingerprint(src)
      if(version!==item.version)throw new ReviewError('文件已变化，请刷新后重新选择',409)
      let dst:string
      if(body.action==='rename-file'){
        const filename=validName(body.filename)
        if(path.extname(filename).toLowerCase()!==path.extname(src).toLowerCase())throw new ReviewError('重命名需保留原扩展名')
        request.filename=filename;dst=path.join(/*turbopackIgnore: true*/ path.dirname(src),filename)
      }else if(body.action==='move-files')dst=path.join(targetRoot!,path.basename(src))
      else dst=path.join(STAGE,'person-deletions',id,String(files.length),path.basename(src))
      guardedPath(dst,MEDIA2,true)
      if(src===dst)throw new ReviewError('源文件与目标路径相同')
      if(fs.existsSync(/*turbopackIgnore: true*/ dst)||targets.has(normalized(dst)))throw new ReviewError(`目标存在同名文件：${path.basename(dst)}，请先改名或选择其他位置`,409)
      targets.add(normalized(dst))
      const vids=idsForPath(src)
      // Unindexed files gain a stable synthetic ID when their path first changes.
      const synthetic=`file:${hash([src,String(fs.statSync(/*turbopackIgnore: true*/ src,{bigint:true}).ino)])}`
      if(!vids.length)vids.push(synthetic)
      request.files.push({vid:vids[0],path:src,version})
      files.push({src,dst,vids,size:fs.statSync(/*turbopackIgnore: true*/ src).size,version})
    }
  }
  for(const file of files)requireMovePermissions(file.src,file.dst)
  for(const directory of directories)requireWritableDirectory(path.dirname(directory),'源目录')
  if(targetRoot)requireWritableDirectory(targetRoot,'目标目录')
  for(const directory of directoryTargets||[])requireWritableDirectory(directory.dst,'目标目录')
  const plan:PersonFilePlan={id,request,version:'',action:body.action,person,target_person:target,files,total_bytes:files.reduce((sum,file)=>sum+file.size,0),directories,
    ...(['rename-person','merge-person','delete-person'].includes(body.action)?{source_directory:sourceRoot,target_directory:targetRoot}:{}),...(directoryTargets?{directory_targets:directoryTargets}:{}),ledger_version:'',metadata_version:''}
  if(['merge-person','delete-person'].includes(plan.action)){
    const names=personRenames()
    plan.source_aliases=Object.keys(names).filter(alias=>canonicalPersonName(alias,names)===person)
    plan.target_aliases=Object.keys(names).filter(alias=>canonicalPersonName(alias,names)===target)
    const metadata=new Map(jsonSnapshot().map(file=>[file.name,file.value])),overrides=metadata.get('template-overrides.json')
    const sourceNames=new Set([person,...plan.source_aliases])
    plan.delete_summary={pending_tasks:(metadata.get('integration-queue.json')||[]).filter((row:any)=>sourceNames.has(row.creator)).length,manual_accounts:(metadata.get('external-profiles/manual-links.json')?.links||[]).filter((row:any)=>sourceNames.has(row.creator)).length}
    if(plan.action==='merge-person')plan.merge_summary={renamed_files:files.filter(file=>file.renamed).length,pending_tasks:(metadata.get('integration-queue.json')||[]).filter((row:any)=>sourceNames.has(row.creator)).length,manual_accounts:(metadata.get('external-profiles/manual-links.json')?.links||[]).filter((row:any)=>sourceNames.has(row.creator)).length,keep_target_template:Object.hasOwn(overrides,person)&&Object.hasOwn(overrides,target)}
  }
  if(['move-files','delete-files'].includes(plan.action)){
    const db=openDatabase(),thumbs=new Set<string>()
    try{const query=db.prepare('SELECT thumb FROM faces WHERE vid=?');for(const file of files)for(const vid of file.vids)for(const row of query.all(vid) as {thumb?:string}[])if(row.thumb)thumbs.add(row.thumb)}finally{db.close()}
    plan.affected_thumbs=[...thumbs].sort()
  }
  plan.metadata_version=metadataVersion(plan)
  const ledger=await personLedger('preview',plan)
  plan.ledger_version=ledger.version!
  plan.version=hash({...plan,version:''})
  return plan
}

function mkdirTracked(directory:string,created:string[]){
  if(fs.existsSync(/*turbopackIgnore: true*/ directory)){if(!fs.lstatSync(/*turbopackIgnore: true*/ directory).isDirectory()||fs.lstatSync(/*turbopackIgnore: true*/ directory).isSymbolicLink())throw new ReviewError('目标父路径不是普通目录',409);return}
  mkdirTracked(path.dirname(directory),created);fs.mkdirSync(directory);created.push(directory)
}
function writeRaw(file:string,raw:string){const temporary=file+'.'+randomUUID()+'.tmp';try{fs.writeFileSync(temporary,raw);fs.renameSync(temporary,file)}finally{fs.rmSync(temporary,{force:true})}}
function applyDatabase(db:Database.Database,plan:PersonFilePlan){
  const columns=new Set((db.prepare('PRAGMA table_info(videos)').all() as any[]).map(r=>r.name))
  const sourceById=new Map(plan.files.flatMap(file=>file.vids.map(vid=>[vid,file] as const)))
  for(const row of db.prepare('SELECT vid,path FROM videos').all() as {vid:string;path:string}[]){
    const file=sourceById.get(row.vid),next=file?.dst||remappedPath(row.path,plan)
    if(next===row.path)continue
    const fields:Record<string,unknown>={path:next}
    if(columns.has('name'))fields.name=path.basename(next)
    if(columns.has('collection'))fields.collection=path.dirname(next)
    db.prepare(`UPDATE videos SET ${Object.keys(fields).map(k=>`${k}=?`).join(',')} WHERE vid=?`).run(...Object.values(fields),row.vid)
  }
  for(const task of jobRows(db)){
    const file=sourceById.get(task.vid)||plan.files.find(file=>file.src===task.path)
    if(!file&&!['rename-person','merge-person','delete-person'].includes(plan.action))continue
    const previousData=JSON.parse(task.data),data=JSON.stringify(transformPersonData(previousData,plan)),next=file?.dst||remappedPath(task.path,plan)
    if(!file&&data===JSON.stringify(previousData)&&next===task.path)continue
    const active=['text','face'].includes(task.stage),stage=file?'excluded':active||plan.action==='delete-person'?'queued':task.stage
    const stat=file?fs.statSync(/*turbopackIgnore: true*/ next,{bigint:true}):null
    db.prepare('UPDATE jobs.tasks SET path=?,name=?,size=?,mtime_ns=?,data=?,stage=?,updated_at=? WHERE vid=?').run(next,path.basename(next),stat?Number(stat.size):task.size,stat?stat.mtimeNs.toString():task.mtime_ns,data,stage,Date.now()/1000,task.vid)
  }
  if(plan.action==='merge-person'&&(db.prepare('PRAGMA database_list').all() as any[]).some(row=>row.name==='jobs'))db.prepare("INSERT INTO jobs.settings(key,value) VALUES('discovery_requested',?) ON CONFLICT(key) DO UPDATE SET value=excluded.value").run(JSON.stringify(Date.now()/1000))
}

export async function applyPersonFiles(request:PersonFileRequest,version:string,lockHeld=false):Promise<PersonFileResult> {
  if(!request||!request.id||!/^[a-f0-9-]{36}$/i.test(request.id)||typeof version!=='string')throw new ReviewError('请先预览操作')
  const journal=path.join(operationsRoot,request.id,'operation.json')
  const release=lockHeld?()=>{}:acquireReviewLock();let keepLock=false,db:Database.Database|undefined
  let moved:{file:PersonFileMove;after:string}[]=[],created:string[]=[],written:{file:string;before:string|null;after:string}[]=[],plan:PersonFilePlan|undefined,ledgerState='absent'
  const record:any={id:request.id,status:'preparing',started_at:new Date().toISOString()}
  try {
    if(fs.existsSync(/*turbopackIgnore: true*/ journal)){
      const previous=JSON.parse(fs.readFileSync(/*turbopackIgnore: true*/ journal,'utf8'))
      if(previous.version===version&&previous.status==='complete'&&JSON.stringify(previous.plan.request)===JSON.stringify(request))return previous.result
      throw new ReviewError('此操作编号已有执行记录，请重新预览',409)
    }
    plan=await previewPersonFiles(request)
    if(plan.version!==version)throw new ReviewError('文件、人物或账本已变化，请重新预览',409)
    db=openDatabase();db.exec('BEGIN IMMEDIATE')
    if(metadataVersion(plan,db)!==plan.metadata_version)throw new ReviewError('后台记录已更新，请重新预览',409)
    fs.mkdirSync(path.dirname(journal),{recursive:true})
    record.version=version;record.plan=plan;record.status='prepared';writeJson(journal,record)
    const snapshots=jsonSnapshot()
    const activePlan=plan,affectedIds=new Set(plan.files.flatMap(file=>file.vids))
    writeJson(path.join(path.dirname(journal),'database-before.json'),{
      videos:(db.prepare('SELECT * FROM videos').all() as any[]).filter(row=>affectedIds.has(row.vid)||remappedPath(row.path,activePlan)!==row.path),
      jobs:databaseState(db,plan).jobs,
    })
    for(const snapshot of snapshots)if(snapshot.raw!==null){const backup=path.join(path.dirname(journal),'before',snapshot.name);fs.mkdirSync(path.dirname(backup),{recursive:true});fs.writeFileSync(backup,snapshot.raw)}
    await personLedger('reserve',plan)
    record.ledger_reserved=true;writeJson(journal,record)
    // Exclusive root reservation also covers a person with only empty subdirectories.
    if(plan.action==='merge-person'){for(const directory of plan.directory_targets||[]){guardedPath(directory.dst,MEDIA2,true);mkdirTracked(directory.dst,created)}}
    else if(plan.target_directory&&plan.directories.length){mkdirTracked(path.dirname(plan.target_directory),created);fs.mkdirSync(plan.target_directory);created.push(plan.target_directory);for(const directory of plan.directories.slice(1))mkdirTracked(path.join(plan.target_directory,path.relative(plan.source_directory!,directory)),created)}
    for(const file of plan.files){
      guardedPath(file.src,personDirectory(plan.person));guardedPath(file.dst,MEDIA2,true)
      if(fingerprint(file.src)!==file.version)throw new ReviewError('执行前文件发生变化，已停止',409)
      mkdirTracked(path.dirname(file.dst),created);await moveExclusiveAsync(file.src,file.dst)
      moved.push({file,after:fingerprint(file.dst)});record.moves=moved;record.created=created;record.status='moving';writeJson(journal,record)
    }
    if(plan.source_directory&&fs.existsSync(/*turbopackIgnore: true*/ plan.source_directory)&&descendants(plan.source_directory).files.length)throw new ReviewError('改名期间人物目录新增了文件，请刷新后重试',409)
    applyDatabase(db,plan)
    for(const snapshot of snapshots){
      const updated=updatedMetadata(snapshot.name,snapshot.value,plan)
      if(JSON.stringify(updated)===JSON.stringify(snapshot.value))continue
      const next=JSON.stringify(updated,null,1)
      const current=fs.existsSync(/*turbopackIgnore: true*/ snapshot.file)?fs.readFileSync(/*turbopackIgnore: true*/ snapshot.file,'utf8'):null
      if(current!==snapshot.raw)throw new ReviewError('后台正在更新记录，本次操作已停止，请重试',409)
      fs.mkdirSync(path.dirname(snapshot.file),{recursive:true});writeRaw(snapshot.file,next);written.push({file:snapshot.file,before:snapshot.raw,after:next})
    }
    record.status='updating_ledger';writeJson(journal,record)
    try{ledgerState=(await personLedger('apply',plan)).state||'unknown'}
    catch(error){
      try{ledgerState=(await personLedger('status',plan)).state||'unknown'}catch{ledgerState='unknown'}
      if(ledgerState!=='committed')throw error
    }
    if(ledgerState!=='committed')throw new Error('Peach 操作尚未确认')
    record.status='committing';writeJson(journal,record)
    db.exec('COMMIT')
    const result:PersonFileResult={ok:true,id:plan.id,person:['rename-person','merge-person'].includes(plan.action)?plan.target_person:plan.person,changed:plan.files.length,staged:['delete-files','delete-person'].includes(plan.action)}
    // All stores are committed. A log/empty-directory cleanup failure must not undo a successful operation.
    try{for(const directory of [...plan.directories].sort((a,b)=>b.length-a.length))fs.rmdirSync(directory)}catch{result.warning='操作已完成，旧目录中还有新文件或空目录待核对'}
    record.status='complete';record.result=result;record.completed_at=new Date().toISOString()
    try{writeJson(journal,record)}catch{result.warning='操作已完成，但完成日志未写入，请勿重复执行'}
    invalidateIntegrationCache();return result
  }catch(error){
    if(db?.inTransaction)db.exec('ROLLBACK')
    const failures:string[]=[]
    if(ledgerState==='unknown')failures.push('Peach 结果未知，保留当前状态等待核对')
    else if(ledgerState==='committed'&&plan){try{await personLedger('rollback',plan)}catch(e){failures.push(`Peach 回退失败：${(e as Error).message}`)}}
    if(!failures.length){
      for(const item of written.reverse())try{
        if(fs.readFileSync(/*turbopackIgnore: true*/ item.file,'utf8')!==item.after)throw new Error('记录被其他进程更新')
        if(item.before===null)fs.unlinkSync(item.file);else writeRaw(item.file,item.before)
      }catch(e){failures.push(`记录回退失败：${path.basename(item.file)} ${(e as Error).message}`)}
      for(const item of moved.reverse())try{
        if(fingerprint(item.file.dst)!==item.after)throw new Error('目标文件已变化')
        await moveExclusiveAsync(item.file.dst,item.file.src)
      }catch(e){failures.push(`文件回退失败：${path.basename(item.file.src)} ${(e as Error).message}`)}
      for(const directory of created.reverse())try{fs.rmdirSync(directory)}catch(e){if(!['ENOENT','ENOTEMPTY'].includes((e as NodeJS.ErrnoException).code||''))failures.push('新目录清理失败')}
    }
    if(moved.length||written.length)invalidateIntegrationCache()
    if(plan&&fs.existsSync(/*turbopackIgnore: true*/ journal)){record.status=failures.length?'recovery_needed':'rolled_back';record.error=(error as Error).message;record.recovery_errors=failures;try{writeJson(journal,record)}catch{failures.push('操作记录写入失败')}}
    if(failures.length){keepLock=true;try{fs.writeFileSync(path.join(BASE,'.review-write.lock'),JSON.stringify({pid:process.pid,at:new Date().toISOString(),operation:request.id,status:'recovery_needed',journal}))}catch{};throw new ReviewError(`操作 ${request.id} 需要核对恢复，已暂停写操作：${failures.join('；')}`,503)}
    throw error
  }finally{db?.close();release(keepLock)}
}
