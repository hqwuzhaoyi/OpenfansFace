'use client'

import { Loader, type LoaderVariant } from '@/components/beui/motion/loader'
import { Skeleton } from '@/components/ui/skeleton'
import { cn } from '@/lib/utils'

const TITLE_WIDTHS = ['w-2/3', 'w-3/5', 'w-3/4', 'w-1/2', 'w-4/5', 'w-3/5'] as const
const SUB_WIDTHS = ['w-1/2', 'w-2/5', 'w-3/5', 'w-1/3', 'w-1/2', 'w-2/5'] as const

export interface ArchiveListSkeletonProps {
  count?: number
  history?: boolean
  label?: string
  className?: string
}

/** Layout-matched skeleton for /integration left batch or folder list. */
export function ArchiveListSkeleton({
  count = 5,
  history = false,
  label = '正在检查视频与目标路径',
  className,
}: ArchiveListSkeletonProps) {
  return (
    <div role="status" aria-busy="true" className={cn('w-full', className)}>
      <span className="sr-only">{label}</span>
      {history ? (
        <div>
          <div className="flex items-center gap-2 border-b border-border bg-sidebar px-3 py-3">
            <Skeleton className="size-4 rounded-sm" />
            <div className="min-w-0 flex-1 space-y-1.5">
              <Skeleton className="h-3 w-32" />
              <Skeleton className="h-2.5 w-48" />
            </div>
            <Skeleton className="h-3 w-6" />
          </div>
          {Array.from({ length: count }, (_, i) => (
            <div
              key={i}
              className="flex min-h-[68px] items-center gap-2.5 border-b border-border px-3 py-2.5 pl-5"
            >
              <Skeleton className="h-[34px] w-[44px] shrink-0 rounded-[4px]" />
              <div className="min-w-0 flex-1 space-y-2">
                <Skeleton className={cn('h-3', TITLE_WIDTHS[i % TITLE_WIDTHS.length])} />
                <Skeleton className={cn('h-2.5', SUB_WIDTHS[i % SUB_WIDTHS.length])} />
              </div>
              <Skeleton className="size-2 shrink-0 rounded-full" />
            </div>
          ))}
        </div>
      ) : (
        Array.from({ length: count }, (_, i) => (
          <div
            key={i}
            className="flex items-start gap-2 border-b border-border px-3.5 py-3 pl-2.5"
          >
            <div className="grid h-9 w-8 shrink-0 place-items-center">
              <Skeleton className="size-[15px] rounded-[4px]" />
            </div>
            <div className="min-w-0 flex-1 space-y-2 pt-0.5">
              <div className="flex items-center justify-between gap-3">
                <Skeleton className={cn('h-3.5', TITLE_WIDTHS[i % TITLE_WIDTHS.length])} />
                <Skeleton className="h-2.5 w-14 shrink-0" />
              </div>
              <div className="flex items-center gap-2">
                <Skeleton className="h-3 w-16 rounded-sm" />
                <Skeleton className="h-3 w-12 rounded-sm" />
              </div>
              <Skeleton className={cn('h-2.5', SUB_WIDTHS[(i + 1) % SUB_WIDTHS.length])} />
            </div>
          </div>
        ))
      )}
    </div>
  )
}

export interface ArchiveDetailSkeletonProps {
  label?: string
  className?: string
}

/** Layout-matched skeleton for /integration right detail & player workspace. */
export function ArchiveDetailSkeleton({
  label = '正在读取归档计划',
  className,
}: ArchiveDetailSkeletonProps) {
  return (
    <div role="status" aria-busy="true" className={cn('flex flex-col gap-5', className)}>
      <span className="sr-only">{label}</span>

      <div className="flex items-start justify-between gap-4">
        <div className="min-w-0 flex-1 space-y-2">
          <Skeleton className="h-5 w-56 max-w-full" />
          <Skeleton className="h-3 w-36" />
        </div>
        <Skeleton className="h-6 w-20 shrink-0 rounded-[5px]" />
      </div>

      <div className="flex aspect-video max-h-[38dvh] min-h-[180px] w-full items-center justify-center rounded-[10px] border border-border bg-card">
        <Loader variant="dot-matrix" size={24} decorative className="text-muted-foreground" />
      </div>

      <div className="space-y-4 rounded-[6px] border border-border p-4">
        <div className="flex items-start gap-3">
          <Skeleton className="mt-0.5 size-4 shrink-0 rounded-sm" />
          <div className="min-w-0 flex-1 space-y-1.5">
            <Skeleton className="h-2.5 w-20" />
            <Skeleton className="h-3.5 w-5/6" />
          </div>
        </div>
        <div className="flex items-center gap-3 pl-0.5">
          <Skeleton className="size-3.5 rounded-full" />
          <Skeleton className="h-2.5 w-28" />
        </div>
        <div className="flex items-start gap-3">
          <Skeleton className="mt-0.5 size-4 shrink-0 rounded-sm" />
          <div className="min-w-0 flex-1 space-y-1.5">
            <Skeleton className="h-2.5 w-24" />
            <Skeleton className="h-3.5 w-4/5" />
          </div>
        </div>
      </div>

      <div className="flex items-center gap-3 border-b border-border py-4">
        <Skeleton className="size-9 shrink-0 rounded-full" />
        <div className="min-w-0 flex-1 space-y-1.5">
          <Skeleton className="h-2.5 w-16" />
          <Skeleton className="h-3.5 w-32" />
        </div>
        <Skeleton className="h-3 w-24 shrink-0" />
      </div>
    </div>
  )
}

export interface StageMediaLoaderProps {
  label?: string
  showLabel?: boolean
  variant?: LoaderVariant
  size?: number
  compact?: boolean
  className?: string
}

/** Centered BEUI loader for video stages & reference preview slots. */
export function StageMediaLoader({
  label = '正在读取视频',
  showLabel = false,
  variant = 'dot-matrix',
  size = 24,
  compact = false,
  className,
}: StageMediaLoaderProps) {
  return (
    <div
      role="status"
      aria-busy="true"
      className={cn(
        'flex w-full flex-col items-center justify-center gap-2.5 text-muted-foreground',
        compact ? 'min-h-[120px] py-6' : 'h-full min-h-[180px] py-10',
        className,
      )}
    >
      <Loader variant={variant} size={size} decorative className="text-muted-foreground" />
      {showLabel ? (
        <span className="text-xs text-muted-foreground">{label}</span>
      ) : (
        <span className="sr-only">{label}</span>
      )}
    </div>
  )
}

export interface ReviewQueueSkeletonProps {
  count?: number
  label?: string
  className?: string
}

/** Layout-matched skeleton for /review queue summary rows. */
export function ReviewQueueSkeleton({
  count = 6,
  label = '正在读取待确认视频',
  className,
}: ReviewQueueSkeletonProps) {
  return (
    <div role="status" aria-busy="true" className={cn('w-full', className)}>
      <span className="sr-only">{label}</span>
      {Array.from({ length: count }, (_, i) => (
        <div
          key={i}
          className="flex items-center gap-2.5 border-b border-border px-2.5 py-2.5"
        >
          <Skeleton className="size-[18px] shrink-0 rounded-[5px]" />
          <Skeleton className="h-[48px] w-[76px] shrink-0 rounded-[6px]" />
          <div className="min-w-0 flex-1 space-y-2">
            <Skeleton className={cn('h-3.5', TITLE_WIDTHS[i % TITLE_WIDTHS.length])} />
            <Skeleton className={cn('h-2.5', SUB_WIDTHS[i % SUB_WIDTHS.length])} />
            <div className="flex items-center gap-1.5">
              <Skeleton className="h-4 w-14 rounded-full" />
              <Skeleton className="h-4 w-12 rounded-full" />
            </div>
          </div>
        </div>
      ))}
    </div>
  )
}

export type WorkbenchListSkeletonVariant = 'default' | 'person' | 'health' | 'task' | 'card'

export interface WorkbenchListSkeletonProps {
  count?: number
  variant?: WorkbenchListSkeletonVariant
  label?: string
  className?: string
}

/** Reusable BEUI/ReUI skeleton rows for /staging, /staging#video-health, /operations, /persons. */
export function WorkbenchListSkeleton({
  count = 5,
  variant = 'default',
  label = '正在加载列表',
  className,
}: WorkbenchListSkeletonProps) {
  return (
    <div role="status" aria-busy="true" className={cn('w-full', className)}>
      <span className="sr-only">{label}</span>
      {variant === 'person' ? (
        <div className="space-y-1">
          {Array.from({ length: count }, (_, i) => (
            <div
              key={i}
              className="flex min-h-[64px] items-center gap-2 rounded-[8px] px-1.5 py-2"
            >
              <Skeleton className="size-[16px] shrink-0 rounded-[4px]" />
              <Skeleton className="size-[26px] shrink-0 rounded-full" />
              <div className="min-w-0 flex-1 space-y-1.5">
                <Skeleton className={cn('h-3.5', TITLE_WIDTHS[i % TITLE_WIDTHS.length])} />
                <Skeleton className={cn('h-2.5', SUB_WIDTHS[i % SUB_WIDTHS.length])} />
              </div>
              <Skeleton className="size-6 shrink-0 rounded-[6px]" />
            </div>
          ))}
        </div>
      ) : variant === 'health' ? (
        <div className="space-y-1">
          {Array.from({ length: count }, (_, i) => (
            <div
              key={i}
              className="grid grid-cols-[minmax(0,1fr)_120px] items-center gap-3 rounded-[8px] px-3 py-3 xl:grid-cols-[minmax(0,1fr)_140px_110px]"
            >
              <div className="flex min-w-0 items-center gap-3">
                <Skeleton className="h-10 w-9 shrink-0 rounded-[8px]" />
                <div className="min-w-0 flex-1 space-y-2">
                  <Skeleton className={cn('h-3.5', TITLE_WIDTHS[i % TITLE_WIDTHS.length])} />
                  <Skeleton className={cn('h-2.5', SUB_WIDTHS[i % SUB_WIDTHS.length])} />
                </div>
              </div>
              <div className="space-y-1.5">
                <Skeleton className="h-3 w-16" />
                <Skeleton className="h-2.5 w-12" />
              </div>
              <Skeleton className="hidden h-3 w-20 xl:block" />
            </div>
          ))}
        </div>
      ) : variant === 'task' ? (
        <div className="divide-y divide-border">
          {Array.from({ length: count }, (_, i) => (
            <div key={i} className="flex items-center gap-3 px-3 py-2.5">
              <Skeleton className="h-9 w-12 shrink-0 rounded-[6px]" />
              <div className="min-w-0 flex-1 space-y-1.5">
                <Skeleton className={cn('h-3.5', TITLE_WIDTHS[i % TITLE_WIDTHS.length])} />
                <Skeleton className={cn('h-2.5', SUB_WIDTHS[i % SUB_WIDTHS.length])} />
              </div>
              <Skeleton className="h-5 w-16 shrink-0 rounded-full" />
            </div>
          ))}
        </div>
      ) : variant === 'card' ? (
        <div className="grid gap-3">
          {Array.from({ length: count }, (_, i) => (
            <div
              key={i}
              className="space-y-3 rounded-[12px] border border-border bg-card p-3.5"
            >
              <div className="flex items-center gap-2.5">
                <Skeleton className="size-9 shrink-0 rounded-[8px]" />
                <div className="min-w-0 flex-1 space-y-1.5">
                  <Skeleton className={cn('h-3.5', TITLE_WIDTHS[i % TITLE_WIDTHS.length])} />
                  <Skeleton className={cn('h-2.5', SUB_WIDTHS[i % SUB_WIDTHS.length])} />
                </div>
              </div>
              <Skeleton className="h-2.5 w-full" />
              <div className="flex items-center justify-between gap-2 pt-1">
                <Skeleton className="h-7 w-24 rounded-[6px]" />
                <Skeleton className="h-7 w-16 rounded-[6px]" />
              </div>
            </div>
          ))}
        </div>
      ) : (
        <div className="space-y-1">
          {Array.from({ length: count }, (_, i) => (
            <div key={i} className="flex items-center gap-2.5 rounded-[8px] px-2.5 py-2.5">
              <Skeleton className="size-[18px] shrink-0 rounded-[5px]" />
              <Skeleton className="h-10 w-9 shrink-0 rounded-[8px]" />
              <div className="min-w-0 flex-1 space-y-2">
                <Skeleton className={cn('h-3.5', TITLE_WIDTHS[i % TITLE_WIDTHS.length])} />
                <Skeleton className={cn('h-2.5', SUB_WIDTHS[i % SUB_WIDTHS.length])} />
              </div>
              <Skeleton className="h-3 w-14 shrink-0" />
            </div>
          ))}
        </div>
      )}
    </div>
  )
}

export interface PersonLibrarySkeletonProps {
  count?: number
  label?: string
  className?: string
}

/** Layout-matched skeleton grid for /persons video library cards. */
export function PersonLibrarySkeleton({
  count = 6,
  label = '正在读取人物目录视频',
  className,
}: PersonLibrarySkeletonProps) {
  return (
    <div role="status" aria-busy="true" className={cn('pw-clips', className)}>
      <span className="sr-only">{label}</span>
      {Array.from({ length: count }, (_, i) => (
        <div key={i} className="pw-card">
          <div className="pw-thumb">
            <Skeleton className="h-full w-full rounded-none" />
          </div>
          <div className="pw-card-body">
            <div className="min-h-[38px] space-y-1.5">
              <Skeleton className={cn('h-3.5', TITLE_WIDTHS[i % TITLE_WIDTHS.length])} />
              <Skeleton className="h-3 w-1/2" />
            </div>
            <div className="mt-2 flex items-center justify-between gap-2">
              <Skeleton className="h-2.5 w-16" />
              <Skeleton className="h-2.5 w-12" />
            </div>
            <div className="pw-card-actions">
              <Skeleton className="h-6 w-12 rounded-[6px]" />
              <div className="flex items-center gap-1.5">
                <Skeleton className="size-6 rounded-[6px]" />
                <Skeleton className="size-6 rounded-[6px]" />
                <Skeleton className="size-6 rounded-[6px]" />
              </div>
            </div>
          </div>
        </div>
      ))}
    </div>
  )
}

export interface InlineBeuiLoaderProps {
  label?: string
  showLabel?: boolean
  variant?: LoaderVariant
  size?: number
  className?: string
}

/** Compact inline BEUI loader for account links, merge previews, storage stats, and headers. */
export function InlineBeuiLoader({
  label = '加载中',
  showLabel = false,
  variant = 'spinner',
  size = 14,
  className,
}: InlineBeuiLoaderProps) {
  return (
    <span
      role="status"
      aria-busy="true"
      className={cn(
        'inline-flex items-center gap-2 text-xs text-muted-foreground',
        className,
      )}
    >
      <Loader variant={variant} size={size} decorative className="shrink-0 text-muted-foreground" />
      {showLabel ? (
        <span>{label}</span>
      ) : (
        <span className="sr-only">{label}</span>
      )}
    </span>
  )
}
