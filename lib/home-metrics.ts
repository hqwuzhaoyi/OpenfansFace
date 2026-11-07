import fs from 'node:fs'
import path from 'node:path'
import { CREATORS,STAGE,MEDIA2 } from './env'
import { loadReport,loadClaims } from './integration'
import { loadQueue,loadDecisions,resolvedVids } from './review-store'
import { reviewScope,pendingClusters } from './review-scope'
import { unclassifiedVideos } from './unclassified'
import type { HomeMetrics } from './home-model'

let cached:HomeMetrics|undefined,pending:Promise<HomeMetrics>|undefined
async function walk(root:string):Promise<{files:number;bytes:number}>{let files=0,bytes=0;const dirs=[root];while(dirs.length){const dir=dirs.pop()!;for(const e of await fs.promises.readdir(dir,{withFileTypes:true})){const file=path.join(dir,e.name);if(e.isDirectory())dirs.push(file);else if(e.isFile()){const s=await fs.promises.stat(file);files++;bytes+=s.size}}}return {files,bytes}}
export async function homeMetrics():Promise<HomeMetrics>{
  if(cached&&Date.now()/1000-cached.at<60)return cached
  if(pending)return pending
  pending=(async()=>{
    const errors:string[]=[],metrics:HomeMetrics['metrics']=[],report:HomeMetrics['report']=[]
    const add=(key:string,label:string,value:number|null,href:string|undefined,description:string,unit?:string)=>metrics.push({key,label,value,href,description,unit})
    try{const d=unclassifiedVideos();add('unclassified','待分类',d.total,'/unclassified','当前可归名的视频，按实际路径去重；不含已归档、待归档及被体检收起的文件。')}
    catch{add('unclassified','待分类',null,'/unclassified','暂时无法读取待分类列表。');errors.push('待分类统计暂不可用')}
    add('archive','待归档',loadQueue().length,'/integration','已加入归档队列、尚未完成文件归档的记录。')
    try{const scope=reviewScope(),groups=pendingClusters(scope);add('groups','待核对分组',groups.length,'/clusters',`仍可处理的相似视频组，共 ${groups.reduce((n,c)=>n+c.n_videos,0)} 个视频。可能与待分类重叠，不能直接相加。`)
      const dec=loadDecisions(),handled=new Set([...resolvedVids(),...dec.approved,...dec.rejected]);const n=loadClaims().filter(c=>c.s>=.5&&c.s<.58&&!handled.has(c.vid)&&scope.eligible(c.vid)).length
      report.push({key:'legacy_review',label:'历史待确认',value:n,href:'/approvals',description:'旧识别报告中尚未确认的候选视频，独立于自动分析队列。'})
    }catch{add('groups','待核对分组',null,'/clusters','暂时无法读取相似分组。');errors.push('识别报告统计暂不可用')}
    const [library,staging,disk]=await Promise.allSettled([(async()=>{const dirs=(await fs.promises.readdir(CREATORS,{withFileTypes:true})).filter(e=>e.isDirectory());let files=0,bytes=0,cursor=0;await Promise.all(Array.from({length:Math.min(3,dirs.length)},async()=>{while(cursor<dirs.length){const d=dirs[cursor++];const s=await walk(path.join(CREATORS,d.name));files+=s.files;bytes+=s.bytes}}));return {files,bytes,people:dirs.length}})(),walk(STAGE),fs.promises.statfs(MEDIA2)])
    if(library.status==='fulfilled'){add('files','在库文件',library.value.files,'/persons',`人物归档目录中的全部文件，包含非视频文件，占用 ${(library.value.bytes/2**30).toFixed(1)} GiB。`);add('people','人物目录',library.value.people,'/persons','人物归档文件夹的数量，不等同于独立身份或有效参考模板数。')}else{add('files','在库文件',null,'/persons','目录统计暂不可用。');add('people','人物目录',null,'/persons','目录统计暂不可用。');errors.push('人物目录统计暂不可用')}
    add('staging','回收站文件',staging.status==='fulfilled'?staging.value.files:null,'/staging',staging.status==='fulfilled'?`暂存目录内的文件，占用 ${(staging.value.bytes/2**30).toFixed(1)} GiB。可查看统计和视频体检。`:'暂存目录暂不可用。')
    const stats=loadReport().stats;report.push({key:'scanned',label:'报告已扫描',value:stats.scanned??null,description:'识别报告覆盖的扫描数量，不是实时媒体总量或剩余量。'},{key:'claimed',label:'报告已认领',value:stats.claimed_n??null,description:'报告中记录的已认领数，不是当前待归档队列。'})
    const space=disk.status==='fulfilled'?{free_gb:disk.value.bavail*disk.value.bsize/2**30,total_gb:disk.value.blocks*disk.value.bsize/2**30,used_pct:disk.value.blocks?100*(1-disk.value.bavail/disk.value.blocks):0}:null
    if(!space)errors.push('磁盘状态暂不可用')
    return cached={at:Date.now()/1000,metrics,report,disk:space,errors}
  })().finally(()=>{pending=undefined})
  return pending
}
