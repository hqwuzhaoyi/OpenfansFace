import { MEDIA2 } from './env'
import { readVideoSources, videoSource, isMediaFile } from './video-sources'
import { creatorFolder } from './source-resolution'
import { reviewClusters } from './cluster-review'
import { loadClusters } from './integration'
import { resolvedVids } from './review-store'
import { healthScope } from './video-health'
import { isStagedPath } from './person-file-paths'

/** Eligibility follows the current file location, never its historical video ID. */
export function reviewScope(sources=readVideoSources()) {
  const paths=new Map<string,string|null>(),eligibility=new Map<string,boolean>()
  const hidden=healthScope()
  const source=(vid:string)=>{if(!paths.has(vid))paths.set(vid,videoSource(vid,sources));return paths.get(vid)!}
  const organized=(vid:string)=>{const p=source(vid);return !!p&&creatorFolder(p,MEDIA2)!==null}
  const eligible=(vid:string)=>{
    if(!eligibility.has(vid)){const p=source(vid);eligibility.set(vid,!!p&&!organized(vid)&&!isStagedPath(p)&&isMediaFile(p)&&!hidden(vid,p))}
    return eligibility.get(vid)!
  }
  return {sources,source,organized,eligible,hidden:(vid:string)=>{const p=source(vid);return !!p&&hidden(vid,p)}}
}
export function pendingClusters(scope=reviewScope(),cid?:string) {
  const handled=resolvedVids()
  const groups=loadClusters().filter(c=>!cid||c.cid===cid).map(c=>({...c,vids:(c.vids||[]).filter((v:any)=>!handled.has(v.vid)&&scope.eligible(v.vid)).map((v:any)=>({...v,path:scope.source(v.vid)}))}))
  return reviewClusters(groups,[])
}
