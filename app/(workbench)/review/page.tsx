"use client"

import Link from 'next/link'
import { useCallback, useEffect, useRef, useState } from 'react'
import { Check, ChevronLeft, ChevronRight, Play, RefreshCw, SlidersHorizontal, X, ArrowLeft, ListChecks } from 'lucide-react'
import * as Popover from '@radix-ui/react-popover'
import { workspaceToast as toast } from '@/components/workbench/feedback'
import { api } from '@/lib/face-api'
import type { ProcessingTask } from '@/lib/processing-model'
import { reviewSelectionKey, toggleReviewSelection, removeReviewSelection, type ReviewSelections } from '@/lib/review-selection'
import { PersonPicker, type PersonChoice } from '@/components/review/person-picker'
import { QueueReceipt } from '@/components/review/workflow'
import { ResponsiveDialogContent } from '@/components/review/responsive-dialog'
import { Dialog, DialogTitle, DialogDescription } from '@/components/ui/dialog'
import { Button as NavigationButton } from '@/components/ui/button'
import { WorkspaceButton as Button, WorkspaceSelect, WorkspaceField, WorkspaceCard, WorkspacePath } from '@/components/workbench/inner'
import { EvidencePanel } from '@/components/review/evidence-panel'
import { ReferencePanel } from '@/components/review/reference-panel'
import { useWorkspaceQuery, useWorkspaceScroll } from '@/components/workbench/state'
import { InputGroupButton } from '@/components/ui/input-group'
import { Checkbox } from '@/components/beui/motion/checkbox'
import { ReviewSummaryRow } from '@/components/review/review-summary-row'
import { WorkspaceSearch, WorkspaceTabs, useWideWorkspace } from '@/components/workbench/controls'

type Group={key:string;label:string;kind:string;count:number}
type Data={counts:Record<string,number>;groups:Group[];group:Group|null;total:number;rows:ProcessingTask[];page:number;limit:number;waiting:number;online:boolean;folders:string[];focus_missing:boolean}
const views=[{value:'all',label:'全部'},{value:'suggested',label:'有候选'},{value:'conflict',label:'证据冲突'},{value:'none',label:'无候选'},{value:'manual',label:'人工接管'},{value:'deferred',label:'暂缓'}]

export default function Review() {
  const [data,setData]=useState<Data|null>(null),[error,setError]=useState(''),[loading,setLoading]=useState(true),[busy,setBusy]=useState(false)
  const {values:filters,ready:initialized,update:updateFilters}=useWorkspaceQuery({view:'all',group:'',q:'',folder:'',page:'0',person:'',cluster:'',vid:''},{view:views.map(v=>v.value)})
  const {view,group,q:query,folder,person:personScope,cluster:clusterScope,vid:linkFocus}=filters,page=Number(filters.page)
  const [search,setSearch]=useState('')
  const setView=(view:string)=>updateFilters({view,group:'',page:'0',vid:''})
  const setGroup=(group:string)=>updateFilters({group})
  const setFolder=(folder:string)=>updateFilters({folder,page:'0',vid:''})
  const setPage=(page:number)=>updateFilters({page:String(page)})
  const setLinkFocus=(vid:string)=>updateFilters({vid})
  const [selections,setSelections]=useState<ReviewSelections>({}),[inspected,setInspected]=useState<Record<string,ProcessingTask>>({})
  const [instantEntry,setInstantEntry]=useState(false)
  const [previewOpen,setPreviewOpen]=useState(false),[detailMode,setDetailMode]=useState<'video'|'selection'>('video')
  const [drafts,setDrafts]=useState<Record<string,{creator:string;watermarkKey:string}>>({}),[notes,setNotes]=useState<Record<string,string>>({}),[submitError,setSubmitError]=useState('')
  const [pickerSearch,setPickerSearch]=useState<Record<string,string>>({})
  const [reference,setReference]=useState<{name:string;returnTo:'video'|'selection';scroll:number}|null>(null)
  const [people,setPeople]=useState<PersonChoice[]>([]),[peopleError,setPeopleError]=useState(''),[receipt,setReceipt]=useState<{vids:string[];creator:string}|null>(null)
  const returnFocus=useRef<HTMLElement|null>(null)
  const wide=useWideWorkspace(),sequence=useRef(0),selectionHeading=useRef<HTMLHeadingElement>(null)
  const activeGroup=data?.group?.key||group,scope=reviewSelectionKey(activeGroup,view==='deferred'),chosen=selections[scope]||[],rows=data?.rows||[]
  const selected=new Set(chosen.map(t=>t.vid)),focused=inspected[scope]||rows[0],hiddenCount=chosen.filter(t=>!rows.some(r=>r.vid===t.vid)).length
  const creator=drafts[scope]?.creator||'',watermarkKey=drafts[scope]?.watermarkKey||''
  const setCreator=(creator:string)=>setDrafts(current=>({...current,[scope]:{...current[scope],creator,watermarkKey:''}}))
  const setWatermarkKey=(watermarkKey:string)=>setDrafts(current=>({...current,[scope]:{creator:current[scope]?.creator||'',watermarkKey}}))
  const disabled=busy||loading||query!==search||!!error
  const currentPage=data?.page??page
  const listKey=JSON.stringify([scope,search,folder,currentPage])
  const listScroll=useWorkspaceScroll('review:'+listKey,!!data&&!loading&&query===search)
  useEffect(()=>{const timer=setTimeout(()=>{setSearch(query)},250);return()=>clearTimeout(timer)},[query])
  const load=useCallback(async()=>{
    if(!initialized)return null
    const token=++sequence.current;setLoading(true)
    try{const params=new URLSearchParams({view,group,q:search,folder,page:String(page),vid:linkFocus,person:personScope,cluster:clusterScope});const next=await api<Data>('/api/review?'+params);if(token===sequence.current){setData(next);setError('');if((!group&&next.group)||page!==next.page)updateFilters({group:group||next.group?.key||'',page:String(next.page)})};return next}
    catch(e:any){if(token===sequence.current)setError(e.message);return null}finally{if(token===sequence.current)setLoading(false)}
  },[initialized,view,group,search,folder,page,linkFocus,personScope,clusterScope,updateFilters])
  const loadPeople=useCallback(async()=>{try{setPeople((await api<{persons:PersonChoice[]}>('/api/persons?picker=1')).persons);setPeopleError('')}catch(e:any){setPeopleError(e.message)}},[])
  useEffect(()=>{void load();return()=>{sequence.current++}},[load])
  useEffect(()=>{void loadPeople()},[loadPeople])

  useEffect(()=>{setDetailMode('video');setReference(null);setSubmitError('')},[scope])
  useEffect(()=>{if(wide)setPreviewOpen(false)},[wide])
  useEffect(()=>{if(busy||previewOpen||data?.rows.length||!initialized)return;const timer=setTimeout(load,10000);return()=>clearTimeout(timer)},[data,busy,previewOpen,initialized,load])
  const commonMarks=chosen.length ? (chosen[0].data.watermark?.candidates||[]).filter(c=>c.kind==='account'&&c.platform!=='Telegram'&&chosen.every(t=>t.data.watermark?.candidates.some(other=>other.key===c.key))) : []
  const changeQuery=(value:string)=>{if(!group&&data?.group)setGroup(data.group.key);updateFilters({q:value,page:'0',vid:''})}
  const chooseGroup=(key:string)=>{setGroup(key);setPage(0);setLinkFocus('');setPreviewOpen(false)}
  const toggle=(tasks:ProcessingTask[])=>{setSelections(current=>({...current,[scope]:toggleReviewSelection(current[scope]||[],tasks)}));setWatermarkKey('')}
  const clear=()=>setSelections(current=>({...current,[scope]:[]}))
  const inspect=(task:ProcessingTask)=>{setReference(null);if(!previewOpen){returnFocus.current=document.activeElement as HTMLElement;setInstantEntry(!!document.activeElement?.matches(':focus-visible')||matchMedia('(prefers-reduced-motion: reduce)').matches)}setInspected(current=>({...current,[scope]:task}));setDetailMode('video');if(!wide)setPreviewOpen(true)}
  const openSelection=()=>{if(!previewOpen){returnFocus.current=document.activeElement as HTMLElement;setInstantEntry(!!document.activeElement?.matches(':focus-visible')||matchMedia('(prefers-reduced-motion: reduce)').matches)}if(!creator)setCreator(data?.group?.kind==='suggested'?data.group.label:'');setReference(null);setSubmitError('');setDetailMode('selection');if(!wide)setPreviewOpen(true);else requestAnimationFrame(()=>selectionHeading.current?.focus())}
  const removeCompleted=(vids:string[])=>{setSelections(current=>removeReviewSelection(current,vids));setInspected(current=>Object.fromEntries(Object.entries(current).filter(([,task])=>!vids.includes(task.vid))))}
  const action=async(action:string,vids:string[],note?:string)=>{
    setBusy(true);setSubmitError('')
    try{await api('/api/review',{method:'POST',body:JSON.stringify({action,vids,note})});if(action!=='note'){removeCompleted(vids);setPreviewOpen(false);setDetailMode('video');setLinkFocus('')}else setInspected(current=>Object.fromEntries(Object.entries(current).map(([key,t])=>[key,vids.includes(t.vid)?{...t,note:note||''}:t])));await load();toast.success({note:'备注已保存',defer:'已移至暂缓',restore:'已恢复核对'}[action]||'已更新')}
    catch(e:any){setSubmitError(e.message);toast.error(e.message)}finally{setBusy(false)}
  }
  const sourceOnly=async(key:string)=>{
    setBusy(true)
    try{await api('/api/watermarks',{method:'PUT',body:JSON.stringify({key,kind:'source'})});setPreviewOpen(false);setSelections({});setInspected({});setDetailMode('video');await load();toast.success('已标为来源，相关人物建议将重新分析');return true}
    catch(e:any){setSubmitError(e.message);toast.error(e.message);return false}finally{setBusy(false)}
  }
  const confirm=async()=>{
    if(!chosen.length||!creator.trim()||busy)return
    setBusy(true);setSubmitError('')
    const vids=chosen.map(t=>t.vid),name=creator.trim()
    try{const result=await api<{rule_warning?:string}>('/api/plan/queue',{method:'POST',body:JSON.stringify({vids,creator:name,source:'review',source_id:activeGroup,review_group:{key:activeGroup,deferred:view==='deferred'},watermark_key:watermarkKey||undefined,fingerprints:Object.fromEntries(chosen.map(t=>[t.vid,{path:t.path,size:t.size,mtime_ns:t.mtime_ns}]))})});setReceipt({vids,creator:name});removeCompleted(vids);setDrafts(current=>({...current,[scope]:{creator:'',watermarkKey:''}}));setPreviewOpen(false);setDetailMode('video');setLinkFocus('');await load();await loadPeople();result.rule_warning?toast.warning(result.rule_warning):toast.success('已加入待归档')}
    catch(e:any){setSubmitError(e.message);toast.error(e.message)}finally{setBusy(false)}
  }
  const undo=async()=>{if(!receipt)return;setBusy(true);try{const result=await api<{warning?:string}>('/api/plan/queue',{method:'DELETE',body:JSON.stringify({vids:receipt.vids})});setReceipt(null);await load();result.warning?toast.warning(result.warning):toast.success('已撤销本次归名')}catch(e:any){toast.error(e.message)}finally{setBusy(false)}}
  const openReference=(name:string)=>{setReference({name,returnTo:detailMode,scroll:document.querySelector('.ws-detail-content')?.scrollTop||0});requestAnimationFrame(()=>selectionHeading.current?.focus())}
  const backReference=()=>{const previous=reference;setReference(null);if(previous)requestAnimationFrame(()=>{const area=document.querySelector('.ws-detail-content');if(area)area.scrollTop=previous.scroll;const trigger=[...document.querySelectorAll<HTMLButtonElement>('.ws-detail-content button')].find(b=>b.getAttribute('aria-label')===`看 ${previous.name} 的对照视频`);trigger?.focus({preventScroll:true})})}
  const detail=<>

    <div className="ws-detail-heading"><h2 ref={selectionHeading} tabIndex={-1}>{reference?`${reference.name} · 对照视频`:detailMode==='selection'?`核对所选 · ${chosen.length}`:'视频与证据'}</h2>{reference?<Button variant="ghost" onClick={backReference}><ArrowLeft size={14}/>返回{reference.returnTo==='selection'?'人物选择':'证据'}</Button>:detailMode==='selection'&&<Button variant="ghost" size="sm" onClick={()=>setDetailMode('video')}><ArrowLeft size={14}/>返回证据</Button>}{wide&&!reference&&detailMode==='video'&&focused&&<span className="ws-muted">{rows.findIndex(t=>t.vid===focused.vid)>=0?`${rows.findIndex(t=>t.vid===focused.vid)+1} / ${rows.length}`:'已查看'}</span>}</div>
    <div className="ws-detail-content" data-sheet-scroll>
      {reference?<ReferencePanel name={reference.name}/>:detailMode==='video'?(focused?<EvidencePanel key={focused.vid} task={focused} note={notes[focused.vid]??focused.note} onNoteChange={note=>setNotes(current=>({...current,[focused.vid]:note}))} onSaveNote={()=>action('note',[focused.vid],notes[focused.vid]??focused.note)} onSource={sourceOnly} busy={busy} creator={creator} onChoose={setCreator} onReference={openReference}/>:<p className="ws-empty">选择视频查看证据</p>):<div className="ws-decision">
        <div><p className="mb-2 text-xs text-muted-foreground">{data?.group?.label||'当前确认组'} · {chosen.length} 个视频{hiddenCount>0?`，其中 ${hiddenCount} 个不在当前页`:''}</p><div className="ws-confirm-list" aria-label="本次完整归名范围">{chosen.map(t=><div key={t.vid} className="ws-confirm-file"><div className="flex items-center gap-2"><Button variant="ghost" className="ws-confirm-name" onClick={()=>inspect(t)} aria-label={`核对所选 ${t.name}`}><Play size={13}/>{t.name}</Button><Button className="ws-icon" variant="ghost" size="sm" aria-label={`取消选择 ${t.name}`} title="取消选择" disabled={busy} onClick={()=>toggle([t])}><X size={14}/></Button></div><WorkspacePath value={t.path}/></div>)}</div></div>
        <PersonPicker search={pickerSearch[scope]||''} onSearchChange={value=>setPickerSearch(current=>({...current,[scope]:value}))} onReference={openReference} compact people={people} value={creator} onChange={setCreator} disabled={busy} error={peopleError} onRetry={loadPeople}/>
        {commonMarks.length>0&&<WorkspaceField label="共同账号规则"><WorkspaceSelect label="保存共同账号规则" value={watermarkKey} disabled={busy} onChange={setWatermarkKey} options={[{value:'',label:'仅归名，不保存账号规则'},...commonMarks.map(c=>({value:c.key,label:`${c.platform} · ${c.token}`}))]}/></WorkspaceField>}
        <p className="text-xs text-muted-foreground">确认后加入待归档，文件暂不移动。</p>
      </div>}
      {submitError&&<p role="alert" className="ws-error">{submitError}</p>}
    </div>
    <div className="ws-detail-actions">{reference?<><Button variant="outline" disabled={busy} onClick={backReference}>返回{reference.returnTo==='selection'?'人物选择':'证据'}</Button><Button disabled={busy} onClick={()=>{setCreator(reference.name);backReference()}}>选择 {reference.name}</Button></>:detailMode==='selection'?<Button disabled={busy||!chosen.length||!creator.trim()} onClick={confirm}>{busy?'正在保存…':`归给 ${creator.trim()||'所选人物'} · ${chosen.length}`}</Button>:<><Button variant="outline" disabled={!focused||busy} onClick={()=>focused&&toggle([focused])}>{focused&&selected.has(focused.vid)?'取消选择此视频':'选择此视频'}</Button>{chosen.length>0&&<Button disabled={busy} onClick={openSelection}>核对所选 · {chosen.length}</Button>}</>}</div>
  </>
  return <main className="ws-page" aria-label="人工确认工作区">
    <header className="ws-header"><h1>人工确认</h1><div className="ws-actions"><NavigationButton variant="ghost" size="sm" asChild><Link href="/operations"><span className="ws-link-label">分析中</span><span className="ws-count">{data?.waiting??'—'}</span><ChevronRight size={14}/></Link></NavigationButton><Button className="ws-icon" variant="ghost" aria-label="刷新人工确认" title="刷新" disabled={busy||loading} onClick={load}><RefreshCw size={15}/></Button></div></header>
    {receipt&&<div className="ws-receipt"><QueueReceipt count={receipt.vids.length} creator={receipt.creator} onUndo={undo} busy={busy}/></div>}
    {error&&<p role="alert" className="ws-error">{error}</p>}
    {data?.focus_missing&&<p className="ws-error">链接中的视频尚未就绪或已归名。</p>}
    {(personScope||clusterScope)&&<div className="ws-toolbar"><span className="ws-muted">{personScope||`相似组 ${clusterScope}`}</span><Button size="sm" variant="ghost" onClick={()=>updateFilters({person:'',cluster:'',group:'',page:'0',vid:''})}>查看全部</Button></div>}
    <div className="ws-view-tabs"><WorkspaceTabs value={view} onChange={value=>{setView(value);setGroup('');setPage(0);setLinkFocus('')}} label="确认状态" disabled={busy} items={views.map(v=>({...v,count:data?.counts[v.value]}))}/></div>
    <div className="ws-workarea">
      <aside className="ws-groups" aria-label="确认组"><div className="ws-panel-heading"><h2>确认组</h2><span>{data?.groups.length??'—'}</span></div><div className="ws-group-list">{data?.groups.map(g=><button key={g.key} type="button" className="ws-group" disabled={busy} aria-pressed={activeGroup===g.key} onClick={()=>chooseGroup(g.key)}><span>{g.label}</span><span className="ws-count">{g.count}</span>{(selections[reviewSelectionKey(g.key,view==='deferred')]||[]).length>0&&<Check size={12} aria-label="有已选视频"/>}</button>)}</div></aside>
      <section className="ws-queue" aria-label="待确认视频">

        <div className="ws-toolbar"><WorkspaceSearch value={query} onChange={changeQuery} label="搜索当前组视频" placeholder="搜索文件名或路径…" disabled={busy}><Popover.Root><Popover.Trigger asChild><InputGroupButton size="icon-sm" disabled={busy} aria-label="分组与筛选" title="分组与筛选" data-active={!!folder}><SlidersHorizontal size={15}/>{folder&&<span className="size-1.5 rounded-full bg-current"/>}</InputGroupButton></Popover.Trigger><Popover.Portal><Popover.Content className="ws-filter-content" align="end" sideOffset={6}><div className="ws-mobile-filter"><WorkspaceField label="确认组"><WorkspaceSelect label="选择确认组" value={activeGroup} disabled={busy} onChange={chooseGroup} options={(data?.groups||[]).map(g=>({value:g.key,label:`${g.label} · ${g.count}`}))} placeholder="暂无确认组"/></WorkspaceField></div><WorkspaceField label="原文件夹"><WorkspaceSelect label="按原文件夹筛选" disabled={busy} value={folder} onChange={value=>{setFolder(value);if(!group&&data?.group)setGroup(data.group.key)}} options={[{value:'',label:'全部文件夹'},...(data?.folders||[]).map(f=>({value:f,label:f}))]}/></WorkspaceField>{folder&&<Button className="mt-3" variant="ghost" size="sm" onClick={()=>{setFolder('');setPage(0)}}>清除筛选</Button>}</Popover.Content></Popover.Portal></Popover.Root></WorkspaceSearch></div>
        <div className="ws-selection"><Checkbox className="ws-check" aria-label="选择当前页视频" motionPreset="instant" checked={rows.length>0&&rows.every(t=>selected.has(t.vid))} indeterminate={rows.some(t=>selected.has(t.vid))&&!rows.every(t=>selected.has(t.vid))} disabled={disabled||!rows.length} onCheckedChange={()=>toggle(rows)}/><div className="ws-selection-summary"><strong>{chosen.length?`已选 ${chosen.length}`:data?.group?.label||'待确认'}</strong><span> · {hiddenCount?`${hiddenCount} 个在其他页或被筛选隐藏`:data?`${data.total} 个视频`:'读取中'}</span></div></div>
        <div ref={listScroll.ref} className="ws-list" aria-busy={loading||query!==search} onScroll={listScroll.onScroll}>
          {loading&&!data?<p role="status" className="ws-empty">加载视频…</p>:!rows.length?<div className="ws-empty"><p>{group&&!data?.group?'该组已处理完，选择另一组继续。':query||folder?'当前筛选下没有视频。':'当前没有待确认视频。'}</p>{(query||folder)&&<Button variant="ghost" onClick={()=>{changeQuery('');setFolder('')}}>清除筛选</Button>}</div>:rows.map(t=><ReviewSummaryRow key={t.vid} task={t} selected={selected.has(t.vid)} active={focused?.vid===t.vid} disabled={disabled} onToggle={()=>toggle([t])} onInspect={()=>inspect(t)}/>)}
        </div>
        {chosen.length>0&&<div className="ws-bulk" aria-label="所选视频操作"><span className="ws-muted">已选 {chosen.length}</span>{chosen.length>0&&<Button className="ws-icon" variant="ghost" size="sm" title="清空当前组选择" aria-label="清空当前组选择" disabled={busy} onClick={clear}><X size={14}/></Button>}<Button variant="outline" size="sm" disabled={disabled||!chosen.length} onClick={()=>action(view==='deferred'?'restore':'defer',chosen.map(t=>t.vid))}>{view==='deferred'?'恢复':'暂缓'}</Button><Button size="sm" disabled={disabled||!chosen.length} onClick={openSelection}><ListChecks size={14}/>核对所选</Button></div>}
        <div className="ws-pagination"><span>{data?`${data.total} 个 · 第 ${currentPage+1} / ${Math.max(1,Math.ceil(data.total/data.limit))} 页`:'读取中'}</span><div className="ws-actions"><Button className="ws-icon" variant="ghost" size="sm" aria-label="上一页" title="上一页" disabled={disabled||currentPage===0} onClick={()=>{setPage(currentPage-1);setLinkFocus('')}}><ChevronLeft size={15}/></Button><Button className="ws-icon" variant="ghost" size="sm" aria-label="下一页" title="下一页" disabled={disabled||!data||(currentPage+1)*data.limit>=data.total} onClick={()=>{setPage(currentPage+1);setLinkFocus('')}}><ChevronRight size={15}/></Button></div></div>
      </section>
      {wide&&<aside className="ws-inspector" aria-label="视频与归名">{detail}</aside>}
    </div>
    {!wide&&<Dialog open={previewOpen} onOpenChange={open=>!busy&&setPreviewOpen(open)}><ResponsiveDialogContent open={previewOpen} className="ws-detail-sheet" style={{animationDuration:instantEntry?'0ms':'180ms'}} overlayProps={{style:{animationDuration:instantEntry?'0ms':'140ms'}}} inert={!previewOpen} onCloseAutoFocus={e=>{if(returnFocus.current?.isConnected){e.preventDefault();returnFocus.current.focus()}}} onEscapeKeyDown={e=>{if(busy)e.preventDefault();else if(reference){e.preventDefault();backReference()}}} onInteractOutside={e=>busy&&e.preventDefault()}><DialogTitle className="sr-only">{reference?`${reference.name} 的对照视频`:detailMode==='selection'?'核对所选视频':'视频与证据'}</DialogTitle><DialogDescription className="sr-only">关闭后保留筛选、勾选与列表位置。</DialogDescription>{detail}</ResponsiveDialogContent></Dialog>}
  </main>
}
