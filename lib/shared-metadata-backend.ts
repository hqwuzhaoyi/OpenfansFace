import { randomUUID } from 'node:crypto'
import { nativeArchiveEnabled } from './archive-native-backend'
import { rustRequest } from './file-operation-backend'
import { invalidateIntegrationCache } from './integration'
export const nativeMetadataEnabled = nativeArchiveEnabled
/** Business writes only. External profile discovery remains a read-only source/cache. */
export async function nativeMetadata<T=Record<string,unknown>>(command:string,body:Record<string,unknown>):Promise<T>{
  const operation_id=body.operation_id||randomUUID()
  try {
    const result=await rustRequest<T>({command,body:{...body,operation_id}},undefined,'v1/metadata')
    invalidateIntegrationCache()
    return result
  } catch(error) {
    const failure=Object.assign(error as Error,{operation_id})
    if('status' in failure&&Number(failure.status)>=500)failure.message+=`（操作编号：${operation_id}）`
    throw failure
  }
}
