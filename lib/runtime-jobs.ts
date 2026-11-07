import fs from 'node:fs'
import path from 'node:path'
import { randomUUID } from 'node:crypto'
import { BASE } from './env'
import { writeJson } from './jsonio'
import type { HomeJob } from './home-model'

/** One record per serial lane. The archive caller already owns the shared review lock. */
export function startArchiveJob(total:number) {
  const dir=path.join(BASE,'jobs');fs.mkdirSync(dir,{recursive:true})
  const file=path.join(dir,'archive.json'),now=Date.now()/1000
  let job:HomeJob={id:randomUUID(),kind:'archive',title:'归档同步',status:'running',stage:'检查归档',done:0,total,unit:'个文件',failed:0,started_at:now,updated_at:now,heartbeat_at:now,href:'/integration'}
  writeJson(file,job)
  const persist=()=>{try{writeJson(file,job)}catch(e){console.error('Archive progress unavailable',e)}}
  const timer=setInterval(()=>{job={...job,heartbeat_at:Date.now()/1000};persist()},10000)
  timer.unref()
  return {
    update(patch:Partial<HomeJob>){job={...job,...patch,updated_at:Date.now()/1000,heartbeat_at:Date.now()/1000};persist()},
    finish(error?:string){clearInterval(timer);job={...job,status:error?'error':'complete',error,stage:error?'同步未完成':'归档完成',finished_at:Date.now()/1000,updated_at:Date.now()/1000,heartbeat_at:Date.now()/1000};persist()},
  }
}
