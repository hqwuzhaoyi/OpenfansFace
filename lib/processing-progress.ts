import path from 'node:path'
import { createHash } from 'node:crypto'
import { BASE } from './env'
import { readJson } from './jsonio'
import type { HomeJob } from './home-model'
export interface ProcessingRun {id:string;vid:string;name:string;batch_id:string;stage:string;started_at:number;finished_at:number|null;status:string;error:string|null;execution?:string|null}
export function stageProgressPath(id:string){return path.join(BASE,'jobs/stages',createHash('sha256').update(id).digest('hex')+'.json')}
export function readRunProgress(run:ProcessingRun,online:boolean):HomeJob {
  const raw=readJson<HomeJob|null>(stageProgressPath(run.id),null)
  const same=raw?.id===run.id&&raw?.vid===run.vid
  const kind=run.stage==='face'?'face':'text'
  const status:HomeJob['status']=run.status==='running'?(online?'running':'offline'):run.status==='done'?'complete':['error','interrupted'].includes(run.status)?'error':'waiting'
  return {id:run.id,kind,title:kind==='text'?'提取水印':'人脸分析',status,stage:run.status==='retry'?'等待自动重试':run.status==='waiting'?'等待资源恢复':same?raw.stage:status==='running'?'准备中':status==='complete'?'阶段完成':run.status==='waiting'?'等待资源恢复':run.error||'阶段已结束',current:run.name,vid:run.vid,
    done:same&&typeof raw.done==='number'&&Number.isFinite(raw.done)&&raw.done>=0?raw.done:null,total:same&&typeof raw.total==='number'&&Number.isFinite(raw.total)&&raw.total>=0?raw.total:null,unit:same?raw.unit:'',failed:status==='error'?1:0,started_at:run.started_at,updated_at:same?raw.updated_at:run.finished_at||run.started_at,
    heartbeat_at:same?raw.heartbeat_at:null,...(run.finished_at?{finished_at:run.finished_at}:{}),...(run.error?{error:run.error}:{}),href:'/operations/tasks?vid='+encodeURIComponent(run.vid)}
}
