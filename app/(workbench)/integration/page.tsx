'use client'

import { useEffect, useMemo, useRef, useState } from 'react'
import { useRouter } from 'next/navigation'
import Link from 'next/link'
import { Archive, ArrowDown, ArrowLeft, ArrowRight, Check, CheckCheck, ChevronLeft, ChevronRight, CircleAlert, CircleCheck, Folder, FolderInput, History, PanelLeft, RefreshCw, Search, ShieldCheck, Undo2, X } from 'lucide-react'
import { toast } from 'sonner'
import { api } from '@/lib/face-api'
import type { ArchivedItem } from '@/lib/review-logic'
import type { PlanRow } from '@/lib/review-plan'
import { batchKey, groupBatches, selectedProblems, statusTitle, nextReviewWave } from '@/lib/review-batches'
import { groupSourceFolders, toggleArchiveSelection } from '@/lib/integration-view'
import { sourceLabel, type ReviewVideo, fileName } from '@/lib/review-ui'
import { ArchiveResolutionDialog } from '@/components/review/archive-resolution'
import { ArchiveCheckbox, ArchiveFolderList, type ArchiveListItem } from '@/components/review/archive-folder-list'
import { VideoThumbnail, VideoPlayer, PathText } from '@/components/review/video'
import { Button } from '@/components/ui/button'
import { AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle } from '@/components/ui/alert-dialog'
import '@/components/review/archive-workbench.css'

type Row = PlanRow
interface ArchiveRecord extends ArchivedItem { name?: string; thumb?: string | null; size_gb?: number }
interface Plan { rows: Row[]; n: number; n_ok: number; total_gb: number; version: string; history: ArchiveRecord[] }
interface ApplyResult { moved: number; staged?: number; remaining: number; errors?: string[] }
interface UndoItem { vid: string; source?: string; return_href?: string }
const toastOptions = { position: 'top-right' as const }
const rowStatus = (row: Row) => row.operation === 'stage-duplicate' ? '重复件待暂存' : statusTitle[row.status] || '需要核对'
const waveTime = (value: string) => {
  const date = new Date(value.includes('T') ? value : value.replace(' ', 'T') + 'Z')
  return Number.isNaN(+date) ? '时间未记录' : new Intl.DateTimeFormat('zh-CN', { timeZone: 'Asia/Shanghai', month: 'numeric', day: 'numeric', hour: '2-digit', minute: '2-digit' }).format(date)
}

export default function Integration() {
  const router = useRouter()
  const [plan, setPlan] = useState<Plan | null>(null)
  const [error, setError] = useState(''), [loading, setLoading] = useState(true), [busy, setBusy] = useState(false)
  const [selected, setSelected] = useState<Set<string>>(new Set())
  const [filter, setFilter] = useState('all'), [query, setQuery] = useState(''), [history, setHistory] = useState(false)
  const [active, setActive] = useState<string | null>(null), [collapsed, setCollapsed] = useState<Set<string>>(new Set())
  const [mobileDetail, setMobileDetail] = useState(false), [listHidden, setListHidden] = useState(false)
  const [playing, setPlaying] = useState<ReviewVideo | null>(null), [confirm, setConfirm] = useState(false)
  const [result, setResult] = useState<ApplyResult | null>(null), [undoResult, setUndoResult] = useState<UndoItem[]>([])
  const [resolving, setResolving] = useState<Row[] | null>(null), [waveKey, setWaveKey] = useState<string | null>(null)
  const [undoRequest, setUndoRequest] = useState<{ vids: string[]; goBack: boolean } | null>(null)
  const requestSequence = useRef(0), writing = useRef(false), searchInput = useRef<HTMLInputElement>(null)
  const scrollList = useRef<HTMLDivElement>(null), scrollDetail = useRef<HTMLDivElement>(null)

  const load = async (reset = false, advance = false) => {
    const sequence = ++requestSequence.current
    setLoading(true); setError('')
    try {
      const data = await api<Plan>('/api/plan/preview')
      if (sequence !== requestSequence.current) return null
      setPlan(data)
      const waves = groupBatches(data.rows)
      if (reset || !waveKey || !waves.some(w => w.key === waveKey) || (advance && !waves.find(w => w.key === waveKey)?.rows.some(r => r.status === 'ok'))) {
        const next = reset || !waveKey ? waves[0] : nextReviewWave(groupBatches(plan?.rows || []), waveKey, waves)
        setWaveKey(next?.key || null); setSelected(new Set()); setActive(null); setCollapsed(new Set())
        setFilter(next?.rows.some(r => r.status === 'ok') ? 'all' : 'issues'); setQuery('')
      } else {
        const remaining = new Set(data.rows.filter(r => batchKey(r) === waveKey).map(r => r.vid))
        setSelected(previous => new Set([...previous].filter(vid => remaining.has(vid))))
      }
      return data
    } catch (cause) {
      if (sequence === requestSequence.current) setError((cause as Error).message)
      return null
    } finally {
      if (sequence === requestSequence.current) setLoading(false)
    }
  }
  useEffect(() => {
    void load(true)
    setHistory(new URLSearchParams(window.location.search).get('view') === 'history')
    return () => { requestSequence.current++ }
  }, [])

  const waves = useMemo(() => groupBatches(plan?.rows || []), [plan])
  const wave = waves.find(w => w.key === waveKey), waveRows = wave?.rows || []
  const readyRows = waveRows.filter(r => r.status === 'ok')
  const chosen = waveRows.filter(r => selected.has(r.vid)), validChosen = chosen.filter(r => r.status === 'ok')
  const invalid = chosen.length - validChosen.length, blockers = selectedProblems(chosen)
  const conflicts = waveRows.filter(r => ['duplicate', 'conflict', 'dupe-target'].includes(r.status) || r.resolution)
  const stagedCount = validChosen.filter(r => r.operation === 'stage-duplicate').length
  const selectGB = validChosen.reduce((sum, row) => sum + (row.size_gb || 0), 0).toFixed(2)
  const disabled = busy || loading || !!error
  const items: ArchiveListItem[] = history
    ? (plan?.history || []).map(r => ({ vid: r.vid, name: r.name || fileName(r.dst || r.win_dst || r.vid), src: r.dst || r.win_dst, thumb: r.thumb || null, creator: r.creator, size_gb: r.size_gb, status: 'archived', statusLabel: '已归档' }))
    : waveRows.map(r => ({ ...r, statusLabel: rowStatus(r) }))
  const filtered = items.filter(r => (history || filter === 'all' || (filter === 'ok' ? r.status === 'ok' : r.status !== 'ok')) &&
    (!query || [r.name, r.vid, r.src || '', r.creator, r.dst || ''].some(value => value.toLowerCase().includes(query.toLowerCase()))))
  const current = filtered.find(r => r.vid === active) || filtered[0]
  const pending = !history && current ? waveRows.find(r => r.vid === current.vid) : undefined
  const archived = history && current ? plan?.history.find(r => r.vid === current.vid) : undefined
  const detailVideo: ReviewVideo | null = current ? { ...current, path: current.src || current.vid } : null
  const folders = groupSourceFolders(filtered)
  const activeIndex = filtered.findIndex(r => r.vid === current?.vid)

  const chooseWave = (key: string) => {
    setWaveKey(key); setFilter('all'); setQuery(''); setSelected(new Set()); setActive(null); setCollapsed(new Set())
    setUndoResult([]); setResult(null); setMobileDetail(false)
    scrollList.current?.scrollTo(0, 0)
  }
  const setView = (value: boolean) => {
    setHistory(value); setSelected(new Set()); setActive(null); setCollapsed(new Set()); setFilter('all'); setQuery(''); setMobileDetail(false)
    const url = new URL(window.location.href)
    if (value) url.searchParams.set('view', 'history'); else url.searchParams.delete('view')
    window.history.replaceState(window.history.state, '', url)
    scrollList.current?.scrollTo(0, 0)
  }
  const toggle = (ids: string[]) => { if (!disabled && !history) setSelected(previous => toggleArchiveSelection(previous, ids)) }
  const openItem = (vid: string, mobile = true) => {
    setActive(vid)
    if (mobile) setMobileDetail(true)
    const directory = folders.find(folder => folder.items.some(item => item.vid === vid))?.path
    if (directory) setCollapsed(previous => { const next = new Set(previous); next.delete(directory); return next })
    scrollDetail.current?.scrollTo(0, 0)
  }
  const move = (offset: number) => {
    if (!filtered.length) return
    openItem(filtered[(activeIndex + offset + filtered.length) % filtered.length].vid, false)
    requestAnimationFrame(() => scrollList.current?.querySelector('[data-active="true"]')?.scrollIntoView({ block: 'nearest' }))
  }
  useEffect(() => {
    const key = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement
      if (event.metaKey || event.ctrlKey || event.altKey || /^(INPUT|TEXTAREA|SELECT)$/.test(target.tagName) || target.isContentEditable ||
        target.closest('[role="dialog"],[role="alertdialog"]') || confirm || undoRequest || resolving || playing) return
      if (event.key === '/') {
        event.preventDefault(); setListHidden(false); setMobileDetail(false)
        requestAnimationFrame(() => searchInput.current?.focus())
      }
      if (event.key.toLowerCase() === 'j') { event.preventDefault(); move(1) }
      if (event.key.toLowerCase() === 'k') { event.preventDefault(); move(-1) }
      if (event.key.toLowerCase() === 'x' && current) { event.preventDefault(); toggle([current.vid]) }
    }
    window.addEventListener('keydown', key)
    return () => window.removeEventListener('keydown', key)
  })

  const undo = async (vids: string[], goBack = false) => {
    if (writing.current || disabled || !vids.length) return
    writing.current = true; setBusy(true)
    try {
      const data = await api<{ removed: UndoItem[] }>('/api/plan/queue', { method: 'DELETE', body: JSON.stringify({ vids }) })
      setUndoResult(data.removed); await load()
      toast.success(`已将 ${data.removed.length} 个退回待确认，文件未删除`, toastOptions)
      if (goBack && data.removed[0]?.return_href) router.push(data.removed[0].return_href)
    } catch (cause) { toast.error((cause as Error).message, toastOptions) }
    finally { writing.current = false; setBusy(false) }
  }
  const apply = async () => {
    if (!plan || writing.current || disabled || !chosen.length || invalid) return
    writing.current = true; setBusy(true); setConfirm(false)
    try {
      const data = await api<ApplyResult>('/api/plan/apply', { method: 'POST', body: JSON.stringify({ vids: chosen.map(r => r.vid), version: plan.version }) })
      setResult(data); setSelected(new Set()); await load(false, data.moved > 0)
      if (data.errors?.length) toast.error(`归档 ${data.moved} 个，有 ${data.errors.length} 项需处理`, toastOptions)
      else toast.success(`已处理 ${data.moved} 个${data.staged ? `，其中 ${data.staged} 个重复源文件已暂存` : ''}，Peach 账本已同步`, toastOptions)
    } catch (cause) { toast.error((cause as Error).message, toastOptions); await load() }
    finally { writing.current = false; setBusy(false) }
  }

  return <div className="archive-workbench">
    {error && <div role="alert" className={`archive-notice archive-warning`}>{error}<Button size="sm" variant="ghost" disabled={loading || busy} onClick={() => load()}>重新加载</Button></div>}
    {result && <div className="archive-notice" role="status"><div className="flex items-start justify-between gap-3"><details open={!!result.errors?.length} className="min-w-0 flex-1"><summary>{result.moved ? `已处理 ${result.moved} 个视频，Peach 账本已同步${result.staged ? `；${result.staged} 个重复源文件已暂存` : ''}。` : '本轮未完成归档，相关任务仍在待归档中。'}</summary>{!!result.errors?.length && <ul className="archive-warning">{result.errors.map((message, i) => <li key={i}>{message}</li>)}</ul>}<p>剩余 {plan?.n ?? result.remaining} 项，失败或未选中的任务继续保留。</p>{result.moved > 0 && <Button size="sm" variant="ghost" onClick={() => setView(true)}>查看归档后的位置<ArrowRight size={14} /></Button>}</details><button aria-label="关闭归档结果" className="archive-iconButton" onClick={() => setResult(null)}><X size={15} /></button></div></div>}
    {!!undoResult.length && !history && <div className="archive-notice" role="status"><div className="flex justify-between gap-3"><p>已将 {undoResult.length} 个视频退回待确认，文件没有移动或删除。</p><button aria-label="关闭退回结果" className="archive-iconButton" onClick={() => setUndoResult([])}><X size={15} /></button></div><div className="archive-noticeLinks">{Array.from(new Map(undoResult.map(r => [r.return_href, r])).values()).map((r, i) => <Link key={r.return_href || i} href={r.return_href || '/clusters'}>回到{sourceLabel[r.source || 'legacy'] || '原确认入口'}</Link>)}</div></div>}
    <div className="archive-split" data-hidden={listHidden} data-mobile-detail={mobileDetail}>
      <section className="archive-list" aria-label="视频列表" aria-busy={loading}>
        <header className="archive-listHeading"><h1>{history ? '已归档' : '待归档'}<span className="archive-count">{items.length}</span></h1><div className="archive-iconActions"><button className="archive-iconButton" aria-label={history ? '返回待归档' : '查看已归档记录'} title={history ? '返回待归档' : '已归档记录'} disabled={busy} onClick={() => setView(!history)}>{history ? <Archive size={17} /> : <History size={17} />}</button><button className="archive-iconButton" aria-label="刷新归档预览" title="刷新" disabled={busy || loading} onClick={() => load()}><RefreshCw size={16} className={loading ? 'animate-spin motion-reduce:animate-none' : ''} /></button></div></header>
        {!history && <div className="archive-wave"><select aria-label="选择要处理的一波" value={waveKey || ''} disabled={disabled || !waves.length} onChange={event => chooseWave(event.target.value)}><option value="" disabled>{loading ? '正在读取波次…' : '暂无待归档波次'}</option>{waves.map(w => <option key={w.key} value={w.key}>{w.creator} · {waveTime(w.at)} · {w.rows.length} 个</option>)}</select>{wave?.inferred && <small>旧记录按人物与加入时间整理</small>}</div>}
        <label className="archive-search"><Search size={15} /><input ref={searchInput} aria-label="搜索视频" placeholder="搜索文件名、路径或人物…" disabled={busy} value={query} onChange={event => { setQuery(event.target.value); setSelected(new Set()) }} /><kbd>/</kbd></label>
        {!history && <div className="archive-filters">{[['all', '全部', waveRows.length], ['ok', '可归档', readyRows.length], ['issues', '需处理', waveRows.length - readyRows.length]].map(([id, label, count]) => <button key={id} disabled={busy} aria-pressed={filter === id} onClick={() => { setFilter(String(id)); setSelected(new Set()) }}>{label}<span>{count}</span></button>)}</div>}
        {!history && <div className="archive-selection"><ArchiveCheckbox label="选择当前筛选结果" disabled={disabled || !filtered.length} checked={!!filtered.length && filtered.every(r => selected.has(r.vid))} mixed={filtered.some(r => selected.has(r.vid)) && !filtered.every(r => selected.has(r.vid))} onChange={() => toggle(filtered.map(r => r.vid))} /><span>已选 {chosen.length} 项</span><button disabled={disabled || !readyRows.length} onClick={() => { setFilter('ok'); setQuery(''); setSelected(new Set(readyRows.map(r => r.vid))) }}>仅选可归档</button><button disabled={disabled || !chosen.length} onClick={() => setSelected(new Set())}>清空</button></div>}
        <div ref={scrollList} className="archive-listScroll">
          {loading && !plan ? <div role="status" className="archive-empty">检查视频与目标路径…</div> : !filtered.length ? <div className="archive-empty"><Archive size={28} /><p>{query || (!history && waveRows.length) ? '没有匹配的视频' : history ? '还没有归档记录' : '当前没有待归档视频'}</p>{query || (!history && waveRows.length) ? <Button variant="ghost" size="sm" onClick={() => { setQuery(''); setFilter('all'); setSelected(new Set()) }}>清除搜索与筛选</Button> : !history && <Link className="underline underline-offset-4" href="/clusters">核对相似视频组</Link>}</div> : <ArchiveFolderList items={filtered} active={current?.vid} selected={selected} collapsed={collapsed} disabled={disabled} history={history} onToggle={toggle} onOpen={openItem} onCollapse={path => setCollapsed(previous => { const next = new Set(previous); next.has(path) ? next.delete(path) : next.add(path); return next })} />}
        </div>
        <footer className="archive-listBottom"><span>{history ? '实际存放目录' : '来源目录'} · {folders.length} 个</span><span><kbd>J</kbd><kbd>K</kbd>浏览 <kbd>X</kbd>勾选</span></footer>
      </section>
      <section className="archive-detail" aria-label="视频详情">
        <header className="archive-detailTop"><button className="archive-mobileBack" onClick={() => setMobileDetail(false)}><ArrowLeft size={15} />返回列表</button><div className="archive-breadcrumb"><button className="archive-iconButton" aria-label={listHidden ? '展开视频列表' : '收起视频列表'} aria-expanded={!listHidden} onClick={() => setListHidden(!listHidden)}><PanelLeft size={17} /></button><i /><span>{history ? '已归档' : '待归档'}</span><ChevronRight size={13} /><b>{current?.creator || '视频详情'}</b></div><div className="archive-paging"><span>{current ? activeIndex + 1 : 0} / {filtered.length}</span><button className="archive-iconButton" aria-label="上一个视频" disabled={!filtered.length} onClick={() => move(-1)}><ChevronLeft size={17} /></button><button className="archive-iconButton" aria-label="下一个视频" disabled={!filtered.length} onClick={() => move(1)}><ChevronRight size={17} /></button></div></header>
        <div ref={scrollDetail} className="archive-detailScroll">
          {current && detailVideo ? <>
            <div className="archive-detailTitle"><div><h2>{current.name}</h2><p>{current.size_gb !== undefined ? `${current.size_gb.toFixed(2)} GB · ` : ''}{sourceLabel[(pending || archived)?.source || 'legacy'] || '归名确认'}</p></div><span className={`archive-badge ${pending && pending.status !== 'ok' ? 'archive-warning' : 'archive-good'}`}>{pending && pending.status !== 'ok' ? <CircleAlert size={13} /> : <CircleCheck size={13} />}{current.statusLabel}</span></div>
            <div className="archive-paths">
              <div className="archive-pathLine"><Folder size={16} /><PathText value={current.src || current.vid} label={history ? '现在位置' : pending?.src ? '当前位置' : '原记录路径（源文件未找到）'} /></div>
              {pending?.dst && <><div className="archive-pathConnector"><ArrowDown size={14} /><span>{pending.operation === 'stage-duplicate' ? '保留已有目标，完整校验后暂存重复源文件' : '归档后移动到'}</span></div><div className="archive-pathLine"><Archive size={16} /><PathText value={pending.dst} label={pending.operation === 'stage-duplicate' ? '保留的目标文件' : '目标人物目录'} /></div></>}
              {archived?.staged_path && <><div className="archive-pathConnector"><ArrowDown size={14} /><span>源文件保留，未删除</span></div><div className="archive-pathLine"><Archive size={16} /><PathText value={archived.staged_path} label="重复源文件暂存位置" /></div></>}
            </div>
            <div className="archive-person"><span className="archive-avatar">{current.creator.slice(0, 1)}</span><div><small>已确认人物</small><strong>{current.creator}<ShieldCheck size={14} className="archive-good" /></strong></div><span>{history ? '归档已完成' : '仅处理当前波次'}</span></div>
            <div className="archive-preview"><VideoThumbnail video={detailVideo} onPlay={setPlaying} className="w-full sm:w-full" /></div>
            <div className={`archive-reason ${pending && pending.status !== 'ok' ? 'archive-warning' : 'archive-good'}`}>{pending && pending.status !== 'ok' ? <CircleAlert size={16} /> : <CheckCheck size={16} />}<span>{pending?.reason || '已完成文件移动与 Peach 账本更新。'}</span></div>
            {pending && <div className="archive-detailsActions">{conflicts.some(r => r.vid === pending.vid) && <Button variant="outline" size="sm" disabled={disabled} onClick={() => setResolving([pending])}>{pending.resolution ? '更改处理方式' : '选择同名文件处理方式'}</Button>}<Button variant="ghost" size="sm" disabled={disabled} onClick={() => setUndoRequest({ vids: [pending.vid], goBack: true })}><Undo2 size={14} />退回待确认，重新选人</Button></div>}
            <div className="archive-metadata"><span>{waveTime((pending || archived)?.at || '')} {history ? '归档记录' : '加入队列'}</span>{pending?.return_href && <Link href={pending.return_href} className="underline underline-offset-4">查看归名来源</Link>}</div>
          </> : <div className="archive-empty"><CircleCheck size={28} /><p>{loading ? '正在读取归档计划…' : '从左侧选择视频查看详情'}</p></div>}
        </div>
      </section>
    </div>
    <footer className="archive-actionbar">
      <div><strong>{history ? `最近 ${plan?.history.length || 0} 条归档记录` : `已选 ${chosen.length} 个视频`}{!history && <span>{selectGB} GB 可归档</span>}</strong>{!history && <p>{busy ? '正在处理，请保留此页面。' : invalid ? `${invalid} 项异常：${[...new Set(blockers.map(b => b.reason))].join('、')}` : '仅处理当前波次；切换搜索或筛选会清空勾选。'}</p>}{invalid > 0 && validChosen.length > 0 && <button className="archive-blocker" disabled={disabled} onClick={() => setSelected(new Set(validChosen.map(r => r.vid)))}>取消异常项，保留可归档的 {validChosen.length} 个</button>}</div>
      {!history && <div className="archive-actionButtons">{conflicts.length > 0 && <Button variant="outline" size="sm" disabled={disabled} onClick={() => setResolving(conflicts)}>处理同名文件 {conflicts.length}</Button>}<Button variant="outline" disabled={disabled || !chosen.length} onClick={() => setUndoRequest({ vids: chosen.map(r => r.vid), goBack: false })}><Undo2 size={15} />退回待确认</Button><Button disabled={disabled || !chosen.length || !!invalid} onClick={() => setConfirm(true)}><FolderInput size={16} />{busy ? '正在处理…' : `归档选中的 ${chosen.length} 个`}</Button></div>}
    </footer>
    <AlertDialog open={!!undoRequest} onOpenChange={open=>!open&&setUndoRequest(null)}><AlertDialogContent><AlertDialogHeader><AlertDialogTitle>将这 {undoRequest?.vids.length||0} 个视频退回待确认？</AlertDialogTitle><AlertDialogDescription>这一步只撤回“这些视频属于谁”的确认。视频不会删除或移动，也不会恢复已丢失的文件。退回后，可回原入口重新选择人物。只是想继续归档时，请点“保留归名”。</AlertDialogDescription></AlertDialogHeader><AlertDialogFooter><AlertDialogCancel>保留归名，返回检查</AlertDialogCancel><AlertDialogAction onClick={()=>{const r=undoRequest;setUndoRequest(null);if(r)undo(r.vids,r.goBack)}}>确定退回待确认</AlertDialogAction></AlertDialogFooter></AlertDialogContent></AlertDialog>
    <AlertDialog open={confirm} onOpenChange={setConfirm}><AlertDialogContent><AlertDialogHeader><AlertDialogTitle>将选中的 {chosen.length} 个视频归档？</AlertDialogTitle><AlertDialogDescription>这一步会实际处理约 {selectGB} GB 文件，并同步 Peach 账本。{stagedCount>0?`其中 ${stagedCount} 个将完整校验内容，重复源文件移入暂存，目标视频保留；其余视频移动到人物目录。`:'视频将移动到各自的 creators/人物目录，使用预览中的文件名。'}只有选中且检查通过的视频会执行，其他任务继续保留。确认前仍可取消并撤销人物归名。</AlertDialogDescription></AlertDialogHeader><div className="space-y-1 rounded-lg border bg-muted/30 p-3 text-sm">{[...new Set(chosen.map(r=>r.creator))].map(creator=><p key={creator} className="break-all">{creator} · {chosen.filter(r=>r.creator===creator).length} 个视频 → creators/{creator}/</p>)}</div><AlertDialogFooter><AlertDialogCancel>返回继续检查</AlertDialogCancel><AlertDialogAction onClick={apply}><Check className="mr-1 h-4 w-4"/>确认移动并归档</AlertDialogAction></AlertDialogFooter></AlertDialogContent></AlertDialog>{resolving&&plan&&<ArchiveResolutionDialog key={resolving.map(r=>r.vid).join()} rows={resolving} version={plan.version} onClose={()=>setResolving(null)} onSaved={async vids=>{const d=await load();if(!d){toast.warning('选择已保存，预览刷新失败，请点刷新后继续。', toastOptions);return}setFilter('all');setQuery('');setSelected(new Set(d.rows.filter(r=>batchKey(r)===waveKey&&vids.includes(r.vid)&&r.status==='ok').map(r=>r.vid)));toast.success('选择已保存。检查路径后，点击归档才会移动文件。', toastOptions)}}/>}<VideoPlayer video={playing} onClose={()=>setPlaying(null)}/>
  </div>
}
