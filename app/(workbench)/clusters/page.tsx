'use client'
import { useEffect, useLayoutEffect, useRef, useState } from 'react'
import { Layers, ArrowRight, ArrowLeft, Check, Search } from 'lucide-react'
import { toast } from 'sonner'
import { api, type Cluster } from '@/lib/face-api'
import { Button } from '@/components/ui/button'
import { Dialog, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { ResponsiveDialogContent } from '@/components/review/responsive-dialog'
import { FolderVideoAccordion } from '@/components/review/folder-video-accordion'
import { VideoPlayer } from '@/components/review/video'
import { PersonPicker, type PersonChoice } from '@/components/review/person-picker'
import { ReviewFlow, QueueReceipt } from '@/components/review/workflow'
import { videoGroups, type ReviewVideo } from '@/lib/review-ui'

export default function Clusters() {
  const [clusters,setClusters] = useState<Cluster[]>([]), [people,setPeople] = useState<PersonChoice[]>([])
  const [query,setQuery] = useState(''), [error,setError] = useState(''), [loading,setLoading] = useState(true)
  const [current,setCurrent] = useState<Cluster|null>(null), [videos,setVideos] = useState<ReviewVideo[]>([])
  const [mobileStep,setMobileStep]=useState<'videos'|'person'>('videos')
  const [peopleLoading,setPeopleLoading]=useState(true),[peopleError,setPeopleError]=useState('')
  const bodyRef=useRef<HTMLDivElement>(null),stepScroll=useRef({videos:0,person:0})
  const changeStep=(next:'videos'|'person')=>{if(bodyRef.current)stepScroll.current[mobileStep]=bodyRef.current.scrollTop;if(document.activeElement instanceof HTMLElement)document.activeElement.blur();setMobileStep(next)}
  useLayoutEffect(()=>{if(matchMedia('(max-width:1023px)').matches&&bodyRef.current)bodyRef.current.scrollTop=stepScroll.current[mobileStep]},[mobileStep])
  const peopleSeq=useRef(0)
  const loadPeople=()=>{const token=++peopleSeq.current;setPeopleLoading(true);setPeopleError('');api<{persons:PersonChoice[]}>('/api/persons?picker=1').then(d=>{if(token===peopleSeq.current)setPeople(d.persons)}).catch(e=>{if(token===peopleSeq.current)setPeopleError(e.message)}).finally(()=>{if(token===peopleSeq.current)setPeopleLoading(false)})}
  const [openFolders,setOpenFolders]=useState<string[]>([])
  const [selected,setSelected] = useState<Set<string>>(new Set()), [name,setName] = useState('')
  const [suggestions,setSuggestions] = useState<PersonChoice[]>([]), [detailLoading,setDetailLoading] = useState(false), [detailError,setDetailError] = useState('')
  const [busy,setBusy] = useState(false), [playing,setPlaying] = useState<ReviewVideo|null>(null)
  const [receipt,setReceipt] = useState<{vids:string[];creator:string;cluster:Cluster}|null>(null)
  const seq = useRef(0), initialOpened = useRef(false)
  const clusterSeq=useRef(0)
  const reload = async () => { const token=++clusterSeq.current;const cs = await api<Cluster[]>('/api/clusters'); if(token===clusterSeq.current)setClusters(cs); return cs }
  useEffect(()=>{
    reload().catch(e=>setError(e.message)).finally(()=>setLoading(false))
  },[])
  useEffect(()=>{
    if(initialOpened.current || !clusters.length) return
    initialOpened.current=true
    const cid=new URLSearchParams(window.location.search).get('cid')
    const c=clusters.find(c=>c.cid===cid); if(c) openCluster(c)
  },[clusters])
  const openCluster = async(c:Cluster)=>{
    loadPeople()
    const token=++seq.current;setCurrent(c);setVideos([]);setSelected(new Set());setOpenFolders([]);setMobileStep('videos');stepScroll.current={videos:0,person:0};setName('');setSuggestions([]);setDetailLoading(true);setDetailError('')
    api<{suggestions:PersonChoice[]}>(`/api/cluster/suggest?cid=${encodeURIComponent(c.cid)}`).then(d=>{if(token===seq.current)setSuggestions(d.suggestions||[])}).catch(()=>{})
    try {const d=await api<{vids:ReviewVideo[]}>(`/api/cluster/${encodeURIComponent(c.cid)}`);if(token===seq.current){setVideos(d.vids);setSelected(new Set())}}
    catch(e:any){if(token===seq.current)setDetailError(e.message)}finally{if(token===seq.current)setDetailLoading(false)}
  }
  const toggle=(ids:string[])=>setSelected(prev=>{const next=new Set(prev), all=ids.every(v=>next.has(v));ids.forEach(v=>all?next.delete(v):next.add(v));return next})
  const nextCluster=(cs:Cluster[],id:string)=>{
    const idx=clusters.findIndex(c=>c.cid===id);const next=cs.find(c=>c.cid!==id&&clusters.findIndex(x=>x.cid===c.cid)>idx)||cs.find(c=>c.cid!==id)
    if(next)openCluster(next);else{seq.current++;setCurrent(null)}
  }
  const enqueue=async()=>{
    if(!current||!selected.size||!name.trim()||busy)return
    setBusy(true);const c=current, target=name.trim(), vids=videos.filter(v=>selected.has(v.vid)).map(v=>v.vid)
    try {
      await api('/api/plan/queue',{method:'POST',body:JSON.stringify({creator:target,vids,source:'cluster',source_id:c.cid})})
      setPeople(ps=>[{name:target,rep_thumb:ps.find(p=>p.name===target)?.rep_thumb||null,recent_at:new Date().toISOString()},...ps.filter(p=>p.name!==target)]);loadPeople();setReceipt({vids,creator:target,cluster:c});const remaining=videos.filter(v=>!selected.has(v.vid));setName('')
      setClusters(cs=>remaining.length?cs.map(x=>x.cid===c.cid?{...x,n_videos:remaining.length,total_gb:+remaining.reduce((n,v)=>n+(v.size_gb||0),0).toFixed(1)}:x):cs.filter(x=>x.cid!==c.cid))
      if(remaining.length){setMobileStep('videos');stepScroll.current={videos:0,person:0};setVideos(remaining);setSelected(new Set());setOpenFolders(prev=>prev.filter(dir=>videoGroups(remaining).some(([path])=>path===dir)));setCurrent({...c,n_videos:remaining.length,total_gb:+remaining.reduce((n,v)=>n+(v.size_gb||0),0).toFixed(1)})}
      else nextCluster(clusters.filter(x=>x.cid!==c.cid),c.cid)
      void reload().catch(()=>toast.warning('归名已经保存，列表暂未刷新。无需重复提交，网络恢复后再刷新。'))
      toast.success(`已加入待归档 ${vids.length} 个，文件尚未移动`)
    }catch(e:any){toast.error(e.message)}finally{setBusy(false)}
  }
  const undo=async()=>{
    if(!receipt||busy)return;setBusy(true)
    try{await api('/api/plan/queue',{method:'DELETE',body:JSON.stringify({vids:receipt.vids})});const c=receipt.cluster;setReceipt(null);await openCluster(c);void reload().catch(()=>toast.warning('撤销已经保存，列表暂未刷新。网络恢复后再刷新即可。'));toast.success('已撤销归名，视频已回到原组')}
    catch(e:any){toast.error(e.message)}finally{setBusy(false)}
  }
  const filtered=clusters.filter(c=>!query||c.cid.includes(query)||(c.collections||[]).some((x:any)=>(x.name||x).toLowerCase().includes(query.toLowerCase())))
  return <div className="space-y-5 p-4 md:p-6">
    <ReviewFlow />
    <div><h1 className="text-2xl font-semibold tracking-tight">相似视频组</h1><p className="mt-2 max-w-3xl text-sm leading-relaxed text-muted-foreground">打开一组，结合视频、文件名和路径核对。默认不选视频；先按路径勾选要处理的文件夹，展开核对后再归名。相似组可能包含多个人。creators 内已归类的视频不在此列表中。</p></div>
    {receipt&&!current&&<QueueReceipt count={receipt.vids.length} creator={receipt.creator} onUndo={undo} busy={busy} />}
    <div className="flex flex-wrap items-center gap-3"><label className="flex w-full max-w-md items-center gap-2 rounded-lg border px-3"><Search className="h-4 w-4 text-muted-foreground"/><input value={query} onChange={e=>setQuery(e.target.value)} placeholder="搜索来源文件夹或组编号" className="h-10 min-w-0 flex-1 bg-transparent text-sm outline-none"/></label><span className="text-sm text-muted-foreground">{filtered.length} 组待确认</span></div>
    {loading?<p role="status" className="py-12 text-center text-muted-foreground">加载相似视频组…</p>:error?<p role="alert" className="text-amber-300">{error}</p>:!filtered.length?<p className="rounded-xl border border-dashed p-10 text-center text-muted-foreground">没有待确认的相似组。已确认的视频可到“待归档”查看或撤销。</p>:<div className="grid gap-3 sm:grid-cols-3 lg:grid-cols-4 xl:grid-cols-5">{filtered.map(c=><button key={c.cid} type="button" onClick={()=>openCluster(c)} className="overflow-hidden rounded-xl border bg-card text-left focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring"><div className="relative aspect-video bg-muted">{c.rep_thumb&&<img src={c.rep_thumb} alt="" loading="lazy" className="h-full w-full object-cover"/>}<span className="absolute bottom-2 left-2 rounded bg-black/75 px-2 py-1 text-xs text-white">{c.n_videos} 个视频 · {c.total_gb} GB</span></div><div className="space-y-2 p-3"><p className="text-xs text-muted-foreground">{c.cid}{c.weak_count?` · ${c.weak_count} 个需仔细核对`:''}</p><p className="break-all text-xs leading-relaxed">{(c.collections||[]).map((x:any)=>`${x.name||x}${x.n?`（${x.n}）`:''}`).join(' · ')}</p><span className="inline-flex items-center gap-1 text-xs font-medium">核对这一组<ArrowRight className="h-3 w-3"/></span></div></button>)}</div>}
    <details className="rounded-xl border p-3 text-xs text-muted-foreground"><summary className="cursor-pointer">系统目前如何划分相似组？</summary><p className="mt-2 leading-relaxed">先把人脸按互为前 4 近邻、相似度至少 0.47 连在一起，再按质心相似度至少 0.50 合并。每个视频分给人脸投票最多的组。少于 2 张脸支持也会纳入，只标为证据较少。因此，同组可能混人；系统推荐也需要人工核对。</p></details>
    <Dialog open={!!current} onOpenChange={o=>{if(!o&&!busy){seq.current++;setCurrent(null)}}}><ResponsiveDialogContent open={!!current} onEscapeKeyDown={e=>{if(busy)e.preventDefault()}} onPointerDownOutside={e=>{if(busy)e.preventDefault()}} className="flex max-h-[94dvh] flex-col gap-0 overflow-hidden p-0 sm:max-w-6xl sm:p-0">
      <DialogHeader className="shrink-0 border-b p-4 pr-14 text-left"><DialogTitle className="flex items-center gap-2"><Layers className="h-4 w-4"/>核对这一组 · {current?.cid}</DialogTitle><DialogDescription className="hidden lg:block">{current?.n_videos} 个视频 · {current?.total_gb} GB。勾路径只选择这个文件夹；点路径展开视频。确认后加入待归档，其他文件夹留下继续处理。</DialogDescription><p data-keyboard-hide className="text-xs text-muted-foreground lg:hidden">{current?.n_videos} 个视频 · 已选 {selected.size} 个{mobileStep==='person'?' · 选择归给谁':' · 勾路径，再展开核对'}</p></DialogHeader>
      {receipt&&<div data-keyboard-hide className="shrink-0 border-b p-3"><QueueReceipt count={receipt.vids.length} creator={receipt.creator} onUndo={undo} busy={busy}/></div>}
      <nav data-keyboard-hide aria-label="本波核对步骤" className="grid shrink-0 grid-cols-2 gap-2 border-b bg-background p-2 lg:hidden"><button type="button" aria-current={mobileStep==='videos'?'step':undefined} onClick={()=>changeStep('videos')} className={`min-h-11 rounded-lg text-sm font-medium ${mobileStep==='videos'?'bg-accent':'text-muted-foreground'}`}>1 · 核对视频</button><button type="button" disabled={!selected.size||detailLoading} aria-current={mobileStep==='person'?'step':undefined} onClick={()=>changeStep('person')} className={`min-h-11 rounded-lg text-sm font-medium disabled:opacity-40 ${mobileStep==='person'?'bg-accent':'text-muted-foreground'}`}>2 · 选择人物</button></nav>
      <div ref={bodyRef} data-sheet-scroll className="grid min-h-0 flex-1 gap-5 overflow-y-auto overscroll-contain p-4 lg:grid-cols-[minmax(0,1.6fr)_minmax(290px,1fr)]">
        <div data-review-panel="videos" className={`${mobileStep==='videos'?'':'hidden'} min-w-0 space-y-4 lg:block`}><div className="flex flex-wrap items-center gap-2 text-sm"><span className="font-medium">已选 {selected.size} / {videos.length} 个视频</span><Button size="sm" variant="ghost" disabled={busy||detailLoading} onClick={()=>setSelected(new Set(videos.map(v=>v.vid)))}>全选所有路径</Button><Button size="sm" variant="ghost" disabled={busy||detailLoading} onClick={()=>setSelected(new Set())}>清空选择</Button></div>
          {detailLoading?<p role="status" className="py-8 text-muted-foreground">加载本组视频…</p>:detailError?<div role="alert" className="space-y-2 text-amber-300"><p>{detailError}</p><Button variant="outline" onClick={()=>current&&openCluster(current)}>重新加载</Button></div>:<FolderVideoAccordion videos={videos} selected={selected} onToggle={toggle} onPlay={setPlaying} disabled={busy} value={openFolders} onValueChange={setOpenFolders}/>}
        </div>
        <div data-review-panel="person" className={`${mobileStep==='person'?'':'hidden'} min-w-0 self-start rounded-xl border bg-card p-3 lg:sticky lg:top-0 lg:block`}><PersonPicker key={`${current?.cid}:${videos.length}`} people={people} suggestions={suggestions} value={name} onChange={setName} disabled={busy} loading={peopleLoading} error={peopleError} onRetry={loadPeople}/></div>
      </div>
      <div className="mobile-bottom-actions shrink-0 border-t bg-background p-3 lg:hidden">
        {mobileStep==='videos'?<><p data-keyboard-hide className="mb-2 text-xs text-muted-foreground">{selected.size?`已选 ${selected.size} 个视频，下一步选择人物`:'先勾选路径或视频，再选择人物'}</p><div className="flex gap-2"><Button variant="ghost" disabled={busy} onClick={()=>current&&nextCluster(clusters,current.cid)}>跳过</Button><Button className="min-h-11 flex-1" disabled={busy||detailLoading||!selected.size} onClick={()=>changeStep('person')}>下一步 · 选人物<ArrowRight className="h-4 w-4"/></Button></div></>:<><p data-keyboard-hide className="mb-2 break-all text-xs text-muted-foreground">{name.trim()?`${selected.size} 个视频 → ${name.trim()}`:'选中人物，或输入名字；确认后进入待归档'}</p><div className="flex gap-2"><Button variant="outline" disabled={busy} onClick={()=>changeStep('videos')}><ArrowLeft className="h-4 w-4"/>核对</Button><Button disabled={busy||!selected.size||!name.trim()} onClick={enqueue} className="h-auto min-h-11 flex-1 whitespace-normal"><Check className="h-4 w-4 shrink-0"/>{busy?'正在加入…':`确认 ${selected.size} 个 → 待归档`}</Button></div></>}
      </div>
      <div className="review-actionbar hidden shrink-0 items-center gap-3 border-t bg-background p-4 lg:flex"><div className="min-w-0 flex-1"><p className="break-all text-sm font-medium">{name.trim()?`选中的 ${selected.size} 个视频 → ${name.trim()}`:'先勾选路径、展开核对，再选择人物'}</p><p className="mt-1 text-xs text-muted-foreground">下一步：待归档。现在不移动文件，可以撤销。</p></div><Button variant="outline" disabled={busy} onClick={()=>current&&nextCluster(clusters,current.cid)}>先跳过</Button><Button disabled={busy||detailLoading||!selected.size||!name.trim()} onClick={enqueue} className="h-auto min-h-10 whitespace-normal"><Check className="h-4 w-4 shrink-0"/>{busy?'正在加入待归档…':`将选中的 ${selected.size} 个视频归名${name.trim()?`为 ${name.trim()}`:''}`}</Button></div>
    </ResponsiveDialogContent></Dialog>
    <VideoPlayer video={playing} onClose={()=>setPlaying(null)}/>
  </div>
}
