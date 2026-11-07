'use client'

import Link from 'next/link'
import { PrivateVideo } from '@/components/private-media'
import { useCallback, useEffect, useRef, useState } from 'react'
import { Activity, AlertTriangle, ArrowUp, CheckCircle2, ChevronDown, CirclePause, CirclePlay, Clock3, ScanFace, ScanText, RefreshCw, X, ChevronLeft, ChevronRight, SlidersHorizontal, FileVideo } from 'lucide-react'
import { toast } from 'sonner'
import { api } from '@/lib/face-api'
import { stageLabel, type ProcessingTask } from '@/lib/processing-model'
import { Button } from '@/components/ui/button'
import * as Popover from '@radix-ui/react-popover'
import { InputGroupButton } from '@/components/ui/input-group'
import { WorkspaceSearch, WorkspaceTabs, useWideWorkspace } from '@/components/workbench/controls'
import { Dialog, DialogTitle, DialogDescription } from '@/components/ui/dialog'
import { ResponsiveDialogContent } from '@/components/review/responsive-dialog'
import { Progress } from '@/components/ui/progress'
import { jobPercent, type HomeTasks } from '@/lib/home-model'

type Data = { online:boolean;paused:boolean;counts:Record<string,number>;all_count:number;total:number;page:number;limit:number;rows:ProcessingTask[];task?:ProcessingTask|null;
  last_discovery:null|{at?:number;attempted_at?:number;registered?:number;eligible?:number;error?:string};references:null|{people:number};
  batches:{id:string;total:number;ready:number;error:number;running:number}[];events:{id:number;at:number;kind:string;message:string}[] }
const date = (n?:number) => n ? new Date(n*1000).toLocaleString('zh-CN',{month:'2-digit',day:'2-digit',hour:'2-digit',minute:'2-digit'}) : '尚未完成'
const elapsed = (seconds:number) => seconds < 60 ? `${Math.floor(seconds)} 秒` : `${Math.floor(seconds/60)} 分 ${Math.floor(seconds%60)} 秒`

export default function Operations() {
  const [data,setData]=useState<Data|null>(null),[error,setError]=useState(''),[busy,setBusy]=useState(false)
  const [stage,setStage]=useState(''),[batch,setBatch]=useState(''),[query,setQuery]=useState(''),[search,setSearch]=useState(''),[page,setPage]=useState(0)
  const [live,setLive]=useState<HomeTasks|null>(null),[progressError,setProgressError]=useState(''),[now,setNow]=useState(Date.now()/1000)
  const [instantEntry,setInstantEntry]=useState(false)
  const [focused,setFocused]=useState<ProcessingTask|null>(null),[detailOpen,setDetailOpen]=useState(false)
  const wide=useWideWorkspace(),listRef=useRef<HTMLDivElement>(null),returnFocus=useRef<HTMLElement|null>(null)
  const focusId=focused?.vid||''
  const sequence=useRef(0)
  useEffect(()=>{const timer=setInterval(()=>setNow(Date.now()/1000),1000);return()=>clearInterval(timer)},[])
  useEffect(()=>{
    const sync=()=>{const value=new URLSearchParams(window.location.search).get('stage')||'';setStage(['queued','text','face','review','error'].includes(value)?value:'')}
    sync()
    window.addEventListener('popstate',sync)
    return()=>window.removeEventListener('popstate',sync)
  },[])
  useEffect(()=>{if(wide||!focused)setDetailOpen(false)},[wide,focused])
  const updateStage=useCallback((next:string)=>{
    setStage(next)
    setPage(0)
    const url=new URL(window.location.href)
    if(next)url.searchParams.set('stage',next)
    else url.searchParams.delete('stage')
    window.history.replaceState(null,'',url.pathname+url.search)
  },[])
  useEffect(()=>{const timer=setTimeout(()=>{setSearch(query);setPage(0)},300);return()=>clearTimeout(timer)},[query])
  const load=useCallback(async()=>{
    const token=++sequence.current
    const params=new URLSearchParams({stage,batch,q:search,page:String(page),...(focusId?{vid:focusId}:{})})
    const [queue,progress]=await Promise.allSettled([api<Data>('/api/processing?'+params,{signal:AbortSignal.timeout(10000)}),api<HomeTasks>('/api/home/tasks',{signal:AbortSignal.timeout(10000)})])
    if(token!==sequence.current)return
    if(queue.status==='fulfilled'){setData(queue.value);setError('');setFocused(previous=>previous?queue.value.task===undefined?queue.value.rows.find(t=>t.vid===previous.vid)||previous:queue.value.task:null)}else setError(queue.reason?.message||'任务状态读取失败')
    if(progress.status==='fulfilled'){setLive(progress.value);setProgressError('')}else setProgressError(progress.reason?.message||'实时进度读取失败')
  },[stage,batch,search,page,focusId])
  useEffect(()=>{let stop=false;let timer:ReturnType<typeof setTimeout>;const run=async()=>{await load();if(!stop)timer=setTimeout(run,5000)};run();return()=>{stop=true;sequence.current++;clearTimeout(timer)}},[load])
  const act=async(action:string,vid?:string)=>{
    setBusy(true)
    try{await api('/api/processing',{method:'POST',body:JSON.stringify({action,vids:vid?[vid]:undefined})});await load();toast.success({pause:'已暂停，当前阶段完成后停止领取新任务',resume:'已恢复自动分析',priority:'已提高任务优先级',retry:'已重新排队',takeover:'已转入人工确认'}[action]||'已更新')}
    catch(e:any){toast.error(e.message)}finally{setBusy(false)}
  }
  const disconnected=!!error||!!progressError||!!data&&!data.online||!!live&&(!live.online||now-live.at>20)
  const current=live?.jobs.find(j=>['text','face'].includes(j.kind)&&j.status==='running')
  const discovery=live?.jobs.find(j=>j.kind==='discovery'&&j.status==='running')
  const active=current||discovery
  const recent=live?.jobs.filter(j=>['text','face'].includes(j.kind)&&j.status==='complete').sort((a,b)=>(b.finished_at||0)-(a.finished_at||0))[0]
  const pct=active?jobPercent(active):null
  const waiting=(data?.counts.queued||0)+(data?.counts.retry||0)+(data?.counts.face_queued||0)
  const ready=data?.counts.review||0
  const status=!data?'正在读取状态':disconnected?'状态连接中断':data.paused?(active?'暂停中，完成当前阶段后停止':'自动分析已暂停'):active?'自动分析运行中':waiting?'正在等待下一项任务':data.counts.error?'分析结束，有异常待处理':'本轮分析已结束'
  const cards=[
    {key:'queued',title:'排队中',value:(data?.counts.queued||0)+(data?.counts.retry||0),icon:Clock3,detail:'新增优先，存量逐步补齐'},
    {key:'text',title:'文字线索',value:data?.counts.text||0,icon:ScanText,detail:'水印、文件名与文件夹'},
    {key:'face',title:'人脸分析',value:(data?.counts.face||0)+(data?.counts.face_queued||0),icon:ScanFace,detail:'仅补充需要的人脸证据'},
    {key:'review',title:'待人工确认',value:data?.counts.review||0,icon:CheckCircle2,detail:'分析就绪或已人工接管'},
    {key:'error',title:'异常任务',value:data?.counts.error||0,icon:AlertTriangle,detail:'重试耗尽，可手动接管'},
  ]
  const inspect=(task:ProcessingTask)=>{returnFocus.current=document.activeElement as HTMLElement;setInstantEntry(!!document.activeElement?.matches(':focus-visible')||matchMedia('(prefers-reduced-motion: reduce)').matches);setFocused(task);if(!wide)setDetailOpen(true)}
  const taskActions=(task:ProcessingTask)=>task.stage==='review'||task.manual?<Button size="sm" asChild><Link href={`/review?vid=${encodeURIComponent(task.vid)}`}>去人工确认<ChevronRight size={14}/></Link></Button>:['error','retry'].includes(task.stage)?<><Button size="sm" variant="outline" disabled={busy} onClick={()=>act('retry',task.vid)}>重试</Button>{task.stage==='error'&&<Button size="sm" variant="outline" disabled={busy} onClick={()=>act('takeover',task.vid)}>人工接管</Button>}</>:<Button size="sm" variant="outline" disabled={busy||['text','face'].includes(task.stage)} onClick={()=>act('priority',task.vid)}><ArrowUp size={14}/>优先处理</Button>
  const detail=focused&&<><div className="ws-detail-heading"><h2>任务详情</h2>{wide&&<Button className="ws-icon" variant="ghost" size="sm" aria-label="关闭任务详情" title="关闭详情" onClick={()=>setFocused(null)}><X size={15}/></Button>}</div><div className="ws-detail-content" data-sheet-scroll><div className="ws-task-info"><h3 className="break-all text-sm font-medium">{focused.name}</h3><PrivateVideo key={focused.vid} src={`/api/video?p=${encodeURIComponent(focused.path)}`} controls playsInline preload="metadata"/><dl><dt>当前阶段</dt><dd>{focused.manual?'人工接管':stageLabel[focused.stage]}</dd><dt>文件大小</dt><dd>{(focused.size/2**30).toFixed(2)} GB</dd><dt>发现批次</dt><dd>{focused.batch_id}</dd><dt>更新时间</dt><dd>{date(focused.updated_at)}</dd><dt>处理次数</dt><dd>文字 {focused.text_attempts} · 人脸 {focused.face_attempts}</dd><dt>完整路径</dt><dd>{focused.path}</dd></dl>{focused.error&&<p role="alert" className="ws-error">{focused.error}</p>}{focused.stage==='retry'&&<p className="ws-muted">{Math.max(0,Math.ceil(focused.next_retry-now))} 秒后重试</p>}{focused.data.face_skipped&&<p className="ws-muted">已确认账号，无需补脸</p>}</div></div><div className="ws-detail-actions">{taskActions(focused)}</div></>
  return <main className="ws-page" aria-label="处理操作台工作区">
    <header className="ws-header"><h1>处理操作台</h1><div className="ws-actions"><Button variant="outline" size="sm" disabled={busy||!data} onClick={()=>act(data?.paused?'resume':'pause')}>{data?.paused?<CirclePlay size={14}/>:<CirclePause size={14}/>}<span>{data?.paused?'恢复分析':'暂停分析'}</span></Button><Button variant="ghost" className="ws-icon" aria-label="刷新处理状态" title="刷新" onClick={load}><RefreshCw size={15}/></Button></div></header>
    <section className="ws-status-strip" aria-label="当前处理进度"><span className="ws-status-icon">{disconnected?<AlertTriangle size={16}/>:active?.kind==='text'?<ScanText size={16}/>:active?.kind==='face'?<ScanFace size={16}/>:<Activity size={16}/>}</span><div className="ws-status-main"><div className="ws-status-line"><strong>{disconnected?'连接中断':active?active.stage:status}</strong>{active&&<span className="ws-muted">{elapsed(Math.max(0,now-(active.started_at||now)))}</span>}</div><p className="ws-status-file" title={active?.current||undefined}>{active?.current||(!data?'读取任务状态…':data.paused?'完成当前阶段后停止领取任务':recent?.current?`最近完成：${recent.current}`:'等待下一项任务')}</p></div>{active&&<div className="ws-status-meter">{pct!==null?<><Progress value={pct} aria-label="当前阶段进度"/><span>{active.done} / {active.total} {active.unit} · {pct}%</span></>:<span>当前阶段暂无百分比</span>}<span>更新 {date(live?.at)}</span></div>}<Button size="sm" variant="outline" asChild><Link href="/review">待确认 {data?ready:'—'}<ChevronRight size={14}/></Link></Button></section>
    {(error||progressError)&&<p role="alert" className="ws-error">{error||progressError}</p>}
    {data?.last_discovery?.error&&<p role="alert" className="ws-error">发现失败：{data.last_discovery.error}</p>}
    <div className="ws-view-tabs"><WorkspaceTabs value={stage||'all'} onChange={value=>updateStage(value==='all'?'':value)} label="任务阶段" items={[{value:'all',label:'全部',count:data?.all_count},...cards.map(c=>({value:c.key,label:c.title,count:data?c.value:undefined}))]}/></div>
    <div className="ws-operations-body"><section className="ws-queue" aria-label="处理任务列表">
      <div className="ws-toolbar"><WorkspaceSearch value={query} onChange={setQuery} label="搜索处理任务" placeholder="搜索文件名或路径…"><Popover.Root><Popover.Trigger asChild><InputGroupButton size="icon-sm" aria-label="筛选发现批次" title="筛选发现批次"><SlidersHorizontal size={15}/>{batch&&<span className="size-1.5 rounded-full bg-current"/>}</InputGroupButton></Popover.Trigger><Popover.Portal><Popover.Content className="ws-filter-content" align="end" sideOffset={6}><label>发现批次<select aria-label="选择发现批次" value={batch} onChange={e=>{setBatch(e.target.value);setPage(0)}}><option value="">全部批次</option>{data?.batches.map(b=><option key={b.id} value={b.id}>{b.id} · {b.total}</option>)}</select></label>{batch&&<Button size="sm" variant="ghost" className="mt-3" onClick={()=>{setBatch('');setPage(0)}}>清除筛选</Button>}</Popover.Content></Popover.Portal></Popover.Root></WorkspaceSearch><span className="ws-muted shrink-0">{data?.total??'—'} 个</span></div>
      <div ref={listRef} className="ws-list">{data?.rows.map(task=><article key={task.vid} className="ws-row ws-operations-row" data-active={task.vid===focused?.vid}><button type="button" className="ws-row-open" aria-label={`查看任务 ${task.name}`} onClick={()=>inspect(task)}><span className="ws-row-thumb"><FileVideo size={18}/></span><span className="ws-row-copy"><span className="ws-row-name" title={task.name}>{task.name}</span><span className="ws-row-meta" title={task.path}>{task.path}</span></span><span className="ws-row-status">{task.manual?'人工接管':stageLabel[task.stage]}</span><ChevronRight size={14} className="shrink-0 text-muted-foreground"/></button></article>)}{!data?.rows.length&&<p role="status" className="ws-empty">{!data?'正在加载任务…':data.all_count?'当前筛选下没有任务。':'正在等待媒体发现。'}</p>}</div>
      <div className="ws-pagination"><span>{data?`第 ${data.page+1} / ${Math.max(1,Math.ceil(data.total/data.limit))} 页`:'读取中'}</span><div className="ws-actions"><Button variant="ghost" size="sm" className="ws-icon" aria-label="上一页" title="上一页" disabled={!data||data.page===0} onClick={()=>{setPage(data!.page-1);listRef.current?.scrollTo(0,0)}}><ChevronLeft size={15}/></Button><Button variant="ghost" size="sm" className="ws-icon" aria-label="下一页" title="下一页" disabled={!data||(data.page+1)*data.limit>=data.total} onClick={()=>{setPage(data!.page+1);listRef.current?.scrollTo(0,0)}}><ChevronRight size={15}/></Button></div></div>
      <details className="ws-log"><summary>运行记录<span className="ws-muted">{data?.references?.people??'—'} 个人物参考<ChevronDown size={13} className="ml-2 inline"/></span></summary><p className="ws-muted">上次发现 {date(data?.last_discovery?.at)} · 新增 {data?.last_discovery?.registered??'—'} 个</p><ol>{data?.events.map(e=><li key={e.id}><time>{date(e.at)}</time><span className="min-w-0 break-all">{e.message}</span></li>)}</ol><div className="flex flex-wrap gap-2 pb-3">{data?.batches.slice(0,4).map(b=><Button key={b.id} size="sm" variant="outline" onClick={()=>{setBatch(b.id);updateStage('')}}>{b.id} · {b.ready}/{b.total}</Button>)}</div></details>
    </section>{wide&&focused&&<aside className="ws-inspector ws-task-inspector" aria-label="任务详情">{detail}</aside>}</div>
    {!wide&&<Dialog open={detailOpen} onOpenChange={setDetailOpen}><ResponsiveDialogContent open={detailOpen} className="ws-detail-sheet" style={{animationDuration:instantEntry?'0ms':'180ms'}} overlayProps={{style:{animationDuration:instantEntry?'0ms':'140ms'}}} inert={!detailOpen} onCloseAutoFocus={e=>{if(returnFocus.current?.isConnected){e.preventDefault();returnFocus.current.focus()}}}><DialogTitle className="sr-only">任务详情</DialogTitle><DialogDescription className="sr-only">关闭后返回当前队列位置。</DialogDescription>{detail}</ResponsiveDialogContent></Dialog>}
  </main>
}
