export type JobStatus = 'running'|'waiting'|'paused'|'complete'|'error'|'offline'
export type JobKind = 'text'|'face'|'discovery'|'archive'|'watermark'|'rebuild'
export interface HomeJob {
  id:string; kind:JobKind; title:string; status:JobStatus; stage:string
  current?:string; vid?:string; done:number|null; total:number|null; unit:string; failed:number
  started_at:number|null; updated_at:number|null; heartbeat_at:number|null; finished_at?:number
  error?:string; href:string
}
export interface HomeTask {vid:string;name:string;path:string;stage:string;batch_id:string;updated_at:number;priority:number;error?:string;manual:number;next_retry:number}
export const homeTaskStage=(task:Pick<HomeTask,'stage'|'manual'>)=>task.manual?'review':task.stage
export function taskMatchesStage(task:Pick<HomeTask,'stage'|'manual'>,stage:string){
  const actual=homeTaskStage(task)
  return !stage||(stage==='queued'?['queued','retry'].includes(actual):stage==='face'?['face','face_queued'].includes(actual):actual===stage)
}
export interface HomeTasks {
  at:number; initialized:boolean; online:boolean; paused:boolean; counts:Record<string,number>
  jobs:HomeJob[]; rows:HomeTask[]; total:number; page:number; limit:number
  events:{id:number;at:number;message:string;kind:string}[]; error?:string
}
export interface HomeMetric {key:string;label:string;value:number|null;unit?:string;href?:string;description:string}
export interface HomeMetrics {at:number;metrics:HomeMetric[];report:HomeMetric[];disk:{free_gb:number;total_gb:number;used_pct:number}|null;errors:string[]}
/** Progress refers to this phase's measured units, never a guessed whole-job percentage. */
export function jobPercent(job:Pick<HomeJob,'done'|'total'>):number|null {
  if(job.done===null||job.total===null||!Number.isFinite(job.done)||!Number.isFinite(job.total)||job.total<=0)return null
  return Math.min(100,Math.max(0,Math.floor(job.done/job.total*100)))
}
export function normalizeJob(job:HomeJob,now:number,heartbeat:number|null):HomeJob {
  const active=job.status==='running'
  const online=heartbeat!==null&&heartbeat<=now+30&&now-heartbeat<90
  return {...job,heartbeat_at:heartbeat,status:active&&!online?'offline':job.status,
    done:job.done!==null&&Number.isFinite(job.done)&&job.done>=0?job.done:null,
    total:job.total!==null&&Number.isFinite(job.total)&&job.total>=0?job.total:null}
}
/** After coordinator recovery, a fresh heartbeat must not revive an interrupted execution. */
export function reconcileProcessingJob(job:HomeJob,owner:HomeTask|undefined,online:boolean):HomeJob {
  if(!online||job.status!=='running'||!['text','face'].includes(job.kind)||owner?.stage===job.kind)return job
  const ended={...job,finished_at:owner?.updated_at??job.updated_at??undefined}
  if(owner&&['retry','error'].includes(owner.stage))return {...ended,status:'error',stage:owner.stage==='retry'?'等待重试':'处理失败',error:owner.error}
  if(owner&&(owner.stage==='review'||job.kind==='text'&&owner.stage==='face_queued'))return {...ended,status:'complete',stage:'阶段完成'}
  return {...ended,status:'waiting',stage:'已离开当前阶段'}
}
export const jobStatus:Record<JobStatus,string>={running:'进行中',waiting:'等待中',paused:'已暂停',complete:'已完成',error:'需处理',offline:'连接中断'}
