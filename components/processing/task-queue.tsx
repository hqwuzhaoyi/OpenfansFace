'use client'

import Link from 'next/link'
import { useCallback, useEffect, useRef, useState } from 'react'
import { Activity, ArrowUp, CirclePause, CirclePlay, RefreshCw, X, ChevronLeft, ChevronRight, SlidersHorizontal, FileVideo } from 'lucide-react'
import { workspaceToast as toast } from '@/components/workbench/feedback'
import { api } from '@/lib/face-api'
import {
  type ProcessingTaskRecord,
  type TaskAttemptRecord,
  type TaskListApiResponse,
  TASK_STAGE_LABELS,
  TASK_ERROR_LABELS,
  formatTaskDuration,
  formatLeaseRemaining,
  getNodeDisplay,
  getAttemptDisplay,
  formatCreatorBadge,
  formatErrorType,
  isOcrFailureType,
  formatOcrFailureBadge,
  formatWaitingReason,
  formatWaitingReasonDescription,
} from '@/lib/processing-task-model'
import { Button as NavigationButton } from '@/components/ui/button'
import { WorkspaceButton as Button, WorkspaceSelect, WorkspaceField, WorkspaceMedia, WorkspacePath, WorkspaceSection, WorkspaceMessage, getMediaPosition, rememberMediaPosition } from '@/components/workbench/inner'
import { VideoPreviewDialog } from '@/components/workbench/video-preview-dialog'
import { useWorkspaceQuery, useWorkspaceScroll, readWorkbenchCache, writeWorkbenchCache } from '@/components/workbench/state'
import * as Popover from '@radix-ui/react-popover'
import { InputGroupButton } from '@/components/ui/input-group'
import { WorkspaceSearch, WorkspaceTabs, useWideWorkspace } from '@/components/workbench/controls'
import { Dialog, DialogTitle, DialogDescription } from '@/components/ui/dialog'
import { ResponsiveDialogContent } from '@/components/review/responsive-dialog'
import { Drawer } from '@/components/beui/motion/drawer'
import { type HomeTasks } from '@/lib/home-model'
import './operations.css'

const date = (n?: number) =>
  n
    ? new Date(n * 1000).toLocaleString('zh-CN', {
        month: '2-digit',
        day: '2-digit',
        hour: '2-digit',
        minute: '2-digit',
      })
    : '尚未完成'

export default function ProcessingTaskQueue() {
  const { values: filters, ready: initialized, update: updateFilters } = useWorkspaceQuery(
    { stage: '', batch: '', node_id: '', waiting_reason: '', q: '', page: '0' },
    { stage: ['', 'queued', 'ocr', 'detect', 'feature', 'text', 'face', 'face_queued', 'running', 'review', 'done', 'error', 'excluded', 'retry'] }
  )
  const { stage, batch, node_id, waiting_reason, q: query } = filters
  const page = Number(filters.page)
  const [search, setSearch] = useState('')
  const [actionError, setActionError] = useState('')
  const [loadedQuery, setLoadedQuery] = useState('')
  const requestKey = JSON.stringify([stage, batch, node_id, waiting_reason, search, page])
  const [data, setData] = useState<TaskListApiResponse | null>(
    () => readWorkbenchCache<TaskListApiResponse>('operations:tasks:' + requestKey) ??
          readWorkbenchCache<TaskListApiResponse>('operations:tasks:last') ?? null
  )
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  const setQuery = (q: string) => updateFilters({ q, page: '0' })
  const setBatch = (batch: string) => updateFilters({ batch, page: '0' })
  const setNode = (node_id: string) => updateFilters({ node_id, page: '0' })
  const setWaitingReason = (waiting_reason: string) => updateFilters({ waiting_reason, page: '0' })
  const setPage = (page: number) => updateFilters({ page: String(page) })
  const [live, setLive] = useState<HomeTasks | null>(() => readWorkbenchCache<HomeTasks>('home:tasks') ?? null)
  const [progressError, setProgressError] = useState('')
  const [now, setNow] = useState(Date.now() / 1000)
  const [instantEntry, setInstantEntry] = useState(false)
  const [focused, setFocused] = useState<ProcessingTaskRecord | null>(null)
  const [detailOpen, setDetailOpen] = useState(false)
  const [attemptDrawerOpen, setAttemptDrawerOpen] = useState(false)
  const [lightboxOpen, setLightboxOpen] = useState(false)
  const [taskAttempts, setTaskAttempts] = useState<TaskAttemptRecord[]>([])
  const [nodesList, setNodesList] = useState<{ node_id: string; hostname?: string }[]>([])
  const wide = useWideWorkspace()

  useEffect(() => {
    api<{ nodes: { node_id: string; hostname?: string }[] }>('/api/processing/nodes')
      .then((res) => {
        if (res?.nodes && Array.isArray(res.nodes)) setNodesList(res.nodes)
      })
      .catch(() => {})
  }, [])

  useEffect(() => {
    if (!focused?.vid) {
      setTaskAttempts([])
      return
    }
    if (Array.isArray(focused.attempts_history) && focused.attempts_history.length > 0) {
      setTaskAttempts(focused.attempts_history)
    }
    let cancelled = false
    // Fetch task attempts from Next.js API route (proxies to backend /v1/agent/task-attempts?vid=)
    api<{ ok: boolean; attempts: TaskAttemptRecord[] }>(`/api/processing/task-attempts?vid=${encodeURIComponent(focused.vid)}`)
      .then((res) => {
        if (!cancelled && res?.ok && Array.isArray(res.attempts)) {
          setTaskAttempts(res.attempts)
        }
      })
      .catch(() => {
        if (!cancelled) setTaskAttempts(focused?.attempts_history || [])
      })
    return () => {
      cancelled = true
    }
  }, [focused?.vid, focused?.attempts_history])
  const returnFocus = useRef<HTMLElement | null>(null)
  const focusId = focused?.vid || ''
  const focusedMediaPath = focused?.video_path || focused?.path || ''
  const listScroll = useWorkspaceScroll(
    'operations:' + JSON.stringify([stage, batch, node_id, waiting_reason, search, data?.page ?? page]),
    !!data && query === search && loadedQuery === requestKey
  )
  const sequence = useRef(0)

  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now() / 1000), 1000)
    return () => clearInterval(timer)
  }, [])

  useEffect(() => {
    if (wide || !focused) setDetailOpen(false)
    if (!focused) setLightboxOpen(false)
  }, [wide, focused])

  const updateStage = (stage: string) => updateFilters({ stage, page: '0' })
  useEffect(() => {
    const timer = setTimeout(() => { setSearch(query) }, 300)
    return () => clearTimeout(timer)
  }, [query])

  const load = useCallback(async () => {
    if (!initialized) return
    const key = JSON.stringify([stage, batch, node_id, waiting_reason, search, page])
    const cached = readWorkbenchCache<TaskListApiResponse>('operations:tasks:' + key)
    if (cached) {
      setData(cached)
      setLoadedQuery(key)
    }
    const token = ++sequence.current
    const params = new URLSearchParams({
      stage,
      batch,
      ...(node_id ? { node_id } : {}),
      ...(waiting_reason ? { waiting_reason } : {}),
      q: search,
      page: String(page),
      ...(focusId ? { vid: focusId } : {}),
    })

    const [queue, progress] = await Promise.allSettled([
      api<TaskListApiResponse>('/api/processing/tasks?' + params, { signal: AbortSignal.timeout(10000) }),
      api<HomeTasks>('/api/home/tasks', { signal: AbortSignal.timeout(10000) }),
    ])

    if (token !== sequence.current) return
    if (queue.status === 'fulfilled') {
      writeWorkbenchCache('operations:tasks:' + key, queue.value)
      writeWorkbenchCache('operations:tasks:last', queue.value)
      setData(queue.value)
      setLoadedQuery(key)
      setError('')
      if (page !== queue.value.page) updateFilters({ page: String(queue.value.page) })
      setFocused((previous) =>
        previous
          ? (queue.value.task == null
            ? queue.value.tasks.find((t) => t.vid === previous.vid) || previous
            : queue.value.task)
          : null
      )
    } else {
      setError(queue.reason?.message || '任务状态读取失败')
    }

    if (progress.status === 'fulfilled') {
      writeWorkbenchCache('home:tasks', progress.value)
      setLive(progress.value)
      setProgressError('')
    } else {
      setProgressError(progress.reason?.message || '实时进度读取失败')
    }
  }, [initialized, stage, batch, node_id, waiting_reason, search, page, focusId, updateFilters])

  useEffect(() => {
    let stop = false
    let timer: ReturnType<typeof setTimeout>
    const run = async () => {
      await load()
      if (!stop) timer = setTimeout(run, 5000)
    }
    run()
    return () => {
      stop = true
      sequence.current++
      clearTimeout(timer)
    }
  }, [load])

  const act = async (action: string, vid?: string) => {
    setBusy(true)
    setActionError('')
    try {
      await api('/api/processing', { method: 'POST', body: JSON.stringify({ action, vids: vid ? [vid] : undefined }) })
      await load()
      toast.success(
        {
          pause: '已暂停，当前阶段完成后停止领取新任务',
          resume: '已恢复自动分析',
          priority: '已提高任务优先级',
          retry: '已重新排队',
          takeover: '已转入人工确认',
        }[action] || '已更新'
      )
    } catch (e: any) {
      setActionError(e.message)
      toast.error(e.message)
    } finally {
      setBusy(false)
    }
  }

  const cards = [
    { key: 'queued', title: '排队中', value: (data?.counts.queued || 0) + (data?.counts.retry || 0) },
    { key: 'ocr', title: '文字识别', value: (data?.counts.ocr || 0) + (data?.counts.text || 0) },
    { key: 'detect', title: '人脸检测', value: data?.counts.detect || 0 },
    { key: 'feature', title: '特征提取', value: (data?.counts.feature || 0) + (data?.counts.face || 0) + (data?.counts.face_queued || 0) },
    { key: 'review', title: '分析就绪', value: data?.counts.review || 0 },
    { key: 'done', title: '已完成', value: data?.counts.done || 0 },
    { key: 'error', title: '异常', value: data?.counts.error || 0 },
  ]

  const inspect = (task: ProcessingTaskRecord) => {
    setActionError('')
    returnFocus.current = document.activeElement as HTMLElement
    setInstantEntry(!!document.activeElement?.matches(':focus-visible') || matchMedia('(prefers-reduced-motion: reduce)').matches)
    setFocused(task)
    if (!wide) setDetailOpen(true)
  }

  const taskActions = (task: ProcessingTaskRecord) =>
    task.stage === 'review' || task.manual ? (
      <NavigationButton size="sm" asChild>
        <Link href={`/review?vid=${encodeURIComponent(task.vid)}`}>
          去人工确认<ChevronRight size={14} />
        </Link>
      </NavigationButton>
    ) : ['error', 'retry'].includes(task.stage) ? (
      <>
        <Button size="sm" variant="outline" disabled={busy} onClick={() => act('retry', task.vid)}>
          重试
        </Button>
        {task.stage === 'error' && (
          <Button size="sm" variant="outline" disabled={busy} onClick={() => act('takeover', task.vid)}>
            人工接管
          </Button>
        )}
      </>
    ) : (
      <Button
        size="sm"
        variant="outline"
        disabled={busy || ['text', 'face', 'detect', 'ocr', 'feature'].includes(task.stage)}
        onClick={() => act('priority', task.vid)}
      >
        <ArrowUp size={14} />优先处理
      </Button>
    )

  const detail = focused && (
    <>
      <div className="ws-detail-heading">
        <h2>任务详情</h2>
        {wide && (
          <Button
            className="ws-icon"
            variant="ghost"
            size="sm"
            aria-label="关闭任务详情"
            title="关闭详情"
            onClick={() => setFocused(null)}
          >
            <X size={15} />
          </Button>
        )}
      </div>
      <div className="ws-detail-content" data-sheet-scroll>
        <div className="ws-task-info">
          <h3 className="break-all text-sm font-medium">{focused.name}</h3>
          {focusedMediaPath ? (
            !attemptDrawerOpen && !lightboxOpen && (wide || detailOpen) ? (
              <WorkspaceMedia
                path={focusedMediaPath}
                name={focused.name}
                onExpand={() => setLightboxOpen(true)}
              />
            ) : (
              <div className="rounded-md border bg-muted/30 p-3 text-center text-xs text-muted-foreground">
                {lightboxOpen ? '已在大窗中打开视频预览' : '视频正在历史尝试抽屉中预览'}
              </div>
            )
          ) : (
            <p className="text-xs text-muted-foreground p-3 border rounded-md text-center">暂无有效视频路径</p>
          )}
          <dl>
            <dt>当前阶段</dt>
            <dd>{focused.manual ? '人工接管' : (TASK_STAGE_LABELS[focused.stage] || focused.stage)}</dd>
            <dt>执行节点</dt>
            <dd>{getNodeDisplay(focused.assigned_node_id)}</dd>
            <dt>执行尝试</dt>
            <dd><code className="font-mono text-xs">{getAttemptDisplay(focused.attempt_id)}</code></dd>
            <dt>有效租约</dt>
            <dd>{formatLeaseRemaining(focused.lease_expires_at, now)}</dd>
            <dt>单次耗时</dt>
            <dd>{formatTaskDuration(focused.duration_ms)}</dd>
            <dt>代际隔离</dt>
            <dd>{focused.coordinator_generation ? String(focused.coordinator_generation) : '未知'}</dd>
            <dt>文件大小</dt>
            <dd>{typeof focused.size === 'number' && focused.size > 0 ? `${(focused.size / 2 ** 30).toFixed(2)} GB` : '未知'}</dd>
            <dt>发现批次</dt>
            <dd>{focused.batch_id || '未知'}</dd>
            <dt>更新时间</dt>
            <dd>{date(focused.updated_at)}</dd>
            <dt>处理次数</dt>
            <dd>文字 {focused.text_attempts} · 人脸 {focused.face_attempts}</dd>
            {taskAttempts.length > 0 && (
              <>
                <dt>尝试与去重</dt>
                <dd className="space-y-1.5">
                  <div className="flex items-center justify-between pb-1">
                    <span className="text-muted-foreground text-xs">{taskAttempts.length} 次尝试记录</span>
                    <Button
                      variant="outline"
                      size="sm"
                      className="text-xs h-7 px-2"
                      onClick={() => setAttemptDrawerOpen(true)}
                    >
                      查看完整时间线
                    </Button>
                  </div>
                  {taskAttempts.map((att) => {
                    const isDone = att.status === 'done' || att.status === 'completed'
                    const isInterrupted = att.status === 'interrupted'
                    const isError = att.status === 'error' || att.status === 'failed'
                    const statusClass = isDone
                      ? 'bg-emerald-500/10 text-emerald-600 dark:text-emerald-400'
                      : isInterrupted
                      ? 'bg-amber-500/10 text-amber-600 dark:text-amber-400'
                      : isError
                      ? 'bg-destructive/10 text-destructive'
                      : 'bg-muted text-muted-foreground'
                    const dedupCount = Math.max(att.delivery_count ?? 0, att.redelivery_count ?? 0)

                    return (
                      <div key={att.attempt_id} className="flex flex-wrap items-center gap-1.5 text-xs font-mono">
                        <span className="font-semibold">{TASK_STAGE_LABELS[att.stage] || att.stage}</span>
                        <span className="text-muted-foreground">{getAttemptDisplay(att.attempt_id)}</span>
                        <span className={`px-1.5 py-0.5 rounded text-[11px] font-sans ${statusClass}`}>
                          {isDone
                            ? '已完成'
                            : isInterrupted
                            ? '中断回收'
                            : isError
                            ? '失败'
                            : '运行中'}
                        </span>
                        {((att.delivery_count > 1) || ((att.redelivery_count ?? 0) > 1)) && (
                          <span className="px-1.5 py-0.5 rounded text-[11px] font-sans bg-blue-500/10 text-blue-600 dark:text-blue-400 font-medium">
                            幂等去重 x{dedupCount}
                          </span>
                        )}
                        {typeof att.duration_ms === 'number' && (
                          <span className="text-muted-foreground">({formatTaskDuration(att.duration_ms)})</span>
                        )}
                      </div>
                    )
                  })}
                </dd>
              </>
            )}
            {focused.waiting_reason && focused.waiting_reason !== 'none' && (
              <>
                <dt>等待原因</dt>
                <dd>
                  <span className="font-medium text-amber-600 dark:text-amber-400">
                    {formatWaitingReason(focused.waiting_reason)}
                  </span>
                  <span className="block text-xs text-muted-foreground mt-0.5">
                    {formatWaitingReasonDescription(focused.waiting_reason)}
                  </span>
                </dd>
              </>
            )}
            {(focused.face_skipped || focused.data?.face_skipped) && (focused.creator || focused.data?.creator || focused.matched_creator) && (
              <>
                <dt>作者规则匹配</dt>
                <dd className="font-medium text-emerald-600 dark:text-emerald-400">
                  {formatCreatorBadge(focused.creator || focused.data?.creator || focused.matched_creator)}
                </dd>
              </>
            )}
            {(focused.ocr_result === 'no_text_found' || focused.data?.ocr_result === 'no_text_found') && (
              <>
                <dt>OCR 结果</dt>
                <dd>未检出文字</dd>
              </>
            )}
            {(focused.error_type || focused.data?.error_type) && (
              <>
                <dt>错误类型</dt>
                <dd className="font-medium text-destructive">
                  {formatErrorType(focused.error_type || focused.data?.error_type)}
                </dd>
              </>
            )}
            {(focused.ocr_text || focused.data?.ocr_text) && (
              <>
                <dt>OCR 检出文字</dt>
                <dd className="break-all font-mono text-xs">
                  {Array.isArray(focused.ocr_text || focused.data?.ocr_text)
                    ? ((focused.ocr_text || focused.data?.ocr_text) as any[])
                        .map((s: any) => (typeof s === 'string' ? s : s?.text || JSON.stringify(s)))
                        .join(' | ')
                    : String(focused.ocr_text || focused.data?.ocr_text)}
                </dd>
              </>
            )}
          </dl>
          {focused.video_path || focused.path ? (
            <WorkspacePath value={focused.video_path || focused.path} />
          ) : (
            <span className="text-muted-foreground text-xs">暂无有效文件路径</span>
          )}
          {focused.error && <WorkspaceMessage error>{focused.error}</WorkspaceMessage>}
          {actionError && <WorkspaceMessage error>{actionError}</WorkspaceMessage>}
          {focused.stage === 'retry' && (
            <p className="ws-muted">{Math.max(0, Math.ceil(focused.next_retry - now))} 秒后重试</p>
          )}
          {focused.data?.face_skipped && <p className="ws-muted">已确认账号，无需补脸</p>}
        </div>
      </div>
      <div className="ws-detail-actions">{taskActions(focused)}</div>
    </>
  )

  const tasksList = data?.tasks || []

  return (
    <main className="ws-page" aria-label="处理任务列表工作区">
      <header className="ws-header">
        <div className="flex items-center gap-3">
          <NavigationButton size="sm" variant="ghost" asChild>
            <Link href="/operations?tab=tasks&view=dashboard">返回看板</Link>
          </NavigationButton>
          <h1>任务列表</h1>
        </div>
        <div className="ws-actions">
          <Button
            variant="outline"
            size="sm"
            disabled={busy || !data}
            onClick={() => act(data?.paused ? 'resume' : 'pause')}
          >
            {data?.paused ? <CirclePlay size={14} /> : <CirclePause size={14} />}
            <span>{data?.paused ? '恢复分析' : '暂停分析'}</span>
          </Button>
          <Button variant="ghost" className="ws-icon" aria-label="刷新处理状态" title="刷新" onClick={load}>
            <RefreshCw size={15} />
          </Button>
        </div>
      </header>
      {(error || progressError) && <p role="alert" className="ws-error">{error || progressError}</p>}
      {data?.last_discovery?.error && (
        <p role="alert" className="ws-error">发现失败：{data.last_discovery.error}</p>
      )}
      <div className="ws-view-tabs">
        <WorkspaceTabs
          value={stage || 'all'}
          onChange={(value) => updateStage(value === 'all' ? '' : value)}
          label="任务阶段"
          items={[
            { value: 'all', label: '全部', count: data?.summary?.all_count ?? data?.all_count },
            ...cards.map((c) => ({ value: c.key, label: c.title, count: data ? c.value : undefined })),
          ]}
        />
      </div>
      <div className="ws-operations-body">
        <section className="ws-queue" aria-label="处理任务列表">
          <div className="ws-toolbar">
            <WorkspaceSearch value={query} onChange={setQuery} label="搜索处理任务" placeholder="搜索文件名或路径…">
              <Popover.Root>
                <Popover.Trigger asChild>
                  <InputGroupButton size="icon-sm" aria-label="筛选发现批次与节点" title="筛选批次与节点">
                    <SlidersHorizontal size={15} />
                    {(batch || node_id || waiting_reason) && <span className="size-1.5 rounded-full bg-current" />}
                  </InputGroupButton>
                </Popover.Trigger>
                <Popover.Portal>
                  <Popover.Content className="ws-filter-content" align="end" sideOffset={6}>
                    <WorkspaceField label="发现批次">
                      <WorkspaceSelect
                        label="选择发现批次"
                        value={batch}
                        onChange={setBatch}
                        options={[
                          { value: '', label: '全部批次' },
                          ...(data?.batches || []).map((b) => ({ value: b.id, label: `${b.id} · ${b.total}` })),
                        ]}
                      />
                    </WorkspaceField>
                    <WorkspaceField label="执行节点">
                      <WorkspaceSelect
                        label="选择执行节点"
                        value={node_id}
                        onChange={setNode}
                        options={[
                          { value: '', label: '全部节点' },
                          ...nodesList.map((n) => ({
                            value: n.node_id,
                            label: `${n.hostname || n.node_id} (${getNodeDisplay(n.node_id)})`,
                          })),
                        ]}
                      />
                    </WorkspaceField>
                    <WorkspaceField label="等待状态">
                      <WorkspaceSelect
                        label="选择等待状态"
                        value={waiting_reason || ''}
                        onChange={setWaitingReason}
                        options={[
                          { value: '', label: '全部等待状态' },
                          { value: 'waiting_for_node', label: '等待计算节点' },
                          { value: 'quota_exhausted', label: '并发配额超限' },
                          { value: 'gpu_busy', label: 'GPU 显存繁忙' },
                          { value: 'rate_limited', label: '并发降频限流' },
                          { value: 'retry_backoff', label: '重试退避等待' },
                          { value: 'model_warmup', label: '模型预热自检' },
                          { value: 'paused', label: '人工暂停接单' },
                          { value: 'none', label: '无等待' },
                        ]}
                      />
                    </WorkspaceField>
                    {(batch || node_id || waiting_reason) && (
                      <Button
                        size="sm"
                        variant="ghost"
                        className="mt-3"
                        onClick={() => {
                          setBatch('')
                          setNode('')
                          setWaitingReason('')
                          setPage(0)
                        }}
                      >
                        清除筛选
                      </Button>
                    )}
                  </Popover.Content>
                </Popover.Portal>
              </Popover.Root>
            </WorkspaceSearch>
            <span className="ws-muted shrink-0">{data?.total ?? '—'} 个</span>
          </div>
          <div ref={listScroll.ref} className="ws-list" onScroll={listScroll.onScroll}>
            {tasksList.map((task) => (
              <article
                key={task.vid}
                className="ws-row ws-operations-row"
                data-active={task.vid === focused?.vid}
                data-processing={
                  !!live?.jobs.some((job) => job.status === 'running' && job.vid === task.vid)
                }
              >
                <button
                  type="button"
                  className="ws-row-open"
                  aria-label={`查看任务 ${task.name}`}
                  onClick={() => inspect(task)}
                >
                  <span className="ws-row-thumb">
                    {live?.jobs.some((job) => job.status === 'running' && job.vid === task.vid) ? (
                      <Activity size={18} />
                    ) : (
                      <FileVideo size={18} />
                    )}
                  </span>
                  <span className="ws-row-copy">
                    <span className="ws-row-name" title={task.name}>{task.name}</span>
                    <span className="ws-row-meta" title={task.path}>
                      {task.assigned_node_id && (
                        <button
                          type="button"
                          className="text-muted-foreground mr-1.5 font-mono text-[11px] hover:text-foreground underline decoration-dotted"
                          title={`筛选节点 ${task.assigned_node_id}`}
                          onClick={(e) => {
                            e.stopPropagation()
                            setNode(task.assigned_node_id!)
                          }}
                        >
                          [{getNodeDisplay(task.assigned_node_id)}]
                        </button>
                      )}
                      {task.path}
                    </span>
                  </span>
                  <span className="ws-row-status">
                    {task.manual ? (
                      '人工接管'
                    ) : (task.face_skipped || task.data?.face_skipped) && (task.creator || task.data?.creator || task.matched_creator) ? (
                      <span className="inline-flex items-center gap-1 font-medium text-emerald-600 dark:text-emerald-400">
                        {formatCreatorBadge(task.creator || task.data?.creator || task.matched_creator)}
                      </span>
                    ) : isOcrFailureType(task.error_type || task.data?.error_type) ? (
                      <span className="font-medium text-destructive">
                        {formatOcrFailureBadge(task.stage)}
                      </span>
                    ) : (task.ocr_result === 'no_text_found' || task.data?.ocr_result === 'no_text_found') && task.stage === 'detect' ? (
                      <span>未检出文字 · 人脸检测中</span>
                    ) : (
                      TASK_STAGE_LABELS[task.stage] || task.stage
                    )}
                  </span>
                  <ChevronRight size={14} className="shrink-0 text-muted-foreground" />
                </button>
              </article>
            ))}
            {!tasksList.length && (
              <p role="status" className="ws-empty">
                {!data
                  ? '正在加载任务…'
                  : (data.summary?.all_count ?? data.all_count)
                  ? '当前筛选下没有任务。'
                  : '正在等待媒体发现。'}
              </p>
            )}
          </div>
          <div className="ws-pagination">
            <span>
              {data
                ? `第 ${data.page + 1} / ${Math.max(1, Math.ceil(data.total / data.limit))} 页`
                : '读取中'}
            </span>
            <div className="ws-actions">
              <Button
                variant="ghost"
                size="sm"
                className="ws-icon"
                aria-label="上一页"
                title="上一页"
                disabled={!data || data.page === 0}
                onClick={() => {
                  setPage(data!.page - 1)
                  listScroll.ref.current?.scrollTo(0, 0)
                }}
              >
                <ChevronLeft size={15} />
              </Button>
              <Button
                variant="ghost"
                size="sm"
                className="ws-icon"
                aria-label="下一页"
                title="下一页"
                disabled={!data || (data.page + 1) * data.limit >= data.total}
                onClick={() => {
                  setPage(data!.page + 1)
                  listScroll.ref.current?.scrollTo(0, 0)
                }}
              >
                <ChevronRight size={15} />
              </Button>
            </div>
          </div>
          <div className="ws-log">
            <WorkspaceSection title="运行记录" defaultOpen={false}>
              <p className="ws-muted">{data?.references?.people ?? '—'} 个人物参考</p>
              <p className="ws-muted">
                上次发现 {date(data?.last_discovery?.at)} · 新增 {data?.last_discovery?.registered ?? '—'} 个
              </p>
              <ol>
                {(data?.events || []).map((e) => (
                  <li key={e.id}>
                    <time>{date(e.at)}</time>
                    <span className="min-w-0 break-all">{e.message}</span>
                  </li>
                ))}
              </ol>
              <div className="flex flex-wrap gap-2 pb-3">
                {(data?.batches || []).slice(0, 4).map((b) => (
                  <Button
                    key={b.id}
                    size="sm"
                    variant="outline"
                    onClick={() => {
                      setBatch(b.id)
                      updateStage('')
                    }}
                  >
                    {b.id} · {b.ready}/{b.total}
                  </Button>
                ))}
              </div>
            </WorkspaceSection>
          </div>
        </section>
        {wide && focused && (
          <aside className="ws-inspector ws-task-inspector" aria-label="任务详情">
            {detail}
          </aside>
        )}
      </div>
      {!wide && (
        <Dialog open={detailOpen} onOpenChange={setDetailOpen}>
          <ResponsiveDialogContent
            open={detailOpen}
            className="ws-detail-sheet"
            style={{ animationDuration: instantEntry ? '0ms' : '180ms' }}
            overlayProps={{ style: { animationDuration: instantEntry ? '0ms' : '140ms' } }}
            inert={!detailOpen}
            onCloseAutoFocus={(e) => {
              if (returnFocus.current?.isConnected) {
                e.preventDefault()
                returnFocus.current.focus()
              }
            }}
          >
            <DialogTitle className="sr-only">任务详情</DialogTitle>
            <DialogDescription className="sr-only">关闭后返回当前队列位置。</DialogDescription>
            {detail}
          </ResponsiveDialogContent>
        </Dialog>
      )}

      {/* Standalone Attempt History Drawer */}
      <Drawer
        open={attemptDrawerOpen}
        onOpenChange={setAttemptDrawerOpen}
        side="right"
        ariaLabel="任务尝试历史与幂等去重抽屉"
        className="w-full sm:w-[540px] max-w-full sm:max-w-[90vw] bg-background border-l shadow-2xl p-6 overflow-y-auto"
      >
        {focused && (
          <div className="ws-attempt-drawer-panel space-y-4">
            <div className="flex items-center justify-between pb-3 border-b">
              <div>
                <h2 className="text-base font-semibold">任务执行尝试历史</h2>
                <p className="text-xs text-muted-foreground font-mono">{focused.vid}</p>
              </div>
              <Button
                variant="ghost"
                size="sm"
                onClick={() => setAttemptDrawerOpen(false)}
                aria-label="关闭尝试历史抽屉"
              >
                <X size={15} />
              </Button>
            </div>

            {/* Video Preview & Path in Drawer */}
            <div className="space-y-2">
              {focusedMediaPath ? (
                <>
                  {attemptDrawerOpen && !lightboxOpen && (
                    <WorkspaceMedia
                      path={focusedMediaPath}
                      name={focused.name}
                      onExpand={() => {
                        setAttemptDrawerOpen(false)
                        setLightboxOpen(true)
                      }}
                    />
                  )}
                  <WorkspacePath value={focusedMediaPath} />
                </>
              ) : (
                <p className="text-xs text-muted-foreground p-3 border rounded-md text-center">暂无有效视频路径</p>
              )}
            </div>

            {/* Summary Strip */}
            <div className="grid grid-cols-3 gap-2 py-2 px-3 bg-muted/40 rounded-lg text-center text-xs">
              <div>
                <span className="text-muted-foreground block">总尝试次数</span>
                <strong className="text-sm font-semibold">{taskAttempts.length} 次</strong>
              </div>
              <div>
                <span className="text-muted-foreground block">幂等去重总计</span>
                <strong className="text-sm font-semibold">
                  {taskAttempts.reduce((acc, a) => acc + Math.max(0, Math.max(a?.delivery_count ?? 0, a?.redelivery_count ?? 0, 1) - 1), 0)} 次
                </strong>
              </div>
              <div>
                <span className="text-muted-foreground block">权威结果</span>
                <strong className="text-sm font-semibold">
                  {taskAttempts.some((a) => a.is_authoritative) ? '已原子提交' : '未提交'}
                </strong>
              </div>
            </div>

            {/* Attempt Cards */}
            <div className="space-y-3 pt-2">
              {taskAttempts.map((att) => {
                const isDone = att.status === 'done' || att.status === 'completed'
                const isInterrupted = att.status === 'interrupted'
                const isError = att.status === 'error' || att.status === 'failed'
                const statusClass = isDone
                  ? 'bg-emerald-500/10 text-emerald-600 dark:text-emerald-400'
                  : isInterrupted
                  ? 'bg-amber-500/10 text-amber-600 dark:text-amber-400'
                  : isError
                  ? 'bg-destructive/10 text-destructive'
                  : 'bg-muted text-muted-foreground'
                const dedupCount = Math.max(att.delivery_count ?? 0, att.redelivery_count ?? 0)

                return (
                  <article key={att.attempt_id} className="p-3 border rounded-lg space-y-1.5 text-xs">
                    <div className="flex items-center justify-between">
                      <div className="flex items-center gap-1.5">
                        <span className="font-semibold">{TASK_STAGE_LABELS[att.stage] || att.stage}</span>
                        <code className="font-mono text-muted-foreground">{getAttemptDisplay(att.attempt_id)}</code>
                        <span className={`px-1.5 py-0.5 rounded text-[11px] ${statusClass}`}>
                          {isDone ? '已完成' : isInterrupted ? '中断回收' : isError ? '失败' : '运行中'}
                        </span>
                      </div>
                      {dedupCount > 1 && (
                        <span className="px-1.5 py-0.5 rounded text-[11px] bg-blue-500/10 text-blue-600 dark:text-blue-400 font-medium">
                          幂等去重 x{dedupCount}
                        </span>
                      )}
                    </div>
                    <div className="grid grid-cols-2 gap-1 text-muted-foreground">
                      <span>执行节点: <strong className="font-mono text-foreground">{getNodeDisplay(att.node_id)}</strong></span>
                      <span>单次耗时: <strong className="text-foreground">{formatTaskDuration(att.duration_ms)}</strong></span>
                      <span>开始时间: {date(att.started_at)}</span>
                      <span>结束时间: {att.finished_at ? date(att.finished_at) : '尚未完成'}</span>
                    </div>
                    {att.error && (
                      <p className="text-destructive bg-destructive/10 p-2 rounded text-[11px] break-all">{att.error}</p>
                    )}
                  </article>
                )
              })}
              {taskAttempts.length === 0 && (
                <p className="text-center py-6 text-muted-foreground">暂无历史尝试记录</p>
              )}
            </div>
          </div>
        )}
      </Drawer>
      {focused && focusedMediaPath && (
        <VideoPreviewDialog
          open={lightboxOpen}
          onOpenChange={setLightboxOpen}
          title={focused.name}
          subtitle={TASK_STAGE_LABELS[focused.stage] || focused.stage}
          path={focusedMediaPath}
          src={`/api/video?p=${encodeURIComponent(focusedMediaPath)}`}
          name={focused.name}
          initialTime={getMediaPosition(focusedMediaPath)}
          onTimeRemember={(t) => rememberMediaPosition(focusedMediaPath, t)}
          instantEntry={instantEntry}
        />
      )}
    </main>
  )
}
