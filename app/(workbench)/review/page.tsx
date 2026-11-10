"use client"

import Link from 'next/link'
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react'
import { Check, ChevronLeft, ChevronRight, Play, RefreshCw, SlidersHorizontal, X, ArrowLeft, ListChecks } from 'lucide-react'
import * as Popover from '@radix-ui/react-popover'
import { workspaceToast as toast } from '@/components/workbench/feedback'
import { api } from '@/lib/face-api'
import type { ProcessingTask } from '@/lib/processing-model'
import { reviewSelectionKey, toggleReviewSelection, removeReviewSelection, type ReviewSelections } from '@/lib/review-selection'
import { PersonPicker, type PersonChoice, type PersonNameHint } from '@/components/review/person-picker'
import { VideoCover } from '@/components/review/video-cover'
import { Card } from '@/components/beui/motion/card'
import { Collapsible, CollapsibleContent } from '@/components/beui/motion/collapsible'
import { AvatarGroup } from '@/components/beui/motion/avatar-group'
import { DigitSwap } from '@/components/beui/motion/digit-swap'
import { reviewListSummary } from '@/lib/review-list-summary'
import { QueueReceipt } from '@/components/review/workflow'
import { ResponsiveDialogContent } from '@/components/review/responsive-dialog'
import { Dialog, DialogTitle, DialogDescription } from '@/components/ui/dialog'
import { Button as NavigationButton } from '@/components/ui/button'
import { WorkspaceButton as Button, WorkspaceSelect, WorkspaceField, WorkspaceCard, WorkspacePath, WorkspaceMedia } from '@/components/workbench/inner'
import { EvidencePanel } from '@/components/review/evidence-panel'
import { ReferencePanel } from '@/components/review/reference-panel'
import { useWorkspaceQuery, useWorkspaceScroll, readWorkbenchCache, writeWorkbenchCache, deleteWorkbenchCache } from '@/components/workbench/state'
import { InputGroupButton } from '@/components/ui/input-group'
import { Checkbox } from '@/components/beui/motion/checkbox'
import { useReviewActivity } from '@/components/workbench/review-activity'
import { AlertDialog,AlertDialogContent,AlertDialogHeader,AlertDialogTitle,AlertDialogDescription,AlertDialogFooter,AlertDialogCancel,AlertDialogAction } from '@/components/ui/alert-dialog'
import { ReviewSummaryRow } from '@/components/review/review-summary-row'
import { WorkspaceSearch, WorkspaceTabs, useWideWorkspace } from '@/components/workbench/controls'

type Group={key:string;label:string;kind:string;count:number}
type Data={has_updates?:boolean;session?:string;session_at?:number;expires_at?:number;remaining?:number;next_page?:number|null;previous_page?:number|null;blocked?:{vid:string;message:string}[];counts:Record<string,number>;groups:Group[];group:Group|null;total:number;rows:ProcessingTask[];page:number;limit:number;waiting:number;online:boolean;folders:string[];focus_missing:boolean}
const views=[{value:'all',label:'全部'},{value:'suggested',label:'有候选'},{value:'conflict',label:'证据冲突'},{value:'none',label:'无候选'},{value:'manual',label:'人工接管'},{value:'deferred',label:'暂缓'}]

const uuid=()=>{if(typeof crypto!=='undefined'&&typeof crypto.randomUUID==='function')return crypto.randomUUID();const bytes=crypto.getRandomValues(new Uint8Array(16));bytes[6]=(bytes[6]&15)|64;bytes[8]=(bytes[8]&63)|128;const hex=[...bytes].map(b=>b.toString(16).padStart(2,'0')).join('');return `${hex.slice(0,8)}-${hex.slice(8,12)}-${hex.slice(12,16)}-${hex.slice(16,20)}-${hex.slice(20)}`}

export default function Review() {
  const [data,setData]=useState<Data|null>(()=>readWorkbenchCache<Data>('review:lastData')??null),[error,setError]=useState(''),[loading,setLoading]=useState(()=>!readWorkbenchCache<Data>('review:lastData')),[busy,setBusy]=useState(false)
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
  const [selectionPreviewVid,setSelectionPreviewVid]=useState<string|null>(null)
  const [reference,setReference]=useState<{name:string;returnTo:'video'|'selection';scroll:number}|null>(null)
  const [people,setPeople]=useState<PersonChoice[]>(()=>readWorkbenchCache<PersonChoice[]>('review:people')??[]),[peopleError,setPeopleError]=useState(''),[receipt,setReceipt]=useState<{vids:string[];creator:string;session:string}|null>(null)
  const pendingMutations=useRef(new Map<string,string>())
  const mutate=async<T=any>(path:string,body:Record<string,unknown>,method='POST'):Promise<T>=>{const key=JSON.stringify([path,method,body]),id=pendingMutations.current.get(key)||uuid();pendingMutations.current.set(key,id);try{const result=await api<T>(path,{method,body:JSON.stringify({...body,operation_id:id})});pendingMutations.current.delete(key);return result}catch(error:any){if(!error.resultUnknown)pendingMutations.current.delete(key);else error.message+=`（操作 ${id}，重试将查询同一回执）`;throw error}}
  const refreshFocus=useRef<HTMLElement|null>(null)
  const returnFocus=useRef<HTMLElement|null>(null),session=useRef<string|undefined>(readWorkbenchCache<string>('review:session')),noteBaseline=useRef<Record<string,string>>({}),notesRef=useRef(notes),polling=useRef(false),scrollAnchor=useRef<{vid:string;offset:number}[]|null>(null)
  notesRef.current=notes
  const [updates,setUpdates]=useState<{has_updates:boolean;refreshing:number;blocked:number;waiting?:number}|null>(null),[refreshConfirm,setRefreshConfirm]=useState(false),[sessionExpired,setSessionExpired]=useState(false)
  const hasSelections=Object.values(selections).some(rows=>rows.length>0),hasDrafts=Object.values(drafts).some(d=>!!d.creator||!!d.watermarkKey)||Object.entries(notes).some(([id,note])=>note!==noteBaseline.current[id])
  const {version:activityVersion,canApply:canAutoApply}=useReviewActivity(busy||loading||hasSelections||hasDrafts||previewOpen||!!reference||refreshConfirm||detailMode==='selection')

  const wide=useWideWorkspace(),sequence=useRef(0),selectionHeading=useRef<HTMLHeadingElement>(null)
  const activeGroup=data?.group?.key||group,scope=reviewSelectionKey(activeGroup,view==='deferred'),chosen=selections[scope]||[],rows=data?.rows||[]
  const blockedIds=new Set((data?.blocked||[]).map(item=>item.vid))
  const selected=new Set(chosen.map(t=>t.vid)),originalFocus=inspected[scope]||rows.find(t=>t.vid===linkFocus)||rows[0],focused=originalFocus?{...originalFocus,review_blocked:data?.blocked?.find(item=>item.vid===originalFocus.vid)?.message}:undefined,hiddenCount=chosen.filter(t=>!rows.some(r=>r.vid===t.vid)).length
  const creator=drafts[scope]?.creator||'',watermarkKey=drafts[scope]?.watermarkKey||''
  const setCreator=(creator:string)=>setDrafts(current=>({...current,[scope]:{...current[scope],creator,watermarkKey:''}}))
  const setWatermarkKey=(watermarkKey:string)=>setDrafts(current=>({...current,[scope]:{creator:current[scope]?.creator||'',watermarkKey}}))
  const disabled=busy||loading||query!==search||!!error||sessionExpired
  const currentPage=data?.page??page
  const listKey=JSON.stringify([scope,search,folder,currentPage])
  const listScroll=useWorkspaceScroll('review:'+listKey,!!data&&!loading&&query===search)
  useEffect(()=>{const timer=setTimeout(()=>{setSearch(query)},250);return()=>clearTimeout(timer)},[query])
  const load=useCallback(async(options:{refresh?:boolean;automatic?:boolean;preservePosition?:boolean}={})=>{
    if(!initialized)return null
    if(options.preservePosition&&listScroll.ref.current){const area=listScroll.ref.current,top=area.getBoundingClientRect().top;scrollAnchor.current=[...area.querySelectorAll<HTMLElement>('[data-vid]')].filter(e=>e.getBoundingClientRect().bottom>top).map(e=>({vid:e.dataset.vid!,offset:e.getBoundingClientRect().top-top}))}
    const cacheKey='review:page:'+JSON.stringify([view,group,search,folder,page,linkFocus,personScope,clusterScope])
    const cachedPage=!options.refresh&&!options.preservePosition?readWorkbenchCache<Data>(cacheKey):undefined
    if(cachedPage){setData(cachedPage);setLoading(false)}
    const token=++sequence.current,activityAtStart=activityVersion()
    if(!options.automatic&&(options.refresh||!cachedPage&&!data))setLoading(true)
    try{
      const params=new URLSearchParams({view,group,q:search,folder,page:String(page),vid:linkFocus,person:personScope,cluster:clusterScope,...(!options.refresh&&session.current?{session:session.current}:{snapshot:'1'})})
      const next=await api<Data>('/api/review?'+params)
      if(token!==sequence.current)return null
      if(options.automatic&&!canAutoApply(activityAtStart)||options.refresh&&activityAtStart!==activityVersion()){setUpdates({has_updates:true,refreshing:0,blocked:0});return null}
      session.current=next.session
      if(options.refresh||options.preservePosition)deleteWorkbenchCache('review:page:')
      if(next.session)writeWorkbenchCache('review:session',next.session)
      writeWorkbenchCache(cacheKey,next);writeWorkbenchCache('review:lastData',next)
      if(next.group?.key&&next.group.key!==group)writeWorkbenchCache('review:page:'+JSON.stringify([view,next.group.key,search,folder,next.page,linkFocus,personScope,clusterScope]),next)
      setData(next);setUpdates(previous=>({has_updates:next.has_updates??false,refreshing:previous?.refreshing||0,blocked:previous?.blocked||0,waiting:next.waiting}));setSelections(current=>Object.fromEntries(Object.entries(current).map(([key,tasks])=>[key,tasks.map(t=>next.rows.find(r=>r.vid===t.vid)||t)])));setInspected(current=>Object.fromEntries(Object.entries(current).map(([key,t])=>[key,next.rows.find(r=>r.vid===t.vid)||t])));setError('');setSessionExpired(false)
      for(const row of next.rows)if(notesRef.current[row.vid]===undefined)noteBaseline.current[row.vid]=row.note
      if(options.refresh){setSelections({});setDrafts({});setNotes({});setPickerSearch({});noteBaseline.current={};setInspected({});setReference(null);setDetailMode('video');setPreviewOpen(false);setSubmitError('');setUpdates(null)}
      if((!group&&next.group)||page!==next.page||options.refresh&&next.group?.key!==group)updateFilters({group:next.group?.key||group,page:String(next.page)})
      return next
    }catch(e:any){if(token===sequence.current){if(options.automatic)setUpdates(previous=>({...previous,has_updates:true,refreshing:previous?.refreshing||0,blocked:previous?.blocked||0}));else setError(e.message);if(/核对已过期|核对会话|本次核对已过期/.test(e.message)){deleteWorkbenchCache('review:session');setSessionExpired(true)}}return null}
    finally{if(token===sequence.current&&!options.automatic)setLoading(false)}
  },[initialized,view,group,search,folder,page,linkFocus,personScope,clusterScope,updateFilters,activityVersion,canAutoApply])
  useLayoutEffect(()=>{
    if(loading||!scrollAnchor.current||!listScroll.ref.current)return
    const area=listScroll.ref.current,top=area.getBoundingClientRect().top,anchors=scrollAnchor.current;scrollAnchor.current=null
    for(const anchor of anchors){const row=[...area.querySelectorAll<HTMLElement>('[data-vid]')].find(e=>e.dataset.vid===anchor.vid);if(row){area.scrollTop+=row.getBoundingClientRect().top-top-anchor.offset;break}}
  },[data,loading,listScroll.ref])
  const loadPeople=useCallback(async()=>{try{const list=(await api<{persons:PersonChoice[]}>('/api/persons?picker=1')).persons;writeWorkbenchCache('review:people',list);setPeople(list);setPeopleError('')}catch(e:any){setPeopleError(e.message)}},[])
  useEffect(()=>{void load();return()=>{sequence.current++}},[load])
  useEffect(()=>{void loadPeople()},[loadPeople])

  useEffect(()=>{setDetailMode('video');setReference(null);setSubmitError('')},[scope])
  useEffect(()=>{if(wide)setPreviewOpen(false)},[wide])
  useEffect(()=>{
    if(!initialized)return
    let cancelled=false
    const poll=async()=>{if(polling.current||!session.current)return;polling.current=true;const id=session.current,requestSequence=sequence.current
      try{const next=await api<{has_updates:boolean;refreshing:number;blocked:number;waiting?:number}>('/api/review?status=1&session='+encodeURIComponent(id));if(cancelled||session.current!==id||requestSequence!==sequence.current)return;setUpdates(next);if(next.has_updates&&canAutoApply())await load({refresh:true,automatic:true})}
      catch(e:any){if(!cancelled&&session.current===id&&/核对已过期|本次核对已过期/.test(e.message)){setSessionExpired(true);if(canAutoApply())await load({refresh:true,automatic:true})}}
      finally{polling.current=false}}
    const timer=setInterval(poll,10000);void poll();return()=>{cancelled=true;clearInterval(timer)}
  },[initialized,load,canAutoApply])
  const evidenceTokens=(vids:string[])=>Object.fromEntries(vids.map(id=>[id,[...rows,...Object.values(selections).flat(),...Object.values(inspected)].find(t=>t.vid===id)?.review_token]));
  useEffect(()=>{if(!loading&&!refreshConfirm&&refreshFocus.current){const target=refreshFocus.current;requestAnimationFrame(()=>{if(!target.matches(':disabled')){target.focus({preventScroll:true});refreshFocus.current=null}})}},[loading,refreshConfirm])
  const requestRefresh=(event:{currentTarget:HTMLElement})=>{if(busy||loading)return;refreshFocus.current=event.currentTarget;if(hasSelections||hasDrafts)setRefreshConfirm(true);else void load({refresh:true})}
  const commonMarks=chosen.length ? (chosen[0].data.watermark?.candidates||[]).filter(c=>c.kind==='account'&&c.platform!=='Telegram'&&chosen.every(t=>t.data.watermark?.candidates.some(other=>other.key===c.key))) : []
  const selectionSuggestions:PersonChoice[]=chosen.length?(()=>{
    const map=new Map<string,{count:number;sim?:number;thumb:string|null;sources:Set<string>}>()
    for(const t of chosen){
      const faceThumb=t.data.face?.thumbs?.[0]?`/thumb?p=${encodeURIComponent(t.data.face.thumbs[0])}`:null
      for(const c of t.data.candidates||[]){
        const prev=map.get(c.name)||{count:0,sim:undefined,thumb:faceThumb,sources:new Set<string>()}
        prev.count++
        if(!prev.thumb&&faceThumb)prev.thumb=faceThumb
        for(const s of c.sources||[])prev.sources.add(s)
        for(const e of c.evidence||[])if(e.source==='face'&&typeof e.score==='number')prev.sim=prev.sim===undefined?e.score:Math.max(prev.sim,e.score)
        map.set(c.name,prev)
      }
    }
    return [...map.entries()].sort((a,b)=>b[1].count-a[1].count||(b[1].sim||0)-(a[1].sim||0)).map(([name,info])=>{
      const existing=people.find(p=>p.name===name)
      const parts=[`${info.count}/${chosen.length} 命中`]
      if(info.sim!==undefined)parts.push(`相似度 ${info.sim.toFixed(2)}`)
      else if(info.sources.has('watermark'))parts.push('水印线索')
      return {name,rep_thumb:existing?.rep_thumb||info.thumb||null,aliases:existing?.aliases,sim:info.sim,match_count:info.count,match_reason:parts.join(' · ')}
    })
  })():[]
  const selectionSampleFaces=chosen.length?[...new Set(chosen.flatMap(t=>(t.data.face?.thumbs||[]).slice(0,2).map(p=>`/thumb?p=${encodeURIComponent(p)}`)))].slice(0,6):[]
  const selectionNameHints:PersonNameHint[]=chosen.length?(()=>{
    const map=new Map<string,string>()
    for(const t of chosen){
      for(const c of t.data.candidates||[])if(c.name.trim()&&!map.has(c.name.trim()))map.set(c.name.trim(),'候选线索')
      for(const w of t.data.watermark?.candidates||[]){
        if(w.platform==='Telegram')continue
        const token=(w.token||w.text||'').replace(/^@+/,'').trim()
        if(token&&token.length>=2&&token.length<=60&&!/[\\/]/.test(token)&&!map.has(token))map.set(token,w.platform?`${w.platform}水印`:'水印线索')
      }
      for(const f of t.data.folder||[])if(f.name.trim()&&!map.has(f.name.trim()))map.set(f.name.trim(),'目录线索')
    }
    return [...map.entries()].slice(0,6).map(([name,source])=>({name,source}))
  })():[]
  const selectionPreviewTask=selectionPreviewVid?chosen.find(t=>t.vid===selectionPreviewVid)||null:null
  const changeQuery=(value:string)=>{if(!group&&data?.group)setGroup(data.group.key);updateFilters({q:value,page:'0',vid:''})}
  const chooseGroup=(key:string)=>{setGroup(key);setPage(0);setLinkFocus('');setPreviewOpen(false)}
  const toggle=(tasks:ProcessingTask[])=>{setSelections(current=>({...current,[scope]:toggleReviewSelection(current[scope]||[],tasks)}));setWatermarkKey('')}
  const clear=()=>setSelections(current=>({...current,[scope]:[]}))
  const inspect=(task:ProcessingTask)=>{setReference(null);if(!previewOpen){returnFocus.current=document.activeElement as HTMLElement;setInstantEntry(!!document.activeElement?.matches(':focus-visible')||matchMedia('(prefers-reduced-motion: reduce)').matches)}setInspected(current=>({...current,[scope]:task}));setDetailMode('video');if(!wide)setPreviewOpen(true)}
  const openSelection=()=>{if(!previewOpen){returnFocus.current=document.activeElement as HTMLElement;setInstantEntry(!!document.activeElement?.matches(':focus-visible')||matchMedia('(prefers-reduced-motion: reduce)').matches)}if(!creator)setCreator(data?.group?.kind==='suggested'?data.group.label:'');setReference(null);setSubmitError('');setDetailMode('selection');if(!wide)setPreviewOpen(true);else requestAnimationFrame(()=>selectionHeading.current?.focus())}
  const removeCompleted=(vids:string[])=>{setSelections(current=>removeReviewSelection(current,vids));setInspected(current=>{const next=Object.fromEntries(Object.entries(current).filter(([,task])=>!vids.includes(task.vid)));if(current[scope]&&vids.includes(current[scope].vid)){const index=rows.findIndex(t=>t.vid===current[scope].vid),nearby=[...rows.slice(index+1),...rows.slice(0,index).reverse()].find(t=>!vids.includes(t.vid));if(nearby)next[scope]=nearby}return next})}
  const action=async(action:string,vids:string[],note?:string)=>{
    setBusy(true);setSubmitError('')
    try{
      await mutate('/api/review',{action,vids,note,review_tokens:evidenceTokens(vids),review_session:session.current,review_group:{key:activeGroup,deferred:view==='deferred'}})
      if(action==='note'){
        for(const id of vids)noteBaseline.current[id]=note||''
        setNotes(current=>Object.fromEntries(Object.entries(current).filter(([id])=>!vids.includes(id))))
        setInspected(current=>Object.fromEntries(Object.entries(current).map(([key,t])=>[key,vids.includes(t.vid)?{...t,note:note||''}:t])))
        // Inspecting a selected item from another page must renew its manual-state token
        // without navigating or replacing the visible page.
        try{
          const renewed=(await Promise.all(vids.filter(id=>!rows.some(t=>t.vid===id)).map(async vid=>{
            const page=await api<Data>('/api/review?'+new URLSearchParams({session:session.current||'',view,vid}))
            return page.rows.find(t=>t.vid===vid)
          }))).filter((t):t is ProcessingTask=>!!t)
          const replace=(t:ProcessingTask)=>renewed.find(r=>r.vid===t.vid)||t
          setSelections(current=>Object.fromEntries(Object.entries(current).map(([key,tasks])=>[key,tasks.map(replace)])))
          setInspected(current=>Object.fromEntries(Object.entries(current).map(([key,t])=>[key,replace(t)])))
        }catch{setSubmitError('备注已保存；核对凭据暂未刷新，请重新打开该视频后继续。')}
      }else{removeCompleted(vids);setPreviewOpen(false);setDetailMode('video');setLinkFocus('')}
      await load({preservePosition:true})
      toast.success({note:'备注已保存',defer:'已移至暂缓',restore:'已恢复核对'}[action]||'已更新')
    }catch(e:any){setSubmitError(e.message);if(/核对已过期|核对会话|本次核对已过期/.test(e.message))setSessionExpired(true);toast.error(e.message)}finally{setBusy(false)}
  }
  const sourceOnly=async(key:string)=>{
    setBusy(true)
    try{await api('/api/watermarks',{method:'PUT',body:JSON.stringify({key,kind:'source'})});setPreviewOpen(false);setSelections({});setDetailMode('video');await load({preservePosition:true});toast.success('已标为来源，相关人物建议将重新分析');return true}
    catch(e:any){setSubmitError(e.message);if(/核对已过期|核对会话|本次核对已过期/.test(e.message))setSessionExpired(true);toast.error(e.message);return false}finally{setBusy(false)}
  }
  const confirm=async()=>{
    if(!session.current||!chosen.length||!creator.trim()||busy||sessionExpired||chosen.some(t=>blockedIds.has(t.vid)))return
    setBusy(true);setSubmitError('')
    const vids=chosen.map(t=>t.vid),name=creator.trim(),confirmationSession=session.current
    try{const result=await mutate<{vids?:string[];rule_warning?:string}>('/api/plan/queue',{vids,creator:name,review_tokens:evidenceTokens(vids),review_session:confirmationSession,source:'review',source_id:activeGroup,review_group:{key:activeGroup,deferred:view==='deferred'},watermark_key:watermarkKey||undefined,fingerprints:Object.fromEntries(chosen.map(t=>[t.vid,{path:t.path,size:t.size,mtime_ns:t.mtime_ns}]))});const completed=result.vids||vids;setReceipt({vids:completed,creator:name,session:confirmationSession});removeCompleted(completed);setDrafts(current=>({...current,[scope]:{creator:'',watermarkKey:''}}));setPreviewOpen(false);setDetailMode('video');setLinkFocus('');await load({preservePosition:true});await loadPeople();result.rule_warning?toast.warning(result.rule_warning):toast.success('已加入待归档')}
    catch(e:any){setSubmitError(e.message);if(/核对已过期|核对会话|本次核对已过期/.test(e.message))setSessionExpired(true);toast.error(e.message)}finally{setBusy(false)}
  }
  const undo=async()=>{if(!receipt)return;setBusy(true);try{const result=await mutate<{warning?:string}>('/api/plan/queue',{vids:receipt.vids,review_session:receipt.session},'DELETE');setReceipt(null);await load({preservePosition:true});result.warning?toast.warning(result.warning):toast.success('已撤销本次归名')}catch(e:any){toast.error(e.message)}finally{setBusy(false)}}
  const openReference=(name:string)=>{setReference({name,returnTo:detailMode,scroll:document.querySelector('.ws-detail-content')?.scrollTop||0});requestAnimationFrame(()=>selectionHeading.current?.focus())}
  const backReference=()=>{const previous=reference;setReference(null);if(previous)requestAnimationFrame(()=>{const area=document.querySelector('.ws-detail-content');if(area)area.scrollTop=previous.scroll;const trigger=[...document.querySelectorAll<HTMLButtonElement>('.ws-detail-content button')].find(b=>b.getAttribute('aria-label')===`看 ${previous.name} 的对照视频`);trigger?.focus({preventScroll:true})})}
  const detail=<>

    <div className="ws-detail-heading"><h2 ref={selectionHeading} tabIndex={-1}>{reference?`${reference.name} · 对照视频`:detailMode==='selection'?<>核对所选 · <DigitSwap value={chosen.length}/></>:'视频与证据'}</h2>{reference?<Button variant="ghost" onClick={backReference}><ArrowLeft size={14}/>返回{reference.returnTo==='selection'?'人物选择':'证据'}</Button>:detailMode==='selection'&&<Button variant="ghost" size="sm" onClick={()=>setDetailMode('video')}><ArrowLeft size={14}/>返回证据</Button>}{wide&&!reference&&detailMode==='video'&&focused&&<span className="ws-muted">{rows.findIndex(t=>t.vid===focused.vid)>=0?`${rows.findIndex(t=>t.vid===focused.vid)+1} / ${rows.length}`:'已查看'}</span>}</div>
    <div className="ws-detail-content" data-sheet-scroll>
      {reference?<ReferencePanel name={reference.name}/>:detailMode==='video'?(focused?<EvidencePanel key={focused.vid} task={focused} note={notes[focused.vid]??focused.note} onNoteChange={note=>{if(noteBaseline.current[focused.vid]===undefined)noteBaseline.current[focused.vid]=focused.note;setNotes(current=>{const next={...current};if(note===focused.note)delete next[focused.vid];else next[focused.vid]=note;return next})}} onSaveNote={()=>action('note',[focused.vid],notes[focused.vid]??focused.note)} onSource={sourceOnly} busy={busy} creator={creator} onChoose={setCreator} onReference={openReference}/>:<p className="ws-empty">选择视频查看证据</p>):<div className="ws-decision">
        <div>
          <p className="mb-2 text-xs text-muted-foreground">{data?.group?.label||'当前确认组'} · <DigitSwap value={chosen.length}/> 个视频{hiddenCount>0?`，其中 ${hiddenCount} 个不在当前页`:''}</p>
          <Collapsible open={Boolean(selectionPreviewTask)}>
            <CollapsibleContent>
              {selectionPreviewTask&&<Card size="compact" variant="muted" className="ws-confirm-preview" role="region" aria-label={`原位预览 ${selectionPreviewTask.name}`}>
                <div className="ws-confirm-preview-head">
                  <strong title={selectionPreviewTask.name}>{selectionPreviewTask.name}</strong>
                  <div className="ws-confirm-preview-actions">
                    <Button variant="ghost" size="sm" onClick={()=>inspect(selectionPreviewTask)}>完整证据</Button>
                    <Button variant="ghost" size="icon" aria-label="收起原位预览" title="收起预览" onClick={()=>setSelectionPreviewVid(null)}><X size={14}/></Button>
                  </div>
                </div>
                <WorkspaceMedia key={selectionPreviewTask.vid} path={selectionPreviewTask.path} name={selectionPreviewTask.name} autoPlay/>
              </Card>}
            </CollapsibleContent>
          </Collapsible>
          <div className="ws-confirm-list" aria-label="本次完整归名范围">{chosen.map(t=>{
            const summary=reviewListSummary(t),faceThumbs=t.data.face?.thumbs?.slice(0,3)||[],isPreviewing=selectionPreviewTask?.vid===t.vid
            return <div key={t.vid} className="ws-confirm-file" data-active={isPreviewing||undefined}>
              <button type="button" className="ws-confirm-thumb" aria-label={`原位播放 ${t.name}`} title="原位播放预览" onClick={()=>{setInspected(current=>({...current,[scope]:t}));setSelectionPreviewVid(prev=>prev===t.vid?null:t.vid)}}>
                <VideoCover video={{path:t.path,thumb:faceThumbs[0]?`/thumb?p=${encodeURIComponent(faceThumbs[0])}`:null,size:t.size,mtime_ns:t.mtime_ns}} sampleTime={summary.sampleTime}/>
                <span className="ws-confirm-thumb-play"><Play size={11}/></span>
              </button>
              <div className="ws-confirm-body">
                <div className="ws-confirm-top">
                  <Button variant="ghost" className="ws-confirm-name" onClick={()=>{setInspected(current=>({...current,[scope]:t}));setSelectionPreviewVid(prev=>prev===t.vid?null:t.vid)}} aria-label={`核对所选 ${t.name}`}><Play size={12}/><span>{t.name}</span></Button>
                  {faceThumbs.length>0&&<AvatarGroup size="sm" max={3} label="检出人脸" className="ws-confirm-faces" members={faceThumbs.map((path,idx)=>({id:`${t.vid}-${idx}`,name:`${t.name} 人脸 ${idx+1}`,src:`/thumb?p=${encodeURIComponent(path)}`}))}/>}
                  <Button className="ws-icon" variant="ghost" size="sm" aria-label={`取消选择 ${t.name}`} title="取消选择" disabled={busy} onClick={()=>toggle([t])}><X size={14}/></Button>
                </div>
                <WorkspacePath value={t.path} compact/>
              </div>
            </div>
          })}</div>
        </div>
        <PersonPicker search={pickerSearch[scope]||''} onSearchChange={value=>setPickerSearch(current=>({...current,[scope]:value}))} onReference={openReference} compact people={people} suggestions={selectionSuggestions} sampleFaces={selectionSampleFaces} nameHints={selectionNameHints} value={creator} onChange={setCreator} disabled={busy} error={peopleError} onRetry={loadPeople}/>
        {commonMarks.length>0&&<WorkspaceField label="共同账号规则"><WorkspaceSelect label="保存共同账号规则" value={watermarkKey} disabled={busy} onChange={setWatermarkKey} options={[{value:'',label:'仅归名，不保存账号规则'},...commonMarks.map(c=>({value:c.key,label:`${c.platform} · ${c.token}`}))]}/></WorkspaceField>}
        <p className="text-xs text-muted-foreground">确认后加入待归档，文件暂不移动。</p>
      </div>}
      {chosen.some(t=>blockedIds.has(t.vid))&&<p role="alert" className="ws-error">部分已选项的文件或规则已变化，请更新列表后重新核对。</p>}
      {focused?.review_blocked&&detailMode==='video'&&<p role="alert" className="ws-error">{focused.review_blocked}</p>}
      {submitError&&<p role="alert" className="ws-error">{submitError}</p>}
    </div>
    <div className="ws-detail-actions">{reference?<><Button variant="outline" disabled={busy} onClick={backReference}>返回{reference.returnTo==='selection'?'人物选择':'证据'}</Button><Button disabled={busy} onClick={()=>{setCreator(reference.name);backReference()}}>选择 {reference.name}</Button></>:detailMode==='selection'?<Button disabled={busy||!chosen.length||!creator.trim()||sessionExpired||chosen.some(t=>blockedIds.has(t.vid))} onClick={confirm}>{busy?'正在保存…':<>归给 {creator.trim()||'所选人物'} · <DigitSwap value={chosen.length}/></>}</Button>:<><Button variant="outline" disabled={!focused||busy||!!focused?.review_blocked&&!selected.has(focused.vid)} onClick={()=>focused&&toggle([focused])}>{focused&&selected.has(focused.vid)?'取消选择此视频':'选择此视频'}</Button>{chosen.length>0&&<Button disabled={busy} onClick={openSelection}>核对所选 · <DigitSwap value={chosen.length}/></Button>}</>}</div>
  </>
  return <main className="ws-page" aria-label="人工确认工作区" data-review-session={data?.session}>
    <header className="ws-header"><h1>人工确认</h1><div className="ws-actions"><NavigationButton variant="ghost" size="sm" asChild><Link href="/operations"><span className="ws-link-label">分析中</span><span className="ws-count">{updates?.waiting??data?.waiting??'—'}</span><ChevronRight size={14}/></Link></NavigationButton><Button variant={updates?.has_updates||sessionExpired?"outline":"ghost"} aria-label="更新人工确认列表" title={updates?.refreshing?`后台正在更新 ${updates.refreshing} 条建议，当前列表保持不变`:'更新建议和分组'} disabled={busy||loading} onClick={requestRefresh}><RefreshCw size={15}/>{updates?.has_updates||sessionExpired?<span>有更新</span>:updates?.refreshing?<span className="ws-muted">后台更新中</span>:null}</Button></div></header>
    {receipt&&<div className="ws-receipt"><QueueReceipt count={receipt.vids.length} creator={receipt.creator} onUndo={undo} busy={busy}/></div>}
    {error&&<p role="alert" className="ws-error">{error}</p>}
    {sessionExpired&&!error&&<p role="status" className="ws-error">本次核对已过期，勾选和草稿仍保留。请更新列表后继续。</p>}
    {data?.focus_missing&&<p className="ws-error">链接中的视频尚未就绪或已归名。</p>}
    {(personScope||clusterScope)&&<div className="ws-toolbar"><span className="ws-muted">{personScope||`相似组 ${clusterScope}`}</span><Button size="sm" variant="ghost" onClick={()=>updateFilters({person:'',cluster:'',group:'',page:'0',vid:''})}>查看全部</Button></div>}
    <div className="ws-view-tabs"><WorkspaceTabs value={view} onChange={value=>{setView(value);setGroup('');setPage(0);setLinkFocus('')}} label="确认状态" disabled={busy} items={views.map(v=>({...v,count:data?.counts[v.value]}))}/></div>
    <div className="ws-workarea">
      <aside className="ws-groups" aria-label="确认组"><div className="ws-panel-heading"><h2>确认组</h2><span>{data?.groups.length??'—'}</span></div><div className="ws-group-list">{data?.groups.map(g=><button key={g.key} type="button" className="ws-group" disabled={busy} aria-pressed={activeGroup===g.key} onClick={()=>chooseGroup(g.key)}><span>{g.label}</span><span className="ws-count">{g.count}</span>{(selections[reviewSelectionKey(g.key,view==='deferred')]||[]).length>0&&<Check size={12} aria-label="有已选视频"/>}</button>)}</div></aside>
      <section className="ws-queue" aria-label="待确认视频">

        <div className="ws-toolbar"><WorkspaceSearch value={query} onChange={changeQuery} label="搜索当前组视频" placeholder="搜索文件名或路径…" disabled={busy}><Popover.Root><Popover.Trigger asChild><InputGroupButton size="icon-sm" disabled={busy} aria-label="分组与筛选" title="分组与筛选" data-active={!!folder}><SlidersHorizontal size={15}/>{folder&&<span className="size-1.5 rounded-full bg-current"/>}</InputGroupButton></Popover.Trigger><Popover.Portal><Popover.Content className="ws-filter-content" align="end" sideOffset={6}><div className="ws-mobile-filter"><WorkspaceField label="确认组"><WorkspaceSelect label="选择确认组" value={activeGroup} disabled={busy} onChange={chooseGroup} options={(data?.groups||[]).map(g=>({value:g.key,label:`${g.label} · ${g.count}`}))} placeholder="暂无确认组"/></WorkspaceField></div><WorkspaceField label="原文件夹"><WorkspaceSelect label="按原文件夹筛选" disabled={busy} value={folder} onChange={value=>{setFolder(value);if(!group&&data?.group)setGroup(data.group.key)}} options={[{value:'',label:'全部文件夹'},...(data?.folders||[]).map(f=>({value:f,label:f}))]}/></WorkspaceField>{folder&&<Button className="mt-3" variant="ghost" size="sm" onClick={()=>{setFolder('');setPage(0)}}>清除筛选</Button>}</Popover.Content></Popover.Portal></Popover.Root></WorkspaceSearch></div>
        <div className="ws-selection"><Checkbox className="ws-check" aria-label="选择当前页视频" motionPreset="instant" checked={rows.length>0&&rows.every(t=>selected.has(t.vid))} indeterminate={rows.some(t=>selected.has(t.vid))&&!rows.every(t=>selected.has(t.vid))} disabled={disabled||!rows.length} onCheckedChange={()=>toggle(rows)}/><div className="ws-selection-summary"><strong>{chosen.length?`已选 ${chosen.length}`:data?.group?.label||'待确认'}</strong><span> · {hiddenCount?`${hiddenCount} 个在其他页或被筛选隐藏`:data?`${data.remaining??data.total} 个待核对`:'读取中'}</span></div></div>
        <div ref={listScroll.ref} className="ws-list" aria-busy={loading||query!==search} onScroll={listScroll.onScroll}>
          {loading&&!data?<p role="status" className="ws-empty">加载视频…</p>:!rows.length?<div className="ws-empty"><p>{group&&!data?.group?'该组已处理完，选择另一组继续。':query||folder?'当前筛选下没有视频。':data?.remaining===0?'本组已完成，可选择其他组继续。':'本批已处理完，可继续下一批。'}</p>{(query||folder)&&<Button variant="ghost" onClick={()=>{changeQuery('');setFolder('')}}>清除筛选</Button>}</div>:rows.map(t=><ReviewSummaryRow key={t.vid} task={t} selected={selected.has(t.vid)} active={focused?.vid===t.vid} disabled={disabled||!!t.review_blocked&&!selected.has(t.vid)} onToggle={()=>toggle([t])} onInspect={()=>inspect(t)}/>)}
        </div>
        {chosen.length>0&&<div className="ws-bulk" aria-label="所选视频操作"><span className="ws-muted">已选 {chosen.length}</span>{chosen.length>0&&<Button className="ws-icon" variant="ghost" size="sm" title="清空当前组选择" aria-label="清空当前组选择" disabled={busy} onClick={clear}><X size={14}/></Button>}<Button variant="outline" size="sm" disabled={disabled||!chosen.length} onClick={()=>action(view==='deferred'?'restore':'defer',chosen.map(t=>t.vid))}>{view==='deferred'?'恢复':'暂缓'}</Button><Button size="sm" disabled={disabled||!chosen.length} onClick={openSelection}><ListChecks size={14}/>核对所选</Button></div>}
        <div className="ws-pagination"><span>{data?`${data.remaining??data.total} 个待核对 · 第 ${currentPage+1} / ${Math.max(1,Math.ceil(data.total/data.limit))} 批`:'读取中'}</span><div className="ws-actions"><Button className="ws-icon" variant="ghost" size="sm" aria-label="上一页" title="上一页" disabled={disabled||(data?.session?data.previous_page==null:currentPage===0)} onClick={()=>{setPage(data?.previous_page??currentPage-1);setLinkFocus('')}}><ChevronLeft size={15}/></Button><Button className="ws-icon" variant="ghost" size="sm" aria-label="下一页" title="下一页" disabled={disabled||!data||(data.session?data.next_page==null:(currentPage+1)*data.limit>=data.total)} onClick={()=>{setPage(data?.next_page??currentPage+1);setLinkFocus('')}}><ChevronRight size={15}/></Button></div></div>
      </section>
      {wide&&<aside className="ws-inspector" aria-label="视频与归名">{detail}</aside>}
    </div>
    {!wide&&<Dialog open={previewOpen} onOpenChange={open=>!busy&&setPreviewOpen(open)}><ResponsiveDialogContent open={previewOpen} className="ws-detail-sheet" style={{animationDuration:instantEntry?'0ms':'180ms'}} overlayProps={{style:{animationDuration:instantEntry?'0ms':'140ms'}}} inert={!previewOpen} onCloseAutoFocus={e=>{e.preventDefault();const target=returnFocus.current;if(target?.isConnected&&!target.matches(':disabled')&&!target.closest('[inert]'))target.focus();else document.querySelector<HTMLButtonElement>('[aria-label="更新人工确认列表"]')?.focus()}} onEscapeKeyDown={e=>{if(busy)e.preventDefault();else if(reference){e.preventDefault();backReference()}}} onInteractOutside={e=>busy&&e.preventDefault()}><DialogTitle className="sr-only">{reference?`${reference.name} 的对照视频`:detailMode==='selection'?'核对所选视频':'视频与证据'}</DialogTitle><DialogDescription className="sr-only">关闭后保留筛选、勾选与列表位置。</DialogDescription>{detail}</ResponsiveDialogContent></Dialog>}
    <AlertDialog open={refreshConfirm} onOpenChange={setRefreshConfirm}><AlertDialogContent onCloseAutoFocus={e=>{e.preventDefault();refreshFocus.current?.focus({preventScroll:true})}}><AlertDialogHeader><AlertDialogTitle>更新当前核对列表？</AlertDialogTitle><AlertDialogDescription>后台的新建议可能调整分组。更新会清空勾选、未保存的备注和人物输入。取消可继续当前核对。</AlertDialogDescription></AlertDialogHeader><AlertDialogFooter><AlertDialogCancel>继续当前核对</AlertDialogCancel><AlertDialogAction onClick={()=>void load({refresh:true})}>更新列表</AlertDialogAction></AlertDialogFooter></AlertDialogContent></AlertDialog>
  </main>
}
