import type { BatchRow } from './review-batches'

export interface ArchiveBatch<T extends BatchRow> { key: string; rows: T[]; at: string; creator: string; inferred: boolean }
export type BatchFilter = 'all' | 'ok' | 'issues' | 'selected'

type SearchableRow = BatchRow & { name: string; src?: string | null; dst?: string | null }

/** Filtering reveals entire batches; selecting a visible batch always means all of its videos. */
export function filterArchiveBatches<T extends SearchableRow>(batches: ArchiveBatch<T>[], query: string, filter: BatchFilter, selected: Set<string>) {
  const needle = query.trim().toLowerCase()
  return batches.filter(batch => {
    const matchesStatus = filter === 'all' || batch.rows.some(row => filter === 'selected' ? selected.has(row.vid) : filter === 'ok' ? row.status === 'ok' : row.status !== 'ok')
    return matchesStatus && (!needle || batch.rows.some(row => [row.creator, row.name, row.vid, row.src || '', row.dst || '', row.at].some(value => value.toLowerCase().includes(needle))))
  })
}

/** Selection belongs to the queue, never to the currently opened or filtered batch. */
export function summarizeArchiveSelection<T extends BatchRow>(rows: T[], selected: Set<string>, visibleIds: Set<string>) {
  const chosen = rows.filter(row => selected.has(row.vid))
  return {
    chosen,
    ready: chosen.filter(row => row.status === 'ok'),
    blocked: chosen.filter(row => row.status !== 'ok'),
    hidden: chosen.filter(row => !visibleIds.has(row.vid)).length,
  }
}

/** A refreshed queue is authoritative after success, partial failure, or a stale version. */
export function retainArchiveSelection(selected: Set<string>, rows: { vid: string }[]) {
  const remaining = new Set(rows.map(row => row.vid))
  return new Set([...selected].filter(vid => remaining.has(vid)))
}
