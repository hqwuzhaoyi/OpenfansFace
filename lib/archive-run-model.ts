import type { HomeJob } from './home-model'
export interface ArchiveFailure {vid:string;name:string;message:string}
export interface ArchiveResult {
  ok:boolean; moved:number; staged?:number; ledger?:number; remaining:number; errors:string[]
  failures?:ArchiveFailure[]; manifest?:string; recovery_needed?:boolean; rolled_back?:boolean
}
export interface ArchiveRun { fingerprint:string; job:HomeJob; result?:ArchiveResult }
