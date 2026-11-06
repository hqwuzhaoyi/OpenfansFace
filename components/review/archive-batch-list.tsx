'use client'

import { Layers } from 'lucide-react'
import type { PlanRow } from '@/lib/review-plan'
import type { ArchiveBatch } from '@/lib/archive-batch-selection'
import { ArchiveCheckbox } from './archive-folder-list'
import { ArchiveRowActions } from './archive-row-actions'

export function archiveTime(value: string) {
  const date = new Date(value.includes('T') ? value : value.replace(' ', 'T') + 'Z')
  return Number.isNaN(+date) ? '时间未记录' : new Intl.DateTimeFormat('zh-CN', { timeZone: 'Asia/Shanghai', month: 'numeric', day: 'numeric', hour: '2-digit', minute: '2-digit' }).format(date)
}

export function batchCreator(batch: ArchiveBatch<PlanRow>) {
  const names = [...new Set(batch.rows.map(row => row.creator))]
  return names.length > 1 ? `${names[0]} 等 ${names.length} 人` : batch.creator
}

export function ArchiveBatchList({ batches, selected, active, disabled, onToggle, onOpen, onResolve, onCustomize, onUndo }: {
  batches: ArchiveBatch<PlanRow>[]; selected: Set<string>; active?: string
  disabled: boolean; onToggle: (ids: string[]) => void; onOpen: (key: string) => void
  onResolve: (rows: PlanRow[], kind: 'rename' | 'stage-duplicate') => void
  onCustomize: (rows: PlanRow[]) => void; onUndo: (rows: PlanRow[]) => void
}) {
  return <>{batches.map(batch => {
    const count = batch.rows.filter(row => selected.has(row.vid)).length
    const ready = batch.rows.filter(row => row.status === 'ok').length
    const issues = batch.rows.length - ready
    const title = batchCreator(batch)
    return <article key={batch.key} className="archive-batch" data-batch={batch.key} data-active={active === batch.key} data-selected={count > 0}>
      <div className="archive-batchMain"><ArchiveCheckbox label={`选择整波 ${title} ${archiveTime(batch.at)} 共 ${batch.rows.length} 个视频`} checked={count === batch.rows.length} mixed={count > 0 && count < batch.rows.length} disabled={disabled} onChange={() => onToggle(batch.rows.map(row => row.vid))} />
      <button data-archive-navigate className="archive-batchOpen" onClick={() => onOpen(batch.key)} aria-current={active === batch.key ? 'true' : undefined} title={`${title} · ${archiveTime(batch.at)}`}>
        <span className="archive-batchTitle"><strong>{title}</strong><time>{archiveTime(batch.at)}</time></span>
        <span className="archive-batchMeta"><span><Layers size={12} />{batch.rows.length} 个视频</span>{issues ? <span className="archive-warning">{issues} 个需处理</span> : <span className="archive-good">可归档</span>}{count > 0 && <span className="archive-selectedCount">已选 {count}/{batch.rows.length}</span>}</span>
        <span className="archive-batchExcerpt">{batch.rows[0]?.name}{batch.rows.length > 1 ? ` 等 ${batch.rows.length} 个` : ''}{batch.inferred ? ' · 旧记录' : ''}</span>
      </button></div>
      <ArchiveRowActions rows={batch.rows} disabled={disabled} onResolve={onResolve} onCustomize={onCustomize} onUndo={onUndo} />
    </article>
  })}</>
}
