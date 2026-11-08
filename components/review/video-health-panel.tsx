'use client'
import { useEffect, useRef, useState } from 'react'
import { api } from '@/lib/face-api'
import { healthTitles, type VideoHealthRecord, type VideoHealthReport } from '@/lib/video-health-model'
import { fileName, type ReviewVideo } from '@/lib/review-ui'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Badge } from '@/components/ui/badge'
import { PathText, VideoPlayer } from './video'

type Report=Omit<VideoHealthReport,'records'>&{rows:VideoHealthRecord[]}
const date=(s?:string)=>s?new Intl.DateTimeFormat('zh-CN',{timeZone:'Asia/Shanghai',month:'numeric',day:'numeric',hour:'2-digit',minute:'2-digit'}).format(new Date(s)):'尚未运行'
export function VideoHealthPanel(){
  const [data,setData]=useState<Report|null>(null),[error,setError]=useState(''),[loading,setLoading]=useState(true)
  const [filter,setFilter]=useState('all'),[query,setQuery]=useState(''),[limit,setLimit]=useState(50),[playing,setPlaying]=useState<ReviewVideo|null>(null)
  const located=useRef(false)
  const load=async()=>{setLoading(true);setError('');try{setData(await api<Report>('/api/video-health'))}catch(e){setError((e as Error).message)}finally{setLoading(false)}}
  useEffect(()=>{load()},[])
  useEffect(()=>{
    if(!data||located.current)return
    located.current=true
    if(window.location.hash!=='#video-health')return
    const frame=requestAnimationFrame(()=>document.getElementById('video-health')?.scrollIntoView({block:'start'}))
    return()=>cancelAnimationFrame(frame)
  },[data])
  useEffect(()=>{if(data?.status!=='running')return;const timer=setInterval(load,15000);return()=>clearInterval(timer)},[data?.status])
  const rows=(data?.rows||[]).filter(r=>(filter==='all'||(filter==='hidden'?r.hidden:!r.hidden))&&(!query||[r.vid,r.path||'',r.detail||''].some(s=>s.toLowerCase().includes(query.toLowerCase()))))
  const completedChecks=Object.entries(data?.counts||{}).filter(([status])=>!['unchecked','present'].includes(status)).reduce((n,[,count])=>n+count,0)
  return <section id="video-health" className="scroll-mt-4 space-y-3 rounded-[12px] border bg-card p-4">
    <div className="flex flex-wrap items-center justify-between gap-3"><div className="flex items-baseline gap-2.5"><h2 className="text-[15px] font-semibold tracking-tight">视频体检与失效记录</h2><span className="text-xs text-muted-foreground">已记录 {completedChecks} 条</span></div><Button size="sm" variant="outline" disabled={loading} onClick={load}>刷新检查结果</Button></div>
    {error&&<div role="alert" className="rounded-[8px] border border-[var(--wb-warning-border)] bg-[var(--wb-warning-soft)] p-3 text-sm text-[var(--wb-warning)]">{error}<Button className="ml-2" size="sm" variant="ghost" onClick={load}>重试</Button></div>}
    {loading&&!data?<p role="status" className="py-6 text-sm text-muted-foreground">加载视频体检记录…</p>:data&&<>
      <div className="flex flex-wrap items-center justify-between gap-2 rounded-[8px] border bg-background px-3 py-2 text-xs text-muted-foreground"><span>{data.status==='running'?'正在检查视频，结果会自动更新。':['skipped','interrupted'].includes(data.status||'')?'本次检查未完成，沿用之前的结果。':data.completed_at?'上次检查已完成。':'第一次自动体检还未完成。'}</span><span>最近成功：{date(data.last_success_at)} · 下次：{data.next_run_at?date(data.next_run_at):'每天 03:30'}</span>{data.error&&<span role="alert" className="w-full break-all text-[var(--wb-warning)]">{data.error}</span>}</div>
      <div className="grid grid-cols-3 gap-2.5">{[{label:'已检查路径',value:data.total??'未知'},{label:'已收起记录',value:data.hidden??0},{label:'等待结构检查',value:data.waiting??'未知'}].map(x=><div key={x.label} className="rounded-[8px] border bg-background p-3"><p className="text-xs text-muted-foreground">{x.label}</p><p className="mt-1.5 text-xl font-semibold tabular-nums">{x.value}</p></div>)}</div>
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="flex flex-wrap gap-1">{[{id:'all',label:'全部异常记录'},{id:'hidden',label:'已收起'},{id:'watch',label:'复查中／需核对'}].map(t=><Button size="sm" key={t.id} variant={filter===t.id?'secondary':'ghost'} onClick={()=>{setFilter(t.id);setLimit(50)}}>{t.label}</Button>)}</div>
        <Input className="max-w-xs" aria-label="搜索失效文件名或路径" placeholder="搜索失效文件名或路径" value={query} onChange={e=>{setQuery(e.target.value);setLimit(50)}}/>
      </div>
      {!rows.length?<p className="rounded-[8px] border border-dashed p-6 text-sm text-muted-foreground">{query?'当前搜索没有匹配的记录。':filter==='hidden'?'还没有连续确认并收起的记录。':!data.last_success_at?'尚无成功检查结果。':'当前没有这类异常记录。'}</p>:<div className="space-y-2">{rows.slice(0,limit).map(r=><details key={r.vid} className="rounded-[8px] border bg-background p-3"><summary className="min-h-8 cursor-pointer space-y-1.5 max-md:min-h-11"><span className="block break-all text-sm font-medium">{fileName(r.path||r.vid)}</span><span className="flex flex-wrap gap-1.5"><Badge variant="outline">{healthTitles[r.status]||'需要复查'}</Badge><Badge variant={r.hidden?'secondary':'outline'}>{r.hidden?'已收起':r.consecutive?`复查 ${r.consecutive}/3`:'保留显示'}</Badge></span></summary><div className="mt-3 space-y-2.5"><p className="break-all text-xs text-muted-foreground">{r.detail||healthTitles[r.status]}</p><PathText value={r.path||r.vid} label="文件路径"/><p className="text-xs text-muted-foreground">最近检查：{date(r.last_probed_at||r.last_checked_at)}{r.codec?` · 编码 ${r.codec}`:''}</p>{!!r.size&&r.path&&<Button size="sm" variant="outline" onClick={()=>setPlaying({vid:r.vid,path:r.path!,name:fileName(r.path!),thumb:null})}>播放核对</Button>}</div></details>)}</div>}
      {rows.length>limit&&<Button className="w-full" size="sm" variant="outline" onClick={()=>setLimit(n=>n+50)}>再显示 50 条（共 {rows.length} 条）</Button>}
    </>}
    <VideoPlayer video={playing} onClose={()=>setPlaying(null)}/>
  </section>
}
