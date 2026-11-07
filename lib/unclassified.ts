import fs from 'fs'
import path from 'path'
import Database from 'better-sqlite3'
import { BASE } from './env'
import { reviewScope } from './review-scope'
import { loadQueue, loadArchive, videoInfo, loadDecisions } from './review-store'
import { loadClusters, loadReport, loadClaims } from './integration'
import { thumbUrl } from './db'

export function unclassifiedVideos(){
  const scope=reviewScope(),meta=videoInfo(),queue=loadQueue(),archive=loadArchive(),dec=loadDecisions()
  const handled=new Set([...queue,...archive].map(v=>v.vid))
  const handledPaths=new Set(queue.map(v=>scope.source(v.vid)).filter(Boolean))
  const visible=new Set<string>()
  for(const c of loadClusters())for(const v of c.vids||[])visible.add(v.vid)
  for(const p of loadReport().identities||[])for(const v of p.vids||[])visible.add(v.vid)
  for(const c of loadClaims())if(c.s>=.5&&c.s<.58&&!dec.approved.includes(c.vid)&&!dec.rejected.includes(c.vid))visible.add(c.vid)
  const visiblePaths=new Set([...visible].map(v=>scope.source(v)).filter(Boolean))
  const db=new Database(`${BASE}/faces.db`,{readonly:true,fileMustExist:true})
  let faces:Map<string,any>
  try{faces=new Map((db.prepare('SELECT vid,COUNT(*) AS n,MAX(thumb) AS thumb FROM faces GROUP BY vid').all() as any[]).map(v=>[v.vid,v]))}finally{db.close()}
  const byPath=new Map<string,any>()
  for(const vid of [...new Set([...scope.sources.keys(),...meta.keys()])].sort()){
    if(handled.has(vid)||!scope.eligible(vid))continue
    const file=scope.source(vid)!;if(handledPaths.has(file)||byPath.has(file))continue
    const stat=fs.statSync(file);if(!stat.size)continue
    const info=meta.get(vid),face=faces.get(vid),uncovered=!visiblePaths.has(file)
    byPath.set(file,{vid,path:file,name:path.basename(file),size_gb:+(stat.size/2**30).toFixed(2),duration:info?.duration,thumb:thumbUrl(info?.thumb||face?.thumb),uncovered,
      reason:uncovered?(face?.n?'有人脸记录，尚未进入原有确认列表':'未检测到人脸，可直接看视频后手动归名'):'也可在原有确认页面处理'})
  }
  const videos=[...byPath.values()].sort((a,b)=>a.path.localeCompare(b.path,'zh-CN'))
  return {videos,total:videos.length,uncovered:videos.filter(v=>v.uncovered).length}
}
