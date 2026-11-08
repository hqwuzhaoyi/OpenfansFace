import { randomUUID } from 'node:crypto'
import { BASE } from './env'
import { invalidateIntegrationCache } from './integration'
import { rustRequest, personFileOperationStatus } from './file-operation-backend'
import { fileOperationOwner, selectedPersonFileBackend, validateFileOperationId, FileOperationRoutingError } from './file-operation-routing'
import type { PersonFileMove, PersonFileResult } from './person-file-model'
export interface StagedFile {vid:string;path:string;name:string;size:number;version:string;origin:null|{original_path:string|null;person?:string;operation?:string;action?:string;source:string}}
export interface StagingList {files:number;matched:number;gb:number;bytes:number;path:string;rows:StagedFile[];offset:number;next_offset:number|null}
export interface RestoreRequest {id:string;action:'restore-files';person:'';files:{vid:string;path:string;version:string}[];target_person?:string;target_folder?:string}
export interface DeleteRequest {id:string;action:'delete-staged'|'clear-staging';person:'';files:{vid:string;path:string;version:string}[]}
export interface RestorePlan {id:string;request:RestoreRequest;version:string;files:PersonFileMove[];target_person:string}
export interface DeletePlan {id:string;request:DeleteRequest;version:string;files:PersonFileMove[];total_bytes:number}
export interface DeleteResult {id:string;ok:boolean;status:'complete'|'partial';completed:number;total:number;files:{file_index:number;src:string;status:string;error?:{error?:string}|null}[];warning?:string}
function enabled(){if(selectedPersonFileBackend('restore-files')!=='rust')throw new FileOperationRoutingError('暂存操作需要启用 Rust 文件操作后端',503)}
export function stagingList(offset:string,query=''){enabled();if(!/^\d+$/.test(offset))throw new FileOperationRoutingError('分页位置无效',400);return rustRequest<StagingList>(null,undefined,'v1/staging?offset='+offset+'&q='+encodeURIComponent(query))}
export async function stagingStatus(id:string){
  validateFileOperationId(id)
  if(selectedPersonFileBackend('restore-files')==='rust')return rustRequest(null,undefined,'v1/staging?id='+id)
  return personFileOperationStatus(id)
}
export async function stagingOperation(mode:unknown,input:RestoreRequest|DeleteRequest,version:unknown,confirmation:unknown){
  if(!input||!['restore-files','delete-staged','clear-staging'].includes(input.action)||!['preview','apply','reconcile'].includes(String(mode)))throw new FileOperationRoutingError('暂存操作无效',400)
  const request={...input,id:input.id||(mode==='preview'?randomUUID():'')}
  validateFileOperationId(request.id)
  if(mode==='reconcile'){
    const owner=fileOperationOwner(BASE,request.id)
    if(owner?.backend!=='rust'||owner.action!==request.action)throw new FileOperationRoutingError('找不到此暂存操作',404)
  }else{enabled();const owner=fileOperationOwner(BASE,request.id);if(owner&&(owner.backend!=='rust'||owner.action!==request.action))throw new FileOperationRoutingError('旧操作请核对原回执并重新预览')}
  if(mode!=='preview'&&typeof version!=='string')throw new FileOperationRoutingError('请先核对预览范围',400)
  if(mode==='apply'&&request.action!=='restore-files'&&confirmation!==(request.action==='clear-staging'?'清空回收站':'永久删除'))throw new FileOperationRoutingError('请输入完整确认文字',400)
  const result=await rustRequest<RestorePlan|DeletePlan|PersonFileResult|DeleteResult|{id:string;status:string}>({mode,request,...(mode!=='preview'?{version}:{}),...(mode==='apply'&&request.action!=='restore-files'?{confirmation}:{})})
  if(result.id!==request.id)throw new FileOperationRoutingError('暂存回执编号不匹配',502)
  if(mode!=='preview')invalidateIntegrationCache()
  return result
}
