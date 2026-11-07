'use client'

import Link from 'next/link'
import { useCallback, useEffect, useRef, useState } from 'react'
import { Check, ChevronLeft, ChevronRight, Folder, Play, Search, Users } from 'lucide-react'
import { toast } from 'sonner'
import { api } from '@/lib/face-api'
import type { ProcessingTask } from '@/lib/processing-model'
import { PersonPicker, type PersonChoice } from '@/components/review/person-picker'
import { PersonReference } from '@/components/review/person-reference'
import { ExternalProfiles } from '@/components/persons/external-profiles'
import { PathText } from '@/components/review/video'
import { QueueReceipt } from '@/components/review/workflow'
import { ResponsiveDialogContent } from '@/components/review/responsive-dialog'
import { Dialog, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'

type Group={key:string;label:string;kind:string;count:number}
type Data={counts:Record<string,number>;groups:Group[];group:Group|null;total:number;rows:ProcessingTask[];page:number;limit:number;waiting:number;online:boolean;folders:string[];focus_missing:boolean}
const thumb=(t:ProcessingTask)=>t.data.face?.thumbs[0] ? '/thumb?p='+encodeURIComponent(t.data.face.thumbs[0]) : null
const sourceName:Record<string,string>={watermark:'水印',folder:'文件名 / 文件夹',face:'人脸相似度'}

function Evidence({task,onNote,onSource,busy}:{task:ProcessingTask;onNote:(vid:string,note:string)=>void;onSource:(key:string)=>void;busy:boolean}) {
  const [note,setNote]=useState(task.note)
  useEffect(()=>setNote(task.note),[task.vid,task.note])
  return <div className="space-y-4 p-4">
    <p className="break-all text-sm font-medium">{task.name}</p>
    <video key={task.vid} src={`/api/video?p=${encodeURIComponent(task.path)}`} controls playsInline preload="metadata" className="max-h-[45dvh] w-full rounded-lg bg-black"/>
    {!!task.manual&&<p className="rounded-[8px] border border-[var(--wb-warning-border)] bg-[var(--wb-warning-soft)] p-2 text-xs text-[var(--wb-warning)]">已人工接管，部分分析未完成。请自行核对后确认人物。</p>}
    <section className="space-y-2"><h3 className="text-xs font-medium text-muted-foreground">人物候选</h3>
      {task.data.candidates?.length ? task.data.candidates.map(c=><div key={c.name} className="rounded-[8px] border bg-background p-3"><div className="flex flex-wrap items-center justify-between gap-2"><span className="break-all text-sm font-semibold">{c.name}</span><span className="text-xs text-muted-foreground">{c.sources.map(s=>sourceName[s]||s).join(' · ')}</span></div>
        <div className="mt-2 space-y-1">{c.evidence.map((e,i)=><p key={i} className="break-all text-xs text-muted-foreground">{e.source==='face'?`人脸相似度 ${e.score?.toFixed(3)} · ${e.support} 张脸支持`:e.detail}</p>)}</div><PersonReference name={c.name} compact/></div>) : <p className="text-sm text-muted-foreground">暂无人物候选，仍可手动选择或新建人物。</p>}
    </section>
    <section className="space-y-2"><h3 className="text-xs font-medium text-muted-foreground">水印证据</h3>{task.data.watermark?.candidates.length ? task.data.watermark.candidates.map(c=><div key={c.key} className="space-y-2 rounded-[8px] border p-2"><img loading="lazy" src={`/api/watermarks/evidence?vid=${encodeURIComponent(task.vid)}&key=${c.key}`} alt={`水印：${c.text}`} className="max-h-28 max-w-full rounded object-contain"/><p className="break-all text-xs">{c.text}</p><p className="text-xs text-muted-foreground">{c.frames} 帧出现 · {c.timestamp.toFixed(1)} 秒处{c.platform==='Telegram'?' · 频道来源线索':''}</p>{c.kind==='account'&&<ExternalProfiles token={c.token} platform={c.platform}/>}<Button variant="ghost" size="sm" disabled={busy} onClick={()=>onSource(c.key)}>仅作来源，不用于人物建议</Button></div>) : <p className="text-xs text-muted-foreground">本轮未检出重复文字，不代表视频完全没有水印。</p>}</section>
    {!!task.data.face?.thumbs.length&&<section className="space-y-2"><h3 className="text-xs font-medium text-muted-foreground">人脸采样</h3><div className="flex flex-wrap gap-2">{task.data.face.thumbs.slice(0,6).map(p=><img key={p} src={'/thumb?p='+encodeURIComponent(p)} alt="人脸采样证据" loading="lazy" className="size-12 rounded-[8px] object-cover"/>)}</div><p className="text-xs text-muted-foreground">检出 {task.data.face.face_count} 张脸；相似度仅供核对。</p></section>}
    {task.data.face_skipped&&<p className="text-xs text-muted-foreground">已确认账号命中且无冲突，本次跳过人脸分析。</p>}
    {task.data.face?.face_count===0&&<p className="text-xs text-muted-foreground">本轮人脸采样未检出可用人脸。</p>}
    {task.data.face?.unknown_group&&<p className="text-xs text-muted-foreground">相似人脸可组成未命名组，组内视频仍需分别核对，不能据此认定是同一个人。</p>}
    <PathText value={task.path}/>
    <label className="block space-y-2 text-xs text-muted-foreground"><span>备注 / 其他出镜人物</span><textarea aria-label="视频备注" value={note} onChange={e=>setNote(e.target.value)} maxLength={1000} rows={3} disabled={busy} className="w-full rounded-[8px] border bg-background p-2 text-sm text-foreground max-md:text-base"/></label>
    <Button size="sm" variant="outline" disabled={busy||note===task.note} onClick={()=>onNote(task.vid,note)}>保存备注</Button>
  </div>
}

export default function Review() {
  const [data,setData]=useState<Data|null>(null),[error,setError]=useState(''),[loading,setLoading]=useState(true),[busy,setBusy]=useState(false)
  const [view,setView]=useState('all'),[group,setGroup]=useState(''),[query,setQuery]=useState(''),[search,setSearch]=useState(''),[folder,setFolder]=useState(''),[page,setPage]=useState(0)
  const [personScope,setPersonScope]=useState(''),[clusterScope,setClusterScope]=useState('')
  const [focus,setFocus]=useState(''),[linkFocus,setLinkFocus]=useState(''),[selected,setSelected]=useState<Set<string>>(new Set())
  const [previewOpen,setPreviewOpen]=useState(false),[confirmOpen,setConfirmOpen]=useState(false),[creator,setCreator]=useState(''),[watermarkKey,setWatermarkKey]=useState('')
  const [people,setPeople]=useState<PersonChoice[]>([]),[peopleError,setPeopleError]=useState(''),[receipt,setReceipt]=useState<{vids:string[];creator:string}|null>(null)
  const sequence=useRef(0)
  useEffect(()=>{const params=new URLSearchParams(window.location.search);setLinkFocus(params.get('vid')||'');setPersonScope(params.get('person')||'');setClusterScope(params.get('cluster')||'')},[])
  useEffect(()=>{const timer=setTimeout(()=>{setSearch(query);setPage(0);setSelected(new Set())},250);return()=>clearTimeout(timer)},[query])
  const load=useCallback(async()=>{
    const token=++sequence.current;setLoading(true)
    try{const params=new URLSearchParams({view,group,q:search,folder,page:String(page),vid:linkFocus,person:personScope,cluster:clusterScope});const next=await api<Data>('/api/review?'+params);if(token===sequence.current){setData(next);setError('');setSelected(prev=>new Set([...prev].filter(id=>next.rows.some(t=>t.vid===id))))};return next}
    catch(e:any){if(token===sequence.current)setError(e.message);return null}finally{if(token===sequence.current)setLoading(false)}
  },[view,group,search,folder,page,linkFocus,personScope,clusterScope])
  const loadPeople=useCallback(async()=>{try{setPeople((await api<{persons:PersonChoice[]}>('/api/persons?picker=1')).persons);setPeopleError('')}catch(e:any){setPeopleError(e.message)}},[])
  useEffect(()=>{load()},[load])
  useEffect(()=>{loadPeople()},[loadPeople])
  useEffect(()=>{
    if(busy||confirmOpen||previewOpen||data?.rows.length)return
    const timer=setTimeout(load,10000)
    return()=>clearTimeout(timer)
  },[data,busy,confirmOpen,previewOpen,load])
  const rows=data?.rows||[],chosen=rows.filter(t=>selected.has(t.vid)),focused=rows.find(t=>t.vid===(focus||linkFocus))||rows[0]
  const commonMarks=chosen.length ? (chosen[0].data.watermark?.candidates||[]).filter(c=>c.kind==='account'&&c.platform!=='Telegram'&&chosen.every(t=>t.data.watermark?.candidates.some(other=>other.key===c.key))) : []
  const disabled=busy||loading||!!error
  const reset=()=>{setPage(0);setFocus('');setLinkFocus('');setSelected(new Set())}
  const chooseGroup=(key:string)=>{setGroup(key);reset()}
  const toggle=(ids:string[])=>setSelected(previous=>{const next=new Set(previous),all=ids.every(id=>next.has(id));ids.forEach(id=>all?next.delete(id):next.add(id));return next})
  const inspect=(task:ProcessingTask)=>{setFocus(task.vid);if(!matchMedia('(min-width:1280px)').matches)setPreviewOpen(true)}
  const action=async(action:string,vids:string[],note?:string)=>{
    setBusy(true)
    try{await api('/api/review',{method:'POST',body:JSON.stringify({action,vids,note})});if(action!=='note'){setSelected(new Set());setFocus('');setPreviewOpen(false)}await load();toast.success({note:'备注已保存',defer:'已暂时跳过，可在“暂缓”中继续',restore:'已恢复到待确认'}[action]||'已更新')}
    catch(e:any){toast.error(e.message)}finally{setBusy(false)}
  }
  const sourceOnly=async(key:string)=>{
    setBusy(true)
    try{await api('/api/watermarks',{method:'PUT',body:JSON.stringify({key,kind:'source'})});setPreviewOpen(false);setSelected(new Set());setFocus('');await load();toast.success('已标为来源，将重新整理相关人物建议')}
    catch(e:any){toast.error(e.message)}finally{setBusy(false)}
  }
  const confirm=async()=>{
    if(!chosen.length||!creator.trim())return
    setBusy(true)
    const vids=chosen.map(t=>t.vid),name=creator.trim()
    try{const result=await api('/api/plan/queue',{method:'POST',body:JSON.stringify({vids,creator:name,source:'review',source_id:data?.group?.key||'',watermark_key:watermarkKey||undefined,
      fingerprints:Object.fromEntries(chosen.map(t=>[t.vid,{path:t.path,size:t.size,mtime_ns:t.mtime_ns}]))})});setReceipt({vids,creator:name});setConfirmOpen(false);setSelected(new Set());setFocus('');setLinkFocus('');await load();await loadPeople();result.rule_warning?toast.warning(result.rule_warning):toast.success('已确认人物并加入待归档')}
    catch(e:any){toast.error(e.message)}finally{setBusy(false)}
  }
  const undo=async()=>{if(!receipt)return;setBusy(true);try{await api('/api/plan/queue',{method:'DELETE',body:JSON.stringify({vids:receipt.vids})});setReceipt(null);await load();toast.success('已撤销本次归名')}catch(e:any){toast.error(e.message)}finally{setBusy(false)}}
  const openConfirm=()=>{setCreator(data?.group?.kind==='suggested'?data.group.label:'');setWatermarkKey('');setConfirmOpen(true)}
  return <main className="wb-page space-y-4">
    <header className="wb-header"><h1 className="wb-title">人工确认</h1><div className="flex flex-wrap items-center gap-2"><Button variant="outline" size="sm" disabled={busy} onClick={load}>刷新</Button><Button variant="outline" size="sm" asChild><Link href="/operations">查看分析进度{data?.waiting?` · ${data.waiting}`:''}</Link></Button></div></header>
    {receipt&&<QueueReceipt count={receipt.vids.length} creator={receipt.creator} onUndo={undo} busy={busy}/>}
    {error&&<p role="alert" className="rounded-[8px] border border-[var(--wb-danger)]/30 bg-[var(--wb-danger-soft)] p-3 text-sm text-[var(--wb-danger)]">{error}</p>}
    {data?.focus_missing&&<p className="text-sm text-[var(--wb-warning)]">链接中的视频尚未就绪或已归名。可到处理操作台查看状态。</p>}
    {(personScope||clusterScope)&&<div className="flex items-center gap-2 text-sm text-muted-foreground"><span>当前范围：{personScope||`原相似组 ${clusterScope}`}</span><Button size="sm" variant="ghost" onClick={()=>{setPersonScope('');setClusterScope('');setGroup('');reset()}}>查看全部</Button></div>}
    <div className="flex flex-wrap gap-1">{[{key:'all',label:'全部待确认'},{key:'suggested',label:'有候选'},{key:'conflict',label:'证据冲突'},{key:'none',label:'无候选'},{key:'manual',label:'人工接管'},{key:'deferred',label:'暂缓'}].map(tab=><Button key={tab.key} size="sm" variant={view===tab.key?'secondary':'ghost'} aria-pressed={view===tab.key} disabled={busy} onClick={()=>{setView(tab.key);setGroup('');reset()}}>{tab.label} {data?.counts[tab.key]??0}</Button>)}</div>
    <div className="flex flex-wrap gap-2"><div className="relative min-w-0 flex-1"><Search className="pointer-events-none absolute left-3 top-1/2 size-4 -translate-y-1/2 text-muted-foreground"/><Input className="pl-9" aria-label="搜索待确认视频" placeholder="搜索人物、文件名或路径" value={query} onChange={e=>setQuery(e.target.value)} disabled={busy}/></div>
      <label className="flex h-8 min-w-0 max-w-full items-center gap-2 rounded-[8px] border bg-background px-2.5 text-xs text-muted-foreground max-md:min-h-11"><Folder className="size-4 shrink-0"/><select aria-label="按原文件夹筛选" value={folder} disabled={busy} onChange={e=>{setFolder(e.target.value);setGroup('');reset()}} className="h-full max-w-72 min-w-0 bg-transparent text-xs text-foreground outline-none max-md:min-h-11 max-md:text-base"><option value="">所有原文件夹</option>{data?.folders.map(f=><option key={f} value={f}>{f}</option>)}</select></label>
    </div>
    <div className="grid min-w-0 gap-4 lg:grid-cols-[180px_minmax(0,1fr)] xl:grid-cols-[200px_minmax(0,1fr)_340px]">
      <aside className="min-w-0 rounded-[12px] border bg-card p-2"><h2 className="flex items-center gap-2 p-2 text-xs font-medium text-muted-foreground"><Users className="size-4"/>候选人物组 · {data?.groups.length||0}</h2>
        <select aria-label="选择确认组" className="h-11 w-full min-w-0 rounded-[8px] border bg-background px-2 text-base lg:hidden" value={data?.group?.key||''} disabled={busy} onChange={e=>chooseGroup(e.target.value)}>{!data?.groups.length&&<option value="">暂无待确认组</option>}{data?.groups.map(g=><option key={g.key} value={g.key}>{g.label} · {g.count}</option>)}</select>
        <div className="hidden max-h-[calc(100dvh-280px)] space-y-1 overflow-y-auto lg:block">{data?.groups.map(g=><button key={g.key} type="button" disabled={busy} aria-pressed={data.group?.key===g.key} onClick={()=>chooseGroup(g.key)} className={`flex min-h-9 w-full items-center gap-2 rounded-[8px] px-2.5 text-left text-xs ${data.group?.key===g.key?'bg-accent font-medium':'hover:bg-accent/50'}`}><span className="min-w-0 flex-1 break-all">{g.label}</span><span className="text-xs tabular-nums text-muted-foreground">{g.count}</span></button>)}</div>
      </aside>
      <section className="min-w-0 space-y-3 rounded-[12px] border bg-card p-3">
        <div className="flex flex-wrap items-center justify-between gap-2"><div className="flex items-baseline gap-2.5"><h2 className="break-all text-[15px] font-semibold">{data?.group?.label||'等待人工确认'}</h2><span className="text-xs text-muted-foreground">{data?.total||0} 个 · 本页 {rows.length} 个</span></div><Button variant="outline" size="sm" disabled={disabled||!rows.length} onClick={()=>toggle(rows.map(t=>t.vid))}>{chosen.length===rows.length&&rows.length?'取消本页全选':'全选当前页'}</Button></div>
        {loading&&!data?<p role="status" className="p-8 text-sm text-muted-foreground">正在加载待确认视频…</p>:!rows.length?<div className="space-y-3 rounded-[8px] border border-dashed p-8 text-center text-sm text-muted-foreground"><p>{data?.waiting?'后台还在分析，视频就绪后会陆续出现在这里。':'当前筛选下没有待确认视频。'}</p><Link href="/operations" className="underline">查看处理操作台</Link></div>:<div className="space-y-2 lg:max-h-[calc(100dvh-360px)] lg:overflow-y-auto">{rows.map(t=><article key={t.vid} data-vid={t.vid} className={`flex min-w-0 items-start gap-2 rounded-[8px] border p-2 ${focused?.vid===t.vid?'border-foreground/30 bg-accent/30':''}`}>
          <label className="flex size-11 shrink-0 cursor-pointer items-center justify-center"><input aria-label={`选择 ${t.name}`} type="checkbox" checked={selected.has(t.vid)} disabled={disabled} onChange={()=>toggle([t.vid])} className="size-4 accent-[var(--primary)]"/></label>
          <button type="button" className="flex min-w-0 flex-1 items-start gap-3 text-left" onClick={()=>inspect(t)} aria-label={`核对 ${t.name}`}><span className="relative flex aspect-video w-20 shrink-0 items-center justify-center overflow-hidden rounded-[6px] bg-muted">{thumb(t)?<img alt="" loading="lazy" src={thumb(t)!} className="size-full object-cover"/>:<Play className="size-5 text-muted-foreground"/>}</span><span className="min-w-0 flex-1"><span className="line-clamp-2 break-all text-sm font-medium">{t.name}</span><span className="mt-1 block text-xs text-muted-foreground">{(t.size/2**30).toFixed(2)} GB · {(t.data.candidates||[]).map(c=>c.sources.map(s=>sourceName[s]).join(' / ')).join(' · ')||'需要人工判断'}</span>{t.note&&<span className="mt-1 line-clamp-1 break-all text-xs text-muted-foreground">备注：{t.note}</span>}</span></button>
        </article>)}</div>}
        {data&&data.total>data.limit&&<div className="flex items-center justify-center gap-3 text-xs"><Button variant="ghost" size="sm" disabled={disabled||data.page===0} onClick={()=>{setPage(data.page-1);setLinkFocus('');setSelected(new Set());setFocus('')}}><ChevronLeft className="size-4"/>上一页</Button><span>{data.page+1}/{Math.ceil(data.total/data.limit)}</span><Button variant="ghost" size="sm" disabled={disabled||(data.page+1)*data.limit>=data.total} onClick={()=>{setPage(data.page+1);setLinkFocus('');setSelected(new Set());setFocus('')}}>下一页<ChevronRight className="size-4"/></Button></div>}
        <div className="space-y-2 border-t pt-3"><p className="text-xs text-muted-foreground">已选 {chosen.length} 个，只操作当前页的勾选项。</p><div className="flex flex-wrap gap-2"><Button disabled={disabled||!chosen.length} onClick={openConfirm} className="flex-1"><Check className="size-4"/>确认所选人物</Button><Button variant="outline" disabled={disabled||!chosen.length} onClick={()=>action(view==='deferred'?'restore':'defer',chosen.map(t=>t.vid))}>{view==='deferred'?'恢复核对':'暂时跳过'}</Button></div></div>
      </section>
      <aside className="hidden min-w-0 overflow-hidden rounded-[12px] border bg-card xl:block"><div className="border-b px-4 py-3 text-xs font-medium text-muted-foreground">视频与证据</div><div className="max-h-[calc(100dvh-260px)] overflow-y-auto">{focused?<Evidence task={focused} onNote={(vid,note)=>action('note',[vid],note)} onSource={sourceOnly} busy={busy}/>:<p className="p-8 text-center text-sm text-muted-foreground">选择一个视频查看证据</p>}</div></aside>
    </div>
    <Dialog open={previewOpen} onOpenChange={setPreviewOpen}><ResponsiveDialogContent open={previewOpen} className="flex max-h-[92dvh] flex-col gap-0 overflow-hidden p-0 sm:max-w-2xl sm:p-0"><DialogHeader className="border-b p-4 pr-14 text-left"><DialogTitle>视频与证据</DialogTitle><DialogDescription>关闭后保留勾选和列表位置。</DialogDescription></DialogHeader><div data-sheet-scroll className="min-h-0 flex-1 overflow-y-auto">{focused&&<Evidence task={focused} onNote={(vid,note)=>action('note',[vid],note)} onSource={sourceOnly} busy={busy}/>}</div><div className="mobile-bottom-actions border-t p-3"><Button variant="outline" className="w-full" onClick={()=>setPreviewOpen(false)}>返回视频列表</Button></div></ResponsiveDialogContent></Dialog>
    <Dialog open={confirmOpen} onOpenChange={open=>!busy&&setConfirmOpen(open)}><ResponsiveDialogContent open={confirmOpen} showCloseButton={!busy} onEscapeKeyDown={e=>busy&&e.preventDefault()} onInteractOutside={e=>busy&&e.preventDefault()} className="flex max-h-[90dvh] flex-col gap-0 overflow-hidden p-0 sm:max-w-2xl sm:p-0"><DialogHeader className="border-b p-4 pr-14 text-left"><DialogTitle>确认这 {chosen.length} 个视频的主归属人物</DialogTitle><DialogDescription>人物确认后加入待归档。其他出镜人物可以写在各视频备注中。</DialogDescription></DialogHeader><div data-sheet-scroll className="min-h-0 flex-1 space-y-4 overflow-y-auto p-4"><PersonPicker people={people} value={creator} onChange={setCreator} disabled={busy} error={peopleError} onRetry={loadPeople}/>{commonMarks.length>0&&<label className="block space-y-2 text-xs text-muted-foreground"><span>可选：记住共同水印的对应人物</span><select aria-label="保存共同账号规则" disabled={busy} value={watermarkKey} onChange={e=>setWatermarkKey(e.target.value)} className="h-8 w-full rounded-[8px] border bg-background px-2 text-xs text-foreground max-md:min-h-11 max-md:text-base"><option value="">本次只归名，不保存账号规则</option>{commonMarks.map(c=><option key={c.key} value={c.key}>{c.platform} · {c.token}</option>)}</select></label>}</div><div className="mobile-bottom-actions border-t p-3"><Button className="w-full" disabled={busy||!creator.trim()||!chosen.length} onClick={confirm}>{busy?'正在保存…':`归给 ${creator.trim()||'所选人物'}，加入待归档`}</Button></div></ResponsiveDialogContent></Dialog>
  </main>
}
