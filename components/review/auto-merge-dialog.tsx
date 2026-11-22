'use client'

import Link from 'next/link'
import { useEffect, useLayoutEffect, useRef, useState } from 'react'
import {
  AlertTriangle,
  ArrowUpRight,
  CheckCircle2,
  CopyPlus,
  FileVideo,
  FolderOpen,
  RefreshCw,
  Sparkles,
} from 'lucide-react'
import { AgentProgress } from '@/components/beui/agents/loading-states/agent-progress'
import {
  Alert,
  AlertContent,
  AlertDescription,
  AlertIcon,
} from '@/components/beui/motion/alert'
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleIndicator,
  CollapsibleTrigger,
} from '@/components/beui/motion/collapsible'
import { DigitSwap } from '@/components/beui/motion/digit-swap'
import { Badge } from '@/components/ui/badge'
import { Button as NavigationButton } from '@/components/ui/button'
import {
  Dialog,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Progress } from '@/components/ui/progress'
import { ResponsiveDialogContent } from '@/components/review/responsive-dialog'
import {
  type AutoMergeTask,
  useBackgroundOperations,
} from '@/components/workbench/background-operations'
import { WorkspaceButton as Button } from '@/components/workbench/inner'
import {
  autoMatchTierLabel,
  buildAutoMergeApplyItems,
  formatAutoMergeSize,
  type AutoWatermarkPreview,
} from '@/lib/archive-actions'

export interface AutoMergeDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  preview: AutoWatermarkPreview | null
  loading?: boolean
  onRefreshPreview: () => Promise<void> | void
  returnFocus?: React.RefObject<HTMLElement | null>
}

export function AutoMergeDialog({
  open,
  onOpenChange,
  preview,
  loading = false,
  onRefreshPreview,
  returnFocus,
}: AutoMergeDialogProps) {
  const { tasks, enqueueAutoMerge, subscribe } = useBackgroundOperations()
  const [instantEntry, setInstantEntry] = useState(false)
  const [trackedTaskId, setTrackedTaskId] = useState<string | null>(null)
  const [settledSnapshot, setSettledSnapshot] = useState<AutoMergeTask | null>(
    null,
  )
  const prevOpenRef = useRef(false)

  const activeRunningTask =
    tasks.find(
      (t): t is AutoMergeTask =>
        t.kind === 'auto-merge' &&
        (t.status === 'queued' || t.status === 'running'),
    ) ?? null

  const liveTrackedTask =
    (trackedTaskId
      ? (tasks.find(
          (t): t is AutoMergeTask =>
            t.kind === 'auto-merge' && t.id === trackedTaskId,
        ) ?? null)
      : null) ?? activeRunningTask

  const trackedTask =
    liveTrackedTask ??
    (settledSnapshot && settledSnapshot.id === trackedTaskId
      ? settledSnapshot
      : null)

  // Capture input modality synchronously before paint on closed -> open so modality switches never replay motion.
  useLayoutEffect(() => {
    const wasOpen = prevOpenRef.current
    prevOpenRef.current = open
    if (!wasOpen && open) {
      const keyboard =
        document.documentElement.dataset.input === 'keyboard' ||
        Boolean(document.activeElement?.matches(':focus-visible'))
      const reduced = window.matchMedia(
        '(prefers-reduced-motion: reduce)',
      ).matches
      setInstantEntry(keyboard || reduced)
      setSettledSnapshot(null)
      setTrackedTaskId(activeRunningTask?.id ?? null)
    }
  }, [open, activeRunningTask])

  // Retain settled task snapshot locally so the provider's 3s auto-clear never dismisses the summary mid-read.
  useEffect(() => {
    if (!trackedTaskId) return
    return subscribe((event) => {
      if (
        event.type === 'auto-merge-settled' &&
        event.task.id === trackedTaskId
      ) {
        setSettledSnapshot(event.task)
      }
    })
  }, [subscribe, trackedTaskId])

  useEffect(() => {
    if (
      liveTrackedTask &&
      ['complete', 'failed', 'rolled_back', 'recovery_needed'].includes(
        liveTrackedTask.status,
      )
    ) {
      setSettledSnapshot((prev) =>
        prev?.id === liveTrackedTask.id &&
        prev.finishedAt === liveTrackedTask.finishedAt
          ? prev
          : liveTrackedTask,
      )
    }
  }, [liveTrackedTask])

  const isRunning =
    trackedTask?.status === 'queued' || trackedTask?.status === 'running'
  const isSettled =
    trackedTask != null &&
    ['complete', 'failed', 'rolled_back', 'recovery_needed'].includes(
      trackedTask.status,
    )

  const eligibleCount = preview?.eligible_count ?? 0
  const groupCount = preview?.group_count ?? preview?.groups.length ?? 0
  const groups = preview?.groups ?? []

  const handleApply = () => {
    if (!preview || eligibleCount <= 0 || isRunning || loading) return
    const items = buildAutoMergeApplyItems(preview)
    if (!items.length) return
    setSettledSnapshot(null)
    const taskId = enqueueAutoMerge({
      version: preview.version,
      items,
      creators: preview.groups.map((g) => g.person),
    })
    setTrackedTaskId(taskId)
  }

  const settledResult = isSettled ? trackedTask.result : null
  const archivedCount = settledResult?.archived ?? settledResult?.moved ?? 0
  const fallbackCount = settledResult?.fallback_queued ?? 0
  const skippedCount = settledResult?.skipped ?? 0
  const nonArchivedItems = (settledResult?.results ?? []).filter(
    (item) => item.status !== 'archived',
  )
  const progressDone = trackedTask?.progress?.done ?? 0
  const progressTotal =
    trackedTask?.progress?.total ?? trackedTask?.vids.length ?? 0
  const progressPercent =
    progressTotal > 0
      ? Math.min(100, Math.round((progressDone / progressTotal) * 100))
      : null
  const currentFile = (
    trackedTask?.progress as { current?: string } | null | undefined
  )?.current

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <ResponsiveDialogContent
        open={open}
        inert={!open}
        aria-hidden={!open || undefined}
        style={{
          animationDuration: instantEntry ? '0ms' : '180ms',
          pointerEvents: open ? 'auto' : 'none',
        }}
        overlayProps={{
          className: 'motion-reduce:animate-none',
          style: { animationDuration: instantEntry ? '0ms' : '140ms' },
        }}
        onCloseAutoFocus={(event) => {
          event.preventDefault()
          const target = returnFocus?.current
          requestAnimationFrame(() => {
            if (target?.isConnected && !target.matches(':disabled')) {
              target.focus({ preventScroll: true })
            } else {
              document
                .querySelector<HTMLButtonElement>(
                  '[aria-label="更新人工确认列表"]',
                )
                ?.focus({ preventScroll: true })
            }
          })
        }}
        className="flex max-h-[88dvh] flex-col gap-0 overflow-hidden p-0 sm:max-w-2xl sm:p-0"
      >
        <DialogHeader className="shrink-0 border-b px-4 py-3.5 pr-14 text-left">
          <div className="flex items-center justify-between gap-2">
            <div className="flex min-w-0 items-center gap-2">
              <Sparkles
                size={16}
                className="shrink-0 text-muted-foreground"
                aria-hidden="true"
              />
              <DialogTitle className="truncate text-[15px] font-semibold">
                自动合入
              </DialogTitle>
              {!isSettled && (
                <span className="ws-count tabular-nums">
                  <DigitSwap value={eligibleCount} /> · {groupCount} 人
                </span>
              )}
            </div>
            {!isRunning && !isSettled && (
              <Button
                variant="ghost"
                size="icon"
                aria-label="刷新自动合入预览"
                title="刷新预览"
                disabled={loading}
                onClick={() => void onRefreshPreview()}
              >
                <RefreshCw
                  size={14}
                  aria-hidden="true"
                  className={
                    loading ? 'animate-spin motion-reduce:animate-none' : ''
                  }
                />
              </Button>
            )}
          </div>
          <DialogDescription className="sr-only">
            预览可直接按水印合入已有人物目录的视频，并执行批量合入。
          </DialogDescription>
        </DialogHeader>

        <div
          data-sheet-scroll
          className="min-h-0 flex-1 space-y-2.5 overflow-y-auto overscroll-contain p-4"
        >
          {isRunning && trackedTask ? (
            <div
              role="status"
              aria-live="polite"
              className="space-y-3 rounded-xl border bg-card p-4"
            >
              <div className="flex items-center justify-between gap-3">
                <AgentProgress
                  label={trackedTask.progress?.stage || '正在直接合入人物目录'}
                  initialSeconds={
                    trackedTask.startedAt
                      ? Math.max(0, (Date.now() - trackedTask.startedAt) / 1000)
                      : 0
                  }
                  labels={{ inProgress: '{label}' }}
                  className="min-w-0 flex-1 text-xs"
                />
                <span className="shrink-0 text-xs font-medium tabular-nums text-muted-foreground">
                  {progressDone} / {progressTotal}
                </span>
              </div>
              <Progress
                aria-label={trackedTask.progress?.stage || '自动合入进度'}
                value={progressPercent}
                className="h-1.5"
              />
              {currentFile && (
                <p
                  className="truncate font-mono text-[11px] text-muted-foreground"
                  title={currentFile}
                >
                  {currentFile}
                </p>
              )}
            </div>
          ) : isSettled && trackedTask ? (
            <div role="status" aria-live="polite" className="space-y-3">
              <div className="grid grid-cols-3 gap-2">
                <div className="rounded-xl border bg-card px-3 py-2.5">
                  <div className="text-[11px] text-muted-foreground">已合入</div>
                  <div
                    className="mt-1 flex items-center gap-1.5 text-lg font-semibold tabular-nums"
                    style={{ color: 'var(--wb-success)' }}
                  >
                    <CheckCircle2 size={15} aria-hidden="true" />
                    <span>{archivedCount}</span>
                  </div>
                </div>
                <div className="rounded-xl border bg-card px-3 py-2.5">
                  <div className="text-[11px] text-muted-foreground">
                    转入待归档
                  </div>
                  <div
                    className="mt-1 flex items-center gap-1.5 text-lg font-semibold tabular-nums"
                    style={{
                      color:
                        fallbackCount > 0
                          ? 'var(--wb-warning)'
                          : 'var(--muted-foreground)',
                    }}
                  >
                    {fallbackCount > 0 && (
                      <AlertTriangle size={15} aria-hidden="true" />
                    )}
                    <span>{fallbackCount}</span>
                  </div>
                </div>
                <div className="rounded-xl border bg-card px-3 py-2.5">
                  <div className="text-[11px] text-muted-foreground">已跳过</div>
                  <div className="mt-1 text-lg font-semibold tabular-nums text-muted-foreground">
                    {skippedCount}
                  </div>
                </div>
              </div>

              {(trackedTask.error || (settledResult?.errors?.length ?? 0) > 0) && (
                <Alert variant="warning" className="rounded-xl p-3 text-xs">
                  <AlertIcon />
                  <AlertContent>
                    <AlertDescription className="text-current">
                      {(settledResult?.errors?.length
                        ? settledResult.errors
                        : [trackedTask.error]
                      )
                        .filter(Boolean)
                        .join('；')}
                    </AlertDescription>
                  </AlertContent>
                </Alert>
              )}

              {nonArchivedItems.length > 0 && (
                <div className="divide-y rounded-xl border bg-card">
                  {nonArchivedItems.map((item) => (
                    <div
                      key={`${item.person}:${item.vid}`}
                      className="flex items-start justify-between gap-3 px-3 py-2 text-xs"
                    >
                      <div className="min-w-0 flex-1">
                        <div className="flex items-center gap-1.5">
                          <span className="truncate font-medium">
                            {item.person}
                          </span>
                          <span className="truncate font-mono text-[11px] text-muted-foreground">
                            {item.vid}
                          </span>
                        </div>
                        {item.reason && (
                          <p className="mt-0.5 break-words text-[11px] text-muted-foreground">
                            {item.reason}
                          </p>
                        )}
                      </div>
                      <Badge
                        variant="outline"
                        className="shrink-0 text-[10px]"
                        style={
                          item.status === 'fallback_queued' ||
                          item.status === 'failed'
                            ? {
                                color: 'var(--wb-warning)',
                                borderColor: 'var(--wb-warning-border)',
                                backgroundColor: 'var(--wb-warning-bg)',
                              }
                            : undefined
                        }
                      >
                        {item.status === 'fallback_queued'
                          ? '已转待归档'
                          : item.status === 'failed'
                            ? '失败'
                            : '已跳过'}
                      </Badge>
                    </div>
                  ))}
                </div>
              )}
            </div>
          ) : groups.length === 0 ? (
            <div className="flex flex-col items-center justify-center gap-2 py-10 text-center text-xs text-muted-foreground">
              <CheckCircle2 size={22} aria-hidden="true" />
              <span>当前没有可直接自动合入的视频</span>
            </div>
          ) : (
            groups.map((group) => (
              <Collapsible
                key={group.person}
                defaultOpen={groups.length <= 3}
                className="rounded-xl border bg-card"
              >
                <CollapsibleTrigger className="min-h-11 rounded-xl px-3 py-2">
                  <FolderOpen
                    size={15}
                    className="shrink-0 text-muted-foreground"
                    aria-hidden="true"
                  />
                  <span className="min-w-0 flex-1 truncate text-[13px] font-medium text-foreground">
                    {group.person}
                  </span>
                  <span className="shrink-0 text-[11px] tabular-nums text-muted-foreground">
                    已有 {group.existing_files}
                  </span>
                  <span
                    className="inline-flex h-5 shrink-0 items-center rounded-md border px-1.5 text-[11px] font-medium tabular-nums"
                    style={{
                      color: 'var(--wb-success)',
                      backgroundColor: 'var(--wb-success-bg)',
                      borderColor: 'var(--wb-success-border)',
                    }}
                  >
                    +{group.count}
                  </span>
                  <CollapsibleIndicator />
                </CollapsibleTrigger>
                <CollapsibleContent>
                  <div className="divide-y border-t">
                    {group.items.map((item) => (
                      <div
                        key={item.vid}
                        className="space-y-1.5 px-3 py-2.5 text-xs"
                      >
                        <div className="flex items-center justify-between gap-2">
                          <div className="flex min-w-0 items-center gap-1.5">
                            <FileVideo
                              size={13}
                              className="shrink-0 text-muted-foreground"
                              aria-hidden="true"
                            />
                            <span
                              className="truncate font-medium text-foreground"
                              title={item.name}
                            >
                              {item.name}
                            </span>
                          </div>
                          <span className="shrink-0 text-[11px] tabular-nums text-muted-foreground">
                            {formatAutoMergeSize(item.size)}
                          </span>
                        </div>

                        <div className="flex flex-wrap items-center gap-1.5">
                          <Badge
                            variant="secondary"
                            className="h-5 px-1.5 text-[10px] font-normal"
                          >
                            {autoMatchTierLabel(item.match_tier)}
                          </Badge>
                          {item.watermark_text && (
                            <span
                              className="inline-flex max-w-full items-center gap-1 truncate rounded-md border bg-background px-1.5 py-0.5 text-[11px] text-muted-foreground"
                              title={item.watermark_text}
                            >
                              {item.watermark_platform &&
                              item.watermark_platform.toLowerCase() !==
                                'unknown'
                                ? `${item.watermark_platform} · `
                                : ''}
                              {item.watermark_text}
                            </span>
                          )}
                          {item.renamed_keep_both && (
                            <span
                              className="inline-flex max-w-full items-center gap-1 truncate rounded-md border px-1.5 py-0.5 text-[11px]"
                              title={`目标同名，将保留两份并命名为 ${item.target_name}`}
                              style={{
                                color: 'var(--wb-warning)',
                                backgroundColor: 'var(--wb-warning-bg)',
                                borderColor: 'var(--wb-warning-border)',
                              }}
                            >
                              <CopyPlus
                                size={11}
                                className="shrink-0"
                                aria-hidden="true"
                              />
                              <span className="truncate">
                                保留两份 → {item.target_name}
                              </span>
                            </span>
                          )}
                        </div>

                        <code
                          translate="no"
                          title={item.path}
                          className="block truncate font-mono text-[11px] text-muted-foreground"
                        >
                          {item.path}
                        </code>
                      </div>
                    ))}
                  </div>
                </CollapsibleContent>
              </Collapsible>
            ))
          )}
        </div>

        <div className="mobile-bottom-actions flex shrink-0 flex-col-reverse gap-2 border-t bg-background px-4 py-3 sm:flex-row sm:items-center sm:justify-end">
          {isRunning ? (
            <Button variant="outline" onClick={() => onOpenChange(false)}>
              后台继续
            </Button>
          ) : isSettled ? (
            <>
              {fallbackCount > 0 && (
                <NavigationButton variant="outline" size="sm" asChild>
                  <Link
                    href="/integration"
                    onClick={() => onOpenChange(false)}
                  >
                    <span>前往待归档 · {fallbackCount}</span>
                    <ArrowUpRight size={14} aria-hidden="true" />
                  </Link>
                </NavigationButton>
              )}
              <Button onClick={() => onOpenChange(false)}>完成</Button>
            </>
          ) : (
            <>
              <Button variant="outline" onClick={() => onOpenChange(false)}>
                取消
              </Button>
              <Button
                disabled={loading || eligibleCount <= 0}
                onClick={handleApply}
              >
                <Sparkles size={14} aria-hidden="true" />
                <span>直接合入 {eligibleCount} 个视频</span>
              </Button>
            </>
          )}
        </div>
      </ResponsiveDialogContent>
    </Dialog>
  )
}
