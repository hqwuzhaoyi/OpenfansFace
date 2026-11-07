'use client'

import { useEffect, useMemo, useRef, useState } from 'react'
import { useRouter } from 'next/navigation'
import Link from 'next/link'
import { Archive, ArrowDown, ArrowLeft, ArrowRight, Check, CheckCheck, ChevronLeft, ChevronRight, CircleAlert, CircleCheck, Folder, FolderInput, History, PanelLeft, RefreshCw, Search, ShieldCheck, Undo2, X } from 'lucide-react'
import { toast } from 'sonner'
import { WorkbenchMotion, WorkbenchStep, useWorkbenchMotion } from '@/components/persons/motion'
import { Progress } from '@/components/ui/progress'
import { jobPercent, type HomeJob } from '@/lib/home-model'
import type { ArchiveResult } from '@/lib/archive-run-model'
import { api } from '@/lib/face-api'
import type { ArchivedItem } from '@/lib/review-logic'
import type { PlanRow } from '@/lib/review-plan'
import { groupBatches, statusTitle } from '@/lib/review-batches'
import { groupSourceFolders, toggleArchiveSelection } from '@/lib/integration-view'
import { filterArchiveBatches, retainArchiveSelection, summarizeArchiveSelection, type BatchFilter } from '@/lib/archive-batch-selection'
import { ArchiveBatchList, archiveTime, batchCreator } from '@/components/review/archive-batch-list'
import { ArchiveVideoPlayer } from '@/components/review/archive-video-player'
import { ArchiveRowActions } from '@/components/review/archive-row-actions'
import { archiveShortcut, canResolveArchiveRow } from '@/lib/archive-actions'
import { sourceLabel, type ReviewVideo, fileName } from '@/lib/review-ui'
import { ArchiveResolutionDialog } from '@/components/review/archive-resolution'
import { ArchiveCheckbox, ArchiveFolderList, type ArchiveListItem } from '@/components/review/archive-folder-list'
import { PathText } from '@/components/review/video'
import { Button } from '@/components/ui/button'
import { AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle } from '@/components/ui/alert-dialog'
import '@/components/review/archive-workbench.css'

type Row = PlanRow
interface ArchiveRecord extends ArchivedItem { name?: string; thumb?: string | null; size_gb?: number }
interface Plan { rows: Row[]; n: number; n_ok: number; total_gb: number; version: string; history: ArchiveRecord[] }
type ApplyResult = ArchiveResult
interface UndoItem { vid: string; source?: string; return_href?: string }
const toastOptions = { position: 'top-right' as const }
const rowStatus = (row: Row) => row.operation === 'stage-duplicate' ? '重复件待暂存' : statusTitle[row.status] || '需要核对'
const waveTime = archiveTime

export default function IntegrationPage(){return <WorkbenchMotion><Integration /></WorkbenchMotion>}

function Integration() {
  const {instant}=useWorkbenchMotion()
  const [watchedId,setWatchedId]=useState<string|null>(null),[job,setJob]=useState<HomeJob|null>(null),[connection,setConnection]=useState('')
  const watchRef=useRef<string|null>(null),runStatus=useRef<HTMLDivElement>(null),archiveTrigger=useRef<HTMLButtonElement>(null)
  const archiving=!!watchedId
  const router = useRouter()
  const [plan, setPlan] = useState<Plan | null>(null)
  const [error, setError] = useState(''), [loading, setLoading] = useState(true), [busy, setBusy] = useState(false)
  const [selected, setSelected] = useState<Set<string>>(new Set())
  const [filter, setFilter] = useState<BatchFilter>('all'), [query, setQuery] = useState(''), [history, setHistory] = useState(false)
  const [active, setActive] = useState<string | null>(null), [collapsed, setCollapsed] = useState<Set<string>>(new Set())
  const [mobileDetail, setMobileDetail] = useState(false), [listHidden, setListHidden] = useState(false)
  const [confirm, setConfirm] = useState(false)
  const [result, setResult] = useState<ApplyResult | null>(null), [undoResult, setUndoResult] = useState<UndoItem[]>([])
  const [resolving, setResolving] = useState<Row[] | null>(null), [waveKey, setWaveKey] = useState<string | null>(null)
  const [undoRequest, setUndoRequest] = useState<{ vids: string[]; goBack: boolean } | null>(null)
  const requestSequence = useRef(0), writing = useRef(false), searchInput = useRef<HTMLInputElement>(null)
  const scrollList = useRef<HTMLDivElement>(null), scrollDetail = useRef<HTMLDivElement>(null)
  const confirmButton = useRef<HTMLButtonElement>(null)

  const load = async () => {
    const sequence = ++requestSequence.current
    setLoading(true); setError('')
    try {
      const data = await api<Plan>('/api/plan/preview')
      if (sequence !== requestSequence.current) return null
      setPlan(data)
      setSelected(previous => retainArchiveSelection(previous, data.rows))
      return data
    } catch (cause) {
      if (sequence === requestSequence.current) setError((cause as Error).message)
      return null
    } finally {
      if (sequence === requestSequence.current) setLoading(false)
    }
  }
  useEffect(() => {
    void load()
    setHistory(new URLSearchParams(window.location.search).get('view') === 'history')
    return () => { requestSequence.current++ }
  }, [])

  const watch=(id:string)=>{watchRef.current=id;setWatchedId(id)}
  const settle=(data:ApplyResult,id:string)=>{
    if(watchRef.current!==id)return
    setResult(data);setJob(null);setConnection('');watchRef.current=null;setWatchedId(null);writing.current=false
    try{if(localStorage.getItem('face-archive-operation')===id)localStorage.removeItem('face-archive-operation')}catch{}
    void load()
  }
  useEffect(()=>{
    let active=true
    let saved:string|null=null;try{saved=localStorage.getItem('face-archive-operation')}catch{}
    if(saved){watch(saved);return}
    void api<{job:HomeJob|null}>('/api/plan/progress').then(data=>{if(active&&data.job&&['running','offline'].includes(data.job.status)){watch(data.job.id);setJob(data.job)}}).catch(()=>{})
    return()=>{active=false}
  },[])
  useEffect(()=>{
    if(!watchedId)return
    let active=true,timer:ReturnType<typeof setTimeout>
    const controller=new AbortController()
    const poll=async()=>{
      try{
        const data=await api<{job:HomeJob|null;result?:ApplyResult}>('/api/plan/progress?id='+encodeURIComponent(watchedId),{signal:controller.signal})
        if(!active||watchRef.current!==watchedId)return
        if(data.job)setJob(data.job)
        if(data.job)setConnection(data.job.status==='offline'?'任务心跳中断，结果待核对，请勿重复归档':'')
        if(data.result){settle(data.result,watchedId);return}
      }catch{if(active)setConnection('连接中断，正在核对本次归档结果…')}
      if(active)timer=setTimeout(poll,1000)
    }
    void poll()
    return()=>{active=false;clearTimeout(timer);controller.abort()}
  },[watchedId])

  const rows = plan?.rows || []
  const waves = useMemo(() => groupBatches(plan?.rows || []), [plan])
  const visibleWaves = filterArchiveBatches(waves, query, filter, selected)
  const wave = visibleWaves.find(w => w.key === waveKey) || visibleWaves[0]
  const waveRows = wave?.rows || []
  const visibleIds = new Set(visibleWaves.flatMap(w => w.rows.map(r => r.vid)))
  const { chosen, ready: validChosen, blocked, hidden: hiddenSelected } = summarizeArchiveSelection(rows, selected, visibleIds)
  const invalid = blocked.length
  const selectedWaves = waves.filter(w => w.rows.some(r => selected.has(r.vid)))
  const readySelectedWaves = waves.filter(w => w.rows.some(r => selected.has(r.vid) && r.status === 'ok'))
  const conflicts = waveRows.filter(canResolveArchiveRow)
  const chosenConflicts = chosen.filter(canResolveArchiveRow)
  const stagedCount = validChosen.filter(r => r.operation === 'stage-duplicate').length
  const selectGB = validChosen.reduce((sum, row) => sum + (row.size_gb || 0), 0).toFixed(2)
  const disabled = busy || archiving || !!result?.recovery_needed || loading || !!error
  const items: ArchiveListItem[] = history
    ? (plan?.history || []).map(r => ({ vid: r.vid, name: r.name || fileName(r.dst || r.win_dst || r.vid), src: r.dst || r.win_dst, thumb: r.thumb || null, creator: r.creator, size_gb: r.size_gb, status: 'archived', statusLabel: '已归档' }))
    : waveRows.map(r => ({ ...r, statusLabel: rowStatus(r) }))
  const filtered = history ? items.filter(r => !query || [r.name, r.vid, r.src || '', r.creator].some(value => value.toLowerCase().includes(query.toLowerCase()))) : items
  const current = filtered.find(r => r.vid === active) || filtered[0]
  const pending = !history && current ? waveRows.find(r => r.vid === current.vid) : undefined
  const archived = history && current ? plan?.history.find(r => r.vid === current.vid) : undefined
  const detailVideo: ReviewVideo | null = current ? { ...current, path: current.src || current.vid } : null
  const folders = groupSourceFolders(filtered)
  const activeIndex = filtered.findIndex(r => r.vid === current?.vid)

  const chooseWave = (key: string, mobile = true) => {
    setWaveKey(key); setActive(null)
    if (mobile) setMobileDetail(true)
    scrollDetail.current?.scrollTo(0, 0)
  }
  const setView = (value: boolean) => {
    setHistory(value); setActive(null); setCollapsed(new Set()); setFilter('all'); setQuery(''); setMobileDetail(false)
    const url = new URL(window.location.href)
    if (value) url.searchParams.set('view', 'history'); else url.searchParams.delete('view')
    window.history.replaceState(window.history.state, '', url)
    scrollList.current?.scrollTo(0, 0)
    scrollDetail.current?.scrollTo(0, 0)
  }
  const showSelected = () => { setHistory(false); setFilter('selected'); setQuery(''); setListHidden(false); setMobileDetail(false) }
  const toggle = (ids: string[]) => { if (!disabled && !history) setSelected(previous => toggleArchiveSelection(previous, ids)) }
  const openItem = (vid: string, mobile = true) => {
    setActive(vid)
    if (mobile) setMobileDetail(true)
    if (history) {
      const directory = folders.find(folder => folder.items.some(item => item.vid === vid))?.path
      if (directory) setCollapsed(previous => { const next = new Set(previous); next.delete(directory); return next })
      scrollDetail.current?.scrollTo(0, 0)
    }
  }
  const move = (offset: number) => {
    if (!filtered.length) return
    openItem(filtered[(activeIndex + offset + filtered.length) % filtered.length].vid, false)
  }
  const moveBatch = (offset: number) => {
    if (!visibleWaves.length) return
    const index = visibleWaves.findIndex(w => w.key === wave?.key)
    chooseWave(visibleWaves[(index + offset + visibleWaves.length) % visibleWaves.length].key, false)
    requestAnimationFrame(() => scrollList.current?.querySelector('[data-active="true"]')?.scrollIntoView({ block: 'nearest' }))
  }
  useEffect(() => {
    const key = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement
      const editableInput = target instanceof HTMLInputElement && !['checkbox', 'radio', 'button', 'submit', 'reset'].includes(target.type)
      const action = archiveShortcut(event, {
        editing: editableInput || /^(TEXTAREA|SELECT|VIDEO)$/.test(target.tagName) || target.isContentEditable || !!target.closest('[contenteditable="true"]'),
        modal: !!target.closest('[role="dialog"],[role="alertdialog"]') || confirm || !!undoRequest || !!resolving,
        nativeAction: !!target.closest('button,a,[role="button"],summary') && !target.closest('[data-archive-navigate]'),
      })
      if (action === 'search') {
        event.preventDefault(); setListHidden(false); setMobileDetail(false)
        requestAnimationFrame(() => searchInput.current?.focus())
      }
      if (action === 'next') { event.preventDefault(); history ? move(1) : moveBatch(1) }
      if (action === 'previous') { event.preventDefault(); history ? move(-1) : moveBatch(-1) }
      if (action === 'toggle' && wave && !history) { event.preventDefault(); toggle(wave.rows.map(row => row.vid)) }
      if (action === 'all' && !history) { event.preventDefault(); toggle([...visibleIds]) }
      if (action === 'confirm' && !history && !disabled) {
        event.preventDefault()
        if (validChosen.length && validChosen.length <= 5000) setConfirm(true)
        else toast.info(chosen.length ? '选中项暂不能归档，请先使用列表中的处理操作。' : '先按 A 全选当前列表，或勾选要归档的波次。', toastOptions)
      }
    }
    window.addEventListener('keydown', key)
    return () => window.removeEventListener('keydown', key)
  })

  const undo = async (vids: string[], goBack = false) => {
    if (writing.current || disabled || !vids.length || vids.length > 5000) return
    writing.current = true; setBusy(true)
    try {
      const data = await api<{ removed: UndoItem[] }>('/api/plan/queue', { method: 'DELETE', body: JSON.stringify({ vids }) })
      setUndoResult(data.removed); await load()
      toast.success(`已将 ${data.removed.length} 个退回待确认，文件未删除`, toastOptions)
      if (goBack && data.removed[0]?.return_href) router.push('/review?vid='+encodeURIComponent(data.removed[0].vid))
    } catch (cause) { toast.error((cause as Error).message, toastOptions) }
    finally { writing.current = false; setBusy(false) }
  }
  const resolveInline = async (targets: Row[], kind: 'rename' | 'stage-duplicate') => {
    if (!plan || writing.current || disabled || !targets.length || targets.length > 5000) return
    writing.current = true; setBusy(true)
    try {
      await api('/api/plan/resolve', { method: 'POST', body: JSON.stringify({ vids: targets.map(row => row.vid), version: plan.version, kind }) })
      await load()
      toast.success(kind === 'rename' ? '已选择保留两份，归档时使用新文件名。' : '已选择暂存重复件，归档时完整校验内容。', toastOptions)
    } catch (cause) { toast.error((cause as Error).message, toastOptions); await load() }
    finally { writing.current = false; setBusy(false) }
  }
  const undoRows = (targets: Row[]) => setUndoRequest({ vids: targets.map(row => row.vid), goBack: false })
  const apply = async () => {
    if (!plan || writing.current || disabled || (!validChosen.length || validChosen.length > 5000)) return
    // getRandomValues also works on the HTTP Tailscale origin (randomUUID requires HTTPS).
    const bytes=crypto.getRandomValues(new Uint8Array(16));bytes[6]=(bytes[6]&15)|64;bytes[8]=(bytes[8]&63)|128
    const hex=[...bytes].map(b=>b.toString(16).padStart(2,'0')).join('')
    const id=`${hex.slice(0,8)}-${hex.slice(8,12)}-${hex.slice(12,16)}-${hex.slice(16,20)}-${hex.slice(20)}`
    writing.current=true;watch(id);setResult(null);setConnection('');setJob(null);setConfirm(false)
    try{localStorage.setItem('face-archive-operation',id)}catch{}
    try {
      const response=await fetch('/api/plan/apply',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({vids:validChosen.map(r=>r.vid),version:plan.version,operation_id:id})})
      const data=await response.json()
      if(!response.ok){settle({ok:false,moved:0,remaining:plan.n,errors:[data.error||'归档请求未被接受']},id);return}
      settle(data,id)
    }catch{if(watchRef.current===id)setConnection('连接中断，正在核对本次归档结果…')}
  }
  const progressPercent=job?jobPercent(job):null
  const resultTitle=result?.recovery_needed?'需要核对恢复，已暂停写操作':result?.rolled_back?'归档失败，文件已恢复原位置':result?.moved?`已归档 ${result.moved} 个${result.errors.length?' · 部分项目未完成':''}`:'本轮未归档，任务已保留'


  return <main className="archive-workbench">
    {error && <div role="alert" className={`archive-notice archive-warning`}>{error}<Button size="sm" variant="ghost" disabled={loading || busy} onClick={() => load()}>重新加载</Button></div>}
    {result?.errors.length ? <details className="archive-notice archive-warning"><summary>{resultTitle} · 查看原因</summary><ul>{result.errors.map((message,i)=><li key={i}>{message}</li>)}</ul></details>:null}
    {!!undoResult.length && !history && <div className="archive-notice" role="status"><div className="flex justify-between gap-3"><p>已将 {undoResult.length} 个视频退回待确认，文件没有移动或删除。</p><button aria-label="关闭退回结果" className="archive-iconButton" onClick={() => setUndoResult([])}><X size={15} /></button></div><div className="archive-noticeLinks">{Array.from(new Map(undoResult.map(r => [r.return_href, r])).values()).map((r, i) => <Link key={r.return_href || i} href={'/review?vid='+encodeURIComponent(r.vid)}>回到{sourceLabel[r.source || 'legacy'] || '原确认入口'}</Link>)}</div></div>}
    <div className="archive-split" data-hidden={listHidden} data-mobile-detail={mobileDetail}>
      <section className="archive-list" aria-label={history ? '归档记录列表' : '待归档波次列表'} aria-busy={loading}>
        <header className="archive-listHeading"><h1>{history ? '已归档' : '待归档'}<span className="archive-count">{history ? items.length : `${waves.length} 波 · ${rows.length} 个`}</span></h1><div className="archive-iconActions"><button className="archive-iconButton" aria-label={history ? '返回待归档' : '查看已归档记录'} title={history ? '返回待归档' : '已归档记录'} disabled={busy} onClick={() => setView(!history)}>{history ? <Archive size={17} /> : <History size={17} />}</button><button className="archive-iconButton" aria-label="刷新归档预览" title="刷新" disabled={busy || loading} onClick={() => load()}><RefreshCw size={16} className={loading ? 'animate-spin motion-reduce:animate-none' : ''} /></button></div></header>
        <label className="archive-search"><Search size={15} /><input ref={searchInput} aria-label="搜索人物、文件或路径" placeholder="搜索人物、文件或路径…" disabled={busy} value={query} onChange={event => setQuery(event.target.value)} /><kbd>/</kbd></label>
        {!history && <div className="archive-filters">{([
          ['all', '全部', waves.length], ['ok', '可归档', waves.filter(w => w.rows.some(r => r.status === 'ok')).length],
          ['issues', '需处理', waves.filter(w => w.rows.some(r => r.status !== 'ok')).length], ['selected', '已选', selectedWaves.length],
        ] as const).map(([id, label, count]) => <button key={id} disabled={busy} aria-pressed={filter === id} onClick={() => id === 'selected' ? showSelected() : setFilter(id)}>{label}<span>{count}</span></button>)}</div>}
        {!history && <div className="archive-selection"><ArchiveCheckbox label="选择当前列表的全部波次" disabled={disabled || !visibleIds.size} checked={!!visibleIds.size && [...visibleIds].every(id => selected.has(id))} mixed={[...visibleIds].some(id => selected.has(id)) && ![...visibleIds].every(id => selected.has(id))} onChange={() => toggle([...visibleIds])} /><span>{visibleWaves.length} 波</span><button disabled={disabled || !visibleWaves.some(w => w.rows.some(r => r.status === 'ok'))} onClick={() => setSelected(previous => new Set([...previous, ...visibleWaves.flatMap(w => w.rows.filter(r => r.status === 'ok').map(r => r.vid))]))}>选中可归档</button><button disabled={disabled || !chosen.length} onClick={() => setSelected(new Set())}>清空</button></div>}
        <div ref={scrollList} className="archive-listScroll">
          {loading && !plan ? <div role="status" className="archive-empty">检查视频与目标路径…</div> : (history ? !filtered.length : !visibleWaves.length) ? <div className="archive-empty"><Archive size={28} /><p>{query || filter !== 'all' ? '没有匹配的记录' : history ? '还没有归档记录' : '当前没有待归档视频'}</p>{query || filter !== 'all' ? <Button variant="ghost" size="sm" onClick={() => { setQuery(''); setFilter('all') }}>查看全部</Button> : !history && <Link className="underline underline-offset-4" href="/clusters">核对相似视频组</Link>}</div> : history ? <ArchiveFolderList items={filtered} active={current?.vid} selected={selected} collapsed={collapsed} disabled={disabled} history onToggle={toggle} onOpen={openItem} onCollapse={path => setCollapsed(previous => { const next = new Set(previous); next.has(path) ? next.delete(path) : next.add(path); return next })} /> : <ArchiveBatchList batches={visibleWaves} selected={selected} active={wave?.key} disabled={disabled} onToggle={toggle} onOpen={chooseWave} onResolve={resolveInline} onCustomize={setResolving} onUndo={undoRows} />}
        </div>
        <footer className="archive-listBottom"><span>{history ? `存放目录 · ${folders.length} 个` : '点击查看详情 · 勾选整波'}</span><span title="A 全选/取消当前列表；Enter 打开归档确认；J/K 切换波次；X 勾选当前波"><kbd>A</kbd>全选 <kbd>↵</kbd>归档 <kbd>J</kbd><kbd>K</kbd>浏览</span></footer>
      </section>
      <section className="archive-detail" aria-label="波次与视频详情">
        <header className="archive-detailTop"><button className="archive-mobileBack" onClick={() => setMobileDetail(false)}><ArrowLeft size={15} />返回列表</button><div className="archive-breadcrumb"><button className="archive-iconButton" aria-label={listHidden ? '展开视频列表' : '收起视频列表'} aria-expanded={!listHidden} onClick={() => setListHidden(!listHidden)}><PanelLeft size={17} /></button><i /><span>{history ? '已归档' : '波次详情'}</span><ChevronRight size={13} /><b>{current?.creator || '视频详情'}</b></div><div className="archive-paging"><span>{current ? activeIndex + 1 : 0} / {filtered.length}</span><button className="archive-iconButton" aria-label="上一个视频" disabled={!filtered.length} onClick={() => move(-1)}><ChevronLeft size={17} /></button><button className="archive-iconButton" aria-label="下一个视频" disabled={!filtered.length} onClick={() => move(1)}><ChevronRight size={17} /></button></div></header>
        <div ref={scrollDetail} className="archive-detailScroll">
          {!history && wave && <section className="archive-waveDetail" aria-label="本波视频">
            <div className="archive-waveHeading"><div><h2>{batchCreator(wave)}</h2><p>{waveTime(wave.at)} · {waveRows.length} 个视频 · 已选 {waveRows.filter(r => selected.has(r.vid)).length} 个</p></div><ArchiveCheckbox label="选择详情中的整波视频" checked={waveRows.every(r => selected.has(r.vid))} mixed={waveRows.some(r => selected.has(r.vid)) && !waveRows.every(r => selected.has(r.vid))} disabled={disabled} onChange={() => toggle(waveRows.map(r => r.vid))} /></div>
            <div className="archive-waveVideos">{waveRows.map(row => <div className="archive-waveVideo" key={row.vid} data-vid={row.vid} data-active={current?.vid === row.vid}>
              <div className="archive-waveVideoMain"><ArchiveCheckbox label={`选择视频 ${row.name}`} checked={selected.has(row.vid)} disabled={disabled} onChange={() => toggle([row.vid])} />
              <button data-archive-navigate aria-label={`查看视频 ${row.name}`} aria-current={current?.vid === row.vid ? 'true' : undefined} onClick={() => openItem(row.vid, false)}><span>{row.name}</span><small>{row.size_gb.toFixed(2)} GB</small><span className={row.status === 'ok' ? 'archive-good' : 'archive-warning'}>{archiving&&job?.vid===row.vid?'正在处理':rowStatus(row)}</span></button></div>
              {result?.failures?.find(f=>f.vid===row.vid)&&<p className="archive-warning px-3 pb-2 text-xs">{result.failures.find(f=>f.vid===row.vid)?.message}</p>}
              <ArchiveRowActions rows={[row]} disabled={disabled} onResolve={resolveInline} onCustomize={setResolving} onUndo={undoRows} />
            </div>)}</div>
          </section>}

          {current && detailVideo ? <>
            <div className="archive-detailTitle"><div><h2>{current.name}</h2><p>{current.size_gb !== undefined ? `${current.size_gb.toFixed(2)} GB · ` : ''}{sourceLabel[(pending || archived)?.source || 'legacy'] || '归名确认'}</p></div><span className={`archive-badge ${pending && pending.status !== 'ok' ? 'archive-warning' : 'archive-good'}`}>{pending && pending.status !== 'ok' ? <CircleAlert size={13} /> : <CircleCheck size={13} />}{current.statusLabel}</span></div>
            <ArchiveVideoPlayer key={detailVideo.path || detailVideo.vid} video={detailVideo} detailVisibleOnMobile={mobileDetail} />
            <div className="archive-paths">
              <div className="archive-pathLine"><Folder size={16} /><PathText value={current.src || current.vid} label={history ? '现在位置' : pending?.src ? '当前位置' : '原记录路径（源文件未找到）'} /></div>
              {pending?.dst && <><div className="archive-pathConnector"><ArrowDown size={14} /><span>{pending.operation === 'stage-duplicate' ? '保留已有目标，完整校验后暂存重复源文件' : '归档后移动到'}</span></div><div className="archive-pathLine"><Archive size={16} /><PathText value={pending.dst} label={pending.operation === 'stage-duplicate' ? '保留的目标文件' : '目标人物目录'} /></div></>}
              {archived?.staged_path && <><div className="archive-pathConnector"><ArrowDown size={14} /><span>源文件保留，未删除</span></div><div className="archive-pathLine"><Archive size={16} /><PathText value={archived.staged_path} label="重复源文件暂存位置" /></div></>}
            </div>
            <div className="archive-person"><span className="archive-avatar">{current.creator.slice(0, 1)}</span><div><small>已确认人物</small><strong>{current.creator}<ShieldCheck size={14} className="archive-good" /></strong></div><span>{history ? '归档已完成' : '可跨波次勾选归档'}</span></div>
            <div className={`archive-reason ${pending && pending.status !== 'ok' ? 'archive-warning' : 'archive-good'}`}>{pending && pending.status !== 'ok' ? <CircleAlert size={16} /> : <CheckCheck size={16} />}<span>{pending?.reason || '已完成文件移动与 Peach 账本更新。'}</span></div>
            {pending && <div className="archive-detailsActions">{conflicts.some(r => r.vid === pending.vid) && <Button variant="outline" size="sm" disabled={disabled} onClick={() => setResolving([pending])}>{pending.resolution ? '更改处理方式' : '选择同名文件处理方式'}</Button>}<Button variant="ghost" size="sm" disabled={disabled} onClick={() => setUndoRequest({ vids: [pending.vid], goBack: true })}><Undo2 size={14} />退回待确认，重新选人</Button></div>}
            {(pending || archived)?.note && <p className="archive-reason">备注：{(pending || archived)?.note}</p>}<div className="archive-metadata"><span>{waveTime((pending || archived)?.at || '')} {history ? '归档记录' : '加入队列'}</span>{pending?.return_href && <Link href={'/review?vid='+encodeURIComponent(pending.vid)} className="underline underline-offset-4">查看归名来源</Link>}</div>
          </> : <div className="archive-empty"><CircleCheck size={28} /><p>{loading ? '正在读取归档计划…' : '从左侧选择视频查看详情'}</p></div>}
        </div>
      </section>
    </div>
    <footer className="archive-actionbar" data-instant={instant}>
      {archiving||result ? <div ref={runStatus} tabIndex={-1} className="archive-run">
        <WorkbenchStep phase={archiving?'running':'result'} className="archive-runBody">
          <strong role="status" aria-live="polite">{archiving?(connection||job?.stage||'正在提交归档…'):resultTitle}{archiving&&job?.done!=null&&job.total!=null?` · ${job.done} / ${job.total} ${job.unit}`:''}</strong>
          {archiving&&job?.current&&<p title={job.current} className="archive-currentFile">{job.current}</p>}
          {archiving&&<Progress aria-label={job?.stage||'正在提交归档'} value={progressPercent} className="archive-progress" data-unknown={progressPercent===null} />}
        </WorkbenchStep>
        {archiving&&(connection||!job)&&<Button variant="outline" onClick={async()=>{if(!watchedId)return;try{const data=await api<{job:HomeJob;result?:ApplyResult}>('/api/plan/progress',{method:'POST',body:JSON.stringify({id:watchedId})});if(data.result)settle(data.result,watchedId);else setJob(data.job)}catch(e){setConnection((e as Error).message)}}}>核对结果</Button>}
        {!archiving&&<div className="archive-actionButtons">{!!result?.moved&&<Button variant="outline" onClick={()=>setView(true)}>查看归档位置<ArrowRight size={14}/></Button>}{!!result?.failures?.length&&<Button variant="outline" onClick={()=>{setSelected(new Set(result.failures!.map(f=>f.vid)));showSelected()}}>查看失败项</Button>}{!result?.recovery_needed&&<Button onClick={()=>{setResult(null);requestAnimationFrame(()=>archiveTrigger.current?.focus())}}>返回检查</Button>}</div>}
      </div>:<>

      <div><strong>{history ? `最近 ${plan?.history.length || 0} 条归档记录` : `已选 ${selectedWaves.length} 波 · ${chosen.length} 个视频`}{!history && <span>{selectGB} GB 可归档</span>}</strong>{!history && <p>{busy ? '正在处理，请保留此页面。' : `${validChosen.length} 个可归档${invalid ? `，${invalid} 个异常留待处理` : ''}；切换详情、搜索和筛选保留勾选。`}</p>}{!history && chosen.length > 5000 && <p className="archive-warning">单次最多处理 5000 个视频，请减少勾选数量。</p>}{!history && hiddenSelected > 0 && <button className="archive-hiddenSelection" onClick={showSelected}>{hiddenSelected} 个已选视频被筛选隐藏 · 查看已选</button>}</div>
      {!history && <div className="archive-actionButtons">{chosenConflicts.length > 0 && <Button variant="outline" size="sm" disabled={disabled} onClick={() => setResolving(chosenConflicts)}>处理已选同名 {chosenConflicts.length}</Button>}<Button variant="outline" disabled={disabled || !chosen.length || chosen.length > 5000} onClick={() => setUndoRequest({ vids: chosen.map(r => r.vid), goBack: false })}><Undo2 size={15} />退回待确认</Button><Button ref={archiveTrigger} title="Enter 打开确认" disabled={disabled || !validChosen.length || validChosen.length > 5000} onClick={() => setConfirm(true)}><FolderInput size={16} />{`归档可执行的 ${validChosen.length} 个`}</Button></div>}
      </>}
    </footer>
    <AlertDialog open={!!undoRequest} onOpenChange={open => !open && setUndoRequest(null)}>
      <AlertDialogContent><AlertDialogHeader><AlertDialogTitle>将这 {undoRequest?.vids.length || 0} 个视频退回待确认？</AlertDialogTitle><AlertDialogDescription>包含所有勾选波次中的对应视频，即使部分已被搜索或筛选隐藏。这里只撤回人物归名，不删除或移动文件。</AlertDialogDescription></AlertDialogHeader><AlertDialogFooter><AlertDialogCancel>保留归名，返回检查</AlertDialogCancel><AlertDialogAction onClick={() => { const request = undoRequest; setUndoRequest(null); if (request) void undo(request.vids, request.goBack) }}>确定退回待确认</AlertDialogAction></AlertDialogFooter></AlertDialogContent>
    </AlertDialog>
    <AlertDialog open={confirm} onOpenChange={setConfirm}>
      <AlertDialogContent inert={!confirm} aria-hidden={!confirm||undefined} style={{pointerEvents:confirm?'auto':'none'}} data-instant={instant} onCloseAutoFocus={event=>{event.preventDefault();requestAnimationFrame(()=>(watchRef.current?runStatus.current:archiveTrigger.current)?.focus())}} className="archive-confirmDialog max-h-[90dvh] overflow-y-auto" onOpenAutoFocus={event => { event.preventDefault(); confirmButton.current?.focus() }} onKeyDownCapture={event => { if (event.key === 'Enter' && (event.repeat || event.nativeEvent.isComposing)) event.preventDefault() }}><AlertDialogHeader><AlertDialogTitle>归档 {readySelectedWaves.length} 波中的 {validChosen.length} 个视频？</AlertDialogTitle><AlertDialogDescription>将实际处理约 {selectGB} GB 文件，并同步 Peach 账本。{stagedCount ? `其中 ${stagedCount} 个重复源文件将完整校验后移入暂存，已有目标保留。` : '视频将移动到各自的人物目录，使用详情中的文件名。'}{hiddenSelected > 0 ? `包含被筛选隐藏的已选视频（共 ${hiddenSelected} 个，异常项不执行）。` : ''}</AlertDialogDescription></AlertDialogHeader>
        {invalid > 0 && <div className="archive-confirmIssues"><strong>{invalid} 个异常项留在待归档，本次不执行</strong><ul>{blocked.slice(0, 5).map(row => <li key={row.vid}>{row.name}：{rowStatus(row)}</li>)}</ul>{invalid > 5 && <p>另有 {invalid - 5} 个异常项。</p>}</div>}
        <div className="max-h-48 space-y-2 overflow-y-auto rounded-lg border bg-muted/30 p-3 text-sm">{[...new Set(validChosen.map(row => row.creator))].map(creator => <p key={creator} className="break-all">{creator} · {validChosen.filter(row => row.creator === creator).length} 个 → creators/{creator}/</p>)}</div>
        <AlertDialogFooter><AlertDialogCancel>返回继续检查（Esc）</AlertDialogCancel><AlertDialogAction ref={confirmButton} onClick={apply}><Check className="mr-1 h-4 w-4" />确认归档 {validChosen.length} 个（Enter）</AlertDialogAction></AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
    {resolving && plan && <ArchiveResolutionDialog key={resolving.map(row => row.vid).join()} rows={resolving} version={plan.version} onClose={() => setResolving(null)} onSaved={async () => {
      const data = await load()
      if (!data) { toast.warning('选择已保存，预览刷新失败，请点刷新后继续。', toastOptions); return }
      toast.success('处理方式已保存，所有仍在队列中的勾选已保留。点击归档才会移动文件。', toastOptions)
    }} />}
  </main>
}
