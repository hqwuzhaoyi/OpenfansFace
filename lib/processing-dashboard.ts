import path from 'node:path'
import { BASE } from './env'
import { readJson } from './jsonio'
import { loadQueue,loadArchive } from './review-store'
import { processingDb,processingState } from './processing'
import { isReviewAvailable } from './review-continuity-model'
import { ReviewError } from './review-logic'
import { nativeJsonRequest } from './native-json-transport'
import { buildProcessingBatches,batchTotals,type CohortItem } from './processing-dashboard-model'
import { readRunProgress,type ProcessingRun } from './processing-progress'
export interface ProcessingHeartbeat {at?:number;protocol?:number;max_concurrency?:number;concurrency?:number;active?:number;inference?:{backend?:string;available?:boolean;name?:string;error?:string}}
function heartbeat(){return readJson<ProcessingHeartbeat>(path.join(BASE,'processing/heartbeat.json'),{})}
function fresh(beat:ProcessingHeartbeat){const age=Date.now()/1000-(beat.at||0);return age>=-30&&age<90}
export function processingSettings(){
  const beat=heartbeat(),db=processingDb()
  if(!db)throw new ReviewError('分析服务尚未初始化，设置未知',503)
  let requested=1
  if(db)try{const row=db.prepare("SELECT value FROM settings WHERE key='concurrency'").get() as {value:string}|undefined;const value=JSON.parse(row?.value||'1');if(typeof value==='number'&&Number.isInteger(value))requested=Math.max(1,Math.min(4,value))}finally{db.close()}
  const maximum=Math.max(1,Math.min(4,Number.isInteger(beat.max_concurrency)?beat.max_concurrency!:1))
  return {schema_version:1 as const,requested,maximum,effective:Math.min(requested,maximum),supported:(beat.protocol||0)>=2,online:fresh(beat),last_seen:beat.at||null}
}
export async function setProcessingConcurrency(value:unknown){
  return nativeJsonRequest<ReturnType<typeof processingSettings>>('v1/metadata',{command:'processing-concurrency',body:{concurrency:value}},{label:'自动分析并发设置',completion:'required',error:(message,status)=>new ReviewError(message,status)})
}
export function processingDashboard(){
  const state=processingState(),beat=heartbeat(),settings=processingSettings(),db=processingDb()
  if(!db)throw new ReviewError('处理账本尚未初始化，当前数量未知',503)
  const handled=new Set([...loadQueue(),...loadArchive()].map(v=>v.vid))
  let items:CohortItem[]=[],runs:ProcessingRun[]=[],active:ProcessingRun[]=[],legacyHistory:{id:number;at:number;vid:string;kind:string;message:string;name?:string}[]=[],ledgerReady=false
  let mediaIssue:unknown=null
  if(db)try{db.transaction(()=>{
    const tables=new Set((db.prepare("SELECT name FROM sqlite_master WHERE type='table'").all() as {name:string}[]).map(r=>r.name))
    ledgerReady=tables.has('batch_items')&&Number(JSON.parse((db.prepare("SELECT value FROM settings WHERE key='batch_ledger_version'").get() as {value:string}|undefined)?.value||'0'))>=1
    if(ledgerReady)items=(db.prepare(`SELECT b.batch_id,b.vid,b.added_at,b.completed,b.text_done,b.face_required,b.face_done,b.face_skipped,b.excluded,b.legacy,
      CASE WHEN t.batch_id=b.batch_id AND t.path=b.path AND t.size=b.size AND t.mtime_ns=b.mtime_ns THEN 1 ELSE 0 END AS current,t.stage,t.manual
      FROM batch_items b LEFT JOIN tasks t ON t.vid=b.vid`).all() as CohortItem[]).map(r=>({...r,handled:handled.has(r.vid)}))
    if(tables.has('processing_runs')){
      active=db.prepare("SELECT * FROM processing_runs WHERE status='running' ORDER BY started_at,id").all() as ProcessingRun[]
      runs=db.prepare('SELECT * FROM processing_runs ORDER BY started_at DESC,id DESC LIMIT 24').all() as ProcessingRun[]
    }
    if(!runs.length)legacyHistory=db.prepare("SELECT e.*,t.name FROM events e LEFT JOIN tasks t ON t.vid=e.vid WHERE kind IN ('ready','stage_complete','retry','error') ORDER BY e.id DESC LIMIT 24").all() as typeof legacyHistory
    const issue=db.prepare("SELECT value FROM settings WHERE key='media_unavailable'").get() as {value:string}|undefined
    if(issue)mediaIssue=JSON.parse(issue.value)
  })()}finally{db.close()}
  const batches=buildProcessingBatches(items),totals=batchTotals(batches)
  const history=runs.length?runs.reverse().map(r=>({id:r.id,label:r.name,status:r.status,stage:r.stage,at:r.started_at,finished_at:r.finished_at,error:r.error,batch:r.batch_id,execution:r.execution?JSON.parse(r.execution):null})):legacyHistory.reverse().map(e=>({id:'event-'+e.id,label:e.name||e.vid||'处理记录',status:e.kind==='ready'||e.kind==='stage_complete'?'done':e.kind,stage:e.kind==='stage_complete'?'text':'unknown',at:e.at,finished_at:e.at,error:e.kind==='error'||e.kind==='retry'?e.message:null,batch:null,execution:null}))
  const current={running:state.tasks.filter(t=>['text','face'].includes(t.stage)).length,waiting:state.tasks.filter(t=>['queued','face_queued','retry'].includes(t.stage)).length,error:state.tasks.filter(t=>t.stage==='error'&&!t.manual).length,review:state.tasks.filter(isReviewAvailable).length}
  const jobs=active.map(r=>readRunProgress(r,state.online))
  if(!jobs.length)for(const t of state.tasks.filter(t=>['text','face'].includes(t.stage)))jobs.push({id:t.vid,kind:t.stage==='face'?'face':'text',title:t.stage==='face'?'人脸分析':'提取水印',status:state.online?'running':'offline',stage:'处理中',current:t.name,vid:t.vid,done:null,total:null,unit:'',failed:0,started_at:t.updated_at,updated_at:t.updated_at,heartbeat_at:beat.at||null,href:'/operations/tasks?vid='+encodeURIComponent(t.vid)})
  return {schema_version:1 as const,preview:process.env.PROCESSING_PREVIEW==='1',preview_id:process.env.PROCESSING_PREVIEW==='1'?process.env.PROCESSING_PREVIEW_ID:undefined,at:Date.now()/1000,online:state.online,paused:state.paused,ledger_ready:ledgerReady,batches,totals,current,jobs,history,history_source:runs.length?'stage_runs':'legacy_events',settings,media_issue:mediaIssue,discovery:state.last_discovery,references:state.references,inference:beat.inference?{...beat.inference,available:fresh(beat)&&beat.inference.available===true}:{backend:'unknown',available:false,name:'处理来源未知',error:'后台未提供推理来源'},events:state.events}
}
export async function retryProcessingErrors(batch?:string){
  if(batch!==undefined&&(typeof batch!=='string'||!batch||batch.length>128))throw new ReviewError('批次格式无效')
  return nativeJsonRequest<{schema_version:1;retried:number}>('v1/metadata',{command:'processing-retry-errors',body:batch===undefined?{}:{batch}},{label:'自动分析异常重试',completion:'required',error:(message,status)=>new ReviewError(message,status)})
}
export type ProcessingDashboardData=ReturnType<typeof processingDashboard>
