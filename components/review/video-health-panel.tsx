'use client'

// Filespace's file browser composition, using the shared BEUI/ReUI workspace.
// Layout reference: https://filespace-drive.reui.io/bin (no template source copied).
import { useCallback, useEffect, useRef, useState } from 'react'
import { Activity, Archive, ChevronRight, FileVideo, HardDrive, Inbox, Play, RefreshCw, X } from 'lucide-react'
import { api } from '@/lib/face-api'
import { healthTitles, type VideoHealthRecord, type VideoHealthReport } from '@/lib/video-health-model'
import { fileName, type ReviewVideo } from '@/lib/review-ui'
import { WorkspaceSearch, WorkspaceTabs, useWideWorkspace } from '@/components/workbench/controls'
import { WorkspaceButton as Button, WorkspaceCard, WorkspaceMessage, WorkspacePath, WorkspaceSection } from '@/components/workbench/inner'
import { VideoPlayer } from './video'
import './staging.css'

type Report = Omit<VideoHealthReport, 'records'> & { rows: VideoHealthRecord[] }
type StagingStats = { files: number; gb: number; path: string }
const number = new Intl.NumberFormat('zh-CN')
const date = (value?: string) => value && Number.isFinite(Date.parse(value))
  ? new Intl.DateTimeFormat('zh-CN', { timeZone: 'Asia/Shanghai', month: 'numeric', day: 'numeric', hour: '2-digit', minute: '2-digit' }).format(new Date(value))
  : '尚无记录'
const size = (bytes?: number | null) => bytes == null ? '未知' : bytes < 1024 ** 2 ? `${number.format(bytes)} B` : bytes < 1024 ** 3 ? `${(bytes / 1024 ** 2).toFixed(1)} MB` : `${(bytes / 1024 ** 3).toFixed(2)} GB`

export function VideoHealthPanel({ onBack }: { onBack?: () => void } = {}) {
  const [data, setData] = useState<Report | null>(null)
  const [storage, setStorage] = useState<StagingStats | null>(null)
  const [error, setError] = useState(''), [storageError, setStorageError] = useState('')
  const [loading, setLoading] = useState(true), [storageLoading, setStorageLoading] = useState(true)
  const [filter, setFilter] = useState('all'), [query, setQuery] = useState(''), [limit, setLimit] = useState(50)
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [playing, setPlaying] = useState<ReviewVideo | null>(null)
  const [instantPlayer, setInstantPlayer] = useState(false)
  const searchRef = useRef<HTMLDivElement>(null), playTrigger = useRef<HTMLButtonElement | null>(null)
  const rowTrigger = useRef<HTMLButtonElement | null>(null)
  const healthSequence = useRef(0), storageSequence = useRef(0)
  const wide = useWideWorkspace()

  const loadHealth = useCallback(async () => {
    const request = ++healthSequence.current
    setLoading(true)
    try {
      const result = await api<Report>('/api/video-health', { signal: AbortSignal.timeout(15000) })
      if (request !== healthSequence.current) return
      setData(result); setError('')
    } catch (cause) {
      if (request === healthSequence.current) setError((cause as Error).message || '体检记录读取失败')
    } finally {
      if (request === healthSequence.current) setLoading(false)
    }
  }, [])
  const loadStorage = useCallback(async () => {
    const request = ++storageSequence.current
    setStorageLoading(true)
    try {
      const result = await api<StagingStats>('/api/staging', { signal: AbortSignal.timeout(15000) })
      if (request !== storageSequence.current) return
      setStorage(result); setStorageError('')
    } catch (cause) {
      if (request === storageSequence.current) { setStorage(null); setStorageError((cause as Error).message || '暂存目录读取失败') }
    } finally {
      if (request === storageSequence.current) setStorageLoading(false)
    }
  }, [])
  useEffect(() => {
    void loadHealth(); void loadStorage()
    return () => { healthSequence.current++; storageSequence.current++ }
  }, [loadHealth, loadStorage])
  useEffect(() => {
    if (data?.status !== 'running') return
    const timer = setInterval(() => { void loadHealth() }, 15000)
    return () => clearInterval(timer)
  }, [data?.status, loadHealth])
  useEffect(() => {
    if (window.location.hash === '#video-health') searchRef.current?.querySelector('input')?.focus({ preventScroll: true })
  }, [])

  const records = data?.rows ?? []
  const hidden = records.filter(record => record.hidden).length
  const normalized = query.trim().toLowerCase()
  const rows = records.filter(record => (filter === 'all' || (filter === 'hidden' ? record.hidden : !record.hidden)) &&
    (!normalized || [record.vid, record.path ?? '', record.detail ?? '', healthTitles[record.status] ?? record.status].some(value => value.toLowerCase().includes(normalized))))
  const selected = rows.find(record => record.vid === selectedId)
  const clearSelection = () => { setSelectedId(null); rowTrigger.current?.focus({ preventScroll: true }) }
  const status = data?.status === 'running' ? '正在检查' : data?.error ? '检查需关注' : ['skipped', 'interrupted'].includes(data?.status || '') ? '本次未完成' : data?.completed_at ? '检查已完成' : '等待首次体检'
  const storageSummary = <>
    <WorkspaceCard className="staging-storage-card">
      <div className="staging-storage-title"><Archive size={16} /><h2>暂存区</h2></div>
      <dl className="staging-storage-stats">
        <div><dt>暂存文件</dt><dd>{storage ? number.format(storage.files) : '—'}<span>个</span></dd></div>
        <div><dt><HardDrive size={12} />占用空间</dt><dd>{storage ? number.format(storage.gb) : '—'}<span>GB</span></dd></div>
      </dl>
      {storageLoading && !storage && <p className="ws-muted" role="status">正在读取暂存目录…</p>}
      {storageError && <><WorkspaceMessage error>{storageError}</WorkspaceMessage><Button variant="outline" disabled={storageLoading} onClick={loadStorage}>重试读取</Button></>}
      {storage?.files === 0 && <p className="ws-muted">暂存目录为空</p>}
    </WorkspaceCard>
    {storage && <WorkspaceSection title="暂存路径" defaultOpen={false}><WorkspacePath value={storage.path} label="暂存路径" /></WorkspaceSection>}
  </>
  const detail = selected && <>
    <div className="ws-detail-heading"><h2>检查详情</h2><Button variant="ghost" size="icon" aria-label="关闭检查详情" title="关闭详情" onClick={clearSelection}><X size={15} /></Button></div>
    <div className="ws-detail-content"><div className="ws-task-info">
      <div className="staging-detail-file"><FileVideo size={24} /><h3>{fileName(selected.path || selected.vid)}</h3></div>
      <WorkspaceCard><span className="staging-health-state" data-hidden={!!selected.hidden}>{healthTitles[selected.status] || '需要复查'}</span><p className="staging-evidence">{selected.detail || healthTitles[selected.status] || '等待检查依据'}</p></WorkspaceCard>
      <dl><dt>记录状态</dt><dd>{selected.hidden ? '已收起' : '需核对'}</dd><dt>复查次数</dt><dd>{selected.consecutive == null ? '尚无记录' : `${selected.consecutive} / 3`}</dd><dt>文件大小</dt><dd>{size(selected.size)}</dd><dt>最近检查</dt><dd>{date(selected.last_probed_at || selected.last_checked_at)}</dd>{selected.first_failure_at && <><dt>首次异常</dt><dd>{date(selected.first_failure_at)}</dd></>}{selected.codec && <><dt>视频编码</dt><dd>{selected.codec}</dd></>}</dl>
      <WorkspacePath value={selected.path || selected.vid} label={selected.path ? '文件路径' : '视频标识'} />
    </div></div>
    {!!selected.size && selected.path && <div className="ws-detail-actions"><Button ref={playTrigger} variant="outline" onClick={event => {
      setInstantPlayer(event.detail === 0 || matchMedia('(prefers-reduced-motion: reduce)').matches)
      setPlaying({ vid: selected.vid, path: selected.path!, name: fileName(selected.path!), thumb: null })
    }}><Play size={14} />播放核对</Button></div>}
  </>

  return <main className="ws-page staging-page" aria-label="回收站工作区">
    <header className="ws-header"><h1>视频体检</h1><div className="ws-actions">{onBack && <Button variant="outline" onClick={onBack}>暂存文件</Button>}<Button variant="outline" disabled={loading || storageLoading} onClick={() => { void loadHealth(); void loadStorage() }}><RefreshCw size={14} />刷新</Button></div></header>
    <div className="staging-workarea">
      <aside className="staging-storage" aria-label="暂存区概况">{storageSummary}</aside>
      <section id="video-health" className="ws-queue staging-health" aria-labelledby="staging-health-title">
        <div className="staging-health-heading"><div><Activity size={16} /><h2 id="staging-health-title">视频体检</h2></div>{data && <span className="staging-run-state" role="status" data-running={data.status === 'running'}>{status}</span>}</div>
        <div className="staging-health-summary" aria-label="体检统计">
          {[{ label: '已检查路径', value: data?.total }, { label: '已收起记录', value: data ? hidden : undefined }, { label: '等待结构检查', value: data?.waiting }].map(item => <div key={item.label}><span>{item.label}</span><strong>{item.value == null ? '—' : number.format(item.value)}</strong></div>)}
        </div>
        {error && <div className="staging-notice"><WorkspaceMessage error>{error}{data ? ' 当前显示上次读取的结果。' : ''}</WorkspaceMessage><Button variant="outline" disabled={loading} onClick={loadHealth}>重试</Button></div>}
        {data?.error && <div className="staging-notice"><WorkspaceMessage error>{data.error}</WorkspaceMessage></div>}
        <div className="ws-view-tabs"><WorkspaceTabs value={filter} label="体检记录范围" onChange={value => { setFilter(value); setLimit(50) }} items={[{ value: 'all', label: '全部异常', count: data ? records.length : undefined }, { value: 'watch', label: '需核对', count: data ? records.length - hidden : undefined }, { value: 'hidden', label: '已收起', count: data ? hidden : undefined }]} /></div>
        <div className="ws-toolbar" ref={searchRef}><WorkspaceSearch label="搜索体检记录" placeholder="搜索文件名、路径或检查依据" value={query} onChange={value => { setQuery(value); setLimit(50) }} /></div>
        <div className="staging-record-columns" aria-hidden="true"><span>文件</span><span>检查结果</span><span>最近检查</span></div>
        <div className="ws-list staging-records" aria-label="异常记录">
          {!data && <div className="ws-empty" role="status"><Inbox size={28} /><p>{loading ? '正在读取体检记录…' : '体检记录暂不可用'}</p></div>}
          {data && !rows.length && <div className="ws-empty" role="status"><Inbox size={28} /><p>{query ? '没有匹配的记录' : records.length ? '当前分类没有异常记录' : !data.last_success_at ? '尚无成功检查结果' : '暂无异常记录'}</p>{query && <Button variant="ghost" onClick={() => { setQuery(''); searchRef.current?.querySelector('input')?.focus() }}>清空搜索</Button>}</div>}
          {rows.slice(0, limit).map(record => <article key={record.vid} className="staging-record" data-active={record.vid === selected?.vid}>
            <button type="button" className="staging-record-open" aria-label={`查看检查详情 ${fileName(record.path || record.vid)}`} aria-expanded={record.vid === selected?.vid} onClick={event => { rowTrigger.current = event.currentTarget; setSelectedId(record.vid === selected?.vid ? null : record.vid) }}>
              <span className="staging-record-name"><span className="staging-file-icon"><FileVideo size={18} /></span><span className="ws-row-copy"><span className="ws-row-name" title={fileName(record.path || record.vid)}>{fileName(record.path || record.vid)}</span><span className="ws-row-meta" title={record.path || record.vid}>{record.path || record.vid}</span></span></span>
              <span className="staging-record-result"><span className="staging-health-state" data-hidden={!!record.hidden}>{healthTitles[record.status] || '需要复查'}</span><span className="ws-row-meta">{record.hidden ? '已收起' : record.consecutive ? `复查 ${record.consecutive}/3` : '保留显示'}</span></span>
              <span className="staging-record-date">{date(record.last_probed_at || record.last_checked_at)}<ChevronRight size={14} /></span>
            </button>
            {!wide && record.vid === selected?.vid && <section className="staging-inline-detail" aria-label="检查详情">{detail}</section>}
          </article>)}
          {rows.length > limit && <Button className="staging-load-more" variant="outline" onClick={() => setLimit(value => value + 50)}>再显示 50 条</Button>}
        </div>
        <footer className="staging-footer"><span>{data ? `${number.format(rows.length)} 条记录` : '记录数未知'}</span>{data && <span>最近成功 {date(data.last_success_at)}<span className="staging-next-run"> · 下次 {data.next_run_at ? date(data.next_run_at) : '每天 03:30'}</span></span>}</footer>
      </section>
      {wide && <aside className="ws-inspector staging-inspector" aria-label="检查详情">{detail || <div className="staging-detail-empty"><FileVideo size={28} /><p>选择记录查看检查依据</p></div>}</aside>}
    </div>
    <VideoPlayer video={playing} onClose={() => setPlaying(null)} returnFocus={playTrigger} instantEntry={instantPlayer} />
  </main>
}
