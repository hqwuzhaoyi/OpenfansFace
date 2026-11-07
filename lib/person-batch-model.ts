import type { PersonFilePlan, PersonFileResult } from './person-file-model'

export type PersonBatchAction = 'merge' | 'delete'
export interface PersonBatchRequest {action:PersonBatchAction;people:string[];target?:string}
export interface PersonBatchPlan {id:string;version:string;request:PersonBatchRequest;plans:PersonFilePlan[]}
export interface PersonBatchResult {
  ok:boolean;completed:{person:string;result:PersonFileResult}[];remaining:string[];target?:string;error?:string;recovery_needed?:boolean
}

export interface PersonBatchProgress {status:'preview'|'running'|'complete';completed:string[];remaining:string[];active_person?:string;error?:string;recovery_needed?:boolean;result?:PersonBatchResult}
