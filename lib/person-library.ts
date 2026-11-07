import path from 'node:path'
import { BASE } from './env'
import { peopleCatalog, publicThumb } from './people-catalog'
import { loadReport, loadClaims } from './integration'
import { loadArchive, loadQueue, videoInfo } from './review-store'
import { readJson } from './jsonio'
import { validateCreator } from './review-logic'
import { loadWatermarkMappings, loadWatermarkRecords, watermarkFile } from './watermarks'
import { watermarkMatches, watermarkSuggestion } from './watermark-model'
import type { LibraryPerson, PersonLibrary, PersonRejection, PersonVideo } from './person-library-model'
import { canonicalPersonName, personRenames } from './person-names'
import { loadPersonAccounts } from './person-account-store'
import { nativeLibraryNames, nativePersonInventory, nativeLibraryObservations } from './person-library-backend'

export const PERSON_REJECTIONS = path.join(BASE,'person-rejections.json')
export const rejectionKey = (name:string,vid:string) => JSON.stringify([name,vid])
export const loadPersonRejections = () => readJson<Record<string,PersonRejection>>(PERSON_REJECTIONS,{})

/** Listing people never walks every person's files or depends on the Peach ledger. */
export async function libraryPeople():Promise<LibraryPerson[]> {
  const queued=new Map<string,number>()
  for(const row of loadQueue()) queued.set(row.creator,(queued.get(row.creator)||0)+1)
  const people=new Map(peopleCatalog().map(p=>[p.name,{...p,queued:queued.get(p.name)||0}]))
  for(const name of await nativeLibraryNames())if(!people.has(name))people.set(name,{name,rep_thumb:null,queued:queued.get(name)||0,recent_at:undefined})
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
  const inventory=await nativePersonInventory(validateCreator(input)),{name,root,folders,directory_exists}=inventory
  const metadata=videoInfo(),history=loadArchive(),queue=loadQueue()
  const assigned=new Set([...queue,...history].map(v=>v.vid))
  const files:PersonVideo[]=inventory.files.map(file=>({...file,thumb:publicThumb(metadata.get(file.vid)?.thumb),duration:metadata.get(file.vid)?.duration})).sort((a,b)=>a.name.localeCompare(b.name,'zh-CN'))
  if(!includeRecommendations)return {name,root,files,recommendations:[],folders,queued:queue.filter(v=>v.creator===name).length,directory_exists}
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
  const observations=await nativeLibraryObservations([...ids].filter(vid=>!assigned.has(vid)))
  const recommendations:PersonVideo[]=[],seen=new Set<string>()
  const reportIds=new Set<string>((identity?.vids||[]).map((v:any)=>v.vid))
  for(const vid of ids) {
    const observed=observations[vid]
    if(assigned.has(vid)||!observed?.eligible||!observed.path)continue
    const source=observed.path,size=observed.size!,mtime_ns=observed.mtime_ns!
    if(seen.has(source)||observed.assigned)continue
    const rejection=rejections[rejectionKey(name,vid)]
    if(rejection&&rejection.path===source&&rejection.size===size&&rejection.mtime_ns===mtime_ns)continue
    const info=metadata.get(vid),evidence:PersonVideo['evidence']=[]
    const claim=claims.filter(c=>c.vid===vid&&c.name===name).sort((a,b)=>Number(b.s)-Number(a.s))[0]
    if(claim) evidence.push({kind:'face',title:'人脸相似候选',detail:'来自人脸候选记录，仍需人工核对。',...(Number.isFinite(claim.s)?{score:claim.s}:{}),...(Number.isFinite(claim.n)?{faces:claim.n}:{})})
    else if(reportIds.has(vid))evidence.push({kind:'report',title:'识别报告推荐',detail:info?.conf==='weak'?'支持归组的人脸较少，请重点核对。':'报告将此视频列为当前人物的候选，未提供独立相似度分数。'})
    const record=records[vid]
    const valid=record?.status==='done'&&watermarkMatches(record,source,size,mtime_ns,pipeline)
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
    recommendations.push({vid,path:source,name:path.basename(source),folder:path.dirname(source),size,size_gb:size/2**30,modified_at:new Date(observed.modified_ms!).toISOString(),duration:info?.duration,thumb:publicThumb(info?.thumb),evidence,conflict,state:'pending'})
  }
  return {name,root,files:files.sort((a,b)=>a.name.localeCompare(b.name,'zh-CN')),recommendations,folders:folders.sort((a,b)=>a.name.localeCompare(b.name,'zh-CN')),queued:queue.filter(v=>v.creator===name).length,directory_exists}
}
