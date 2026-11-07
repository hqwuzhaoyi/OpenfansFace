import { invalidateIntegrationCache } from './integration'
/** Thin transport only. Rust owns queue, metadata, media and Peach mutations. */
export function nativeArchiveEnabled():boolean {
  const selected=process.env.FILE_OPERATIONS_BACKEND||'legacy'
  if(!['legacy','rust'].includes(selected))throw Object.assign(new Error('文件操作后端配置无效'),{status:503})
  if(selected==='rust'&&process.env.FILE_OPERATIONS_ENABLE_RUST!=='1')throw Object.assign(new Error('Rust 文件操作尚未启用'),{status:503})
  return selected==='rust'
}
export async function nativeArchive(command:string,body:unknown={}):Promise<unknown>{
  const result=await request({command,body})
  if(command!=='archive-preview')invalidateIntegrationCache()
  return result
}
export async function nativeArchiveProgress(id:string|null):Promise<unknown>{return request(null,id||'')}
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
