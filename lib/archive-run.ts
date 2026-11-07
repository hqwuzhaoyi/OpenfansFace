import fs from 'node:fs'
import path from 'node:path'
import { BASE } from './env'
import { writeJson } from './jsonio'
import type { ArchiveRun } from './archive-run-model'

export function validArchiveId(id:unknown):id is string{return typeof id==='string'&&/^[a-f0-9]{8}-[a-f0-9]{4}-4[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}$/i.test(id)}
function location(id:string){if(!validArchiveId(id))throw new Error('归档操作编号无效');return path.join(BASE,'archive-runs',id+'.json')}
export function readArchiveRun(id:string):ArchiveRun|null {
  try{return JSON.parse(fs.readFileSync(location(id),'utf8'))}
  catch(e){if((e as NodeJS.ErrnoException).code==='ENOENT')return null;throw e}
}
export function saveArchiveRun(run:ArchiveRun){const file=location(run.job.id);fs.mkdirSync(path.dirname(file),{recursive:true});writeJson(file,run)}
