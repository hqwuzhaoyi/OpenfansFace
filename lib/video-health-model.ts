export interface VideoHealthRecord {
  vid: string; path: string | null; status: string; size?: number | null; mtime_ns?: string | null
  hidden?: boolean; consecutive?: number; detail?: string; codec?: string; in_review?: boolean
  first_failure_at?: string; last_checked_at?: string; last_probed_at?: string
}
export interface VideoHealthReport {
  schema?: number; status?: string; records: Record<string, VideoHealthRecord>
  started_at?: string; completed_at?: string; last_success_at?: string; next_run_at?: string; schedule?: string
  counts?: Record<string, number>; hidden?: number; total?: number; waiting?: number; probed?: number; error?: string
}
export const healthTitles: Record<string,string> = {
  missing:'找不到源文件',not_file:'记录指向文件夹',empty:'文件是空的',corrupt:'视频结构损坏',
  unavailable:'暂时无法读取',unmapped:'路径需要核对',compatibility:'手机编码兼容性需核对',
  playback_warning:'播放失败，结构检查正常',unchecked:'等待结构检查',healthy:'结构检查正常',present:'等待结构检查',
}
/** A replacement or relocated file becomes visible immediately, before the next scheduled scan. */
export function healthMatches(record: VideoHealthRecord | undefined, path: string, size: number, mtimeNs: string): boolean {
  return !!record?.hidden && record.path === path && record.size === size && record.mtime_ns === mtimeNs
}
