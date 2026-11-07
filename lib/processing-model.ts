export type ProcessingStage = 'queued' | 'text' | 'face_queued' | 'face' | 'retry' | 'review' | 'error' | 'excluded'
export interface IdentityCandidate {
  name: string; sources: string[]; score: number | null
  evidence: { name: string; source: string; confirmed?: boolean; score?: number; support?: number; detail: string; thumb?: string; timestamp?: number; key?: string }[]
}
export interface ProcessingTask {
  vid: string; path: string; name: string; size: number; mtime_ns: string; duration?: number
  stage: ProcessingStage; retry_stage: string; text_attempts: number; face_attempts: number
  next_retry: number; priority: number; is_new: number; batch_id: string; created_at: number; updated_at: number
  error?: string; manual: number; deferred: number; note: string
  data: {
    watermark?: import('./watermark-model').WatermarkRecord
    folder?: { name: string; source: string; detail: string }[]
    candidates?: IdentityCandidate[]; kind?: 'none' | 'conflict' | 'suggested'; suggested?: string | null
    face_skipped?: boolean; text_complete?: boolean; face_complete?: boolean; ready?: boolean
    face?: { matches: unknown[]; face_count: number; thumbs: string[]; cache_reused: boolean; reference_people: number; sampled_frames: number | null; unknown_group?: string | null }
  }
}
export const stageLabel: Record<ProcessingStage, string> = { queued: '排队中', text: '文字提取中', face_queued: '等待人脸分析', face: '人脸分析中', retry: '等待重试', review: '待人工确认', error: '异常', excluded: '已移出' }
export function reviewBucket(task: ProcessingTask): 'suggested' | 'conflict' | 'none' | 'manual' {
  return task.manual ? 'manual' : task.data.kind || 'none'
}
export function reviewGroup(task: ProcessingTask) {
  const bucket = reviewBucket(task)
  if (bucket === 'suggested' && task.data.suggested) return { key: 'person:' + task.data.suggested, label: task.data.suggested, kind: bucket }
  if (bucket === 'none' && task.data.face?.unknown_group) return { key: 'face:' + task.data.face.unknown_group, label: '未命名相似组 ' + task.data.face.unknown_group.slice(-5), kind: bucket }
  return { key: bucket, label: { conflict: '证据冲突', none: '无候选人物', manual: '人工接管', suggested: '无候选人物' }[bucket], kind: bucket }
}
export function groupReviewTasks(tasks: ProcessingTask[]) {
  const groups = new Map<string, ReturnType<typeof reviewGroup> & { count: number }>()
  for (const task of tasks) {
    const group = reviewGroup(task), old = groups.get(group.key)
    groups.set(group.key, { ...group, count: (old?.count || 0) + 1 })
  }
  return [...groups.values()].sort((a, b) => Number(a.kind !== 'suggested') - Number(b.kind !== 'suggested') || b.count - a.count || a.label.localeCompare(b.label, 'zh-CN'))
}
