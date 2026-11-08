import fs from 'node:fs'
import path from 'node:path'
import Database from 'better-sqlite3'
import { BASE } from './env'
import { readJson,loadQueue } from './jsonio'
import { readRunProgress,type ProcessingRun } from './processing-progress'
import { loadArchive } from './review-store'
import { normalizeJob,reconcileProcessingJob,homeTaskStage,taskMatchesStage,type HomeJob,type HomeTask,type HomeTasks } from './home-model'

const jobNames=['text','face','discovery','archive','watermark'] as const
export function homeTasks(params:URLSearchParams):HomeTasks {
  const now=Date.now()/1000,heartbeat=readJson<{at?:number}>(path.join(BASE,'processing/heartbeat.json'),{}).at??null
  const online=heartbeat!==null&&now-heartbeat<90&&now-heartbeat>=-30
  const file=path.join(BASE,'processing/processing.db'),initialized=fs.existsSync(file)
  const jobs:HomeJob[]=[],counts:Record<string,number>={};let stageRuns:ProcessingRun[]=[],parallelProgress=false;let tasks:HomeTask[]=[],paused=false,events:HomeTasks['events']=[]
  if(initialized){const db=new Database(file,{readonly:true,fileMustExist:true});db.pragma('busy_timeout = 1500')
    try{
      paused=JSON.parse((db.prepare("SELECT value FROM settings WHERE key='paused'").get() as {value:string}|undefined)?.value||'false')===true
      const handled=new Set([...loadQueue(),...loadArchive()].map(v=>v.vid))
      tasks=(db.prepare("SELECT vid,name,path,stage,batch_id,updated_at,priority,error,manual,next_retry FROM tasks WHERE stage!='excluded' ORDER BY priority DESC,is_new DESC,created_at DESC,vid").all() as HomeTask[]).filter(t=>!handled.has(t.vid))
      for(const t of tasks){const key=homeTaskStage(t);counts[key]=(counts[key]||0)+1}
      parallelProgress=!!db.prepare("SELECT 1 FROM sqlite_master WHERE type='table' AND name='processing_runs'").get()
      if(parallelProgress){
        stageRuns=db.prepare("SELECT * FROM processing_runs WHERE status='running' ORDER BY started_at").all() as ProcessingRun[]
        for(const kind of ['text','face'])if(!stageRuns.some(r=>r.stage===kind)){const latest=db.prepare('SELECT * FROM processing_runs WHERE stage=? ORDER BY started_at DESC LIMIT 1').get(kind) as ProcessingRun|undefined;if(latest)stageRuns.push(latest)}
      }
      events=db.prepare('SELECT id,at,kind,message FROM events ORDER BY id DESC LIMIT 10').all() as HomeTasks['events']
    }finally{db.close()}
  }
  if(parallelProgress)for(const run of stageRuns)jobs.push(readRunProgress(run,online))
  for(const kind of jobNames){if(parallelProgress&&(kind==='text'||kind==='face'))continue;const raw=readJson<HomeJob|null>(path.join(BASE,'jobs',kind+'.json'),null)
    if(raw?.kind===kind&&raw.id&&(!raw.finished_at||now-raw.finished_at<86400)){
      const beat=kind==='archive'?raw.heartbeat_at:kind==='watermark'?Date.parse(readJson<{at?:string}>(path.join(BASE,'watermarks/worker.json'),{}).at||'')/1000:heartbeat
      jobs.push(normalizeJob(reconcileProcessingJob(raw,tasks.find(t=>t.vid===raw.vid),online),now,Number.isFinite(beat)?beat:null))
    }
  }
  // Keep manually submitted watermark batches visible, even on older workers.
  if(!jobs.some(j=>j.kind==='watermark')){
    const request=readJson<any>(path.join(BASE,'watermarks/job.json'),null),progress=readJson<any>(path.join(BASE,'watermarks/progress.json'),null)
    if(request&&progress?.job_id===request.id&&progress.status==='running'){
      const file=path.join(BASE,'watermarks/progress.json'),beat=Date.parse(readJson<{at?:string}>(path.join(BASE,'watermarks/worker.json'),{}).at||'')/1000
      jobs.push(normalizeJob({id:request.id,kind:'watermark',title:'水印批次',status:'running',stage:progress.stage||'提取水印',done:progress.processed??null,total:progress.total??null,unit:'个视频',failed:progress.failed||0,started_at:Date.parse(request.created_at)/1000||null,updated_at:fs.statSync(file).mtimeMs/1000,heartbeat_at:Number.isFinite(beat)?beat:null,href:'/watermarks'},now,Number.isFinite(beat)?beat:null))
    }
  }
  for(const kind of ['text','face'] as const){const current=tasks.find(t=>t.stage===kind)
    const old=jobs.find(j=>j.kind===kind)
    if(parallelProgress&&jobs.some(j=>j.kind===kind))continue
    if(current&&(!old||old.vid!==current.vid||old.status==='complete'||old.status==='error')){
      if(old)jobs.splice(jobs.indexOf(old),1)
      jobs.push({id:kind+':'+current.vid,kind,title:kind==='text'?'提取水印':'人脸分析',status:online?'running':'offline',stage:kind==='text'?'提取文字线索':'分析人脸',current:current.name,vid:current.vid,done:null,total:null,unit:'',failed:0,started_at:current.updated_at,updated_at:current.updated_at,heartbeat_at:heartbeat,href:'/review?vid='+encodeURIComponent(current.vid)})
    }else if(!old){jobs.push({id:kind,kind,title:kind==='text'?'提取水印':'人脸分析',status:!online?'offline':paused?'paused':'waiting',stage:paused?'已暂停领取新任务':kind==='text'?'等待视频进入队列':'等待文字线索完成',done:null,total:null,unit:'',failed:0,started_at:null,updated_at:null,heartbeat_at:heartbeat,href:'/#task-queue'})}
  }
  const stage=params.get('stage')||'',q=(params.get('q')||'').trim().toLowerCase()
  const filtered=tasks.filter(t=>taskMatchesStage(t,stage)&&(!q||`${t.name} ${t.path}`.toLowerCase().includes(q)))
  const limit=20,page=Math.min(Math.max(0,Math.floor(Number(params.get('page'))||0)),Math.max(0,Math.ceil(filtered.length/limit)-1))
  jobs.sort((a,b)=>Number(b.status==='running')-Number(a.status==='running')||Number(b.status==='error')-Number(a.status==='error')||(['face','text','discovery','archive','watermark'].indexOf(a.kind)-['face','text','discovery','archive','watermark'].indexOf(b.kind)))
  return {at:now,initialized,online,paused,counts,jobs,rows:filtered.slice(page*limit,(page+1)*limit),total:filtered.length,page,limit,events}
}
