import path from 'node:path'
import { toWin } from './paths'
import type { PersonFilePlan } from './person-file-model'

export const metadataFiles=['people-deleted.json','report.json','claims-detail.json','clusters.json','integration-queue.json','archive-history.json','people-manual.json','template-overrides.json','watermarks/mappings.json','person-rejections.json','video-path-overrides.json','person-renames.json','external-profiles/manual-links.json','external-profiles/reviewed-links.json']
export function remappedPath(value:string,plan:PersonFilePlan):string {
  if(typeof value!=='string')return value
  for(const file of plan.files){if(value===file.src)return file.dst;if(value===toWin(file.src))return toWin(file.dst)}
  if(['rename-person','merge-person','delete-person'].includes(plan.action)&&plan.source_directory&&plan.target_directory){
    const directories=[...(plan.directory_targets||[]),{src:plan.source_directory,dst:plan.target_directory}].sort((a,b)=>b.src.length-a.src.length)
    for(const directory of directories)for(const [from,to,separator] of [[directory.src,directory.dst,path.sep],[toWin(directory.src),toWin(directory.dst),'\\']]){
      if(value===from)return to
      if(value.startsWith(from+separator))return to+value.slice(from.length)
    }
  }
  return value
}
export function transformPersonData(value:any,plan:PersonFilePlan):any {
  const moves=new Map(plan.files.flatMap(file=>file.vids.map(vid=>[vid,file] as const)))
  const identityChange=['rename-person','merge-person'].includes(plan.action),names=new Set([plan.person,...(plan.source_aliases||[]),...(plan.target_aliases||[])])
  function visit(v:any,key='',parent:any=null):any {
    if(typeof v==='string'){
      if(plan.action==='delete-person'&&key==='suggested'&&names.has(v))return null
      if(key==='vid'||key==='original_vid')return v // stable IDs can contain historical paths
      if(['path','src','dst','win_dst','win_src','original_dst','canonical_dst','collection','root'].includes(key))return remappedPath(v,plan)
      if(identityChange&&['name','creator','suggested'].includes(key)&&names.has(v))return plan.target_person
      const move=parent?.vid?moves.get(parent.vid):undefined
      if(move&&key==='name'&&v===path.basename(move.src))return path.basename(move.dst)
      if(move&&plan.action==='move-files'&&['name','creator'].includes(key)&&v===plan.person)return plan.target_person
      if(key==='source_id'&&parent?.source==='person'&&names.has(v)&&identityChange)return plan.target_person
      if(key==='return_href'&&identityChange&&[...names].some(name=>v===`/persons/${encodeURIComponent(name)}`))return `/persons/${encodeURIComponent(plan.target_person)}`
      return v
    }
    if(Array.isArray(v)){
      const result=v.filter(item=>!(plan.action==='delete-person'&&['candidates','matches'].includes(key)&&item&&names.has(item.name))).map(item=>visit(item))
      if(plan.action==='merge-person'&&['candidates','matches'].includes(key)&&v.some(item=>item&&names.has(item.name))&&result.every(item=>item&&typeof item.name==='string')){
        const combined=new Map<string,any>()
        for(const item of result){const old=combined.get(item.name);if(!old){combined.set(item.name,item);continue}combined.set(item.name,{...old,...item,
          ...(old.sources||item.sources?{sources:[...new Set([...(old.sources||[]),...(item.sources||[])])]}:{}),
          ...(old.evidence||item.evidence?{evidence:[...new Map([...(old.evidence||[]),...(item.evidence||[])].map(e=>[JSON.stringify(e),e])).values()]}:{}),
          ...(typeof old.score==='number'||typeof item.score==='number'?{score:Math.max(old.score??-Infinity,item.score??-Infinity)}:{}),
          ...(typeof old.support==='number'||typeof item.support==='number'?{support:Math.max(old.support||0,item.support||0)}:{})})}
        return [...combined.values()]
      }
      return result
    }
    if(v&&typeof v==='object'){
      const result=Object.fromEntries(Object.entries(v).map(([k,x])=>[k,visit(x,k,v)]))
      if(plan.action==='merge-person'&&result.kind==='conflict'&&Array.isArray(result.candidates)&&Array.isArray(v.candidates)&&v.candidates.length>result.candidates.length&&result.candidates.length===1){result.kind='suggested';result.suggested=result.candidates[0].name}
      if(plan.action==='delete-person'&&Array.isArray(v.candidates)&&result.candidates.length!==v.candidates.length){result.ready=false;result.suggested=null;result.kind=result.candidates.length>1?'conflict':'none'}
      return result
    }
    return v
  }
  return visit(value)
}
export function updatedMetadata(filename:string,value:any,plan:PersonFilePlan):any {
  const identityChange=['rename-person','merge-person'].includes(plan.action),merging=plan.action==='merge-person'
  const sourceNames=new Set([plan.person,...(plan.source_aliases||[])]),targetNames=new Set([plan.target_person,...(plan.target_aliases||[])])
  if(plan.action==='delete-person'){
    if(filename==='people-deleted.json')return {...value,...Object.fromEntries([...sourceNames].map(name=>[name,{operation:plan.id}]))}
    if(filename==='report.json')return {...transformPersonData(value,plan),identities:(value.identities||[]).filter((row:any)=>!sourceNames.has(row.name))}
    if(['people-manual.json','claims-detail.json','integration-queue.json'].includes(filename))value=value.filter((row:any)=>!sourceNames.has(filename==='integration-queue.json'?row.creator:row.name))
    if(['template-overrides.json','person-rejections.json','watermarks/mappings.json'].includes(filename))value=Object.fromEntries(Object.entries(value||{}).filter(([key,row]:[string,any])=>!sourceNames.has(filename==='template-overrides.json'?key:row.name||row.creator)))
    if(['external-profiles/manual-links.json','external-profiles/reviewed-links.json'].includes(filename))return {...value,links:(value.links||[]).filter((row:any)=>!sourceNames.has(row.creator||row.person))}
  }
  if(['external-profiles/manual-links.json','external-profiles/reviewed-links.json'].includes(filename)){
    if(!identityChange||!Array.isArray(value?.links))return value
    const field=filename.includes('manual-links')?'creator':'person'
    let links=value.links.map((link:any)=>sourceNames.has(link[field])||(merging&&targetNames.has(link[field]))?{...link,[field]:plan.target_person}:link)
    if(merging){
      const byKey=new Map<string,any>()
      // Prefer the target's existing manual-link IDs so open management dialogs stay valid.
      for(const index of value.links.map((_:any,i:number)=>i).filter((i:number)=>links[i][field]===plan.target_person).sort((a:number,b:number)=>Number(targetNames.has(value.links[a][field]))-Number(targetNames.has(value.links[b][field])))){
        const link=links[index],key=JSON.stringify([link[field],link.profile?.id||link.profile_id]),old=byKey.get(key)
        if(field==='person'&&old?.confidence==='candidate'&&link.confidence==='reference')continue
        byKey.set(key,link)
      }
      links=[...links.filter((link:any)=>link[field]!==plan.target_person),...byKey.values()]
    }
    return {...value,links}
  }
  const ids=new Set(plan.files.flatMap(f=>f.vids)),removing=['delete-files','delete-person'].includes(plan.action)
  let next=transformPersonData(value,plan)
  if(filename==='video-path-overrides.json') {
    next=Object.fromEntries(Object.entries(value||{}).map(([id,p])=>[id,typeof p==='string'?remappedPath(p,plan):p]))
    for(const file of plan.files)for(const vid of file.vids)Object.defineProperty(next,vid,{value:file.dst,enumerable:true,writable:true,configurable:true})
  }
  if(filename==='person-renames.json'&&identityChange) {
    next={...Object.fromEntries(Object.entries(value||{}).map(([old,current])=>[old,current===plan.person?plan.target_person:current])),[plan.person]:plan.target_person};delete next[plan.target_person]
  }
  if(filename==='template-overrides.json'&&identityChange){
    for(const source of sourceNames)if(Object.hasOwn(next,source)){if(!merging||!Object.hasOwn(next,plan.target_person))next={...next,[plan.target_person]:next[source]};delete next[source]}
  }
  if(filename==='person-rejections.json'){
    const original=Object.values(value||{}) as any[],rows=Object.values(next||{}) as any[]
    const order=rows.map((_,index)=>index).sort((a,b)=>merging?Number(targetNames.has(original[a].name))-Number(targetNames.has(original[b].name)):0)
    next=Object.fromEntries(order.map(index=>[JSON.stringify([rows[index].name,rows[index].vid]),rows[index]]))
  }
  if(filename==='integration-queue.json'&&removing)next=next.filter((row:any)=>!ids.has(row.vid))
  if(filename==='claims-detail.json'&&removing)next=next.filter((row:any)=>!ids.has(row.vid))
  if(filename==='clusters.json'&&removing)next=next.map((row:any)=>({...row,vids:(row.vids||[]).filter((v:any)=>!ids.has(v.vid))}))
  if(filename==='archive-history.json')next=next.map((row:any)=>ids.has(row.vid)?{...row,...(removing?{file_state:'staged'}:{}),last_file_operation:plan.id}:row)
  if(merging&&filename==='report.json'){
    const originals=value.identities||[],rows=next.identities||[],related=rows.filter((row:any)=>row.name===plan.target_person)
    if(related.length){
      const preferredIndex=originals.findIndex((row:any)=>row.name===plan.target_person),base=preferredIndex>=0?rows[preferredIndex]:related[0]
      const ordered=[...related.filter((row:any)=>row!==base),base]
      const combined={...base,rep_thumb:base.rep_thumb||related.find((row:any)=>row.rep_thumb)?.rep_thumb||null,
        aliases:[...new Set([...ordered.flatMap((row:any)=>row.aliases||[]),...sourceNames])].filter(alias=>alias!==plan.target_person),
        vids:[...new Map(ordered.flatMap((row:any)=>row.vids||[]).map((row:any)=>[row.vid,row])).values()]}
      next.identities=[...rows.filter((row:any)=>row.name!==plan.target_person),combined]
    }
  }
  if(merging&&filename==='people-manual.json'){
    const related=next.filter((row:any)=>row.name===plan.target_person),targetIndex=value.findIndex((row:any)=>row.name===plan.target_person)
    const preferred=targetIndex>=0?next[targetIndex]:related[0]||{created_at:new Date().toISOString(),rep_thumb:null}
    next=[...next.filter((row:any)=>row.name!==plan.target_person),{...preferred,name:plan.target_person,last_used_at:new Date().toISOString()}]
  }
  if(merging&&['claims-detail.json','integration-queue.json'].includes(filename)){
    const field=filename==='claims-detail.json'?'name':'creator',byVid=new Map<string,any>(),unrelated=next.filter((row:any)=>row[field]!==plan.target_person)
    const ordered=next.map((row:any,index:number)=>({row,index})).filter(({row}:any)=>row[field]===plan.target_person).sort((a:any,b:any)=>Number(targetNames.has(value[a.index][field]))-Number(targetNames.has(value[b.index][field])))
    for(const {row}of ordered){const old=byVid.get(row.vid);if(filename==='claims-detail.json'&&old&&Number(old.s)>Number(row.s))continue;byVid.set(row.vid,row)}
    next=[...unrelated,...byVid.values()]
  }
  if(filename==='report.json'&&(removing||plan.action==='move-files')) {
    const moved:any[]=[]
    next.identities=(next.identities||[]).map((person:any)=>{
      const belongs=person.name===plan.target_person&&!removing
      const removed=(person.vids||[]).filter((v:any)=>ids.has(v.vid)&&!belongs)
      moved.push(...removed)
      const vids=(person.vids||[]).filter((v:any)=>!ids.has(v.vid)||belongs)
      const rawThumb=typeof person.rep_thumb==='string'&&person.rep_thumb.startsWith('/thumb?')?new URLSearchParams(person.rep_thumb.split('?')[1]).get('p'):person.rep_thumb
      const invalidThumb=rawThumb&&(plan.affected_thumbs||[]).includes(rawThumb)
      return {...person,vids,...((!vids.length&&removed.length)||invalidThumb?{rep_thumb:null,tpl_faces:0}:{})}
    })
    if(!removing&&moved.length){
      let target=next.identities.find((p:any)=>p.name===plan.target_person)
      if(!target){target={name:plan.target_person,vids:[],tpl_faces:0};next.identities.push(target)}
      target.vids=[...new Map([...target.vids,...moved].map((v:any)=>[v.vid,v])).values()]
    }
  }
  if(filename==='people-manual.json'&&plan.action==='move-files'&&!next.some((p:any)=>p.name===plan.target_person))next.push({name:plan.target_person,created_at:new Date().toISOString(),last_used_at:new Date().toISOString(),rep_thumb:null})
  return next
}
