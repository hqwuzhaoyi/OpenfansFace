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
  const configured=process.env.FILE_OPERATIONS_RUST_URL
  if(!configured)throw Object.assign(new Error('Rust 文件操作服务地址未配置'),{status:503})
  let url:URL
  try{url=new URL('v1/archive',configured.endsWith('/')?configured:configured+'/');if(!['http:','https:'].includes(url.protocol))throw new Error()}
  catch{throw Object.assign(new Error('Rust 文件操作服务地址无效'),{status:503})}
  if(id)url.searchParams.set('id',id)
  const headers:Record<string,string>={'Content-Type':'application/json'}
  if(process.env.FILE_OPERATIONS_RUST_TOKEN)headers.Authorization='Bearer '+process.env.FILE_OPERATIONS_RUST_TOKEN
  let response:Response
  try{response=await fetch(url,{method:id===undefined?'POST':'GET',headers,...(id===undefined?{body:JSON.stringify(payload)}:{}),cache:'no-store',signal:AbortSignal.timeout(120_000)})}
  catch{throw Object.assign(new Error('后端连接中断，结果尚未确认；请查询原操作回执'),{status:503})}
  const value=await response.json().catch(()=>null)
  if(!response.ok)throw Object.assign(new Error(value?.error||'归档后端拒绝请求'),{status:response.status,code:value?.code,path:value?.path,step:value?.step})
  if(!value||typeof value!=='object')throw Object.assign(new Error('归档后端未返回可核对的结果'),{status:502})
  return value
}
