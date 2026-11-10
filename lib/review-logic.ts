export type ReviewSource = 'cluster' | 'person' | 'approval' | 'all' | 'watermark' | 'review' | 'legacy'
export type ArchiveResolution = { kind: 'rename'; name: string } | { kind: 'stage-duplicate' }
export interface QueueItem {
  vid: string; creator: string; at: string
  source?: ReviewSource; source_id?: string; return_href?: string; batch_id?: string
  resolution?: ArchiveResolution
  note?: string
}
export interface Decisions { approved: string[]; rejected: string[] }
export interface ArchivedItem extends QueueItem { dst: string; win_dst: string; manifest: string; staged_path?: string }
export class ReviewError extends Error {
  status: number
  constructor(message: string, status = 400) { super(message); this.status = status }
}
export function validateCreator(value: unknown): string {
  if (typeof value !== 'string' || !value.trim() || value.trim().length > 120 || /[\\/\x00-\x1f]/.test(value) || ['.', '..'].includes(value.trim())) {
    throw new ReviewError('请输入有效人物名，不能包含路径分隔符')
  }
  return value.trim()
}
export function validateVids(value: unknown): string[] {
  if (!Array.isArray(value) || !value.length || value.length > 5000 || value.some(v => typeof v !== 'string' || !v || v.includes('\0'))) throw new ReviewError('请选择要处理的视频')
  return [...new Set(value)]
}
export function origin(source: ReviewSource, sourceId: string): Pick<QueueItem, 'source' | 'source_id' | 'return_href'> {
  if (source === 'cluster' && !/^p\d+$/.test(sourceId)) throw new ReviewError('相似组编号无效')
  return { source, source_id: sourceId, return_href: source === 'review' ? '/review' : source === 'watermark' ? '/watermarks' : source === 'all' ? '/unclassified' : source === 'approval' ? `/approvals${sourceId ? `?vid=${encodeURIComponent(sourceId)}` : ''}`
    : source === 'person' ? `/persons/${encodeURIComponent(sourceId)}`
    : source === 'cluster' ? `/clusters?cid=${encodeURIComponent(sourceId)}` : '/clusters' }
}
function uuid(): string {
  if (typeof globalThis.crypto?.randomUUID === 'function') return globalThis.crypto.randomUUID()
  const bytes = globalThis.crypto.getRandomValues(new Uint8Array(16))
  bytes[6] = (bytes[6] & 15) | 64
  bytes[8] = (bytes[8] & 63) | 128
  const hex = [...bytes].map(b => b.toString(16).padStart(2, '0')).join('')
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`
}
export function enqueueItems(queue: QueueItem[], creator: string, vids: string[], provenance: ReturnType<typeof origin>) {
  const existing = new Map(queue.map(v => [v.vid, v]))
  const conflicts = vids.filter(v => existing.has(v) && existing.get(v)!.creator !== creator)
  if (conflicts.length) throw new ReviewError(`${conflicts.length} 个视频已归给其他人物，请先到待归档撤销`, 409)
  const batch_id = uuid()
  const added: QueueItem[] = vids.filter(v => !existing.has(v)).map(vid => ({ vid, creator, at: new Date().toISOString(), batch_id, ...provenance }))
  return { queue: [...queue, ...added], added }
}
export function undoItems(queue: QueueItem[], decisions: Decisions, vids: string[]) {
  const ids = new Set(vids)
  const removed = queue.filter(v => ids.has(v.vid))
  const restored = new Set(removed.map(v => v.vid))
  return { queue: queue.filter(v => !ids.has(v.vid)), removed,
    decisions: { approved: decisions.approved.filter(v => !restored.has(v)), rejected: decisions.rejected.filter(v => !restored.has(v)) } }
}
export function decodedName(value: string): string {
  try { return decodeURIComponent(value) } catch { return value }
}
