'use client'

import { useEffect, useRef, type ReactNode, type RefObject } from 'react'
import { ChevronLeft, ChevronRight } from 'lucide-react'
import { Dialog, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { ResponsiveDialogContent } from '@/components/review/responsive-dialog'
import { WorkspacePath } from '@/components/workbench/inner'
import { WorkbenchVideoPlayer } from '@/components/workbench/video-player'

export interface VideoPreviewDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  title: string
  subtitle?: string
  badge?: ReactNode
  src: string
  name: string
  poster?: string
  path?: string
  initialTime?: number
  onTimeRemember?: (time: number) => void
  onError?: (event: React.SyntheticEvent<HTMLVideoElement>) => void
  errorSlot?: ReactNode
  headerTabs?: ReactNode
  headerExtra?: ReactNode
  footerActions?: ReactNode
  stripSlot?: ReactNode
  onPrev?: () => void
  onNext?: () => void
  hasPrev?: boolean
  hasNext?: boolean
  counterText?: string
  returnFocus?: RefObject<HTMLElement | null>
  instantEntry?: boolean
}

export function VideoPreviewDialog({
  open,
  onOpenChange,
  title,
  subtitle,
  badge,
  src,
  name,
  poster,
  path,
  initialTime = 0,
  onTimeRemember,
  onError,
  errorSlot,
  headerTabs,
  headerExtra,
  footerActions,
  stripSlot,
  onPrev,
  onNext,
  hasPrev = false,
  hasNext = false,
  counterText,
  returnFocus,
  instantEntry,
}: VideoPreviewDialogProps) {
  const fallbackTrigger = useRef<HTMLElement | null>(null)

  useEffect(() => {
    if (!open || (!onPrev && !onNext)) return
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.defaultPrevented || e.metaKey || e.ctrlKey || e.altKey) return
      const target = e.target as HTMLElement | null
      if (target && (['INPUT', 'TEXTAREA', 'SELECT'].includes(target.tagName) || target.isContentEditable)) return
      if (e.key === '[' && hasPrev && onPrev) {
        e.preventDefault()
        onPrev()
      } else if (e.key === ']' && hasNext && onNext) {
        e.preventDefault()
        onNext()
      }
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [open, onPrev, onNext, hasPrev, hasNext])

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <ResponsiveDialogContent
        open={open}
        inert={!open}
        style={
          instantEntry === undefined
            ? undefined
            : {
                animationName: open ? 'review-dialog-in' : 'review-dialog-out',
                animationDuration: instantEntry ? '0ms' : '180ms',
              }
        }
        overlayProps={
          instantEntry === undefined
            ? undefined
            : {
                style: {
                  animationName: open ? 'review-overlay-in' : 'review-dialog-out',
                  animationDuration: instantEntry ? '0ms' : '140ms',
                },
              }
        }
        onOpenAutoFocus={() => {
          fallbackTrigger.current = document.activeElement instanceof HTMLElement ? document.activeElement : null
        }}
        onCloseAutoFocus={event => {
          event.preventDefault()
          const target = returnFocus?.current?.isConnected ? returnFocus.current : fallbackTrigger.current
          if (target?.isConnected) target.focus({ preventScroll: true })
        }}
        className="w-full max-w-[min(1120px,94vw)] lg:max-w-[min(1120px,94vw)] h-[min(88dvh,860px)] max-h-[92dvh] flex! flex-col! gap-0! p-0! overflow-hidden!"
      >
        <DialogHeader className="shrink-0 flex-row items-center justify-between gap-2 border-b px-4 py-2.5 pr-14 text-left">
          <div className="flex min-w-0 flex-1 items-center gap-2">
            <DialogTitle className="truncate text-sm font-semibold">{title}</DialogTitle>
            {badge}
            {subtitle && <span className="hidden truncate text-xs text-muted-foreground sm:inline">{subtitle}</span>}
          </div>
          {headerTabs && <div className="shrink-0">{headerTabs}</div>}
          <div className="flex shrink-0 items-center gap-1">
            {(onPrev || onNext) && (
              <>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon"
                  aria-label="上一条视频"
                  title="上一条 ([)"
                  disabled={!hasPrev}
                  onClick={onPrev}
                >
                  <ChevronLeft size={15} />
                </Button>
                {counterText && (
                  <span className="px-1 text-xs tabular-nums text-muted-foreground">{counterText}</span>
                )}
                <Button
                  type="button"
                  variant="ghost"
                  size="icon"
                  aria-label="下一条视频"
                  title="下一条 (])"
                  disabled={!hasNext}
                  onClick={onNext}
                >
                  <ChevronRight size={15} />
                </Button>
              </>
            )}
            {headerExtra}
          </div>
          <DialogDescription className="sr-only">
            居中大号视频预览窗口，支持键盘快捷键与隐私模式遮挡切换。
          </DialogDescription>
        </DialogHeader>

        {open && (
          <>
            <div className="min-h-0 flex-1 flex flex-col overflow-hidden bg-black">
              <WorkbenchVideoPlayer
                key={src}
                src={src}
                name={name}
                poster={poster}
                initialTime={initialTime}
                fill
                autoPlay
                showVolumeSlider
                enableGlobalHotkeys
                onTimeRemember={onTimeRemember}
                onError={onError}
              />
            </div>

            {errorSlot && <div className="shrink-0 border-t px-4 py-2">{errorSlot}</div>}
            {stripSlot}

            {(path || footerActions) && (
              <div className="shrink-0 flex flex-wrap items-center justify-between gap-3 border-t bg-background px-4 py-2.5">
                <div className="min-w-0 flex-1">
                  {path && <WorkspacePath value={path} compact />}
                </div>
                {footerActions && <div className="flex shrink-0 items-center gap-2">{footerActions}</div>}
              </div>
            )}
          </>
        )}
      </ResponsiveDialogContent>
    </Dialog>
  )
}
