import { NextResponse } from 'next/server'
import { reviewScope } from '@/lib/review-scope'
import { creatorFolder } from '@/lib/source-resolution'
import { MEDIA2 } from '@/lib/env'
import { loadReport } from '@/lib/integration'
import { loadQueue,loadArchive,videoInfo } from '@/lib/review-store'
import { peopleCatalog,publicThumb } from '@/lib/people-catalog'
import { fileName } from '@/lib/review-ui'

export async function GET() {
  const report=loadReport(),scope=reviewScope(),metadata=videoInfo()
  const models=new Map((report.identities||[]).map((i:any)=>[i.name,i]))
  const queue=loadQueue(),history=loadArchive(),queued=new Map(queue.map(v=>[v.vid,v])),archived=new Map(history.map(v=>[v.vid,v]))
  const identities=peopleCatalog().map(person=>{
    const model:any=models.get(person.name)||{name:person.name,aliases:[],tpl_faces:0,vids:[]}
    const videos=new Map<string,any>((model.vids||[]).map((v:any)=>[v.vid,v]))
    for(const item of [...queue,...history])if(item.creator===person.name)videos.set(item.vid,{...metadata.get(item.vid),...item})
    const vids=[...videos.values()].filter(v=>{
      const assignment=queued.get(v.vid)||archived.get(v.vid)
      if(assignment&&assignment.creator!==person.name)return false
      if(!assignment&&scope.hidden(v.vid))return false
      const p=archived.get(v.vid)?.win_dst||scope.source(v.vid),folder=p&&creatorFolder(p,MEDIA2)
      return !folder||[person.name,...(model.aliases||[])].some((n:string)=>n.toLowerCase()===folder.toLowerCase())
    }).map(v=>{
      const actual=archived.get(v.vid)?.win_dst||scope.source(v.vid)||v.vid
      const organized=creatorFolder(actual,MEDIA2)!==null
      return {vid:v.vid,path:actual,name:fileName(actual),size_gb:v.size_gb??+((v.size||0)/2**30).toFixed(2),duration:v.duration,
        thumb:publicThumb(v.thumb),conf:v.conf||'ok',organized,
        state:archived.has(v.vid)||organized?'archived':queued.has(v.vid)?'queued':'pending',queued_creator:queued.get(v.vid)?.creator}
    })
    const pending=vids.filter(v=>v.state==='pending')
    return {name:person.name,claimed_n:pending.length,claimed_gb:+pending.reduce((n,v)=>n+v.size_gb,0).toFixed(1),
      folder_n:model.folder_n||0,folder_gb:model.folder_gb||0,tpl_faces:model.tpl_faces||0,rep_thumb:person.rep_thumb,aliases:(model.aliases||[]).slice(0,3),vids}
  })
  return NextResponse.json({stats:report.stats||{},identities})
}
