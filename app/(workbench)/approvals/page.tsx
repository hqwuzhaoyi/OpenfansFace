'use client'
import { useEffect, useRef, useState } from 'react'
import { Check, ArrowRight, SkipForward, RefreshCw } from 'lucide-react'
import { toast } from 'sonner'
import { api, type Claim } from '@/lib/face-api'
import { fileName, type ReviewVideo } from '@/lib/review-ui'
import { Button } from '@/components/ui/button'
import { Dialog, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { PathText, VideoThumbnail, VideoPlayer } from '@/components/review/video'
import { ResponsiveDialogContent } from '@/components/review/responsive-dialog'
import { PersonPicker, type PersonChoice } from '@/components/review/person-picker'
import { PersonReference } from '@/components/review/person-reference'
import { ReviewFlow, QueueReceipt } from '@/components/review/workflow'

export default function Approvals(){
  const [claims,setClaims]=useState<Claim[]>([]),[people,setPeople]=useState<PersonChoice[]>([]),[total,setTotal]=useState(0),[offset,setOffset]=useState(0)
  const [loading,setLoading]=useState(true),[busy,setBusy]=useState(false),[error,setError]=useState(''),[focus,setFocus]=useState('')
  const [playing,setPlaying]=useState<ReviewVideo|null>(null),[changing,setChanging]=useState<Claim|null>(null),[target,setTarget]=useState('')
  const [skipped,setSkipped]=useState<Set<string>>(new Set()),[receipt,setReceipt]=useState<{vids:string[];creator:string}|null>(null)
  const [peopleLoading,setPeopleLoading]=useState(true),[peopleError,setPeopleError]=useState('')
  const peopleSeq=useRef(0)
  const loadPeople=()=>{const token=++peopleSeq.current;setPeopleLoading(true);setPeopleError('');api<{persons:PersonChoice[]}>('/api/persons?picker=1').then(d=>{if(token===peopleSeq.current)setPeople(d.persons)}).catch(e=>{if(token===peopleSeq.current)setPeopleError(e.message)}).finally(()=>{if(token===peopleSeq.current)setPeopleLoading(false)})}
  const limit=8
  const load=async(off:number,focusInitial=false)=>{
    setLoading(true);setError('')
    const vid=focusInitial?new URLSearchParams(window.location.search).get('vid'):null
    if(vid)setFocus(vid)
    try{const d=await api<{total:number;claims:Claim[];offset:number}>(`/api/claims/pending?limit=${limit}&offset=${off}${vid?`&vid=${encodeURIComponent(vid)}`:''}`);setClaims(d.claims);setTotal(d.total);setOffset(d.offset??off);setSkipped(new Set())}
    catch(e:any){setError(e.message)}finally{setLoading(false)}
  }
  useEffect(()=>{load(0,true);loadPeople()},[])
  useEffect(()=>{if(focus&&!loading){const el=Array.from(document.querySelectorAll<HTMLElement>('[data-vid]')).find(el=>el.dataset.vid===focus);el?.scrollIntoView({block:'center'});setFocus('')}},[claims,loading,focus])
  const approve=async(c:Claim,creator=c.name)=>{if(busy)return;setBusy(true);try{await api('/api/claims/decide',{method:'POST',body:JSON.stringify({vid:c.vid,approve:true,creator})});loadPeople();setReceipt({vids:[c.vid],creator});setChanging(null);await load(offset);toast.success(`已确认是 ${creator}，已加入待归档`)}catch(e:any){toast.error(e.message)}finally{setBusy(false)}}
  const undo=async()=>{if(!receipt||busy)return;setBusy(true);try{await api('/api/plan/queue',{method:'DELETE',body:JSON.stringify({vids:receipt.vids})});setReceipt(null);await load(offset);toast.success('已撤销，恢复为待确认')}catch(e:any){toast.error(e.message)}finally{setBusy(false)}}
  return <div className="space-y-5 p-4 md:p-6"><ReviewFlow/><div><h1 className="text-2xl font-semibold tracking-tight">识别待确认</h1><p className="mt-2 text-sm leading-relaxed text-muted-foreground">系统猜到了一个人物，但需要你核对。先播放原视频，再看推荐人物的对照视频；确认后加入待归档，文件暂不移动。</p></div>
    {receipt&&<QueueReceipt count={receipt.vids.length} creator={receipt.creator} onUndo={undo} busy={busy}/>}
    <div className="flex flex-wrap items-center gap-3"><span className="mr-auto text-sm text-muted-foreground">还有 {total} 个待确认{total?` · 第 ${Math.floor(offset/limit)+1} 页`:''}</span><Button variant="outline" size="sm" disabled={loading||busy} onClick={()=>load(offset)}><RefreshCw className="h-3.5 w-3.5"/>刷新</Button><Button variant="outline" size="sm" disabled={offset===0||loading||busy} onClick={()=>load(Math.max(0,offset-limit))}>上一页</Button><Button variant="outline" size="sm" disabled={offset+limit>=total||loading||busy} onClick={()=>load(offset+limit)}>下一页</Button></div>
    {loading?<p role="status" className="py-12 text-center text-muted-foreground">加载待确认视频…</p>:error?<p role="alert" className="text-amber-300">{error}</p>:!claims.length?<div className="rounded-xl border border-dashed p-8 text-center"><p className="text-sm text-muted-foreground">没有待确认的视频了。已确认的可以去待归档继续检查。</p><Button asChild className="mt-4"><a href="/integration">查看待归档<ArrowRight className="h-4 w-4"/></a></Button></div>:<div className="space-y-4">{claims.filter(c=>!skipped.has(c.vid)).map(c=>{
      const v:ReviewVideo={vid:c.vid,path:c.path,name:c.file_name||fileName(c.path||c.vid),thumb:c.thumb}, person=people.find(p=>p.name===c.name)
      return <article key={c.vid} data-vid={c.vid} className="grid min-w-0 gap-4 rounded-xl border bg-card p-4 lg:grid-cols-[minmax(0,1.5fr)_minmax(280px,1fr)]"><div className="min-w-0 space-y-3"><p className="text-xs font-medium text-muted-foreground">1 · 看原视频</p><div className="flex flex-col items-start gap-3 sm:flex-row"><VideoThumbnail video={v} onPlay={setPlaying}/><p className="min-w-0 break-all text-sm font-medium leading-relaxed">{v.name}</p></div><PathText value={c.path||c.vid}/></div>
        <div className="min-w-0 space-y-3 rounded-lg border bg-background p-3"><p className="text-xs font-medium text-muted-foreground">2 · 核对推荐人物</p><div className="flex items-center gap-3">{person?.rep_thumb&&<img src={person.rep_thumb} alt="" loading="lazy" className="h-12 w-12 rounded-full object-cover"/>}<div className="min-w-0"><p className="break-all text-base font-semibold">系统推荐：{c.name}</p><p className="mt-1 text-xs text-muted-foreground">相似度 {c.score.toFixed(3)} · 检出 {c.n_faces} 张脸，仍需人工确认</p></div></div><PersonReference name={c.name}/><div className="space-y-2 border-t pt-3"><p className="text-xs text-muted-foreground">3 · 确认后进入待归档，之后再统一搬文件</p><Button disabled={busy} onClick={()=>approve(c)} className="h-auto min-h-10 w-full whitespace-normal"><Check className="h-4 w-4 shrink-0"/>确认是 {c.name}，加入待归档</Button><div className="flex flex-wrap gap-2"><Button variant="outline" disabled={busy} size="sm" onClick={()=>{loadPeople();setChanging(c);setTarget('')}}>不是她，重新选人</Button><Button variant="ghost" disabled={busy} size="sm" onClick={()=>{setSkipped(s=>new Set([...s,c.vid]));toast('已跳过，仍保留在待确认中')}}><SkipForward className="h-3.5 w-3.5"/>先跳过</Button></div></div></div></article>
    })}</div>}
    {!!skipped.size&&<Button variant="outline" onClick={()=>setSkipped(new Set())}>重新显示本页跳过的 {skipped.size} 个</Button>}
    <Dialog open={!!changing} onOpenChange={o=>!o&&!busy&&setChanging(null)}><ResponsiveDialogContent open={!!changing} className="flex max-h-[90dvh] flex-col gap-0 overflow-hidden p-0 sm:max-w-xl sm:p-0"><DialogHeader className="shrink-0 border-b p-4 pr-14 text-left"><DialogTitle>重新选择人物</DialogTitle><DialogDescription data-keyboard-hide>原推荐为 {changing?.name}。选中人物后，再点底部按钮确认。</DialogDescription></DialogHeader>{changing&&<><div data-sheet-scroll className="min-h-0 flex-1 space-y-3 overflow-y-auto p-4"><p data-keyboard-hide className="break-all text-xs text-muted-foreground">{fileName(changing.path||changing.vid)}</p><PersonPicker people={people} value={target} onChange={setTarget} disabled={busy} loading={peopleLoading} error={peopleError} onRetry={loadPeople}/></div><div className="mobile-bottom-actions shrink-0 border-t p-3"><Button disabled={busy||!target.trim()} onClick={()=>approve(changing,target.trim())} className="h-auto min-h-11 w-full whitespace-normal">{busy?'正在加入待归档…':`确认是 ${target.trim()||'所选人物'}，加入待归档`}</Button></div></>}</ResponsiveDialogContent></Dialog><VideoPlayer video={playing} onClose={()=>setPlaying(null)}/>
  </div>
}
