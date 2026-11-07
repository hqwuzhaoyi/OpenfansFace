import type { ReviewVideo } from './review-ui'

export interface WatermarkCandidate {
  key: string
  text: string
  token: string
  platform: string
  kind: 'account' | 'text'
  frames: number
  score: number
  timestamp: number
  evidence: string
  box?: number[]
  frame_size?: number[]
  timestamps?: number[]
}
export interface WatermarkRecord {
  vid: string
  path: string
  size: number
  mtime_ns: string
  pipeline: string
  job_id: string
  status: 'done' | 'error'
  error?: string
  checked_at: string
  sampled_frames: number
  candidates: WatermarkCandidate[]
}
export interface WatermarkMapping {
  key: string
  text: string
  token: string
  platform: string
  kind: 'person' | 'source'
  creator?: string
  confirmed_at: string
}
export interface WatermarkJob {
  id: string
  created_at: string
  force: boolean
  videos: { vid: string; path: string; size: number; mtime_ns: string }[]
}
export interface WatermarkProgress {
  job_id: string
  status: 'running' | 'complete' | 'error'
  processed: number
  total: number
  failed: number
  stage?: string
  error?: string
}
export interface WatermarkVideo extends ReviewVideo {
  watermark: WatermarkRecord | null
  suggested_creator: string | null
  conflict: boolean
  face_candidates: string[]
}
export interface WatermarkData {
  videos: WatermarkVideo[]
  mappings: Record<string, WatermarkMapping>
  worker: { online: boolean; error?: string; pipeline?: string }
  job: { id: string; total: number; created_at: string; vids: string[] } | null
  progress: WatermarkProgress | null
  active: boolean
}

export function watermarkMatches(record: WatermarkRecord | undefined, path: string, size: number, mtimeNs: string, pipeline?: string): record is WatermarkRecord {
  return !!record && record.path === path && record.size === size && record.mtime_ns === mtimeNs && (!pipeline || record.pipeline === pipeline)
}

/** OCR confidence measures text recognition, never the probability of an identity. */
export function watermarkSuggestion(candidates: WatermarkCandidate[], mappings: Record<string, WatermarkMapping>, faceCandidates: string[] = []) {
  const people = new Set(candidates.filter(c => c.frames >= 2).map(c => mappings[c.key])
    .filter(m => m?.kind === 'person' && m.creator).map(m => m.creator!))
  const faceConflict = people.size > 0 && faceCandidates.some(name => !people.has(name))
  return { suggested_creator: people.size === 1 && !faceConflict ? [...people][0] : null, conflict: people.size > 1 || faceConflict }
}

export function watermarkGroups(videos: WatermarkVideo[], mappings: Record<string, WatermarkMapping>) {
  const groups = new Map<string, { key: string; label: string; candidate?: WatermarkCandidate; mapping?: WatermarkMapping; videos: WatermarkVideo[] }>()
  for (const video of videos) {
    const candidates = video.watermark?.candidates || []
    const keys = candidates.length ? candidates.map(c => c.key) : [!video.watermark ? 'unscanned' : video.watermark.status === 'error' ? 'error' : 'empty']
    for (const key of keys) {
      if (!groups.has(key)) {
        const candidate = candidates.find(c => c.key === key)
        groups.set(key, { key, candidate, mapping: mappings[key], videos: [], label: candidate ? `${candidate.platform === 'unknown' ? '未知平台' : candidate.platform} · ${candidate.token}` : ({ unscanned: '待提取 / 文件已变化', error: '提取失败', empty: '未检出重复水印' }[key] || key) })
      }
      groups.get(key)!.videos.push(video)
    }
  }
  return [...groups.values()].sort((a, b) => Number(!!b.candidate) - Number(!!a.candidate) || b.videos.length - a.videos.length || a.label.localeCompare(b.label, 'zh-CN'))
}

export type WatermarkFilter = 'results' | 'unscanned' | 'empty' | 'error' | 'source'
/** Search inside a group narrows its selection scope to matching videos. */
export function filterWatermarkGroups(groups: ReturnType<typeof watermarkGroups>, filter: WatermarkFilter, query: string) {
  const search = query.trim().toLowerCase()
  return groups.filter(g => filter === 'results' ? !!g.candidate : filter === 'source' ? g.mapping?.kind === 'source' : g.key === filter)
    .map(group => {
      if (!search || [group.label, group.mapping?.creator || ''].some(s => s.toLowerCase().includes(search))) return group
      return { ...group, videos: group.videos.filter(v => `${v.name} ${v.path}`.toLowerCase().includes(search)) }
    }).filter(group => group.videos.length > 0)
}
