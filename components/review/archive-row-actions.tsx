'use client'

import type { PlanRow } from '@/lib/review-plan'
import { canResolveArchiveRow, canStageArchiveRow, isArchiveResolutionApplied } from '@/lib/archive-actions'

export function ArchiveRowActions({ rows, disabled, onResolve, onStage, onCustomize, onUndo }: {
  rows: PlanRow[]; disabled: boolean
  onResolve: (rows: PlanRow[], kind: 'rename') => void
  onStage: (rows: PlanRow[]) => void
  onCustomize: (rows: PlanRow[]) => void
  onUndo: (rows: PlanRow[]) => void
}) {
  const conflicts = rows.filter(canResolveArchiveRow)
  const stageable = conflicts.filter(canStageArchiveRow)
  const issues = rows.filter(row => row.status !== 'ok')
  if (!conflicts.length && !issues.length) return null
  const pendingRename = conflicts.filter(row => !isArchiveResolutionApplied(row, 'rename'))
  const count = (n: number) => rows.length > 1 ? ` ${n}` : ''
  return <div className="archive-rowActions" aria-label="列表处理操作">
    {issues.length > 0 && <div className="archive-rowReasons">{issues.slice(0, 2).map(row => <p key={row.vid}>{rows.length > 1 && <span>{row.name}：</span>}{row.reason}</p>)}{issues.length > 2 && <p>另有 {issues.length - 2} 个异常，可在右侧逐项查看。</p>}</div>}
    <div className="archive-rowActionButtons">
      {!!conflicts.length && <>
        <button disabled={disabled || !pendingRename.length} aria-pressed={!pendingRename.length || undefined} onClick={() => onResolve(pendingRename, 'rename')}>{pendingRename.length ? `保留两份${count(pendingRename.length)}` : '已选保留两份'}</button>
        {!!stageable.length && <button disabled={disabled} title="保留目标文件，将源文件移入全局回收站" onClick={() => onStage(stageable)}>删除源文件{count(stageable.length)}</button>}
        <button disabled={disabled} onClick={() => onCustomize(conflicts)}>核对…</button>
      </>}
      {!!issues.length && <button disabled={disabled} onClick={() => onUndo(issues)}>退回待确认{count(issues.length)}</button>}
    </div>
  </div>
}
