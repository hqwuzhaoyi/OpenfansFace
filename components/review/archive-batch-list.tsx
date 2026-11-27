'use client'

import { Layers } from 'lucide-react'
import { AnimatedBadge } from '@/components/beui/motion/animated-badge'
import { lockBadgeStatus, resolveEntityLock, type ScopeLockInfo } from '@/lib/locks-model'
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

export function ArchiveBatchList({ batches, selected, active, disabled, lockedPeople, lockedVids, onToggle, onOpen, onResolve, onStage, onCustomize, onUndo }: {
  batches: ArchiveBatch<PlanRow>[]; selected: Set<string>; active?: string
  disabled: boolean
  lockedPeople?: ReadonlyMap<string, ScopeLockInfo>
  lockedVids?: ReadonlyMap<string, ScopeLockInfo>
  onToggle: (ids: string[]) => void; onOpen: (key: string) => void
  onResolve: (rows: PlanRow[], kind: 'rename') => void
  onStage: (rows: PlanRow[]) => void
  onCustomize: (rows: PlanRow[]) => void; onUndo: (rows: PlanRow[]) => void
}) {
  return <>{batches.map(batch => {
    const rowLock = (row: PlanRow) => resolveEntityLock(lockedVids?.get(row.vid), lockedPeople?.get(row.creator))
    const batchLock = resolveEntityLock(...batch.rows.map(rowLock), lockedPeople?.get(batch.creator))
    const unlockedVids = batch.rows.filter(row => !rowLock(row)).map(row => row.vid)
    const count = batch.rows.filter(row => selected.has(row.vid)).length
    const unlockedCount = unlockedVids.filter(vid => selected.has(vid)).length
    const ready = batch.rows.filter(row => row.status === 'ok').length
    const issues = batch.rows.length - ready
    const autoCount = batch.rows.filter(row => row.source === 'auto-watermark').length
    const title = batchCreator(batch)
    const allLocked = unlockedVids.length === 0
    return <article key={batch.key} className="archive-batch" data-batch={batch.key} data-active={active === batch.key} data-selected={count > 0} data-locked={Boolean(batchLock) || undefined}>
      <div className="archive-batchMain"><ArchiveCheckbox label={`选择整波 ${title} ${archiveTime(batch.at)} 共 ${batch.rows.length} 个视频`} checked={unlockedVids.length > 0 && unlockedCount === unlockedVids.length} mixed={unlockedCount > 0 && unlockedCount < unlockedVids.length} disabled={disabled || allLocked} onChange={() => onToggle(unlockedVids)} />
      <button data-archive-navigate className="archive-batchOpen" onClick={() => onOpen(batch.key)} aria-current={active === batch.key ? 'true' : undefined} title={`${title} · ${archiveTime(batch.at)}${batchLock ? ` · ${batchLock.label}` : ''}`}>
        <span className="archive-batchTitle"><strong>{title}</strong><time>{archiveTime(batch.at)}</time></span>
        <span className="archive-batchMeta"><span><Layers size={12} />{batch.rows.length} 个视频</span>{batchLock && <AnimatedBadge size="sm" status={lockBadgeStatus(batchLock.status)} className="archive-lockBadge">{batchLock.label}</AnimatedBadge>}{autoCount > 0 && <span className="archive-autoBadge">自动水印{autoCount < batch.rows.length ? ` ${autoCount}` : ''}</span>}{!batchLock && (issues ? <span className="archive-warning">{issues} 个需处理</span> : <span className="archive-good">可归档</span>)}{count > 0 && <span className="archive-selectedCount">已选 {count}/{batch.rows.length}</span>}</span>
        <span className="archive-batchExcerpt">{batch.rows[0]?.name}{batch.rows.length > 1 ? ` 等 ${batch.rows.length} 个` : ''}{batch.inferred ? ' · 旧记录' : ''}</span>
      </button></div>
      <ArchiveRowActions rows={batch.rows} disabled={disabled || Boolean(batchLock)} onResolve={onResolve} onStage={onStage} onCustomize={onCustomize} onUndo={onUndo} />
    </article>
  })}</>
}
