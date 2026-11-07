'use client'

import Link from 'next/link'
import { useCallback, useEffect, useRef, useState } from 'react'
import { Activity, AlertTriangle, ArrowUp, CheckCircle2, ChevronDown, CirclePause, CirclePlay, Clock3, ScanFace, ScanText } from 'lucide-react'
import { toast } from 'sonner'
import { api } from '@/lib/face-api'
import { stageLabel, type ProcessingTask } from '@/lib/processing-model'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Card, CardContent } from '@/components/ui/card'
import { Progress } from '@/components/ui/progress'
import { jobPercent, type HomeTasks } from '@/lib/home-model'

type Data = { online:boolean;paused:boolean;counts:Record<string,number>;all_count:number;total:number;page:number;limit:number;rows:ProcessingTask[];
  last_discovery:null|{at?:number;attempted_at?:number;registered?:number;eligible?:number;error?:string};references:null|{people:number};
  batches:{id:string;total:number;ready:number;error:number;running:number}[];events:{id:number;at:number;kind:string;message:string}[] }
const date = (n?:number) => n ? new Date(n*1000).toLocaleString('zh-CN',{month:'2-digit',day:'2-digit',hour:'2-digit',minute:'2-digit'}) : '尚未完成'
const elapsed = (seconds:number) => seconds < 60 ? `${Math.floor(seconds)} 秒` : `${Math.floor(seconds/60)} 分 ${Math.floor(seconds%60)} 秒`
const tone = (stage:string) => stage==='review' ? 'bg-[var(--wb-success-soft)] text-[var(--wb-success)]' : stage==='error' ? 'bg-[var(--wb-danger-soft)] text-[var(--wb-danger)]' : ['text','face'].includes(stage) ? 'bg-[var(--wb-info-soft)] text-[var(--wb-info)]' : 'bg-muted text-muted-foreground'

export default function Operations() {
  const [data,setData]=useState<Data|null>(null),[error,setError]=useState(''),[busy,setBusy]=useState(false)
  const [stage,setStage]=useState(''),[batch,setBatch]=useState(''),[query,setQuery]=useState(''),[search,setSearch]=useState(''),[page,setPage]=useState(0)
  const [live,setLive]=useState<HomeTasks|null>(null),[progressError,setProgressError]=useState(''),[now,setNow]=useState(Date.now()/1000)
  const sequence=useRef(0)
  useEffect(()=>{const timer=setInterval(()=>setNow(Date.now()/1000),1000);return()=>clearInterval(timer)},[])
  useEffect(()=>{
    const sync=()=>{const value=new URLSearchParams(window.location.search).get('stage')||'';setStage(['queued','text','face','review','error'].includes(value)?value:'')}
    sync()
    window.addEventListener('popstate',sync)
    return()=>window.removeEventListener('popstate',sync)
  },[])
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
    const params=new URLSearchParams({stage,batch,q:search,page:String(page)})
    const [queue,progress]=await Promise.allSettled([api<Data>('/api/processing?'+params,{signal:AbortSignal.timeout(10000)}),api<HomeTasks>('/api/home/tasks',{signal:AbortSignal.timeout(10000)})])
    if(token!==sequence.current)return
    if(queue.status==='fulfilled'){setData(queue.value);setError('')}else setError(queue.reason?.message||'任务状态读取失败')
    if(progress.status==='fulfilled'){setLive(progress.value);setProgressError('')}else setProgressError(progress.reason?.message||'实时进度读取失败')
  },[stage,batch,search,page])
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
  return <main className="wb-page space-y-4">
    <header className="wb-header"><div className="flex flex-wrap items-center gap-3"><h1 className="wb-title">处理操作台</h1><span className="inline-flex items-center gap-2 rounded-full border bg-card px-2.5 py-1 text-xs text-muted-foreground"><span className={`size-2 rounded-full ${disconnected?'bg-[var(--wb-danger)]':!data||data.paused?'bg-[var(--wb-warning)]':'bg-[var(--wb-success)]'}`}/>{status}</span></div>
      <div className="flex flex-wrap items-center gap-2"><Button variant="outline" size="sm" disabled={busy||!data} onClick={()=>act(data?.paused?'resume':'pause')}>{data?.paused?<CirclePlay className="size-4"/>:<CirclePause className="size-4"/>}{data?.paused?'恢复自动分析':'暂停自动分析'}</Button><Button size="sm" asChild><Link href="/review">进入人工确认</Link></Button></div>
    </header>
    <Card aria-label="当前处理进度" className="border-[var(--wb-info)]/30">
      <CardContent className="space-y-3 pt-0">
        <div className="flex flex-wrap items-start justify-between gap-3">
          <div className="min-w-0 space-y-1"><h2 className="flex items-center gap-2 text-lg font-semibold"><Activity className="size-5 shrink-0"/>{disconnected?'暂时无法确认当前进度':active?`正在${active.title}`:!live?'正在读取实时进度':data?.paused?'已暂停，等待你恢复':waiting?'等待下一项任务':data?.counts.error?'有异常任务待处理':'分析已完成'}</h2>
            <p className="text-sm text-muted-foreground">{disconnected?'保留上次结果，正在重新连接。':data?.paused?(active?'当前阶段完成后停止；点击上方“恢复自动分析”可继续。':'点击上方“恢复自动分析”继续。'):active?'无需操作，可以离开页面，后台会继续处理。':waiting?'当前没有模型任务在运行；查看下方运行记录。':'可以进入人工确认核对人物。'}</p>
          </div>
          {data&&<Button size="sm" variant="secondary" asChild><Link href="/review">去确认 {ready} 个视频</Link></Button>}
        </div>
        {active&&<div className="space-y-2">
          <p className="break-all text-sm font-medium">{active.current||active.title}</p>
          <div className="flex flex-wrap items-center justify-between gap-2 text-xs text-muted-foreground"><span>{active.stage}{active.done!==null&&active.total!==null?` · ${active.done} / ${active.total} ${active.unit}`:''}</span><span>{disconnected?'上次进度':pct!==null?`当前阶段 ${pct}%`:'当前阶段暂无百分比'}</span></div>
          {pct!==null&&<Progress value={pct} aria-label={`${active.title}当前阶段进度`} aria-valuetext={`${active.done} / ${active.total} ${active.unit}`} className="h-2 [&_[data-slot=progress-indicator]]:transition-none"/>}
          <div className="flex flex-wrap gap-x-4 gap-y-1 text-xs text-muted-foreground"><span>已用 {elapsed(Math.max(0,(disconnected?(live?.at||now):now)-(active.started_at||now)))}</span><span>最近进展：{active.updated_at?new Date(active.updated_at*1000).toLocaleTimeString('zh-CN'):'未知'}</span>{live&&<span>状态更新：{new Date(live.at*1000).toLocaleTimeString('zh-CN')} · 每 5 秒刷新</span>}</div>
        </div>}
        {progressError&&<p role="alert" className="text-sm text-[var(--wb-danger)]">{progressError}</p>}
        {data&&<div className="flex flex-wrap items-center gap-x-5 gap-y-2 border-t pt-3 text-sm"><span><strong className="tabular-nums">{ready}</strong> 个可人工确认</span><span className="text-muted-foreground"><strong className="tabular-nums">{waiting}</strong> 个待分析</span><span className="text-xs text-muted-foreground">一次分析一个视频</span></div>}
        {recent&&<p className="truncate text-xs text-muted-foreground" title={recent.current}>最近完成{recent.kind==='text'?'文字阶段':'人脸阶段'}：{recent.current}</p>}
      </CardContent>
    </Card>
    <div className="flex flex-wrap items-center gap-x-5 gap-y-2 rounded-[12px] border bg-card px-3.5 py-2 text-xs text-muted-foreground">
      <span>上次发现：{date(data?.last_discovery?.at)}</span><span>本次新增：{data?.last_discovery?.registered??'—'} 个</span><span>可用人物参考：{data?.references?.people??'—'}</span><Button variant="ghost" size="sm" className="ml-auto" onClick={load}>刷新状态</Button>
    </div>
    {error&&<p role="alert" className="rounded-[8px] border border-[var(--wb-danger)]/30 bg-[var(--wb-danger-soft)] p-3 text-sm text-[var(--wb-danger)]">{error}</p>}
    {data?.last_discovery?.error&&<p role="alert" className="rounded-[8px] border border-[var(--wb-warning-border)] bg-[var(--wb-warning-soft)] p-3 text-sm text-[var(--wb-warning)]">新增发现暂时失败：{data.last_discovery.error}。系统将自动再次检查，已有任务保留。</p>}
    <section aria-label="阶段状态" className="grid grid-cols-2 gap-3 lg:grid-cols-5">{cards.map(card=><button key={card.key} type="button" aria-pressed={stage===card.key} onClick={()=>updateStage(stage===card.key?'':card.key)} className={`rounded-[12px] border p-3.5 text-left transition-colors ${stage===card.key?'border-foreground/40 bg-accent':'bg-card hover:bg-accent/40'}`}>
      <span className="flex items-center justify-between text-xs text-muted-foreground">{card.title}<card.icon className="size-4"/></span><span className="my-2 block text-2xl font-semibold tabular-nums">{data?card.value:'—'}</span><span className="block text-xs leading-relaxed text-muted-foreground">{card.detail}</span>
    </button>)}</section>
    <section className="space-y-3"><div className="flex flex-wrap items-center justify-between gap-3"><div className="flex items-baseline gap-2.5"><h2 className="text-[15px] font-semibold tracking-tight">任务队列</h2><span className="text-xs text-muted-foreground">{data?.all_count??0} 个在库任务</span></div><Input value={query} onChange={e=>setQuery(e.target.value)} placeholder="搜索文件名或路径" aria-label="搜索处理任务" className="max-w-xs"/></div>
      <div className="flex flex-wrap items-center gap-2"><Button size="sm" variant={!stage?'secondary':'ghost'} onClick={()=>updateStage('')}>全部阶段</Button>{stage&&<span className="text-xs text-muted-foreground">列表仅显示：{stageLabel[stage as keyof typeof stageLabel]} · 顶部进度不受筛选影响</span>}
        <select aria-label="筛选发现批次" value={batch} onChange={e=>{setBatch(e.target.value);setPage(0)}} className="ml-auto h-8 max-w-full rounded-[8px] border bg-background px-2.5 text-xs max-md:min-h-11 max-md:text-base"><option value="">所有发现批次</option>{data?.batches.map(b=><option key={b.id} value={b.id}>{b.id} · {b.total} 个</option>)}</select></div>
      <div className="overflow-x-auto rounded-[12px] border bg-card"><table className="w-full table-fixed text-left text-sm"><thead className="bg-muted/50 text-xs text-muted-foreground"><tr><th className="p-3">视频</th><th className="w-28 p-3 sm:w-36">阶段</th><th className="hidden w-24 p-3 lg:table-cell">队列</th><th className="hidden w-32 p-3 lg:table-cell">更新</th><th className="w-28 p-3 text-right sm:w-36">操作</th></tr></thead><tbody>
        {data?.rows.map(task=><tr key={task.vid} className="border-t"><td className="min-w-0 p-3"><p className="truncate font-medium" title={task.name}>{task.name}</p><p title={task.path} className="mt-1 truncate text-xs text-muted-foreground">{task.path}</p>{task.error&&<p className="mt-1 line-clamp-2 break-all text-xs text-[var(--wb-danger)]">{task.error}</p>}</td>
          <td className="p-3"><span className={`inline-flex whitespace-nowrap rounded-full px-2 py-0.5 text-xs ${tone(task.stage)}`}>{task.manual?'人工接管':stageLabel[task.stage]}</span>{task.stage==='retry'&&<p className="mt-1 text-xs text-muted-foreground">{Math.max(0,Math.ceil(task.next_retry-Date.now()/1000))} 秒后重试</p>}{task.data.face_skipped&&<p className="mt-1 text-xs text-muted-foreground">已确认账号，无需补脸</p>}</td>
          <td className="hidden p-3 text-xs text-muted-foreground lg:table-cell">{task.priority?'优先任务':task.is_new?'新增视频':'历史存量'}</td><td className="hidden whitespace-nowrap p-3 text-xs text-muted-foreground lg:table-cell">{date(task.updated_at)}</td>
          <td className="p-2 text-right"><div className="flex flex-wrap justify-end gap-1">{task.stage==='review'||task.manual?<Button size="sm" variant="outline" asChild><Link href={`/review?vid=${encodeURIComponent(task.vid)}`}>去确认</Link></Button>:['error','retry'].includes(task.stage)?<><Button size="sm" variant="outline" disabled={busy} onClick={()=>act('retry',task.vid)}>重试</Button>{task.stage==='error'&&<Button size="sm" variant="ghost" disabled={busy} onClick={()=>act('takeover',task.vid)}>人工接管</Button>}</>:<Button size="sm" variant="ghost" disabled={busy||['text','face'].includes(task.stage)} onClick={()=>act('priority',task.vid)}><ArrowUp className="size-3"/>优先</Button>}</div></td>
        </tr>)}
        {!data?.rows.length&&<tr><td colSpan={5} className="p-10 text-center text-sm text-muted-foreground">{!data?'正在加载任务…':data.all_count?'当前筛选下没有任务。':'正在等待媒体发现，任务会自动进入队列。'}</td></tr>}
      </tbody></table></div>
      {data&&data.total>data.limit&&<div className="flex items-center justify-end gap-3 text-sm"><span>第 {data.page+1} / {Math.ceil(data.total/data.limit)} 页</span><Button variant="outline" size="sm" disabled={data.page===0} onClick={()=>setPage(data.page-1)}>上一页</Button><Button variant="outline" size="sm" disabled={(data.page+1)*data.limit>=data.total} onClick={()=>setPage(data.page+1)}>下一页</Button></div>}
    </section>
    <div className="grid gap-4 lg:grid-cols-2"><section className="rounded-[12px] border bg-card p-4"><h2 className="mb-3 text-[15px] font-semibold">发现批次</h2><div className="space-y-2">{data?.batches.slice(0,6).map(b=><details key={b.id} className="rounded-[8px] border px-3"><summary className="flex min-h-9 cursor-pointer items-center justify-between gap-2 text-xs max-md:min-h-11"><span className="break-all">{b.id}</span><span className="whitespace-nowrap text-muted-foreground">{b.ready}/{b.total} 已就绪</span><ChevronDown className="size-3"/></summary><div className="space-y-2 pb-3 text-xs text-muted-foreground"><progress className="w-full" aria-label={`批次 ${b.id} 就绪进度`} value={b.ready} max={b.total||1}/><p>正在处理 {b.running} · 异常 {b.error} · 其余等待分析</p><Button size="sm" variant="outline" onClick={()=>{setBatch(b.id);updateStage('')}}>查看本批任务</Button></div></details>)}</div></section>
      <section className="rounded-[12px] border bg-card p-4"><h2 className="mb-3 flex items-center gap-2 text-[15px] font-semibold"><Activity className="size-4"/>最近运行记录</h2><ol className="space-y-2.5">{data?.events.slice(0,6).map(e=><li key={e.id} className="flex gap-3 text-xs"><span className="shrink-0 text-muted-foreground">{date(e.at)}</span><span className="min-w-0 break-all">{e.message}</span></li>)}</ol></section></div>
  </main>
}
