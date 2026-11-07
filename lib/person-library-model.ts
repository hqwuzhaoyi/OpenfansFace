import type { ReviewVideo } from './review-ui'

export interface LibraryPerson { name:string; rep_thumb:string|null; queued:number; recent_at?:string; aliases?:string[]; video_count?:number|null; total_bytes?:number|null; stats_error?:string; account_count?:number|null; account_error?:string }
export interface PersonDirectoryStats {video_count:number|null;total_bytes:number|null;stats_error?:string}
export interface PersonEvidence {
  kind:'report'|'face'|'watermark'; title:string; detail:string
  score?:number; faces?:number; timestamp?:number; image?:string
}
export interface PersonVideo extends ReviewVideo {
  path:string; folder:string; size:number; modified_at:string; version?:string
  evidence:PersonEvidence[]; conflict?:boolean
}
export interface PersonLibrary {
  name:string; root:string; files:PersonVideo[]; recommendations:PersonVideo[]
  folders:{name:string;count:number}[]; queued:number; directory_exists:boolean
}
export interface PersonRejection { id:string; name:string; vid:string; path:string; size:number; mtime_ns:string; at:string }

export interface PersonLibraryPreview {name:string;root:string;total_videos:number;total_bytes:number;files:PersonVideo[];next_offset:number|null}
