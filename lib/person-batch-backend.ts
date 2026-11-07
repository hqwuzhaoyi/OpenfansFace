import fs from 'node:fs'
import path from 'node:path'
import { randomUUID } from 'node:crypto'
import { BASE } from './env'
import { rustRequest } from './file-operation-backend'
import { claimFileOperation, fileOperationOwner, selectedPersonFileBackend, validateFileOperationId, FileOperationRoutingError } from './file-operation-routing'
import { previewPersonBatch, applyPersonBatch, personBatchProgress } from './person-batch-operations'
import { invalidateIntegrationCache } from './integration'
import type { PersonBatchRequest, PersonBatchPlan, PersonBatchProgress, PersonBatchResult } from './person-batch-model'
const route='v1/person-batch'
function legacyJournal(id:string){
  validateFileOperationId(id)
  try{return JSON.parse(fs.readFileSync(path.join(BASE,'person-operations/batches',id+'.json'),'utf8'))}
  catch(error){if((error as NodeJS.ErrnoException).code==='ENOENT')return null;throw error}
}
function owner(id:string){
  validateFileOperationId(id)
  const saved=fileOperationOwner(BASE,id)
  if(saved){if(saved.action!=='person-batch')throw new FileOperationRoutingError('操作编号不属于人物批次');return saved.backend}
  if(legacyJournal(id))return 'legacy'
  throw new FileOperationRoutingError('找不到此批次',404)
}
export async function previewPersonBatchOperation(request:PersonBatchRequest):Promise<PersonBatchPlan>{
  if(selectedPersonFileBackend('merge-person')==='legacy'){
    const plan=await previewPersonBatch(request);claimFileOperation(BASE,plan.id,'person-batch','legacy');return plan
  }
  const id=randomUUID();claimFileOperation(BASE,id,'person-batch','rust')
  const plan=await rustRequest<PersonBatchPlan>({mode:'preview',id,request},undefined,route)
  if(plan.id!==id||typeof plan.version!=='string'||!Array.isArray(plan.plans))throw new FileOperationRoutingError('批次预览回执不匹配',502)
  return plan
}
export async function applyPersonBatchOperation(id:string,version:string):Promise<PersonBatchResult>{
  if(typeof version!=='string')throw new FileOperationRoutingError('请先预览操作',400)
  if(owner(id)==='legacy'){
    const saved=legacyJournal(id)
    if(saved?.plan.version!==version)throw new FileOperationRoutingError('预览已失效，请重新预览')
    if(saved.status==='complete'&&saved.result)return saved.result
    if(selectedPersonFileBackend('merge-person')==='rust')throw new FileOperationRoutingError('旧批次未执行，请重新预览以使用 Rust 后端')
    return applyPersonBatch(id,version)
  }
  const result=await rustRequest<PersonBatchResult>({mode:'apply',id,version},undefined,route)
  if(typeof result.ok!=='boolean'||!Array.isArray(result.completed)||!Array.isArray(result.remaining))throw new FileOperationRoutingError('批次执行回执不完整，请查询原编号',502)
  invalidateIntegrationCache();return result
}
export async function personBatchOperationProgress(id:string):Promise<PersonBatchProgress>{
  if(owner(id)==='legacy')return personBatchProgress(id)
  const progress=await rustRequest<PersonBatchProgress>(null,id,route)
  if(progress.status==='complete')invalidateIntegrationCache()
  return progress
}

export async function reconcilePersonBatchOperation(id:string,version:string):Promise<PersonBatchResult>{
  if(owner(id)!=='rust')throw new FileOperationRoutingError('此批次没有 Rust 执行记录')
  if(typeof version!=='string')throw new FileOperationRoutingError('缺少原预览版本',400)
  const result=await rustRequest<PersonBatchResult>({mode:'reconcile',id,version},undefined,route)
  invalidateIntegrationCache();return result
}
