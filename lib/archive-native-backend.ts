import { nativeJsonRequest } from './native-json-transport'
import fs from 'node:fs'
import path from 'node:path'
import { BASE } from './env'
import { validateFileOperationId } from './file-operation-routing'
import type { ArchiveRun } from './archive-run-model'
import { invalidateIntegrationCache } from './integration'
/** Thin transport only. Rust owns queue, metadata, media and Peach mutations. */
export function nativeArchiveEnabled():true { return true }
export async function nativeArchive(command:string,body:unknown={}):Promise<unknown>{
  const result=await request({command,body})
  if(!['archive-preview','review-page','review-status'].includes(command))invalidateIntegrationCache()
  return result
}
function legacyProgress(id:string|null):Pick<ArchiveRun,'job'|'result'>|null {
  if(!id){
    try { id=JSON.parse(fs.readFileSync(path.join(BASE,'jobs/archive.json'),'utf8')).id }
    catch(error){if((error as NodeJS.ErrnoException).code==='ENOENT')return null;throw error}
  }
  validateFileOperationId(id)
  try { const run=JSON.parse(fs.readFileSync(path.join(BASE,'archive-runs',id+'.json'),'utf8')) as ArchiveRun;return {job:run.job,result:run.result} }
  catch(error){if((error as NodeJS.ErrnoException).code==='ENOENT')return null;throw error}
}
export async function nativeArchiveProgress(id:string|null):Promise<Pick<ArchiveRun,'job'|'result'>|{job:null}> {
  const old=legacyProgress(id)
  if(id&&old)return old // Receipt read only; never dispatch an old ID to an executor.
  const current=await request(null,id||'') as Pick<ArchiveRun,'job'|'result'>|{job:null}
  if(old&&(!current.job||Number(old.job.started_at)>Number(current.job.started_at)))return old
  return current
}
async function request(payload:unknown,id?:string):Promise<unknown>{
  return nativeJsonRequest('v1/archive',payload,{label:'归档',completion:'required',error:(message,status)=>Object.assign(new Error(message),{status})},id)
}
