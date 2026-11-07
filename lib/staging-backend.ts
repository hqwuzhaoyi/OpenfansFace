import { randomUUID } from 'node:crypto'
import { BASE } from './env'
import { invalidateIntegrationCache } from './integration'
import { rustRequest } from './file-operation-backend'
import { claimFileOperation, fileOperationOwner, selectedPersonFileBackend, validateFileOperationId, FileOperationRoutingError } from './file-operation-routing'
import type { PersonFileMove, PersonFileResult } from './person-file-model'
export interface StagedFile {vid:string;path:string;name:string;size:number;version:string;origin:null|{original_path:string|null;person?:string;operation?:string;action?:string;source:string}}
export interface StagingList {files:number;gb:number;bytes:number;path:string;rows:StagedFile[];offset:number;next_offset:number|null}
export interface RestoreRequest {id:string;action:'restore-files';person:'';files:{vid:string;path:string;version:string}[];target_person?:string;target_folder?:string}
export interface RestorePlan {id:string;request:RestoreRequest;version:string;files:PersonFileMove[];target_person:string}
function enabled(){if(selectedPersonFileBackend('restore-files')!=='rust')throw new FileOperationRoutingError('暂存恢复需要启用 Rust 文件操作后端',503)}
export function stagingList(offset:string){enabled();if(!/^\d+$/.test(offset))throw new FileOperationRoutingError('分页位置无效',400);return rustRequest<StagingList>(null,undefined,'v1/staging?offset='+offset)}
export async function stagingOperation(mode:unknown,input:RestoreRequest,version:unknown){
  if(!input||input.action!=='restore-files'||!['preview','apply','reconcile'].includes(String(mode)))throw new FileOperationRoutingError('暂存区仅支持恢复预览、执行和核对',400)
  const request={...input,id:input.id||(mode==='preview'?randomUUID():'')}
  validateFileOperationId(request.id)
  if(mode==='reconcile'){
    const owner=fileOperationOwner(BASE,request.id)
    if(owner?.backend!=='rust'||owner.action!=='restore-files')throw new FileOperationRoutingError('找不到此恢复操作',404)
  }else{enabled();claimFileOperation(BASE,request.id,'restore-files','rust')}
  if(mode!=='preview'&&typeof version!=='string')throw new FileOperationRoutingError('请先预览恢复路径',400)
  const result=await rustRequest<RestorePlan|PersonFileResult|{id:string;status:string}>({mode,request,...(mode!=='preview'?{version}:{})})
  if(result.id!==request.id)throw new FileOperationRoutingError('恢复回执编号不匹配',502)
  if(mode!=='preview')invalidateIntegrationCache()
  return result
}
