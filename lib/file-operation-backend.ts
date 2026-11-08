import { nativeJsonRequest } from './native-json-transport'
import fs from 'node:fs'
import path from 'node:path'
import { randomUUID } from 'node:crypto'
import { BASE } from './env'
import { invalidateIntegrationCache } from './integration'
import { fileOperationOwner, selectedPersonFileBackend, validateFileOperationId, FileOperationRoutingError, type FileOperationBackendName } from './file-operation-routing'
import type { PersonFileRequest, PersonFilePlan, PersonFileResult } from './person-file-model'

export interface PersonFileOperationStatus {
  id:string; backend:FileOperationBackendName; status:string
  result?:PersonFileResult; error?:string; revision?:number; phase?:string; io_state?:string
}
interface PersonFileBackend {
  preview(request:PersonFileRequest):Promise<PersonFilePlan>
  apply(request:PersonFileRequest,version:string):Promise<PersonFileResult>
  status(id:string):Promise<PersonFileOperationStatus>
}
const legacy:PersonFileBackend={
  async preview(){throw new FileOperationRoutingError('旧执行器已退役，请重新预览',410)},
  async apply(){throw new FileOperationRoutingError('旧执行器已退役，请核对历史回执',410)},
  async status(id){
    try{
      const operation=JSON.parse(fs.readFileSync(path.join(BASE,'person-operations',id,'operation.json'),'utf8'))
      return {id,backend:'legacy',status:operation.status,result:operation.result,error:operation.error}
    }catch(e){if((e as NodeJS.ErrnoException).code==='ENOENT')return {id,backend:'legacy',status:'not_started'};throw e}
  },
}
export async function rustRequest<T>(body:unknown,id?:string,route="v1/person-files"):Promise<T>{
  return nativeJsonRequest<T>(route,body,{label:'文件操作',completion:'required',error:(message,status)=>new FileOperationRoutingError(message,status)},id)
}
const rust:PersonFileBackend={
  async preview(request){
    const plan=await rustRequest<PersonFilePlan>({mode:'preview',request})
    if(plan.id!==request.id||plan.request?.id!==request.id||plan.action!==request.action||typeof plan.version!=='string')throw new FileOperationRoutingError('预览回执与请求不匹配',502)
    return {...plan,backend:'rust' as const}
  },
  async apply(request,version){
    const result=await rustRequest<PersonFileResult>({mode:'apply',request,version})
    if(result.id!==request.id||typeof result.ok!=='boolean')throw new FileOperationRoutingError('执行结果与操作编号不匹配，请核对原操作',502)
    if(result.ok)invalidateIntegrationCache()
    return result
  },
  async status(id){
    const status=await rustRequest<PersonFileOperationStatus>(null,id)
    if(status.id!==id||typeof status.status!=='string')throw new FileOperationRoutingError('查询结果与操作编号不匹配',502)
    if(status.status==='complete'||status.status==='rolled_back')invalidateIntegrationCache()
    return {...status,backend:'rust'}
  },
}
const backends={legacy,rust}
function dispatch(request:PersonFileRequest):PersonFileBackend {
  if(!request||typeof request!=='object'||typeof request.action!=='string')throw new FileOperationRoutingError('缺少操作请求',400)
  validateFileOperationId(request.id)
  const backend=selectedPersonFileBackend(request.action)
  const owner=fileOperationOwner(BASE,request.id)
  if(owner && (owner.backend!==backend||owner.action!==request.action))throw new FileOperationRoutingError('旧操作不能转交新执行器，请重新预览')
  return backends[backend]
}
export function previewPersonFileOperation(request:PersonFileRequest){
  if(!request||typeof request!=='object')throw new FileOperationRoutingError('缺少操作请求',400)
  const identified={...request,id:request.id||randomUUID()}
  return dispatch(identified).preview(identified)
}
export function applyPersonFileOperation(request:PersonFileRequest,version:string){
  if(typeof version!=='string')throw new FileOperationRoutingError('请先预览操作',400)
  return dispatch(request).apply(request,version)
}
export function personFileOperationStatus(id:string){
  validateFileOperationId(id)
  const owner=fileOperationOwner(BASE,id)
  if(!owner)throw new FileOperationRoutingError('找不到此操作',404)
  // Result lookup follows persisted ownership even after configuration changes; it never executes.
  return backends[owner.backend].status(id)
}

export async function reconcilePersonFileOperation(request:PersonFileRequest,version:string){
  validateFileOperationId(request.id)
  const owner=fileOperationOwner(BASE,request.id!)
  if(owner?.backend!=='rust'||owner.action!==request.action)throw new FileOperationRoutingError('此操作没有 Rust 执行记录',409)
  const status=await rustRequest<PersonFileOperationStatus>({mode:'reconcile',request,version})
  if(status.id!==request.id)throw new FileOperationRoutingError('核对回执编号不匹配',502)
  if(status.status==='complete'||status.status==='rolled_back')invalidateIntegrationCache()
  return {...status,backend:'rust' as const}
}
