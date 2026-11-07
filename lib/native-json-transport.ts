/** Shared HTTP mechanics only; adapters retain receipt validation and domain effects. */
export interface NativeResponsePolicy {
  label:string
  // These synchronous adapters require a completed response. Acceptance alone
  // must never become a confirmation or a successful file-operation receipt.
  completion:'required'
  error:(message:string,status:number)=>Error
}
export async function nativeJsonRequest<T>(route:string,body:unknown,policy:NativeResponsePolicy,id?:string):Promise<T>{
  const configured=process.env.FILE_OPERATIONS_RUST_URL
  if(!configured)throw policy.error('Rust 文件操作服务地址未配置',503)
  let url:URL
  try{url=new URL(route,configured.endsWith('/')?configured:configured+'/');if(!['http:','https:'].includes(url.protocol))throw new Error()}
  catch{throw policy.error('Rust 文件操作服务地址无效',503)}
  if(id)url.searchParams.set('id',id)
  const headers:Record<string,string>={'Content-Type':'application/json'}
  if(process.env.FILE_OPERATIONS_RUST_TOKEN)headers.Authorization='Bearer '+process.env.FILE_OPERATIONS_RUST_TOKEN
  let response:Response
  try{response=await fetch(url,{method:body===null?'GET':'POST',headers,...(body===null?{}:{body:JSON.stringify(body)}),cache:'no-store',signal:AbortSignal.timeout(120_000)})}
  catch{throw policy.error(`${policy.label}后端连接中断，结果尚未确认；请查询原操作回执，不要重复提交`,503)}
  const value=await response.json().catch(()=>null)
  if(response.status===202&&policy.completion==='required')throw Object.assign(policy.error('操作已受理但尚未完成，请查询原操作回执',503),{code:'operation_pending'})
  if(!response.ok){
    const message=[typeof value?.error==='string'?value.error:`${policy.label}后端拒绝请求`,value?.step,value?.path].filter(part=>typeof part==='string'&&part).join(' · ')
    throw Object.assign(policy.error(message,response.status),{code:value?.code,path:value?.path,step:value?.step})
  }
  if(!value||typeof value!=='object'||Array.isArray(value))throw policy.error(`${policy.label}后端未返回可核对的回执`,502)
  return value as T
}
