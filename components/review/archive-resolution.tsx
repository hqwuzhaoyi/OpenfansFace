'use client'
import { useState } from 'react'
import { Dialog, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { ResponsiveDialogContent } from './responsive-dialog'
import { Button } from '@/components/ui/button'
import { PathText, VideoPlayer } from './video'
import { canStageArchiveRow, isSameSizeConflict } from '@/lib/archive-actions'
import type { ReviewVideo } from '@/lib/review-ui'
import type { ArchiveResolution } from '@/lib/review-logic'

export interface ConflictVideo extends ReviewVideo {
  status: string
  creator?: string
  src: string | null
  dst: string | null
  original_dst?: string
  suggested_name?: string
  size: number
  dst_size?: number
  original_size?: number
  resolution?: ArchiveResolution
}
const sizeText = (n?: number) => n === undefined ? '未读取' : `${(n / 2 ** 20).toFixed(2)} MiB`

function ConflictComparisonCard({ row, defaultOpen, onPlay }: {
  row: ConflictVideo
  defaultOpen: boolean
  onPlay: (video: ReviewVideo) => void
}) {
  const sameSize = isSameSizeConflict(row)
  const targetPath = row.original_dst || row.dst || ''
  const content = (
    <div className="grid gap-3 pt-3 sm:grid-cols-2">
      <div className="space-y-2 rounded-lg border bg-muted/20 p-3">
        <PathText value={row.src || row.vid} label={`源文件 · ${sizeText(row.size)}`} />
        <Button className="min-h-11" size="sm" variant="outline" onClick={() => onPlay({ ...row, path: row.src || row.vid, name: `源视频 · ${row.name}` })}>播放源视频</Button>
      </div>
      <div className="space-y-2 rounded-lg border bg-muted/20 p-3">
        <PathText value={targetPath} label={`目标文件（保留） · ${sizeText(row.original_size)}`} />
        {targetPath && (
          <Button className="min-h-11" size="sm" variant="outline" onClick={() => onPlay({ ...row, path: targetPath, thumb: null, name: `目标视频 · ${row.name}` })}>播放目标视频</Button>
        )}
      </div>
    </div>
  )

  if (defaultOpen) {
    return (
      <section className="rounded-xl border p-3">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <div className="min-w-0 flex-1">
            <h3 className="break-all text-sm font-medium">{row.name}</h3>
            <p className="mt-0.5 text-xs text-muted-foreground tabular-nums">源 {sizeText(row.size)} / 目标 {sizeText(row.original_size)}</p>
          </div>
          {sameSize && <span className="archive-autoBadge">同体积</span>}
        </div>
        {content}
      </section>
    )
  }

  return (
    <details className="rounded-xl border p-3">
      <summary className="flex min-h-11 cursor-pointer items-center justify-between gap-2 text-sm font-medium">
        <span className="break-all">{row.name} · 源 {sizeText(row.size)} / 目标 {sizeText(row.original_size)}</span>
        {sameSize && <span className="archive-autoBadge">同体积</span>}
      </summary>
      {content}
    </details>
  )
}

export function ArchiveResolutionDialog({ rows, onClose, onKeepBoth, onStageSource, onUndo, onReset, fallbackFocus }: {
  rows: ConflictVideo[]; onClose: () => void
  onKeepBoth: (rows: ConflictVideo[]) => Promise<void>
  onStageSource: (rows: ConflictVideo[]) => Promise<void>
  onUndo: (rows: ConflictVideo[]) => Promise<void>
  onReset?: (rows: ConflictVideo[]) => Promise<void>
  fallbackFocus?: React.RefObject<HTMLElement | null>
}) {
  const [trigger] = useState(() => typeof document !== 'undefined' && document.activeElement instanceof HTMLElement ? document.activeElement : null)
  const [saving, setSaving] = useState(false), [error, setError] = useState('')
  const [playing, setPlaying] = useState<ReviewVideo | null>(null)
  const stageableRows = rows.filter(canStageArchiveRow)
  const hasResolution = rows.some(r => Boolean(r.resolution))

  const runAction = async (fn: () => Promise<void>) => {
    if (saving) return
    setSaving(true); setError('')
    try {
      await fn()
      onClose()
    } catch (e) {
      setError((e as Error).message)
    } finally {
      setSaving(false)
    }
  }

  return <><Dialog open onOpenChange={open => !open && !saving && onClose()}>
    <ResponsiveDialogContent
      open
      onCloseAutoFocus={event => {
        event.preventDefault()
        const target = trigger
        requestAnimationFrame(() => {
          if (target?.isConnected && !target.matches(':disabled')) target.focus()
          else fallbackFocus?.current?.focus()
        })
      }}
      className="flex max-h-[90dvh] flex-col gap-0 overflow-hidden p-0 sm:max-w-2xl sm:p-0"
      onInteractOutside={e => saving && e.preventDefault()}
      onEscapeKeyDown={e => saving && e.preventDefault()}
      showCloseButton={!saving}
    >
      <DialogHeader className="shrink-0 border-b p-4 pr-14 text-left">
        <DialogTitle>同名文件核对 · {rows.length} 个</DialogTitle>
        <DialogDescription className="sr-only">核对源视频与目标视频，并选择处理方式</DialogDescription>
      </DialogHeader>
      <div data-sheet-scroll className="min-h-0 flex-1 space-y-3 overflow-y-auto p-4">
        {rows.map(r => (
          <ConflictComparisonCard key={r.vid} row={r} defaultOpen={rows.length <= 3} onPlay={setPlaying} />
        ))}
        {error && <p role="alert" className="rounded-lg border border-amber-500/30 p-3 text-sm text-amber-300">{error}</p>}
      </div>
      <div className="mobile-bottom-actions flex shrink-0 flex-col gap-2 border-t bg-background p-3 sm:flex-row sm:items-center sm:justify-between">
        <div className="flex flex-wrap gap-2">
          <Button className="min-h-11" variant="ghost" disabled={saving} onClick={() => void runAction(() => onUndo(rows))}>退回待确认</Button>
          {hasResolution && onReset && (
            <Button className="min-h-11" variant="ghost" disabled={saving} onClick={() => void runAction(() => onReset(rows))}>重置选择</Button>
          )}
        </div>
        <div className="flex flex-col gap-2 sm:flex-row">
          <Button
            className="min-h-11"
            variant="destructive"
            disabled={saving || stageableRows.length === 0}
            title={stageableRows.length === 0 ? '目标目录尚无同名常规文件，请选择保留两份或退回待确认' : '保留目标文件，将源文件移入全局回收站'}
            onClick={() => void runAction(() => onStageSource(stageableRows))}
          >
            删除源文件{stageableRows.length > 0 && stageableRows.length !== rows.length ? ` ${stageableRows.length}` : ''}
          </Button>
          <Button className="min-h-11" disabled={saving} onClick={() => void runAction(() => onKeepBoth(rows))}>
            保留两份
          </Button>
        </div>
      </div>
    </ResponsiveDialogContent>
  </Dialog><VideoPlayer video={playing} onClose={() => setPlaying(null)} /></>
}
