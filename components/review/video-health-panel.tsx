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
  return <section id="video-health" className="scroll-mt-4 space-y-4 rounded-xl border bg-card p-4 sm:p-5">
    <div className="flex flex-wrap items-start justify-between gap-3"><div className="space-y-2"><h2 className="text-xl font-semibold">视频体检与失效记录</h2><p className="max-w-2xl text-sm leading-relaxed text-muted-foreground">每天凌晨 3:30 自动检查。连续 3 次确认缺失或损坏后，从待确认列表收起；文件和识别记录保留。路径或文件恢复后，会重新显示。</p></div><Button className="min-h-11" variant="outline" disabled={loading} onClick={load}>刷新检查结果</Button></div>
    {error&&<div role="alert" className="rounded-lg border border-amber-500/30 p-3 text-sm">{error}<Button className="ml-2 min-h-11" variant="ghost" onClick={load}>重试</Button></div>}
    {loading&&!data?<p role="status" className="py-6 text-sm text-muted-foreground">加载视频体检记录…</p>:data&&<>
      <div className="space-y-2 rounded-lg border bg-background p-3 text-sm"><p>{data.status==='running'?'正在检查视频，结果会自动更新。':data.status==='skipped'?'本次检查暂停，沿用之前的结果，没有新增收起记录。':data.completed_at?'上次检查已完成。':'第一次自动体检还未完成。'}</p><p className="text-xs text-muted-foreground">最近成功：{date(data.last_success_at)} · 下次：{data.next_run_at?date(data.next_run_at):'每天凌晨 3:30'}</p>{data.error&&<p role="alert" className="break-all text-xs text-amber-300">{data.error}</p>}</div>
      <div className="grid grid-cols-3 gap-2">{[{label:'已检查路径',value:data.total??0},{label:'已收起记录',value:data.hidden??0},{label:'等待结构检查',value:data.waiting??0}].map(x=><div key={x.label} className="rounded-lg border p-3"><p className="text-xs text-muted-foreground">{x.label}</p><p className="mt-2 text-xl font-semibold tabular-nums">{x.value}</p></div>)}</div>
      <p className="text-xs leading-relaxed text-muted-foreground">已记录 {completedChecks} 条检查结果。结构检查分批进行，每晚最多检查 300 个，播放失败的视频优先。网络超时和编码兼容性问题会保留显示。</p>
      <div className="flex flex-wrap gap-2">{[{id:'all',label:'全部异常记录'},{id:'hidden',label:'已收起'},{id:'watch',label:'复查中／需核对'}].map(t=><Button className="min-h-11" key={t.id} variant={filter===t.id?'secondary':'ghost'} onClick={()=>{setFilter(t.id);setLimit(50)}}>{t.label}</Button>)}</div>
      <Input className="h-11 text-base" aria-label="搜索失效文件名或路径" placeholder="搜索失效文件名或路径" value={query} onChange={e=>{setQuery(e.target.value);setLimit(50)}}/>
      {!rows.length?<p className="rounded-lg border border-dashed p-6 text-sm text-muted-foreground">{query?'当前搜索没有匹配的记录。':filter==='hidden'?'还没有连续确认并收起的记录。':'当前没有这类异常记录。'}</p>:<div className="space-y-2">{rows.slice(0,limit).map(r=><details key={r.vid} className="rounded-lg border bg-background p-3"><summary className="min-h-11 cursor-pointer space-y-2"><span className="block break-all text-sm font-medium">{fileName(r.path||r.vid)}</span><span className="flex flex-wrap gap-2"><Badge variant="outline">{healthTitles[r.status]||'需要复查'}</Badge><Badge variant={r.hidden?'secondary':'outline'}>{r.hidden?'已收起':r.consecutive?`复查 ${r.consecutive}/3`:'保留显示'}</Badge></span></summary><div className="mt-4 space-y-3"><p className="break-all text-sm text-muted-foreground">{r.detail||healthTitles[r.status]}</p><PathText value={r.path||r.vid} label="文件路径"/><p className="text-xs text-muted-foreground">最近检查：{date(r.last_probed_at||r.last_checked_at)}{r.codec?` · 编码 ${r.codec}`:''}</p>{!!r.size&&r.path&&<Button className="min-h-11" variant="outline" onClick={()=>setPlaying({vid:r.vid,path:r.path!,name:fileName(r.path!),thumb:null})}>播放核对</Button>}<p className="text-xs leading-relaxed text-muted-foreground">{r.hidden?'记录已从待确认入口收起。文件恢复后会重新显示，无需重新创建人物。':'系统会继续复查。手机播放失败本身不会触发自动收起。'}</p></div></details>)}</div>}
      {rows.length>limit&&<Button className="min-h-11 w-full" variant="outline" onClick={()=>setLimit(n=>n+50)}>再显示 50 条（共 {rows.length} 条）</Button>}
    </>}
    <VideoPlayer video={playing} onClose={()=>setPlaying(null)}/>
  </section>
}
