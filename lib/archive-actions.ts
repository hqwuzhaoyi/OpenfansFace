import type { PlanRow } from './review-plan'

export function canResolveArchiveRow(row: Pick<PlanRow, 'status' | 'resolution'>) {
  return ['duplicate', 'conflict', 'dupe-target'].includes(row.status) || (row.status === 'ok' && !!row.resolution)
}
export function canStageArchiveRow(row: Pick<PlanRow, 'status' | 'resolution' | 'size' | 'original_size' | 'src'>) {
  return canResolveArchiveRow(row) && !!row.src && row.size > 0 && row.original_size === row.size
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
