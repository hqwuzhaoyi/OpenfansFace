'use client'

import type { PlanRow } from '@/lib/review-plan'
import { canResolveArchiveRow, canStageArchiveRow } from '@/lib/archive-actions'

export function ArchiveRowActions({ rows, disabled, onResolve, onCustomize, onUndo }: {
  rows: PlanRow[]; disabled: boolean
  onResolve: (rows: PlanRow[], kind: 'rename' | 'stage-duplicate') => void
  onCustomize: (rows: PlanRow[]) => void
  onUndo: (rows: PlanRow[]) => void
}) {
  const conflicts = rows.filter(canResolveArchiveRow)
  const duplicates = conflicts.filter(canStageArchiveRow)
  const issues = rows.filter(row => row.status !== 'ok')
  if (!conflicts.length && !issues.length) return null
  const count = (n: number) => rows.length > 1 ? ` ${n}` : ''
  return <div className="archive-rowActions" aria-label="列表处理操作">
    {issues.length > 0 && <div className="archive-rowReasons">{issues.slice(0, 2).map(row => <p key={row.vid}>{rows.length > 1 && <span>{row.name}：</span>}{row.reason}</p>)}{issues.length > 2 && <p>另有 {issues.length - 2} 个异常，可在右侧逐项查看。</p>}</div>}
    <div className="archive-rowActionButtons">
      {!!conflicts.length && <><button disabled={disabled} onClick={() => onResolve(conflicts, 'rename')}>保留两份{count(conflicts.length)}</button>{!!duplicates.length && <button disabled={disabled} title="只保存处理方式；归档时校验完整内容，相同才暂存源文件" onClick={() => onResolve(duplicates, 'stage-duplicate')}>暂存重复件{count(duplicates.length)}</button>}<button disabled={disabled} onClick={() => onCustomize(conflicts)}>更多处理…</button></>}
      {!!issues.length && <button disabled={disabled} onClick={() => onUndo(issues)}>退回待确认{count(issues.length)}</button>}
    </div>
    {!!conflicts.length && <small>仅保存处理方式，归档时才移动文件。</small>}
    {issues.some(row => ['organized', 'already'].includes(row.status)) && <small>退回只撤回待归档记录，已归类的文件保留原位置。</small>}
  </div>
}
