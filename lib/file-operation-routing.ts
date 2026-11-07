import fs from 'node:fs'
import path from 'node:path'

export type FileOperationBackendName = 'legacy' | 'rust'
export interface FileOperationOwner { id:string; backend:FileOperationBackendName; action:string }
export class FileOperationRoutingError extends Error {
  status:number
  constructor(message:string,status=409){super(message);this.status=status}
}
export function validateFileOperationId(value:unknown):asserts value is string {
  if(typeof value!=='string'||!/^[a-f0-9]{8}-[a-f0-9]{4}-4[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}$/i.test(value))throw new FileOperationRoutingError('操作编号无效',400)
}

/** Migrated file capabilities opt into the same Rust authority; other actions retain their owner. */
export function selectedPersonFileBackend(action:string,env:NodeJS.ProcessEnv=process.env):FileOperationBackendName {
  const selected=env.FILE_OPERATIONS_BACKEND||'legacy'
  if(selected!=='legacy'&&selected!=='rust')throw new FileOperationRoutingError('文件操作后端配置无效',503)
  if(!['rename-file','move-files','delete-files','restore-files','delete-staged','clear-staging','rename-person'].includes(action)||selected==='legacy')return 'legacy'
  if(env.FILE_OPERATIONS_ENABLE_RUST!=='1')throw new FileOperationRoutingError('Rust 文件操作尚未启用',503)
  return 'rust'
}
function ownerPath(base:string,id:string){validateFileOperationId(id);return path.join(base,'file-operation-routing',id+'.json')}

export function fileOperationOwner(base:string,id:string):FileOperationOwner|null {
  const file=ownerPath(base,id)
  let value:unknown
  try{value=JSON.parse(fs.readFileSync(file,'utf8'))}
  catch(e){
    if((e as NodeJS.ErrnoException).code!=='ENOENT')throw e
    try {
      const old=JSON.parse(fs.readFileSync(path.join(base,'staging-operations',id,'operation.json'),'utf8'))
      return {id,backend:'legacy',action:old.plan?.action||'staging'}
    } catch(error) {if((error as NodeJS.ErrnoException).code!=='ENOENT')throw error}
    // Already executed operations predate routing records and must remain owned by legacy.
    try{
      const old=JSON.parse(fs.readFileSync(path.join(base,'person-operations',id,'operation.json'),'utf8'))
      value={id,backend:'legacy',action:old.plan?.action||old.plan?.request?.action}
    }catch(error){if((error as NodeJS.ErrnoException).code==='ENOENT')return null;throw error}
  }
  const owner=value as FileOperationOwner
  if(!owner||owner.id!==id||!['legacy','rust'].includes(owner.backend)||typeof owner.action!=='string')throw new FileOperationRoutingError('操作执行归属记录损坏，请核对后重试',503)
  return owner
}

/** An immutable, exclusively created owner record prevents dispatching one ID to two executors. */
export function claimFileOperation(base:string,id:string,action:string,backend:FileOperationBackendName):FileOperationOwner {
  const file=ownerPath(base,id)
  const check=(owner:FileOperationOwner)=>{
    if(owner.backend!==backend||owner.action!==action)throw new FileOperationRoutingError('此操作已绑定另一执行后端或操作类型，请核对原结果后重新预览')
    return owner
  }
  const existing=fileOperationOwner(base,id)
  if(existing)return check(existing)
  const owner={id,backend,action}
  fs.mkdirSync(path.dirname(file),{recursive:true})
  try{fs.writeFileSync(file,JSON.stringify(owner),{flag:'wx'})}
  catch(e){if((e as NodeJS.ErrnoException).code!=='EEXIST')throw e;return check(fileOperationOwner(base,id)!)}
  return owner
}
