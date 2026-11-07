"use client"

import { PrivateImage, PrivateVideo } from '@/components/private-media'
import Link from 'next/link'
import { useCallback, useEffect, useRef, useState } from 'react'
import { Check, ChevronLeft, ChevronRight, Play, RefreshCw, SlidersHorizontal, X, ArrowLeft, ListChecks } from 'lucide-react'
import * as Popover from '@radix-ui/react-popover'
import { toast } from 'sonner'
import { api } from '@/lib/face-api'
import type { ProcessingTask } from '@/lib/processing-model'
import { reviewSelectionKey, toggleReviewSelection, removeReviewSelection, type ReviewSelections } from '@/lib/review-selection'
import { PersonPicker, type PersonChoice } from '@/components/review/person-picker'
import { PersonReference } from '@/components/review/person-reference'
import { ExternalProfiles } from '@/components/persons/external-profiles'
import { PathText } from '@/components/review/video'
import { QueueReceipt } from '@/components/review/workflow'
import { ResponsiveDialogContent } from '@/components/review/responsive-dialog'
import { Dialog, DialogTitle, DialogDescription } from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { InputGroupButton } from '@/components/ui/input-group'
import { Checkbox } from '@/components/beui/motion/checkbox'
import { WorkspaceSearch, WorkspaceTabs, useWideWorkspace } from '@/components/workbench/controls'

type Group={key:string;label:string;kind:string;count:number}
type Data={counts:Record<string,number>;groups:Group[];group:Group|null;total:number;rows:ProcessingTask[];page:number;limit:number;waiting:number;online:boolean;folders:string[];focus_missing:boolean}
const thumb=(t:ProcessingTask)=>t.data.face?.thumbs[0] ? '/thumb?p='+encodeURIComponent(t.data.face.thumbs[0]) : null
const sourceName:Record<string,string>={watermark:'水印',folder:'文件名 / 文件夹',face:'人脸相似度'}
const views=[{value:'all',label:'全部'},{value:'suggested',label:'有候选'},{value:'conflict',label:'证据冲突'},{value:'none',label:'无候选'},{value:'manual',label:'人工接管'},{value:'deferred',label:'暂缓'}]

function Evidence({task,onNote,onSource,busy}:{task:ProcessingTask;onNote:(vid:string,note:string)=>void;onSource:(key:string)=>void;busy:boolean}) {
  const [note,setNote]=useState(task.note)
  useEffect(()=>setNote(task.note),[task.vid,task.note])
  return <div className="space-y-4 p-4">
    <p className="break-all text-sm font-medium">{task.name}</p>
    <PrivateVideo key={task.vid} src={`/api/video?p=${encodeURIComponent(task.path)}`} controls playsInline preload="metadata" className="max-h-[45dvh] w-full rounded-lg bg-black"/>
    {!!task.manual&&<p className="rounded-[8px] border border-[var(--wb-warning-border)] bg-[var(--wb-warning-soft)] p-2 text-xs text-[var(--wb-warning)]">已人工接管，部分分析未完成。请自行核对后确认人物。</p>}
    <section className="space-y-2"><h3 className="text-xs font-medium text-muted-foreground">人物候选</h3>
      {task.data.candidates?.length ? task.data.candidates.map(c=><div key={c.name} className="rounded-[8px] border bg-background p-3"><div className="flex flex-wrap items-center justify-between gap-2"><span className="break-all text-sm font-semibold">{c.name}</span><span className="text-xs text-muted-foreground">{c.sources.map(s=>sourceName[s]||s).join(' · ')}</span></div>
        <div className="mt-2 space-y-1">{c.evidence.map((e,i)=><p key={i} className="break-all text-xs text-muted-foreground">{e.source==='face'?`人脸相似度 ${e.score?.toFixed(3)} · ${e.support} 张脸支持`:e.detail}</p>)}</div><PersonReference name={c.name} compact/></div>) : <p className="text-sm text-muted-foreground">暂无人物候选，仍可手动选择或新建人物。</p>}
    </section>
    <section className="space-y-2"><h3 className="text-xs font-medium text-muted-foreground">水印证据</h3>{task.data.watermark?.candidates.length ? task.data.watermark.candidates.map(c=><div key={c.key} className="space-y-2 rounded-[8px] border p-2"><PrivateImage loading="lazy" src={`/api/watermarks/evidence?vid=${encodeURIComponent(task.vid)}&key=${c.key}`} alt={`水印：${c.text}`} className="max-h-28 max-w-full rounded object-contain"/><p className="break-all text-xs">{c.text}</p><p className="text-xs text-muted-foreground">{c.frames} 帧出现 · {c.timestamp.toFixed(1)} 秒处{c.platform==='Telegram'?' · 频道来源线索':''}</p>{c.kind==='account'&&<ExternalProfiles token={c.token} platform={c.platform}/>}<Button variant="ghost" size="sm" disabled={busy} onClick={()=>onSource(c.key)}>仅作来源，不用于人物建议</Button></div>) : <p className="text-xs text-muted-foreground">本轮未检出重复文字，不代表视频完全没有水印。</p>}</section>
    {!!task.data.face?.thumbs.length&&<section className="space-y-2"><h3 className="text-xs font-medium text-muted-foreground">人脸采样</h3><div className="flex flex-wrap gap-2">{task.data.face.thumbs.slice(0,6).map(p=><PrivateImage compact key={`${task.vid}-${p}`} src={'/thumb?p='+encodeURIComponent(p)} alt="人脸采样证据" loading="lazy" className="size-12 rounded-[8px] object-cover"/>)}</div><p className="text-xs text-muted-foreground">检出 {task.data.face.face_count} 张脸；相似度仅供核对。</p></section>}
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
  const [personScope,setPersonScope]=useState(''),[clusterScope,setClusterScope]=useState(''),[linkFocus,setLinkFocus]=useState(''),[initialized,setInitialized]=useState(false)
  const [selections,setSelections]=useState<ReviewSelections>({}),[inspected,setInspected]=useState<Record<string,ProcessingTask>>({})
  const [instantEntry,setInstantEntry]=useState(false)
  const [previewOpen,setPreviewOpen]=useState(false),[detailMode,setDetailMode]=useState<'video'|'selection'>('video')
  const [creator,setCreator]=useState(''),[watermarkKey,setWatermarkKey]=useState(''),[submitError,setSubmitError]=useState('')
  const [people,setPeople]=useState<PersonChoice[]>([]),[peopleError,setPeopleError]=useState(''),[receipt,setReceipt]=useState<{vids:string[];creator:string}|null>(null)
  const returnFocus=useRef<HTMLElement|null>(null)
  const wide=useWideWorkspace(),sequence=useRef(0),listRef=useRef<HTMLDivElement>(null),scrollPositions=useRef<Record<string,number>>({}),selectionHeading=useRef<HTMLHeadingElement>(null)
  const activeGroup=data?.group?.key||group,scope=reviewSelectionKey(activeGroup,view==='deferred'),chosen=selections[scope]||[],rows=data?.rows||[]
  const selected=new Set(chosen.map(t=>t.vid)),focused=inspected[scope]||rows[0],hiddenCount=chosen.filter(t=>!rows.some(r=>r.vid===t.vid)).length
  const disabled=busy||loading||!!error
  const currentPage=data?.page??page
  const listKey=JSON.stringify([scope,search,folder,currentPage])
  useEffect(()=>{const params=new URLSearchParams(window.location.search);setLinkFocus(params.get('vid')||'');setPersonScope(params.get('person')||'');setClusterScope(params.get('cluster')||'');setView(views.some(v=>v.value===params.get('view'))?params.get('view')!:'all');setGroup(params.get('group')||'');setQuery(params.get('q')||'');setFolder(params.get('folder')||'');setPage(Number(params.get('page'))||0);setInitialized(true)},[])
  useEffect(()=>{const timer=setTimeout(()=>{setSearch(query)},250);return()=>clearTimeout(timer)},[query])
  const load=useCallback(async()=>{
    if(!initialized)return null
    const token=++sequence.current;setLoading(true)
    try{const params=new URLSearchParams({view,group,q:search,folder,page:String(page),vid:linkFocus,person:personScope,cluster:clusterScope});const next=await api<Data>('/api/review?'+params);if(token===sequence.current){setData(next);setError('')};return next}
    catch(e:any){if(token===sequence.current)setError(e.message);return null}finally{if(token===sequence.current)setLoading(false)}
  },[initialized,view,group,search,folder,page,linkFocus,personScope,clusterScope])
  const loadPeople=useCallback(async()=>{try{setPeople((await api<{persons:PersonChoice[]}>('/api/persons?picker=1')).persons);setPeopleError('')}catch(e:any){setPeopleError(e.message)}},[])
  useEffect(()=>{void load();return()=>{sequence.current++}},[load])
  useEffect(()=>{void loadPeople()},[loadPeople])
  useEffect(()=>{if(!initialized||loading||!data)return;const url=new URL(window.location.href);for(const [key,value] of Object.entries({view,group:data.group?.key||group,q:search,folder,page:String(data.page),person:personScope,cluster:clusterScope,vid:linkFocus})){if(value&&!(key==='view'&&value==='all')&&!(key==='page'&&value==='0'))url.searchParams.set(key,value);else url.searchParams.delete(key)}window.history.replaceState(null,'',url.pathname+url.search)},[initialized,loading,data,view,group,search,folder,personScope,clusterScope,linkFocus])
  useEffect(()=>{if(listRef.current)listRef.current.scrollTop=scrollPositions.current[listKey]||0},[listKey])
  useEffect(()=>{setDetailMode('video');setCreator('');setWatermarkKey('');setSubmitError('')},[scope])
  useEffect(()=>{if(wide)setPreviewOpen(false)},[wide])
  useEffect(()=>{if(busy||previewOpen||data?.rows.length||!initialized)return;const timer=setTimeout(load,10000);return()=>clearTimeout(timer)},[data,busy,previewOpen,initialized,load])
  const commonMarks=chosen.length ? (chosen[0].data.watermark?.candidates||[]).filter(c=>c.kind==='account'&&c.platform!=='Telegram'&&chosen.every(t=>t.data.watermark?.candidates.some(other=>other.key===c.key))) : []
  const changeQuery=(value:string)=>{if(!group&&data?.group)setGroup(data.group.key);setQuery(value);setPage(0);setLinkFocus('')}
  const chooseGroup=(key:string)=>{setGroup(key);setPage(0);setLinkFocus('');setPreviewOpen(false)}
  const toggle=(tasks:ProcessingTask[])=>setSelections(current=>({...current,[scope]:toggleReviewSelection(current[scope]||[],tasks)}))
  const clear=()=>setSelections(current=>({...current,[scope]:[]}))
  const inspect=(task:ProcessingTask)=>{if(!previewOpen){returnFocus.current=document.activeElement as HTMLElement;setInstantEntry(!!document.activeElement?.matches(':focus-visible')||matchMedia('(prefers-reduced-motion: reduce)').matches)}setInspected(current=>({...current,[scope]:task}));setDetailMode('video');if(!wide)setPreviewOpen(true)}
  const openSelection=()=>{if(!previewOpen){returnFocus.current=document.activeElement as HTMLElement;setInstantEntry(!!document.activeElement?.matches(':focus-visible')||matchMedia('(prefers-reduced-motion: reduce)').matches)}setCreator(current=>current||(data?.group?.kind==='suggested'?data.group.label:''));setSubmitError('');setDetailMode('selection');if(!wide)setPreviewOpen(true);else requestAnimationFrame(()=>selectionHeading.current?.focus())}
  const removeCompleted=(vids:string[])=>{setSelections(current=>removeReviewSelection(current,vids));setInspected(current=>Object.fromEntries(Object.entries(current).filter(([,task])=>!vids.includes(task.vid))))}
  const action=async(action:string,vids:string[],note?:string)=>{
    setBusy(true);setSubmitError('')
    try{await api('/api/review',{method:'POST',body:JSON.stringify({action,vids,note})});if(action!=='note'){removeCompleted(vids);setPreviewOpen(false);setDetailMode('video');setLinkFocus('')}else setInspected(current=>Object.fromEntries(Object.entries(current).map(([key,t])=>[key,vids.includes(t.vid)?{...t,note:note||''}:t])));await load();toast.success({note:'备注已保存',defer:'已移至暂缓',restore:'已恢复核对'}[action]||'已更新')}
    catch(e:any){setSubmitError(e.message);toast.error(e.message)}finally{setBusy(false)}
  }
  const sourceOnly=async(key:string)=>{
    setBusy(true)
    try{await api('/api/watermarks',{method:'PUT',body:JSON.stringify({key,kind:'source'})});setPreviewOpen(false);setSelections({});setInspected({});setDetailMode('video');await load();toast.success('已标为来源，相关人物建议将重新分析')}
    catch(e:any){toast.error(e.message)}finally{setBusy(false)}
  }
  const confirm=async()=>{
    if(!chosen.length||!creator.trim()||busy)return
    setBusy(true);setSubmitError('')
    const vids=chosen.map(t=>t.vid),name=creator.trim()
    try{const result=await api<{rule_warning?:string}>('/api/plan/queue',{method:'POST',body:JSON.stringify({vids,creator:name,source:'review',source_id:activeGroup,review_group:{key:activeGroup,deferred:view==='deferred'},watermark_key:watermarkKey||undefined,fingerprints:Object.fromEntries(chosen.map(t=>[t.vid,{path:t.path,size:t.size,mtime_ns:t.mtime_ns}]))})});setReceipt({vids,creator:name});removeCompleted(vids);setPreviewOpen(false);setDetailMode('video');setLinkFocus('');await load();await loadPeople();result.rule_warning?toast.warning(result.rule_warning):toast.success('已加入待归档')}
    catch(e:any){setSubmitError(e.message);toast.error(e.message)}finally{setBusy(false)}
  }
  const undo=async()=>{if(!receipt)return;setBusy(true);try{const result=await api<{warning?:string}>('/api/plan/queue',{method:'DELETE',body:JSON.stringify({vids:receipt.vids})});setReceipt(null);await load();result.warning?toast.warning(result.warning):toast.success('已撤销本次归名')}catch(e:any){toast.error(e.message)}finally{setBusy(false)}}
  const detail=<>
    <div className="ws-detail-heading"><h2 ref={selectionHeading} tabIndex={-1}>{detailMode==='selection'?`核对所选 · ${chosen.length}`:'视频与证据'}</h2>{detailMode==='selection'&&<Button variant="ghost" size="sm" onClick={()=>setDetailMode('video')}><ArrowLeft size={14}/>返回证据</Button>}{wide&&detailMode==='video'&&focused&&<span className="ws-muted">{rows.findIndex(t=>t.vid===focused.vid)>=0?`${rows.findIndex(t=>t.vid===focused.vid)+1} / ${rows.length}`:'已查看'}</span>}</div>
    <div className="ws-detail-content" data-sheet-scroll>
      {detailMode==='video'?(focused?<Evidence task={focused} onNote={(vid,note)=>action('note',[vid],note)} onSource={sourceOnly} busy={busy}/>:<p className="ws-empty">选择视频查看证据</p>):<div className="ws-decision">
        <div><p className="mb-2 text-xs text-muted-foreground">{data?.group?.label||'当前确认组'} · {chosen.length} 个视频{hiddenCount>0?`，其中 ${hiddenCount} 个不在当前页`:''}</p><div className="ws-confirm-list" aria-label="本次完整归名范围">{chosen.map(t=><div key={t.vid} className="ws-confirm-file"><div className="flex items-center gap-2"><strong className="min-w-0 flex-1">{t.name}</strong><Button className="ws-icon" variant="ghost" size="sm" aria-label={`取消选择 ${t.name}`} title="取消选择" disabled={busy} onClick={()=>toggle([t])}><X size={14}/></Button></div><code>{t.path}</code></div>)}</div></div>
        <PersonPicker compact people={people} value={creator} onChange={setCreator} disabled={busy} error={peopleError} onRetry={loadPeople}/>
        {commonMarks.length>0&&<label className="grid gap-2 text-xs text-muted-foreground">共同账号规则<select aria-label="保存共同账号规则" value={watermarkKey} disabled={busy} onChange={e=>setWatermarkKey(e.target.value)}><option value="">仅归名，不保存账号规则</option>{commonMarks.map(c=><option key={c.key} value={c.key}>{c.platform} · {c.token}</option>)}</select></label>}
        <p className="text-xs text-muted-foreground">确认后加入待归档，文件暂不移动。</p>
      </div>}
      {submitError&&<p role="alert" className="ws-error">{submitError}</p>}
    </div>
    <div className="ws-detail-actions">{detailMode==='selection'?<Button disabled={busy||!chosen.length||!creator.trim()} onClick={confirm}>{busy?'正在保存…':`归给 ${creator.trim()||'所选人物'} · ${chosen.length}`}</Button>:<><Button variant="outline" disabled={!focused||busy} onClick={()=>focused&&toggle([focused])}>{focused&&selected.has(focused.vid)?'取消选择此视频':'选择此视频'}</Button>{chosen.length>0&&<Button disabled={busy} onClick={openSelection}>核对所选 · {chosen.length}</Button>}</>}</div>
  </>
  return <main className="ws-page" aria-label="人工确认工作区">
    <header className="ws-header"><h1>人工确认</h1><div className="ws-actions"><Button variant="ghost" size="sm" asChild><Link href="/operations"><span className="ws-link-label">分析中</span><span className="ws-count">{data?.waiting??'—'}</span><ChevronRight size={14}/></Link></Button><Button className="ws-icon" variant="ghost" aria-label="刷新人工确认" title="刷新" disabled={busy||loading} onClick={load}><RefreshCw size={15}/></Button></div></header>
    {receipt&&<div className="ws-receipt"><QueueReceipt count={receipt.vids.length} creator={receipt.creator} onUndo={undo} busy={busy}/></div>}
    {error&&<p role="alert" className="ws-error">{error}</p>}
    {data?.focus_missing&&<p className="ws-error">链接中的视频尚未就绪或已归名。</p>}
    {(personScope||clusterScope)&&<div className="ws-toolbar"><span className="ws-muted">{personScope||`相似组 ${clusterScope}`}</span><Button size="sm" variant="ghost" onClick={()=>{setPersonScope('');setClusterScope('');setGroup('');setPage(0);setLinkFocus('')}}>查看全部</Button></div>}
    <div className="ws-view-tabs"><WorkspaceTabs value={view} onChange={value=>{setView(value);setGroup('');setPage(0);setLinkFocus('')}} label="确认状态" disabled={busy} items={views.map(v=>({...v,count:data?.counts[v.value]}))}/></div>
    <div className="ws-workarea">
      <aside className="ws-groups" aria-label="确认组"><div className="ws-panel-heading"><h2>确认组</h2><span>{data?.groups.length??'—'}</span></div><div className="ws-group-list">{data?.groups.map(g=><button key={g.key} type="button" className="ws-group" disabled={busy} aria-pressed={activeGroup===g.key} onClick={()=>chooseGroup(g.key)}><span>{g.label}</span><span className="ws-count">{g.count}</span>{(selections[reviewSelectionKey(g.key,view==='deferred')]||[]).length>0&&<Check size={12} aria-label="有已选视频"/>}</button>)}</div></aside>
      <section className="ws-queue" aria-label="待确认视频">

        <div className="ws-toolbar"><WorkspaceSearch value={query} onChange={changeQuery} label="搜索当前组视频" placeholder="搜索文件名或路径…" disabled={busy}><Popover.Root><Popover.Trigger asChild><InputGroupButton size="icon-sm" aria-label="分组与筛选" title="分组与筛选" data-active={!!folder}><SlidersHorizontal size={15}/>{folder&&<span className="size-1.5 rounded-full bg-current"/>}</InputGroupButton></Popover.Trigger><Popover.Portal><Popover.Content className="ws-filter-content" align="end" sideOffset={6}><label className="ws-mobile-filter">确认组<select aria-label="选择确认组" value={activeGroup} disabled={busy} onChange={e=>chooseGroup(e.target.value)}>{!data?.groups.length&&<option value="">暂无确认组</option>}{data?.groups.map(g=><option key={g.key} value={g.key}>{g.label} · {g.count}</option>)}</select></label><label>原文件夹<select aria-label="按原文件夹筛选" value={folder} onChange={e=>{setFolder(e.target.value);if(!group&&data?.group)setGroup(data.group.key);setPage(0);setLinkFocus('')}}><option value="">全部文件夹</option>{data?.folders.map(f=><option key={f} value={f}>{f}</option>)}</select></label>{folder&&<Button className="mt-3" variant="ghost" size="sm" onClick={()=>{setFolder('');setPage(0)}}>清除筛选</Button>}</Popover.Content></Popover.Portal></Popover.Root></WorkspaceSearch></div>
        <div className="ws-selection"><Checkbox className="ws-check" aria-label="选择当前页视频" motionPreset="instant" checked={rows.length>0&&rows.every(t=>selected.has(t.vid))} indeterminate={rows.some(t=>selected.has(t.vid))&&!rows.every(t=>selected.has(t.vid))} disabled={disabled||!rows.length} onCheckedChange={()=>toggle(rows)}/><div className="ws-selection-summary"><strong>{chosen.length?`已选 ${chosen.length}`:data?.group?.label||'待确认'}</strong><span> · {hiddenCount?`${hiddenCount} 个在其他页或被筛选隐藏`:data?`${data.total} 个视频`:'读取中'}</span></div></div>
        <div ref={listRef} className="ws-list" aria-busy={loading} onScroll={e=>{scrollPositions.current[listKey]=e.currentTarget.scrollTop}}>
          {loading&&!data?<p role="status" className="ws-empty">加载视频…</p>:!rows.length?<div className="ws-empty"><p>{group&&!data?.group?'该组已处理完，选择另一组继续。':query||folder?'当前筛选下没有视频。':'当前没有待确认视频。'}</p>{(query||folder)&&<Button variant="ghost" onClick={()=>{changeQuery('');setFolder('')}}>清除筛选</Button>}</div>:rows.map(t=><article key={t.vid} data-vid={t.vid} className="ws-row" data-active={focused?.vid===t.vid} data-selected={selected.has(t.vid)}><Checkbox className="ws-check" aria-label={`选择 ${t.name}`} motionPreset="instant" checked={selected.has(t.vid)} disabled={disabled} onCheckedChange={()=>toggle([t])}/><button type="button" className="ws-row-open" onClick={()=>inspect(t)} aria-label={`核对 ${t.name}`}><span className="ws-row-thumb">{thumb(t)?<img data-private-media draggable={false} alt="" loading="lazy" src={thumb(t)!}/>:<Play size={18}/>}</span><span className="ws-row-copy"><span className="ws-row-name" title={t.name}>{t.name}</span><span className="ws-row-meta">{(t.size/2**30).toFixed(2)} GB · {(t.data.candidates||[]).map(c=>c.sources.map(s=>sourceName[s]).join(' / ')).join(' · ')||'需要人工判断'}</span>{t.note&&<span className="ws-row-meta">{t.note}</span>}</span></button></article>)}
        </div>
        {chosen.length>0&&<div className="ws-bulk" aria-label="所选视频操作"><span className="ws-muted">已选 {chosen.length}</span>{chosen.length>0&&<Button className="ws-icon" variant="ghost" size="sm" title="清空当前组选择" aria-label="清空当前组选择" disabled={busy} onClick={clear}><X size={14}/></Button>}<Button variant="outline" size="sm" disabled={disabled||!chosen.length} onClick={()=>action(view==='deferred'?'restore':'defer',chosen.map(t=>t.vid))}>{view==='deferred'?'恢复':'暂缓'}</Button><Button size="sm" disabled={disabled||!chosen.length} onClick={openSelection}><ListChecks size={14}/>核对所选</Button></div>}
        <div className="ws-pagination"><span>{data?`${data.total} 个 · 第 ${currentPage+1} / ${Math.max(1,Math.ceil(data.total/data.limit))} 页`:'读取中'}</span><div className="ws-actions"><Button className="ws-icon" variant="ghost" size="sm" aria-label="上一页" title="上一页" disabled={disabled||currentPage===0} onClick={()=>{setPage(currentPage-1);setLinkFocus('')}}><ChevronLeft size={15}/></Button><Button className="ws-icon" variant="ghost" size="sm" aria-label="下一页" title="下一页" disabled={disabled||!data||(currentPage+1)*data.limit>=data.total} onClick={()=>{setPage(currentPage+1);setLinkFocus('')}}><ChevronRight size={15}/></Button></div></div>
      </section>
      {wide&&<aside className="ws-inspector" aria-label="视频与归名">{detail}</aside>}
    </div>
    {!wide&&<Dialog open={previewOpen} onOpenChange={open=>!busy&&setPreviewOpen(open)}><ResponsiveDialogContent open={previewOpen} className="ws-detail-sheet" style={{animationDuration:instantEntry?'0ms':'180ms'}} overlayProps={{style:{animationDuration:instantEntry?'0ms':'140ms'}}} inert={!previewOpen} onCloseAutoFocus={e=>{if(returnFocus.current?.isConnected){e.preventDefault();returnFocus.current.focus()}}} onEscapeKeyDown={e=>busy&&e.preventDefault()} onInteractOutside={e=>busy&&e.preventDefault()}><DialogTitle className="sr-only">{detailMode==='selection'?'核对所选视频':'视频与证据'}</DialogTitle><DialogDescription className="sr-only">关闭后保留筛选、勾选与列表位置。</DialogDescription>{detail}</ResponsiveDialogContent></Dialog>}
  </main>
}
