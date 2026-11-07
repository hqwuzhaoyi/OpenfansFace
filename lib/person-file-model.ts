export type PersonFileAction = 'rename-file'|'move-files'|'delete-files'|'rename-person'|'merge-person'|'delete-person'
export interface PersonFileRequest {
  id?:string; action:PersonFileAction; person:string
  files?:{vid:string;path:string;version:string}[]
  filename?:string; target_person?:string; target_folder?:string; new_name?:string
}
export interface PersonFileMove {vids:string[];src:string;dst:string;size:number;version:string;renamed?:boolean}
export interface PersonFilePlan {
  id:string;request:PersonFileRequest;version:string;action:PersonFileAction
  person:string;target_person:string;files:PersonFileMove[];total_bytes:number
  source_directory?:string;target_directory?:string;directories:string[]
  ledger_version:string;metadata_version:string
  affected_thumbs?:string[]
  directory_targets?:{src:string;dst:string}[]
  merge_summary?:{renamed_files:number;pending_tasks:number;manual_accounts:number;keep_target_template:boolean}
  source_aliases?:string[];target_aliases?:string[]
  delete_summary?:{pending_tasks:number;manual_accounts:number}
}
export interface PersonFileResult {ok:boolean;id:string;person:string;changed:number;staged:boolean;warning?:string}
