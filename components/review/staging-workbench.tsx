'use client'

// Filespace-style layout composed from the shared BEUI/ReUI controls.
// All listing, preview, execution and recovery use the native staging gateway.
import { useCallback, useEffect, useRef, useState } from 'react'
import Link from 'next/link'
import { Button as NavigationButton } from '@/components/ui/button'
import { Archive, ChevronLeft, ChevronRight, File, FileVideo, FolderOpen, Inbox, Play, RefreshCw, RotateCcw, Trash2, X } from 'lucide-react'
import { api } from '@/lib/face-api'
import type { StagedFile, StagingList, RestorePlan, DeletePlan, DeleteResult } from '@/lib/staging-backend'
import type { PersonFileResult } from '@/lib/person-file-model'
import type { ReviewVideo } from '@/lib/review-ui'
import { WorkspaceSearch, WorkspaceTabs, useWideWorkspace } from '@/components/workbench/controls'
import { WorkspaceButton as Button, WorkspaceCard, WorkspaceField, WorkspaceInput, WorkspaceMessage, WorkspacePath, WorkspaceSection, WorkspaceSelect } from '@/components/workbench/inner'
import { Checkbox } from '@/components/beui/motion/checkbox'
import { Dialog, DialogDescription, DialogTitle } from '@/components/ui/dialog'
import { ResponsiveDialogContent } from './responsive-dialog'
import { VideoHealthPanel } from './video-health-panel'
import { VideoPlayer } from './video'
import './staging.css'

type Plan = RestorePlan | DeletePlan
type Status = { id: string; status: string; phase?: string; io_state?: string; committed?: number; completed?: number; total?: number; files?: DeleteResult['files']; result?: DeleteResult | PersonFileResult; error?: string }
type Action = 'restore-files' | 'delete-staged' | 'clear-staging'
const labels: Record<Action, string> = { 'restore-files': '恢复文件', 'delete-staged': '永久删除', 'clear-staging': '清空回收站' }
const pendingKey = 'face-staging-native-operation', legacyPendingKey = 'face-staging-native-delete'
const videoFile = /\.(mp4|m4v|mov|mkv|webm|avi|wmv|ts|mts|m2ts)$/i
const bytes = (value: number) => value < 1024 ? `${value} B` : value < 2 ** 20 ? `${(value / 1024).toFixed(1)} KB` : value < 2 ** 30 ? `${(value / 2 ** 20).toFixed(1)} MB` : `${(value / 2 ** 30).toFixed(2)} GB`
const origin = (file: StagedFile) => !file.origin?.original_path ? '来源待核对' : file.origin.action === 'stage-duplicate' ? '归档重复件' : file.origin.action?.startsWith('delete-') ? '人物删除' : '来源已记录'
const picked = ({ vid, path, version }: StagedFile) => ({ vid, path, version })
const terminal = (status?: string) => ['complete', 'partial', 'rolled_back', 'cancelled', 'recovery_needed'].includes(status || '')
const statusLabel = (state?: string) => ({ complete: '已完成', partial: '部分完成', rolled_back: '已回退', cancelled: '已取消', recovery_needed: '需要核对恢复', not_started: '尚未开始', deleted_pending_records: '内容已删除，记录待核对', failed: '失败', running: '处理中' } as Record<string, string>)[state || ''] || '处理中'
function phaseLabel(phase?: string) {
  if (/ledger/.test(phase || '')) return '同步账本'
  if (/faces|jobs|json/.test(phase || '')) return '同步文件记录'
  if (/unlink|delete/.test(phase || '')) return '永久删除'
  if (/file|source|destination|copy|directory/.test(phase || '')) return '处理文件'
  return statusLabel(phase)
}
function savedPlan(value: unknown): value is Plan {
  const plan = value as Plan | null
  return !!plan && typeof plan.id === 'string' && typeof plan.version === 'string' && plan.request?.id === plan.id && Object.hasOwn(labels, plan.request.action) && Array.isArray(plan.request.files) && Array.isArray(plan.files)
}

export function StagingWorkbench() {
  const [health, setHealth] = useState(false), [list, setList] = useState<StagingList | null>(null), [query, setQuery] = useState('')
  const [loading, setLoading] = useState(true), [selecting, setSelecting] = useState(false), [error, setError] = useState('')
  const [selected, setSelected] = useState<Map<string, StagedFile>>(new Map()), [focused, setFocused] = useState<StagedFile | null>(null)
  const [playing, setPlaying] = useState<ReviewVideo | null>(null), [instant, setInstant] = useState(false)
  const [open, setOpen] = useState(false), [action, setAction] = useState<Action>('restore-files'), [restoreFile, setRestoreFile] = useState<StagedFile | null>(null)
  const [destinationMode, setDestinationMode] = useState('original'), [person, setPerson] = useState(''), [folder, setFolder] = useState(''), [confirmation, setConfirmation] = useState('')
  const [plan, setPlan] = useState<Plan | null>(null), [status, setStatus] = useState<Status | null>(null), [preparing, setPreparing] = useState(false)
  const [submitting, setSubmitting] = useState(false), [sent, setSent] = useState(false), [uncertain, setUncertain] = useState(false), [operationError, setOperationError] = useState('')
  const requestSequence = useRef(0), previewSequence = useRef(0), selectSequence = useRef(0), queryRef = useRef(''), offsetRef = useRef(0), planRef = useRef<Plan | null>(null)
  const appliedReceipt = useRef(''), searchRef = useRef<HTMLDivElement>(null), dialogTrigger = useRef<HTMLElement | null>(null), rowTrigger = useRef<HTMLElement | null>(null), playTrigger = useRef<HTMLButtonElement | null>(null)
  const wide = useWideWorkspace()
  const busy = submitting || uncertain || (sent && !!status && !terminal(status.status) && status.status !== 'not_started')
  const setCurrentPlan = (value: Plan | null) => { planRef.current = value; setPlan(value) }
  const load = useCallback(async (offset = 0, search = queryRef.current) => {
    const sequence = ++requestSequence.current; setLoading(true)
    try {
      let next = await api<StagingList>(`/api/staging?offset=${offset}&q=${encodeURIComponent(search)}`)
      if (!next.rows.length && next.matched > 0 && offset > 0) next = await api<StagingList>(`/api/staging?offset=${Math.floor((next.matched - 1) / 100) * 100}&q=${encodeURIComponent(search)}`)
      if (sequence !== requestSequence.current) return
      offsetRef.current = next.offset; setList(next); setError('')
      setFocused(previous => previous ? next.rows.find(row => row.path === previous.path) || previous : null)
    } catch (cause) { if (sequence === requestSequence.current) setError((cause as Error).message) }
    finally { if (sequence === requestSequence.current) setLoading(false) }
  }, [])
  useEffect(() => { queryRef.current = query; selectSequence.current++; void load(0, query) }, [query, load])
  const acceptStatus = useCallback((next: Status, currentPlan: Plan) => {
    if (planRef.current?.id !== currentPlan.id || next.id !== currentPlan.id) return
    if (next.status === 'complete' && next.result && 'files' in next.result && next.result.status === 'partial') next = { ...next, status: 'partial' }
    setStatus(next)
    if (next.error) setOperationError(next.error)
    if (terminal(next.status)) {
      setUncertain(false)
      const key = JSON.stringify([next.id, next.status, next.result])
      if (appliedReceipt.current !== key) {
        appliedReceipt.current = key
        const complete = new Set(next.files?.filter(file => file.status === 'complete').map(file => file.src) || [])
        if (next.result && 'files' in next.result) for (const file of next.result.files) if (file.status === 'complete') complete.add(file.src)
        if (next.status === 'complete' && currentPlan.request.action === 'restore-files') for (const file of currentPlan.files) complete.add(file.src)
        setSelected(previous => new Map([...previous].filter(([path]) => !complete.has(path))))
        setFocused(previous => previous && complete.has(previous.path) ? null : previous)
        void load(offsetRef.current)
      }
    }
  }, [load])
  const checkStatus = useCallback(async (currentPlan: Plan) => {
    try { acceptStatus(await api<Status>(`/api/staging?id=${encodeURIComponent(currentPlan.id)}`), currentPlan) }
    catch (cause) { if (planRef.current?.id === currentPlan.id) setOperationError(`回执读取失败：${(cause as Error).message}`) }
  }, [acceptStatus])
  useEffect(() => {
    const change = () => setHealth(window.location.hash === '#video-health'); change(); window.addEventListener('hashchange', change)
    try {
      const raw = localStorage.getItem(pendingKey) || localStorage.getItem(legacyPendingKey)
      if (raw) { const previous: unknown = JSON.parse(raw); if (savedPlan(previous)) { setCurrentPlan(previous); setAction(previous.request.action); setSent(true); setUncertain(true); void checkStatus(previous) } }
    } catch { /* A browser hint never executes a mutation. */ }
    return () => { requestSequence.current++; selectSequence.current++; window.removeEventListener('hashchange', change) }
  }, [checkStatus])
  useEffect(() => {
    if (!plan || !sent || terminal(status?.status)) return
    let cancelled = false, timer: ReturnType<typeof setTimeout>
    const poll = async () => { await checkStatus(plan); if (!cancelled) timer = setTimeout(poll, 1500) }
    timer = setTimeout(poll, 1500)
    return () => { cancelled = true; clearTimeout(timer) }
  }, [plan, sent, status?.status, checkStatus])
  const rows = list?.rows || [], pageSelected = rows.filter(row => selected.has(row.path)).length
  const toggle = (row: StagedFile, checked: boolean) => setSelected(previous => { const next = new Map(previous); if (checked) next.set(row.path, row); else next.delete(row.path); return next })
  const selectResults = async () => {
    const sequence = ++selectSequence.current, search = queryRef.current; setSelecting(true)
    try {
      const all: StagedFile[] = []; let offset: number | null = 0
      do { const page: StagingList = await api(`/api/staging?offset=${offset}&q=${encodeURIComponent(search)}`); all.push(...page.rows); if (page.matched > 500 || all.length > 500) throw new Error('当前搜索结果超过 500 项，请缩小搜索范围。清空回收站会单独预览全量文件。'); offset = page.next_offset } while (offset !== null && sequence === selectSequence.current)
      if (sequence === selectSequence.current) setSelected(previous => new Map([...previous, ...all.map(row => [row.path, row] as const)]))
    } catch (cause) { if (sequence === selectSequence.current) setError((cause as Error).message) }
    finally { setSelecting(false) }
  }
  const captureTrigger = () => { dialogTrigger.current = document.activeElement as HTMLElement; setInstant(document.documentElement.dataset.input === 'keyboard' || matchMedia('(prefers-reduced-motion: reduce)').matches) }
  const prepare = async (kind: Action, files: StagedFile[]) => {
    const sequence = ++previewSequence.current; setPreparing(true); setOperationError(''); setCurrentPlan(null)
    try {
      const request = { action: kind, person: '', files: kind === 'clear-staging' ? [] : files.map(picked), ...(kind === 'restore-files' && destinationMode === 'custom' ? { target_person: person.trim(), ...(folder.trim() ? { target_folder: folder.trim() } : {}) } : {}) }
      const next = await api<Plan>('/api/staging', { method: 'POST', body: JSON.stringify({ mode: 'preview', request }) })
      if (sequence === previewSequence.current) setCurrentPlan(next)
    } catch (cause) { if (sequence === previewSequence.current) setOperationError((cause as Error).message) }
    finally { if (sequence === previewSequence.current) setPreparing(false) }
  }
  const begin = (kind: Action, single?: StagedFile) => {
    previewSequence.current++; setPreparing(false)
    captureTrigger(); setAction(kind); setCurrentPlan(null); setStatus(null); setSent(false); setUncertain(false); setOperationError(''); setConfirmation(''); setPerson(''); setFolder(''); setOpen(true)
    const file = single || [...selected.values()][0]; setRestoreFile(file || null); setDestinationMode(file?.origin?.original_path ? 'original' : 'custom')
    if (kind !== 'restore-files') void prepare(kind, single ? [single] : [...selected.values()])
  }
  const execute = async (reconcile = false) => {
    if (!plan) return
    setSubmitting(true); setOperationError('')
    if (!reconcile) {
      setSent(true); setUncertain(true)
      try { localStorage.setItem(pendingKey, JSON.stringify(plan)) } catch { setOperationError(`本机未能保存回执入口，请保留操作编号 ${plan.id}`) }
    }
    try {
      const value = await api<Status | DeleteResult | PersonFileResult>('/api/staging', { method: 'POST', body: JSON.stringify({ mode: reconcile ? 'reconcile' : 'apply', request: plan.request, version: plan.version, ...(plan.request.action !== 'restore-files' ? { confirmation } : {}) }) })
      if (reconcile) { acceptStatus(value as Status, plan); if ((value as Status).status === 'not_started') setUncertain(false) }
      else acceptStatus({ id: plan.id, status: 'status' in value ? value.status : 'complete', result: value as DeleteResult | PersonFileResult }, plan)
      await checkStatus(plan)
    } catch (cause) { setOperationError((cause as Error).message); await checkStatus(plan) }
    finally { setSubmitting(false) }
  }
  const receiptFiles = status?.files || (status?.result && 'files' in status.result ? status.result.files : [])
  const closeDialog = () => { previewSequence.current++; setPreparing(false); setOpen(false) }
  const showReceipt = () => { captureTrigger(); setOpen(true) }
  const changeView = (value: string) => { setHealth(value === 'health'); history.replaceState(null, '', value === 'health' ? '/staging#video-health' : '/staging') }
  const detail = focused && <>
    <div className="ws-detail-heading"><h2>文件详情</h2><Button variant="ghost" size="icon" aria-label="关闭文件详情" title="关闭详情" onClick={() => { setFocused(null); rowTrigger.current?.focus({ preventScroll: true }) }}><X size={15} /></Button></div>
    <div className="ws-detail-content"><div className="ws-task-info"><div className="staging-detail-file"><FileVideo size={24} /><h3>{focused.name}</h3></div><dl><dt>文件大小</dt><dd>{bytes(focused.size)}</dd><dt>来源</dt><dd>{origin(focused)}</dd></dl>
      {focused.origin?.original_path ? <WorkspacePath value={focused.origin.original_path} label="原路径" /> : <WorkspaceMessage>没有原路径记录，恢复时需指定目标人物。</WorkspaceMessage>}<WorkspacePath value={focused.path} label="暂存路径" />
      {focused.origin?.operation && <p className="staging-operation-id">操作编号 {focused.origin.operation}</p>}
      {videoFile.test(focused.name) && focused.size > 0 && <Button ref={playTrigger} variant="outline" onClick={event => { setInstant(event.detail === 0 || matchMedia('(prefers-reduced-motion: reduce)').matches); setPlaying({ vid: focused.vid, path: focused.path, name: focused.name, thumb: null }) }}><Play size={14} />播放核对</Button>}
    </div></div><div className="ws-detail-actions"><Button variant="outline" disabled={busy || !!error} onClick={() => begin('restore-files', focused)}><RotateCcw size={14} />恢复</Button><Button className="staging-danger" variant="outline" disabled={busy || !!error} onClick={() => begin('delete-staged', focused)}><Trash2 size={14} />永久删除</Button></div>
  </>
  if (health) return <VideoHealthPanel onBack={() => { changeView('files'); void load(offsetRef.current) }} />
  return <main className="ws-page staging-page" aria-label="回收站文件工作区">
    <header className="ws-header"><h1>回收站</h1><div className="ws-actions"><Button variant="ghost" size="icon" aria-label="刷新回收站" title="刷新回收站" disabled={loading || busy} onClick={() => load(offsetRef.current)}><RefreshCw size={15} /></Button><Button className="staging-danger" variant="outline" disabled={!list?.files || !!error || busy} onClick={() => begin('clear-staging')}><Trash2 size={14} />清空回收站</Button></div></header>
    <div className="ws-view-tabs"><WorkspaceTabs value="files" onChange={changeView} label="回收站视图" items={[{ value: 'files', label: '暂存文件', count: list?.files }, { value: 'health', label: '视频体检' }]} /></div>
    {error && <div className="staging-notice"><WorkspaceMessage error>{error}{list ? ' 当前列表为上次读取的结果。' : ''}</WorkspaceMessage><Button variant="outline" onClick={() => load(offsetRef.current)}>重试</Button></div>}
    {plan && sent && <div className="staging-receipt" role="status"><span>{labels[plan.request.action]} · {uncertain && !status ? '结果待确认' : statusLabel(status?.status)}{status?.total != null ? ` ${status.committed ?? status.completed ?? 0} / ${status.total}` : ''}</span><Button variant="ghost" onClick={showReceipt}>查看回执</Button></div>}
    {operationError && sent && !open && <div className="staging-notice"><WorkspaceMessage error>{operationError}</WorkspaceMessage></div>}
    <div className="staging-workarea"><aside className="staging-storage" aria-label="暂存区概况"><WorkspaceCard className="staging-storage-card"><div className="staging-storage-title"><Archive size={16} /><h2>暂存区</h2></div><dl className="staging-storage-stats"><div><dt>暂存文件</dt><dd>{!error && list ? list.files.toLocaleString() : '—'}<span>个</span></dd></div><div><dt>占用空间</dt><dd className="staging-total-size">{!error && list ? bytes(list.bytes) : '—'}</dd></div></dl></WorkspaceCard>{list && <WorkspaceSection title="暂存路径" defaultOpen={false}><WorkspacePath value={list.path} label="暂存路径" /></WorkspaceSection>}</aside>
      <section className="ws-queue" aria-label="暂存文件列表"><div className="ws-toolbar" ref={searchRef}><WorkspaceSearch label="搜索暂存文件" placeholder="搜索文件名或暂存路径" value={query} onChange={setQuery} /><Button variant="outline" disabled={loading || selecting || busy || !list?.matched} onClick={selectResults}>{selecting ? '读取中…' : '全选搜索结果'}</Button></div>
        <div className="staging-file-list-head"><Checkbox className="ws-check" motionPreset="instant" checked={!!rows.length && pageSelected === rows.length} indeterminate={pageSelected > 0 && pageSelected < rows.length} disabled={!rows.length || busy || !!error} aria-label="全选本页文件" onCheckedChange={checked => setSelected(previous => { const next = new Map(previous); for (const row of rows) { if (checked) next.set(row.path, row); else next.delete(row.path) } return next })} /><span>文件</span><span>大小</span></div>
        <div className="ws-list staging-records">{!rows.length && <div className="ws-empty" role="status"><Inbox size={28} /><p>{loading && !list ? '正在读取暂存文件…' : !list ? '暂存文件暂不可用' : list.files ? '没有匹配的暂存文件' : '回收站为空'}</p></div>}
          {rows.map(row => <article key={row.path} className="staging-record" data-active={focused?.path === row.path}><div className="staging-file-line"><Checkbox className="ws-check" motionPreset="instant" checked={selected.has(row.path)} disabled={busy || !!error} aria-label={`选择文件 ${row.path}`} onCheckedChange={checked => toggle(row, checked)} /><button type="button" className="staging-file-open" aria-label={`查看文件 ${row.path}`} aria-expanded={focused?.path === row.path} onClick={event => { rowTrigger.current = event.currentTarget; setFocused(focused?.path === row.path ? null : row) }}><span className="staging-file-icon">{videoFile.test(row.name) ? <FileVideo size={18} /> : <File size={18} />}</span><span className="ws-row-copy"><span className="ws-row-name" title={row.name}>{row.name}</span><span className="ws-row-meta" title={row.origin?.original_path || row.path}>{origin(row)} · {row.origin?.original_path || row.path}</span></span><span className="staging-file-size">{bytes(row.size)}</span></button></div>{!wide && focused?.path === row.path && <section className="staging-inline-detail">{detail}</section>}</article>)}
        </div>
        {selected.size > 0 && <div className="ws-selection staging-file-actions"><span className="ws-selection-summary">已选 {selected.size} 个</span><Button variant="ghost" disabled={busy} onClick={() => setSelected(new Map())}>取消选择</Button><Button variant="outline" disabled={busy || !!error || selected.size !== 1} title={selected.size !== 1 ? '每次恢复一个文件，分别核对目标' : '恢复所选文件'} onClick={() => begin('restore-files')}><RotateCcw size={14} />恢复</Button><Button className="staging-danger" variant="outline" disabled={busy || !!error || selected.size > 500} onClick={() => begin('delete-staged')}><Trash2 size={14} />永久删除</Button></div>}
        <footer className="ws-pagination"><span>{list ? `${list.matched.toLocaleString()} 个匹配 · ${list.matched ? list.offset + 1 : 0}–${list.offset + rows.length}` : '数量未知'}</span><div className="ws-actions"><Button variant="ghost" size="icon" aria-label="上一页" title="上一页" disabled={loading || !list?.offset} onClick={() => load(Math.max(0, (list?.offset || 0) - 100))}><ChevronLeft size={15} /></Button><Button variant="ghost" size="icon" aria-label="下一页" title="下一页" disabled={loading || list?.next_offset == null} onClick={() => load(list!.next_offset!)}><ChevronRight size={15} /></Button></div></footer>
      </section>{wide && <aside className="ws-inspector staging-inspector" aria-label="文件详情">{detail || <div className="staging-detail-empty"><FolderOpen size={28} /><p>选择文件查看原路径</p></div>}</aside>}
    </div>
    <Dialog open={open} onOpenChange={value => { setOpen(value); if (!value) { previewSequence.current++; setPreparing(false) } }}><ResponsiveDialogContent open={open} inert={!open} className="ws-detail-sheet staging-confirm" style={{ animationName: open ? 'review-dialog-in' : 'review-dialog-out', animationDuration: instant ? '0ms' : '180ms' }} overlayProps={{ style: { animationName: open ? 'review-overlay-in' : 'review-dialog-out', animationDuration: instant ? '0ms' : '140ms' } }} onCloseAutoFocus={event => { event.preventDefault(); if (dialogTrigger.current?.isConnected) dialogTrigger.current.focus(); else searchRef.current?.querySelector('input')?.focus() }}>
      <div className="ws-detail-heading"><DialogTitle>{sent ? '操作回执' : labels[action]}</DialogTitle><DialogDescription className="sr-only">核对文件范围与目标路径后确认执行。</DialogDescription></div>
      <div className="ws-detail-content staging-confirm-content" data-sheet-scroll>{operationError && <WorkspaceMessage error>{operationError}</WorkspaceMessage>}
        {sent ? <><p role="status">{status ? statusLabel(status.status) : '正在查询结果…'}{status?.total != null ? ` · ${status.committed ?? status.completed ?? 0} / ${status.total}` : ''}{status?.phase && !terminal(status.status) ? ` · ${phaseLabel(status.phase)}` : ''}</p><p className="staging-operation-id">{plan?.id}</p>{status?.result?.warning && <WorkspaceMessage>{status.result.warning}</WorkspaceMessage>}{status?.status === 'recovery_needed' && <WorkspaceMessage error>保留操作编号并核对回执；已删除的内容不能恢复，未确认的步骤不会作为成功显示。</WorkspaceMessage>}
          <div className="staging-preview-list">{receiptFiles.length ? receiptFiles.map(file => <div key={file.src}><strong>{file.src.split('/').pop()}</strong><span>{statusLabel(file.status)}</span>{file.error?.error && <WorkspaceMessage error>{file.error.error}</WorkspaceMessage>}<code>{file.src}</code>{plan?.request.action === 'restore-files' && <code>→ {plan.files.find(planned => planned.src === file.src)?.dst}</code>}</div>) : plan?.files.map(file => <div key={file.src}><strong>{file.src.split('/').pop()}</strong><code>{file.src}</code>{plan.request.action === 'restore-files' && <code>→ {file.dst}</code>}</div>)}</div></> : <>
          {action === 'restore-files' && !plan && <><WorkspaceField label="恢复位置"><WorkspaceSelect label="恢复位置" value={destinationMode} onChange={setDestinationMode} options={[{ value: 'original', label: '原路径', disabled: !restoreFile?.origin?.original_path }, { value: 'custom', label: '指定人物目录' }]} /></WorkspaceField>{destinationMode === 'custom' && <><WorkspaceField label="目标人物"><WorkspaceInput value={person} onChange={event => setPerson(event.target.value)} placeholder="人物名称" /></WorkspaceField><WorkspaceField label="子目录（可选）"><WorkspaceInput value={folder} onChange={event => setFolder(event.target.value)} placeholder="相对人物目录的子目录" /></WorkspaceField></>}<p className="ws-muted">逐个核对恢复目标。同名冲突会停止，请改选子目录后重新预览。</p></>}
          {preparing && <WorkspaceMessage>正在核对文件与账本…</WorkspaceMessage>}{plan && <><p>{plan.files.length} 个文件 · {bytes(plan.files.reduce((sum, file) => sum + file.size, 0))}</p>{action !== 'restore-files' && <WorkspaceMessage error>{action === 'clear-staging' ? '将永久删除回收站内本次预览的全部文件，搜索筛选不限制清空范围。' : '将永久删除以下文件。'}执行后无法从回收站恢复。</WorkspaceMessage>}<div className="staging-preview-list">{plan.files.map(file => <div key={file.src}><strong>{file.src.split('/').pop()}</strong><span>{bytes(file.size)}</span><code>{file.src}</code>{action === 'restore-files' && <code>→ {file.dst}</code>}</div>)}</div>{action !== 'restore-files' && <WorkspaceField label={`输入“${labels[action]}”确认`}><WorkspaceInput aria-label="删除确认文字" autoComplete="off" value={confirmation} onChange={event => setConfirmation(event.target.value)} /></WorkspaceField>}</>}
        </>}
      </div><div className="ws-detail-actions"><Button variant="outline" onClick={closeDialog}>{submitting ? '后台继续' : '关闭'}</Button>{!sent && !plan && action === 'restore-files' && <Button disabled={preparing || !restoreFile || (destinationMode === 'custom' && !person.trim())} onClick={() => prepare(action, [restoreFile!])}>预览恢复</Button>}{!sent && plan && <Button variant={action === 'restore-files' ? 'primary' : 'outline'} className={action !== 'restore-files' ? 'staging-danger' : ''} disabled={submitting || (action !== 'restore-files' && confirmation !== labels[action])} onClick={() => execute()}>确认{labels[action]}</Button>}{status?.status === 'complete' && status.result && 'person' in status.result && status.result.person && <NavigationButton variant="outline" size="sm" asChild><Link href={`/persons/${encodeURIComponent(status.result.person)}`}>查看人物目录</Link></NavigationButton>}{sent && plan && <><Button variant="outline" disabled={submitting} onClick={() => execute(true)}>核对恢复状态</Button><Button variant="ghost" disabled={submitting} onClick={() => checkStatus(plan)}>刷新回执</Button></>}</div>
    </ResponsiveDialogContent></Dialog><VideoPlayer video={playing} onClose={() => setPlaying(null)} returnFocus={playTrigger} instantEntry={instant} />
  </main>
}
