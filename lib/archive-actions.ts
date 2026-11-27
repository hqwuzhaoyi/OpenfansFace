import type { PlanRow } from './review-plan'

export type AutoWatermarkMatchTier = 'confirmed_rule' | 'direct_handle'

export interface AutoWatermarkPreviewItem {
  vid: string
  name: string
  path: string
  size: number
  mtime_ns: string | number
  target_name: string
  renamed_keep_both: boolean
  match_tier: AutoWatermarkMatchTier | string
  watermark_text: string
  watermark_platform?: string | null
  watermark_key?: string | null
}

export interface AutoWatermarkPreviewGroup {
  person: string
  directory: string
  existing_files: number
  count: number
  items: AutoWatermarkPreviewItem[]
}

export interface AutoWatermarkSkippedSummary {
  no_existing_person: number
  face_contradicts: number
  multi_watermark: number
  folder_conflict: number
  space_truncated: number
  other: number
}

export interface AutoWatermarkPreview {
  ok: boolean
  version: string
  eligible_count: number
  group_count: number
  groups: AutoWatermarkPreviewGroup[]
  skipped_summary?: AutoWatermarkSkippedSummary
}

export interface AutoWatermarkApplyItemInput {
  vid: string
  person: string
  path: string
  size: number
  mtime_ns: string | number
}

export interface AutoWatermarkApplyItemResult {
  vid: string
  person: string
  status: 'archived' | 'fallback_queued' | 'skipped' | 'failed' | string
  dst?: string | null
  renamed_keep_both?: boolean
  reason?: string | null
}

export interface AutoWatermarkApplyResult {
  ok: boolean
  id?: string
  archived: number
  moved?: number
  fallback_queued: number
  skipped: number
  results: AutoWatermarkApplyItemResult[]
  errors?: string[]
  recovery_needed?: boolean
  rolled_back?: boolean
}

export interface UnarchiveResult {
  ok: boolean
  id?: string
  vid: string
  person: string
  restored_path: string
  renamed_keep_both?: boolean
  auto_opt_out: boolean
}

export function autoMatchTierLabel(tier: string): string {
  if (tier === 'confirmed_rule') return '已确认规则'
  if (tier === 'direct_handle') return '直连账号名'
  return tier || '自动水印'
}

export function autoArchiveReasonLabel(reason?: string | null): string | null {
  if (!reason) return null
  if (reason === 'confirmed_rule') return '由已确认水印规则自动合入人物目录并同步 Peach 账本。'
  if (reason === 'direct_handle') return '由直连账号水印自动合入人物目录并同步 Peach 账本。'
  if (reason === 'fallback_queue') return '自动水印转待归档后已完成移动与 Peach 账本更新。'
  return reason
}

export function buildAutoMergeApplyItems(preview: AutoWatermarkPreview): AutoWatermarkApplyItemInput[] {
  return preview.groups.flatMap(group =>
    group.items.map(item => ({
      vid: item.vid,
      person: group.person,
      path: item.path,
      size: item.size,
      mtime_ns: item.mtime_ns,
    })),
  )
}

export function canUnarchiveHistoryItem(
  item?: {
    original_path?: string | null
    undone?: boolean
    operation?: string | null
    staged_path?: string | null
  } | null,
): boolean {
  return Boolean(
    item &&
      item.original_path &&
      !item.undone &&
      item.operation !== 'stage-duplicate' &&
      !item.staged_path,
  )
}

export function formatAutoMergeSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return '0 MB'
  if (bytes >= 2 ** 30) return `${(bytes / 2 ** 30).toFixed(2)} GB`
  return `${(bytes / 2 ** 20).toFixed(1)} MB`
}

async function requestJson<T>(url: string, init?: RequestInit): Promise<T> {
  const res = await fetch(url, {
    ...init,
    headers: { 'Content-Type': 'application/json', ...init?.headers },
  })
  const payload = await res.json().catch(() => ({}))
  if (!res.ok) {
    throw Object.assign(new Error(payload?.error || `请求失败（${res.status}）`), {
      status: res.status,
      code: payload?.code,
    })
  }
  return payload as T
}

export function fetchAutoMergePreview(signal?: AbortSignal): Promise<AutoWatermarkPreview> {
  return requestJson<AutoWatermarkPreview>('/api/review/auto-merge/preview', { signal })
}

export function requestUnarchive(input: {
  vid: string
  op_id?: string
  operation_id?: string
}): Promise<UnarchiveResult> {
  return requestJson<UnarchiveResult>('/api/plan/unarchive', {
    method: 'POST',
    body: JSON.stringify(input),
  })
}

export function canResolveArchiveRow(row: Pick<PlanRow, 'status' | 'resolution'>) {
  return ['duplicate', 'conflict', 'dupe-target'].includes(row.status) || (row.status === 'ok' && !!row.resolution)
}
export function canStageArchiveRow(row: Pick<PlanRow, 'status' | 'resolution' | 'size' | 'original_size' | 'src'>) {
  return canResolveArchiveRow(row) && !!row.src && row.size > 0 && typeof row.original_size === 'number' && row.original_size > 0
}
export function isSameSizeConflict(row: Pick<PlanRow, 'size' | 'original_size'>) {
  return row.size > 0 && typeof row.original_size === 'number' && row.original_size === row.size
}
export function isArchiveResolutionApplied(row: Pick<PlanRow, 'status' | 'resolution'>, kind: 'rename' | 'stage-duplicate') {
  return row.status === 'ok' && row.resolution?.kind === kind
}
export type ArchiveShortcut = 'all' | 'confirm' | 'search' | 'next' | 'previous' | 'toggle'
export function archiveShortcut(event: { key: string; repeat?: boolean; isComposing?: boolean; metaKey?: boolean; ctrlKey?: boolean; altKey?: boolean; shiftKey?: boolean }, context: { editing: boolean; modal: boolean; nativeAction: boolean }): ArchiveShortcut | null {
  if (event.repeat || event.isComposing || event.metaKey || event.ctrlKey || event.altKey || context.editing || context.modal) return null
  const key = event.key.toLowerCase()
  if (key === 'a') return 'all'
  if (key === 'enter') return !context.nativeAction && !event.shiftKey ? 'confirm' : null
  if (key === '/') return 'search'
  if (key === 'j') return 'next'
  if (key === 'k') return 'previous'
  if (key === 'x') return 'toggle'
  return null
}
