import fs from 'node:fs'
import path from 'node:path'
import { BASE, CREATORS, MEDIA2 } from './env'
import { peopleCatalog, publicThumb } from './people-catalog'
import { loadReport, loadClaims } from './integration'
import { loadArchive, loadQueue, videoInfo } from './review-store'
import { readJson } from './jsonio'
import { validateCreator, ReviewError } from './review-logic'
import { readVideoSources, videoSource } from './video-sources'
import { sourceCandidates } from './source-resolution'
import { reviewScope } from './review-scope'
import { loadWatermarkMappings, loadWatermarkRecords, watermarkFile } from './watermarks'
import { watermarkMatches, watermarkSuggestion } from './watermark-model'
import type { LibraryPerson, PersonLibrary, PersonRejection, PersonVideo } from './person-library-model'
import { fingerprint } from './person-file-paths'
import { canonicalPersonName, personRenames } from './person-names'
import { personVideoExtension } from './person-library-stats'
import { loadPersonAccounts } from './person-account-store'

export const PERSON_REJECTIONS = path.join(BASE,'person-rejections.json')
export const rejectionKey = (name:string,vid:string) => JSON.stringify([name,vid])
export const loadPersonRejections = () => readJson<Record<string,PersonRejection>>(PERSON_REJECTIONS,{})

/** Listing people never walks every person's files or depends on the Peach ledger. */
export async function libraryPeople():Promise<LibraryPerson[]> {
  const queued=new Map<string,number>()
  for(const row of loadQueue()) queued.set(row.creator,(queued.get(row.creator)||0)+1)
  const people=new Map(peopleCatalog().map(p=>[p.name,{...p,queued:queued.get(p.name)||0}]))
  try {
    for(const entry of await fs.promises.readdir(CREATORS,{withFileTypes:true})) {
      if(entry.isDirectory()&&!entry.name.startsWith('.')&&!people.has(entry.name)) people.set(entry.name,{name:entry.name,rep_thumb:null,queued:queued.get(entry.name)||0,recent_at:undefined})
    }
  } catch(error) { if((error as NodeJS.ErrnoException).code!=='ENOENT')throw error }
  const aliases=new Map<string,string[]>((loadReport().identities||[]).map((person:any)=>[person.name,Array.isArray(person.aliases)?person.aliases.filter((alias:unknown)=>typeof alias==='string'):[]]))
  const renamed=personRenames()
  for(const alias of Object.keys(renamed)){const current=canonicalPersonName(alias,renamed);aliases.set(current,[...(aliases.get(current)||[]),alias])}
  const accountIds=new Map<string,Set<string>>();let accountError:string|undefined
  try{for(const link of loadPersonAccounts()){if(!accountIds.has(link.creator))accountIds.set(link.creator,new Set());accountIds.get(link.creator)!.add(link.profile.id)}}
  catch{accountError='账号关联记录读取失败，请重试'}
  const deleted=readJson<Record<string,unknown>>(path.join(BASE,'people-deleted.json'),{})
  return [...people.values()].filter(person=>!Object.hasOwn(deleted,person.name)).map(person=>({...person,account_count:accountError?null:accountIds.get(person.name)?.size||0,...(accountError?{account_error:accountError}:{}),aliases:[...new Set(aliases.get(person.name)||[])]}))
}

export async function personLibrary(input:string,includeRecommendations=true):Promise<PersonLibrary> {
  const name=canonicalPersonName(validateCreator(input)),root=path.join(CREATORS,name)
  if(Object.hasOwn(readJson<Record<string,unknown>>(path.join(BASE,'people-deleted.json'),{}),name))throw new ReviewError('该人物已删除',404)
  const sources=readVideoSources(),scope=reviewScope(sources),metadata=videoInfo(),history=loadArchive(),queue=loadQueue()
  const assigned=new Set([...queue,...history].map(v=>v.vid))
  const assignedPaths=new Set([...queue,...history].map(v=>scope.source(v.vid)).filter(Boolean))
  const pathIds=new Map<string,string>()
  const effectiveSources=new Map(sources)
  for(const [vid,stored] of sources) for(const candidate of sourceCandidates(vid,stored,MEDIA2)) if(!pathIds.has(candidate))pathIds.set(candidate,vid)
  for(const row of history){effectiveSources.set(row.vid,row.win_dst);for(const candidate of sourceCandidates(row.vid,row.win_dst,MEDIA2))pathIds.set(candidate,row.vid)}
  const files:PersonVideo[]=[],folders=new Map<string,number>()
  let directory_exists=true
  try {
    const rootStat=await fs.promises.lstat(root)
    if(!rootStat.isDirectory()||rootStat.isSymbolicLink())throw new ReviewError('人物路径不是可读取的目录',409)
  }catch(error){if((error as NodeJS.ErrnoException).code==='ENOENT')directory_exists=false;else throw error}
  async function walk(directory:string) {
    const entries=await fs.promises.readdir(directory,{withFileTypes:true})
    for(const entry of entries) {
      // Do not escape the person's directory through symlinks.
      if(entry.isSymbolicLink()||entry.name.startsWith('.'))continue
      const filename=path.join(directory,entry.name)
      if(entry.isDirectory()){await walk(filename);continue}
      if(!entry.isFile()||!personVideoExtension.test(entry.name))continue
      let stat:fs.Stats
      try {stat=await fs.promises.stat(filename)}catch(error){if((error as NodeJS.ErrnoException).code==='ENOENT')continue;throw error}
      const mapped=pathIds.get(filename),vid=mapped&&videoSource(mapped,effectiveSources)===filename?mapped:filename,info=metadata.get(vid),folder=path.relative(root,directory).split(path.sep).join('/')||'.'
      files.push({vid,path:filename,name:entry.name,folder,size:stat.size,size_gb:stat.size/2**30,modified_at:stat.mtime.toISOString(),version:fingerprint(filename),thumb:publicThumb(info?.thumb),duration:info?.duration,evidence:[],state:'archived',organized:true})
      folders.set(folder,(folders.get(folder)||0)+1)
    }
  }
  if(directory_exists)await walk(root)
  if(!includeRecommendations)return {name,root,files:files.sort((a,b)=>a.name.localeCompare(b.name,'zh-CN')),recommendations:[],folders:[...folders].map(([name,count])=>({name,count})),queued:queue.filter(v=>v.creator===name).length,directory_exists}
  const report=loadReport(),models=report.identities||[],identity=models.find((p:any)=>p.name===name)
  const claims=loadClaims(),records=loadWatermarkRecords(),mappings=loadWatermarkMappings(),rejections=loadPersonRejections()
  const pipeline=readJson<{pipeline?:string}>(watermarkFile('worker.json'),{}).pipeline
  const ids=new Set<string>((identity?.vids||[]).map((v:any)=>v.vid))
  for(const claim of claims)if(claim.name===name)ids.add(claim.vid)
  for(const [vid,record]of Object.entries(records))if(record.status==='done'&&record.candidates.some(c=>c.frames>=2&&mappings[c.key]?.kind==='person'&&mappings[c.key].creator===name))ids.add(vid)
  const facePeople=new Map<string,Set<string>>()
  for(const person of models)for(const video of person.vids||[]) {
    if(!ids.has(video.vid))continue
    if(!facePeople.has(video.vid))facePeople.set(video.vid,new Set())
    facePeople.get(video.vid)!.add(person.name)
  }
  for(const claim of claims)if(ids.has(claim.vid)){
    if(!facePeople.has(claim.vid))facePeople.set(claim.vid,new Set())
    facePeople.get(claim.vid)!.add(claim.name)
  }
  const recommendations:PersonVideo[]=[],seen=new Set<string>()
  const reportIds=new Set<string>((identity?.vids||[]).map((v:any)=>v.vid))
  for(const vid of ids) {
    if(assigned.has(vid)||!scope.eligible(vid))continue
    const source=scope.source(vid)!
    if(seen.has(source)||assignedPaths.has(source))continue
    let stat:fs.BigIntStats
    try{stat=await fs.promises.stat(source,{bigint:true})}catch(error){if((error as NodeJS.ErrnoException).code==='ENOENT')continue;throw error}
    const rejection=rejections[rejectionKey(name,vid)]
    if(rejection&&rejection.path===source&&rejection.size===Number(stat.size)&&rejection.mtime_ns===stat.mtimeNs.toString())continue
    const info=metadata.get(vid),evidence:PersonVideo['evidence']=[]
    const claim=claims.filter(c=>c.vid===vid&&c.name===name).sort((a,b)=>Number(b.s)-Number(a.s))[0]
    if(claim) evidence.push({kind:'face',title:'人脸相似候选',detail:'来自人脸候选记录，仍需人工核对。',...(Number.isFinite(claim.s)?{score:claim.s}:{}),...(Number.isFinite(claim.n)?{faces:claim.n}:{})})
    else if(reportIds.has(vid))evidence.push({kind:'report',title:'识别报告推荐',detail:info?.conf==='weak'?'支持归组的人脸较少，请重点核对。':'报告将此视频列为当前人物的候选，未提供独立相似度分数。'})
    const record=records[vid]
    const valid=record?.status==='done'&&watermarkMatches(record,source,Number(stat.size),stat.mtimeNs.toString(),pipeline)
    let conflict=(facePeople.get(vid)?.size||0)>1
    if(valid) {
      const result=watermarkSuggestion(record.candidates,mappings,[...(facePeople.get(vid)||[])])
      conflict=conflict||result.conflict
      for(const candidate of record.candidates) {
        const mapping=mappings[candidate.key]
        if(candidate.frames<2||mapping?.kind!=='person'||mapping.creator!==name)continue
        evidence.push({kind:'watermark',title:'已确认的水印账号映射',detail:`${candidate.text} · ${candidate.frames} 个采样帧；账号已映射为 ${name}`,timestamp:candidate.timestamp,image:`/api/watermarks/evidence?vid=${encodeURIComponent(vid)}&key=${encodeURIComponent(candidate.key)}`})
      }
    }
    if(!evidence.length)continue
    seen.add(source)
    recommendations.push({vid,path:source,name:path.basename(source),folder:path.dirname(source),size:Number(stat.size),size_gb:Number(stat.size)/2**30,modified_at:new Date(Number(stat.mtimeMs)).toISOString(),duration:info?.duration,thumb:publicThumb(info?.thumb),evidence,conflict,state:'pending'})
  }
  return {name,root,files:files.sort((a,b)=>a.name.localeCompare(b.name,'zh-CN')),recommendations,folders:[...folders].map(([name,count])=>({name,count})).sort((a,b)=>a.name.localeCompare(b.name,'zh-CN')),queued:queue.filter(v=>v.creator===name).length,directory_exists}
}
