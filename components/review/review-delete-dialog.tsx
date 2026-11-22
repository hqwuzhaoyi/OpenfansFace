'use client'

import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react'
import { AlertTriangle, FolderOutput, RefreshCw, Trash2 } from 'lucide-react'
import {
  Alert,
  AlertContent,
  AlertDescription,
  AlertIcon,
} from '@/components/beui/motion/alert'
import { DigitSwap } from '@/components/beui/motion/digit-swap'
import { ResponsiveDialogContent } from '@/components/review/responsive-dialog'
import { InlineBeuiLoader } from '@/components/workbench/loading-skeletons'
import {
  WorkspaceButton as Button,
  WorkspacePath,
} from '@/components/workbench/inner'
import {
  Dialog,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { api } from '@/lib/face-api'
import type { PersonFilePlan } from '@/lib/person-file-model'
import type { ProcessingTask } from '@/lib/processing-model'

export interface ReviewDeleteDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  mode: 'single' | 'batch'
  targets: ProcessingTask[]
  groupLabel?: string
  isAutoSelected?: boolean
  otherSelectedCount?: number
  reviewSession?: string
  reviewGroup: { key: string; deferred: boolean }
  onConfirm: (plan: PersonFilePlan, targets: ProcessingTask[]) => void
  onSessionExpired?: () => void
  returnFocus?: React.RefObject<HTMLElement | null>
}

function formatBytes(value: number): string {
  if (value < 1024) return `${value} B`
  if (value < 2 ** 20) return `${(value / 1024).toFixed(1)} KB`
  if (value < 2 ** 30) return `${(value / 2 ** 20).toFixed(1)} MB`
  return `${(value / 2 ** 30).toFixed(2)} GB`
}

function resolveStagingTargetDir(plan: PersonFilePlan): string | null {
  const fromIntent = plan.execution?.data?.target_directory
  if (typeof fromIntent === 'string' && fromIntent) return fromIntent
  const firstDst = plan.files[0]?.dst
  if (!firstDst) return null
  const marker = `/review-deletions/${plan.id}`
  const idx = firstDst.indexOf(marker)
  return idx >= 0 ? firstDst.slice(0, idx + marker.length) : firstDst
}

export function ReviewDeleteDialog({
  open,
  onOpenChange,
  mode,
  targets,
  groupLabel,
  isAutoSelected = false,
  otherSelectedCount = 0,
  reviewSession,
  reviewGroup,
  onConfirm,
  onSessionExpired,
  returnFocus,
}: ReviewDeleteDialogProps) {
  const [instantEntry, setInstantEntry] = useState(false)
  const [loading, setLoading] = useState(false)
  const [plan, setPlan] = useState<PersonFilePlan | null>(null)
  const [error, setError] = useState('')
  const prevOpenRef = useRef(false)
  const seqRef = useRef(0)
  const abortRef = useRef<AbortController | null>(null)
  const fallbackTrigger = useRef<HTMLElement | null>(null)

  const targetsRef = useRef(targets)
  targetsRef.current = targets
  const reviewSessionRef = useRef(reviewSession)
  reviewSessionRef.current = reviewSession
  const reviewGroupRef = useRef(reviewGroup)
  reviewGroupRef.current = reviewGroup
  const onSessionExpiredRef = useRef(onSessionExpired)
  onSessionExpiredRef.current = onSessionExpired

  const fetchPreview = useCallback(async () => {
    const currentTargets = targetsRef.current
    const currentSession = reviewSessionRef.current
    const currentGroup = reviewGroupRef.current
    if (!currentTargets.length || !currentSession) {
      setError('当前核对会话已失效，请更新列表后重试')
      setLoading(false)
      return
    }
    abortRef.current?.abort()
    const controller = new AbortController()
    abortRef.current = controller
    const seq = ++seqRef.current
    setLoading(true)
    setError('')
    setPlan(null)
    try {
      const next = await api<PersonFilePlan>('/api/persons/manage', {
        method: 'POST',
        signal: controller.signal,
        body: JSON.stringify({
          mode: 'preview',
          request: {
            action: 'review-delete',
            person: '',
            payload: {
              vids: currentTargets.map(t => t.vid),
              review_session: currentSession,
              review_tokens: Object.fromEntries(
                currentTargets.map(t => [t.vid, t.review_token]),
              ),
              review_group: currentGroup,
              fingerprints: Object.fromEntries(
                currentTargets.map(t => [
                  t.vid,
                  { path: t.path, size: t.size, mtime_ns: t.mtime_ns },
                ]),
              ),
            },
          },
        }),
      })
      if (seq !== seqRef.current || controller.signal.aborted) return
      setPlan(next)
    } catch (e: unknown) {
      if (seq !== seqRef.current || controller.signal.aborted) return
      const err = e as { message?: string; code?: string }
      const message = err?.message || '预览回收站删除范围失败'
      setError(message)
      if (/核对已过期|核对会话|本次核对已过期/.test(message)) {
        onSessionExpiredRef.current?.()
      }
    } finally {
      if (seq === seqRef.current && !controller.signal.aborted) {
        setLoading(false)
      }
    }
  }, [])

  // Lock entry motion once when opening so modality switches never replay animation.
  useLayoutEffect(() => {
    const wasOpen = prevOpenRef.current
    prevOpenRef.current = open
    if (!wasOpen && open) {
      if (typeof document !== 'undefined') {
        const active = document.activeElement
        if (active instanceof HTMLElement && !active.closest('[role="dialog"]')) {
          fallbackTrigger.current = active
        }
        const keyboard =
          document.documentElement.dataset.input === 'keyboard' ||
          Boolean(active?.matches(':focus-visible'))
        const reduced = window.matchMedia(
          '(prefers-reduced-motion: reduce)',
        ).matches
        setInstantEntry(keyboard || reduced)
      }
      void fetchPreview()
    } else if (wasOpen && !open) {
      seqRef.current++
      abortRef.current?.abort()
      abortRef.current = null
      setLoading(false)
      setPlan(null)
      setError('')
    }
  }, [open, fetchPreview])

  useEffect(() => {
    return () => {
      abortRef.current?.abort()
    }
  }, [])

  const targetDir = plan ? resolveStagingTargetDir(plan) : null
  const displayCount = plan ? plan.files.length : targets.length
  const totalBytes = plan
    ? plan.total_bytes
    : targets.reduce((sum, t) => sum + (Number(t.size) || 0), 0)

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <ResponsiveDialogContent
        open={open}
        inert={!open}
        style={{
          animationName: open ? 'review-dialog-in' : 'review-dialog-out',
          animationDuration: instantEntry ? '0ms' : '180ms',
        }}
        overlayProps={{
          style: {
            animationName: open ? 'review-overlay-in' : 'review-dialog-out',
            animationDuration: instantEntry ? '0ms' : '140ms',
          },
        }}
        onCloseAutoFocus={event => {
          event.preventDefault()
          const target = returnFocus?.current?.isConnected
            ? returnFocus.current
            : fallbackTrigger.current?.isConnected
              ? fallbackTrigger.current
              : null
          if (target && !target.matches(':disabled') && !target.closest('[inert]')) {
            target.focus({ preventScroll: true })
          }
        }}
        className="w-full max-w-[min(560px,94vw)] flex! flex-col! gap-0! p-0! overflow-hidden! max-h-[88dvh]"
      >
        <DialogHeader className="shrink-0 border-b px-4 py-3 pr-12 text-left">
          <DialogTitle className="flex items-center gap-2 text-sm font-semibold">
            <Trash2 size={15} className="text-[var(--wb-danger)] shrink-0" aria-hidden="true" />
            {mode === 'single' ? (
              <span>移入回收站 · 1 个视频</span>
            ) : (
              <span>
                批量移入回收站 · <DigitSwap value={displayCount} /> 个视频
              </span>
            )}
          </DialogTitle>
          <DialogDescription className="text-xs text-muted-foreground">
            {groupLabel ? `${groupLabel} · ` : ''}
            文件将移入全局回收站并从待确认移除，可在「回收站」恢复原路径或永久删除。
          </DialogDescription>
        </DialogHeader>

        <div className="min-h-0 flex-1 overflow-auto overscroll-contain p-4 grid gap-3" data-sheet-scroll>
          {mode === 'batch' && isAutoSelected && targets.length > 1 && (
            <Alert variant="warning" className="rounded-lg p-3 text-xs">
              <AlertIcon className="mt-0.5 size-4" />
              <AlertContent className="space-y-0.5">
                <AlertDescription className="leading-relaxed">
                  当前包含推荐组默认全选的 {targets.length} 个视频，请核对下方清单后再确认。
                </AlertDescription>
              </AlertContent>
            </Alert>
          )}

          {mode === 'single' && otherSelectedCount > 0 && (
            <p className="text-xs text-muted-foreground">
              仅移入当前查看的这 1 个视频，不影响列表中已勾选的其他项。
            </p>
          )}

          {loading ? (
            <div className="py-6 flex justify-center">
              <InlineBeuiLoader label="正在核对文件指纹、权限与账本" showLabel />
            </div>
          ) : error ? (
            <div className="grid gap-3">
              <Alert variant="warning" className="rounded-lg p-3 text-xs">
                <AlertTriangle className="mt-0.5 size-4 text-[var(--wb-danger)] shrink-0" aria-hidden="true" />
                <AlertContent>
                  <AlertDescription className="text-[var(--wb-danger)] leading-relaxed">
                    {error}
                  </AlertDescription>
                </AlertContent>
              </Alert>
              <div>
                <Button variant="outline" size="sm" onClick={() => void fetchPreview()}>
                  <RefreshCw size={13} aria-hidden="true" />
                  <span>重新核对</span>
                </Button>
              </div>
            </div>
          ) : plan ? (
            <>
              <div className="flex flex-wrap items-center justify-between gap-2 text-xs">
                <span className="font-medium tabular-nums">
                  {plan.files.length} 个视频 · {formatBytes(totalBytes)}
                </span>
                <span className="text-muted-foreground">已校验指纹与目录权限</span>
              </div>

              {targetDir && (
                <div className="rounded-lg border bg-muted/40 p-2.5 grid gap-1.5 min-w-0">
                  <div className="flex items-center gap-1.5 text-[11px] text-muted-foreground">
                    <FolderOutput size={13} aria-hidden="true" />
                    <span>回收站目标目录</span>
                  </div>
                  <WorkspacePath value={targetDir} compact label="回收站目标目录" />
                </div>
              )}

              <div
                className="max-h-[260px] overflow-auto overscroll-contain rounded-lg border divide-y"
                aria-label="本次移入回收站文件清单"
              >
                {plan.files.map(file => {
                  const fileName = file.src.split('/').pop() || file.src
                  return (
                    <div key={file.src} className="p-2.5 grid gap-1 min-w-0">
                      <div className="flex items-baseline justify-between gap-2 min-w-0">
                        <strong
                          className="text-xs font-medium min-w-0 flex-1 [overflow-wrap:anywhere]"
                          title={fileName}
                        >
                          {fileName}
                        </strong>
                        <span className="text-[11px] text-muted-foreground tabular-nums shrink-0">
                          {formatBytes(file.size)}
                        </span>
                      </div>
                      <WorkspacePath value={file.src} compact label="原路径" />
                    </div>
                  )
                })}
              </div>
            </>
          ) : null}
        </div>

        <div className="shrink-0 flex items-center justify-end gap-2 border-t bg-background px-4 py-3 pb-[max(12px,env(safe-area-inset-bottom))]">
          <Button variant="outline" size="sm" onClick={() => onOpenChange(false)}>
            取消
          </Button>
          <Button
            className="ws-danger-btn"
            variant="outline"
            size="sm"
            disabled={loading || !plan || !!error}
            onClick={() => {
              if (!plan) return
              onConfirm(plan, targets)
            }}
          >
            <Trash2 size={14} aria-hidden="true" />
            <span>确认移入回收站</span>
            {plan && plan.files.length > 1 && (
              <span className="ws-btn-badge">
                · <DigitSwap value={plan.files.length} />
              </span>
            )}
          </Button>
        </div>
      </ResponsiveDialogContent>
    </Dialog>
  )
}
