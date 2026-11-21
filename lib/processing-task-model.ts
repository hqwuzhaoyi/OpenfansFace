/**
 * Processing Task Queue & Dispatch Capability Model (Milestone 3, Issue #23)
 * Authoritative type definitions and Truth-in-UI formatters.
 */

export type TaskStage =
  | 'queued'
  | 'text'
  | 'face_queued'
  | 'face'
  | 'retry'
  | 'review'
  | 'error'
  | 'excluded'
  | 'detect'
  | 'ocr'
  | 'feature'
  | 'done'

export type TaskWaitingReason =
  | 'none'
  | 'waiting_for_node'
  | 'waiting_node'
  | 'quota_exhausted'
  | 'quota_limit'
  | 'gpu_busy'
  | 'rate_limited'
  | 'retry_backoff'
  | 'model_warmup'
  | 'paused'
  | 'unknown'
  | string

export interface TaskExecutionProof {
  node_id?: string | null
  execution_provider?: string | null
  duration_ms?: number | null
  detections_count?: number | null
  ocr_snippets_count?: number | null
  drift?: number | null
  timestamp?: number | null
  [key: string]: any
}

export interface TaskAttemptRecord {
  attempt_id: string
  node_id: string
  stage: string
  status: 'running' | 'done' | 'error' | 'interrupted' | 'completed' | 'failed' | string
  started_at: number
  finished_at?: number | null
  duration_ms?: number | null
  error?: string | null
  delivery_count: number
  redelivery_count?: number
  is_authoritative: boolean
}

export interface ProcessingTaskRecord {
  vid: string
  name: string
  path: string
  video_path?: string | null
  size: number | null
  duration?: number | null
  stage: TaskStage
  retry_stage: string
  text_attempts: number
  face_attempts: number
  next_retry: number
  priority: number
  is_new: number
  batch_id: string
  created_at: number
  updated_at: number
  error?: string | null
  manual: boolean
  deferred: boolean
  note: string

  // Milestone 3 Capability Dispatch & Native Receipt Fields
  assigned_node_id: string | null
  attempt_id: string | null
  lease_id: string | null
  coordinator_generation: string | number | null
  lease_expires_at: number | null
  duration_ms: number | null
  waiting_reason?: TaskWaitingReason | null
  execution_proof?: TaskExecutionProof | null

  // Milestone 4 Truth-in-UI Fields
  creator?: string | null
  face_skipped?: boolean | null
  ocr_result?: string | null
  error_type?: string | null
  matched_creator?: string | null
  ocr_text?: string[] | string | null
  skip_reason?: string | null

  // Milestone 5 Attempt History & Deduplication Fields
  attempts_history?: TaskAttemptRecord[]

  data?: {
    kind?: 'none' | 'conflict' | 'suggested'
    suggested?: string | null
    face_skipped?: boolean
    creator?: string | null
    matched_creator?: string | null
    skip_reason?: string | null
    ocr_result?: string | null
    ocr_text?: string[] | string | null
    error_type?: string | null
    watermark?: any
    face?: any
    detections?: any[]
    vectors?: any[]
    vectors_count?: number
  }
}

export interface TaskListApiResponse {
  schema_version: 1
  summary: {
    total: number
    all_count: number
    running_count: number
    waiting_count: number
    error_count: number
    review_count: number
    evaluated_at: number
  }
  counts: Record<string, number>
  filters: {
    stage?: string
    batch?: string
    node_id?: string
    q?: string
    waiting_reason?: string
  }
  page: number
  limit: number
  total: number
  tasks: ProcessingTaskRecord[]
  task?: ProcessingTaskRecord | null

  // Operational Context
  online?: boolean
  paused?: boolean
  review_available?: number
  all_count?: number
  batches?: { id: string; total: number; ready: number; error: number; running: number }[]
  events?: { id: number; at: number; kind: string; message: string }[]
  references?: null | { people: number }
  last_discovery?: null | { at?: number; attempted_at?: number; registered?: number; eligible?: number; error?: string }
}

export interface TaskListQueryParams {
  stage?: string
  batch?: string
  node_id?: string
  waiting_reason?: string
  q?: string
  page?: string | number
  limit?: string | number
  vid?: string
}

export const TASK_STAGE_LABELS: Record<string, string> = {
  queued: '排队中',
  text: '文字提取中',
  face_queued: '等待人脸分析',
  face: '人脸分析中',
  retry: '等待重试',
  review: '待人工确认',
  error: '异常',
  excluded: '已移出',
  detect: '人脸检测中',
  ocr: '文字识别中',
  feature: '特征提取中',
  done: '已完成',
}

export const TASK_ERROR_LABELS: Record<string, string> = {
  parse_failure: '图片解析损坏',
  model_error: 'OCR 引擎异常',
  no_text_found: '未检出文字',
  timeout: '租约超时',
  capability_mismatch: '设备能力不匹配',
  quota_exceeded: '配额超限',
}

export const TASK_WAITING_REASON_LABELS: Record<string, { label: string; description: string }> = {
  none: { label: '无等待', description: '任务正常流转中' },
  waiting_for_node: { label: '等待计算节点', description: '当前无空闲或满足硬件能力要求的计算节点' },
  waiting_node: { label: '等待计算节点', description: '当前无空闲或满足硬件能力要求的计算节点' },
  quota_exhausted: { label: '并发配额超限', description: '已达到全局并发上限或物理设备配额阶梯' },
  quota_limit: { label: '并发配额超限', description: '已达到全局并发上限或物理设备配额阶梯' },
  gpu_busy: { label: 'GPU 显存繁忙', description: '显存或算力不足以承载新检测任务' },
  rate_limited: { label: '并发降频限流', description: '系统正在执行优雅并发控制，在途任务完成后方可领取' },
  retry_backoff: { label: '重试退避等待', description: '前次执行失败，处于退避等待窗口' },
  model_warmup: { label: '模型预热自检', description: '计算节点正在执行启动预热与基准漂移校验' },
  paused: { label: '人工暂停接单', description: '操作员已暂停调度' },
  unknown: { label: '未知等待原因', description: '未知等待原因' },
}

export function formatWaitingReason(reason?: string | null): string {
  if (!reason || reason === 'none') return '无等待'
  const trimmed = reason.trim()
  if (!trimmed) return '无等待'
  return TASK_WAITING_REASON_LABELS[trimmed as TaskWaitingReason]?.label || '未知等待原因'
}

export const formatTaskWaitingReason = formatWaitingReason

export function formatWaitingReasonDescription(reason?: string | null): string {
  if (!reason || reason === 'none') return '任务正常流转中'
  const trimmed = reason.trim()
  if (!trimmed) return '任务正常流转中'
  return TASK_WAITING_REASON_LABELS[trimmed as TaskWaitingReason]?.description || '未知等待原因'
}

export const formatTaskWaitingReasonDescription = formatWaitingReasonDescription

// Truth-in-UI Formatting Functions

export function formatTaskDuration(durationMs?: number | null): string {
  if (
    durationMs === null ||
    durationMs === undefined ||
    !Number.isFinite(durationMs) ||
    durationMs <= 0
  ) {
    return '未知'
  }
  if (durationMs < 1000) {
    return `${Math.round(durationMs)} ms`
  }
  const sec = durationMs / 1000
  if (sec < 60) {
    return `${sec.toFixed(1)} 秒`
  }
  return `${Math.floor(sec / 60)} 分 ${Math.floor(sec % 60)} 秒`
}

export function formatLeaseRemaining(
  leaseExpiresAt?: number | null,
  nowSec?: number,
): string {
  if (
    !leaseExpiresAt ||
    !Number.isFinite(leaseExpiresAt) ||
    leaseExpiresAt <= 0
  ) {
    return '无有效租约'
  }
  const now = nowSec ?? Date.now() / 1000
  const remaining = leaseExpiresAt - now
  if (remaining <= 0) {
    return '租约已过期'
  }
  if (remaining < 60) {
    return `剩 ${Math.ceil(remaining)} 秒`
  }
  return `剩 ${Math.floor(remaining / 60)} 分 ${Math.ceil(remaining % 60)} 秒`
}

export function getNodeDisplay(nodeId?: string | null): string {
  if (!nodeId || typeof nodeId !== 'string' || !nodeId.trim()) {
    return '未知'
  }
  return nodeId.trim()
}

export function getAttemptDisplay(attemptId?: string | null): string {
  if (!attemptId || typeof attemptId !== 'string' || !attemptId.trim()) {
    return '未知'
  }
  const trimmed = attemptId.trim()
  return trimmed.length > 8 ? trimmed.slice(0, 8) : trimmed
}

export function formatCreatorHandle(name?: string | null): string {
  if (!name) return ''
  const trimmed = String(name).trim()
  if (!trimmed) return ''
  return trimmed.startsWith('@') ? trimmed : `@${trimmed}`
}

export function formatCreatorBadge(creator?: string | null): string {
  const handle = formatCreatorHandle(creator)
  return handle ? `已匹配作者: ${handle} (跳过人脸分析)` : ''
}

export function formatErrorType(errorType?: string | null): string {
  if (!errorType) return '未知错误'
  const trimmed = String(errorType).trim()
  return TASK_ERROR_LABELS[trimmed] || trimmed || '未知错误'
}

export function isOcrFailureType(errorType?: string | null): boolean {
  if (!errorType) return false
  const trimmed = String(errorType).trim()
  return trimmed === 'parse_failure' || trimmed === 'model_error'
}

export function formatOcrFailureBadge(stage?: string | null): string {
  if (stage === 'retry') return 'OCR 解析失败 (重试中)'
  if (stage === 'error') return 'OCR 解析失败 (重试已耗尽)'
  return 'OCR 解析失败'
}

export const formatOcrFailureStatus = formatOcrFailureBadge

export function formatAttemptStatus(status?: string | null): {
  isDone: boolean
  isInterrupted: boolean
  isError: boolean
  label: '已完成' | '中断回收' | '失败' | '运行中'
  className: string
} {
  const norm = (status || '').toLowerCase().trim()
  const isDone = norm === 'done' || norm === 'completed'
  const isInterrupted = norm === 'interrupted'
  const isError = norm === 'error' || norm === 'failed'
  const label = isDone ? '已完成' : isInterrupted ? '中断回收' : isError ? '失败' : '运行中'
  const className = isDone
    ? 'bg-emerald-500/10 text-emerald-600 dark:text-emerald-400'
    : isInterrupted
    ? 'bg-amber-500/10 text-amber-600 dark:text-amber-400'
    : isError
    ? 'bg-destructive/10 text-destructive'
    : 'bg-muted text-muted-foreground'
  return { isDone, isInterrupted, isError, label, className }
}

export const normalizeAttemptStatus = formatAttemptStatus

export function formatAttemptStage(stage?: string | null): string {
  if (!stage || typeof stage !== 'string' || !stage.trim()) return '未知'
  const trimmed = stage.trim()
  return TASK_STAGE_LABELS[trimmed] || trimmed
}

export function getAttemptDeduplicationCount(att?: {
  delivery_count?: number | null
  redelivery_count?: number | null
} | null): number {
  if (!att) return 1
  const del =
    typeof att.delivery_count === 'number' && Number.isFinite(att.delivery_count)
      ? Math.max(0, Math.round(att.delivery_count))
      : 0
  const redel =
    typeof att.redelivery_count === 'number' && Number.isFinite(att.redelivery_count)
      ? Math.max(0, Math.round(att.redelivery_count))
      : 0
  return Math.max(del, redel)
}

export const getAttemptDedupCount = getAttemptDeduplicationCount
