export interface ReviewVideo {
  vid: string; name: string; path?: string; size_gb?: number; duration?: number
  thumb: string | null; conf?: string; state?: string; queued_creator?: string; organized?: boolean
}
export function fileName(p: string) { return p.replace(/\\/g, '/').split('/').pop() || p }
export function directoryOf(p: string) { const i = Math.max(p.lastIndexOf('/'), p.lastIndexOf('\\')); return i < 0 ? '未记录目录' : p.slice(0, i) }
export function videoGroups(videos: ReviewVideo[]) {
  const groups = new Map<string, ReviewVideo[]>()
  for (const v of videos) { const dir = directoryOf(v.path || v.vid); groups.set(dir, [...(groups.get(dir) || []), v]) }
  return [...groups.entries()].sort((a, b) => b[1].length - a[1].length)
}
export const sourceLabel: Record<string, string> = { all: '全部待分类', cluster: '相似视频组', person: '人物页确认', approval: '识别待确认', legacy: '旧队列' }
