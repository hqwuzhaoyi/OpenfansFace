"use client"

import Link from 'next/link'
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { Check, ChevronLeft, ChevronRight, ChevronsLeft, ChevronsRight, Film, Maximize2, Play, RefreshCw, Search, SlidersHorizontal, Sparkles, Trash2, X, ArrowLeft, ListChecks } from 'lucide-react'
import * as Popover from '@radix-ui/react-popover'
import { workspaceToast as toast } from '@/components/workbench/feedback'
import { api } from '@/lib/face-api'
import type { ProcessingTask } from '@/lib/processing-model'
import { reviewSelectionKey, toggleReviewSelection, removeReviewSelection, type ReviewSelections } from '@/lib/review-selection'
import { PersonPicker, type PersonChoice, type PersonNameHint } from '@/components/review/person-picker'
import { VideoCover } from '@/components/review/video-cover'
import { Card } from '@/components/beui/motion/card'
import { AnimatedBadge } from '@/components/beui/motion/animated-badge'
import { Collapsible, CollapsibleContent } from '@/components/beui/motion/collapsible'
import { AvatarGroup } from '@/components/beui/motion/avatar-group'
import { DigitSwap } from '@/components/beui/motion/digit-swap'
import { reviewListSummary } from '@/lib/review-list-summary'
import { QueueReceipt } from '@/components/review/workflow'
import { ResponsiveDialogContent } from '@/components/review/responsive-dialog'
import { Dialog, DialogTitle, DialogDescription } from '@/components/ui/dialog'
import { Button as NavigationButton } from '@/components/ui/button'
import { WorkspaceButton as Button, WorkspaceSelect, WorkspaceField, WorkspaceCard, WorkspacePath, WorkspaceMedia, getMediaPosition, rememberMediaPosition } from '@/components/workbench/inner'
import { EvidencePanel } from '@/components/review/evidence-panel'
import { ReferencePanel, usePersonReferences } from '@/components/review/reference-panel'
import { useWorkspaceQuery, useWorkspaceScroll, readWorkbenchCache, readPersistedWorkbenchCache, writeWorkbenchCache, deleteWorkbenchCache } from '@/components/workbench/state'
import { applyPendingReviewMutations, hasProtectedReviewSelections, pruneSettledReviewMutations, resolveEmptyReviewPageTarget, reviewTaskBucket, shouldAutoRecoverExpiredReviewSession, shouldDiscardReviewRefreshResponse, type PendingReviewMutation } from '@/lib/review-continuity-model'
import { requestVideoPoster } from '@/lib/video-poster'
import { InputGroupButton } from '@/components/ui/input-group'
import { Checkbox } from '@/components/beui/motion/checkbox'
import { useReviewActivity } from '@/components/workbench/review-activity'
import { useBackgroundOperations } from '@/components/workbench/background-operations'
import { isAutoMergeAction, isDestructivePersonAction, lockBadgeStatus } from '@/lib/locks-model'
import { AlertDialog,AlertDialogContent,AlertDialogHeader,AlertDialogTitle,AlertDialogDescription,AlertDialogFooter,AlertDialogCancel,AlertDialogAction } from '@/components/ui/alert-dialog'
import { ReviewSummaryRow } from '@/components/review/review-summary-row'
import { AutoMergeDialog } from '@/components/review/auto-merge-dialog'
import { ReviewDeleteDialog } from '@/components/review/review-delete-dialog'
import type { PersonFilePlan } from '@/lib/person-file-model'
import { fetchAutoMergePreview, type AutoWatermarkPreview } from '@/lib/archive-actions'
import { VideoPreviewDialog } from '@/components/workbench/video-preview-dialog'
import { ReviewQueueSkeleton, StageMediaLoader } from '@/components/workbench/loading-skeletons'
import { Skeleton } from '@/components/ui/skeleton'
import { WorkspaceSearch, WorkspaceTabs, useReviewDesktop } from '@/components/workbench/controls'
import { WorkbenchHeaderActions, useSyncWorkbenchBreadcrumb, type WorkbenchBreadcrumbSegment } from '@/components/workbench/breadcrumb'

type Group={key:string;label:string;kind:string;count:number}
type Data={has_updates?:boolean;session?:string;session_at?:number;expires_at?:number;remaining?:number;next_page?:number|null;previous_page?:number|null;blocked?:{vid:string;message:string}[];counts:Record<string,number>;groups:Group[];group:Group|null;total:number;rows:ProcessingTask[];page:number;limit:number;waiting:number;online:boolean;folders:string[];focus_missing:boolean}
const views=[{value:'all',label:'全部'},{value:'suggested',label:'有候选'},{value:'conflict',label:'证据冲突'},{value:'none',label:'无候选'},{value:'manual',label:'人工接管'},{value:'deferred',label:'暂缓'}]

const REVIEW_CACHE_TTL_MS=5*60*1000
const uuid=()=>{if(typeof crypto!=='undefined'&&typeof crypto.randomUUID==='function')return crypto.randomUUID();const bytes=crypto.getRandomValues(new Uint8Array(16));bytes[6]=(bytes[6]&15)|64;bytes[8]=(bytes[8]&63)|128;const hex=[...bytes].map(b=>b.toString(16).padStart(2,'0')).join('');return `${hex.slice(0,8)}-${hex.slice(8,12)}-${hex.slice(12,16)}-${hex.slice(16,20)}-${hex.slice(20)}`}

export default function Review() {
  const {trackReviewMutation,enqueuePersonFile,subscribe,lockedVids,lockedPeople,tasks,serverActiveOps}=useBackgroundOperations()
  const lockedVidsRef=useRef(lockedVids)
  lockedVidsRef.current=lockedVids
  const lockedPeopleRef=useRef(lockedPeople)
  lockedPeopleRef.current=lockedPeople
  const localReviewOpIdsRef=useRef(new Set<string>())
  const {values:filters,ready:initialized,update:updateFilters}=useWorkspaceQuery({view:'all',group:'',q:'',folder:'',page:'0',person:'',cluster:'',vid:''},{view:views.map(v=>v.value)})
  const {view,group,q:query,folder,person:personScope,cluster:clusterScope,vid:linkFocus}=filters,page=Number(filters.page)
  const [rawData,setRawData]=useState<{payload:Data;fetchedAtSeq:number}|null>(null),[error,setError]=useState(''),[loading,setLoading]=useState(true),[busy,setBusy]=useState(false)
  const [pendingOps,setPendingOps]=useState<PendingReviewMutation[]>([])
  const pendingOpsRef=useRef<PendingReviewMutation[]>([])
  const syncSeqRef=useRef(0)
  const pendingVids=useMemo(()=>new Set(pendingOps.flatMap(op=>op.vids)),[pendingOps])
  const data=useMemo(()=>rawData?applyPendingReviewMutations(rawData.payload,rawData.fetchedAtSeq,pendingOps,view):null,[rawData,pendingOps,view])
  const pendingVidsRef=useRef(new Set<string>()),inFlightCountRef=useRef(0),initializedScopesRef=useRef(new Set<string>()),forceSelectAllScopeRef=useRef<string|null>(null),advanceSequenceRef=useRef(0)
  pendingVidsRef.current=pendingVids
  const [search,setSearch]=useState('')
  const setView=(view:string)=>{advanceSequenceRef.current++;updateFilters({view,group:'',page:'0',vid:''})}
  const setGroup=(group:string)=>updateFilters({group})
  const setFolder=(folder:string)=>{advanceSequenceRef.current++;updateFilters({folder,page:'0',vid:''})}
  const setPage=(page:number)=>updateFilters({page:String(page)})
  const setLinkFocus=(vid:string)=>updateFilters({vid})
  const [selections,setSelections]=useState<ReviewSelections>({}),[inspected,setInspected]=useState<Record<string,ProcessingTask>>({})
  const [autoSelectedScopes,setAutoSelectedScopes]=useState<Set<string>>(()=>new Set())
  const autoSelectedScopesRef=useRef(autoSelectedScopes)
  autoSelectedScopesRef.current=autoSelectedScopes
  const selectionsRef=useRef(selections)
  selectionsRef.current=selections
  const markScopeManual=useCallback((targetScope:string)=>{
    setAutoSelectedScopes(prev=>{
      if(!prev.has(targetScope))return prev
      const next=new Set(prev)
      next.delete(targetScope)
      autoSelectedScopesRef.current=next
      return next
    })
  },[])
  const [instantEntry,setInstantEntry]=useState(false)
  const [previewOpen,setPreviewOpen]=useState(false),[detailMode,setDetailMode]=useState<'video'|'selection'>('video')
  const [drafts,setDrafts]=useState<Record<string,{creator?:string;watermarkKey:string}>>({}),[notes,setNotes]=useState<Record<string,string>>({}),[submitError,setSubmitError]=useState('')
  const [pickerSearch,setPickerSearch]=useState<Record<string,string>>({})
  const [selectionPreviewVid,setSelectionPreviewVid]=useState<string|null>(null)
  const [reference,setReference]=useState<{name:string;returnTo:'video'|'selection';scroll:number}|null>(null)
  const [people,setPeople]=useState<PersonChoice[]>(()=>readWorkbenchCache<PersonChoice[]>('review:people',REVIEW_CACHE_TTL_MS)??[]),[peopleError,setPeopleError]=useState(''),[receipt,setReceipt]=useState<{vids:string[];creator:string;session:string}|null>(null)
  const pendingMutations=useRef(new Map<string,string>())
  const mutate=async<T=any>(path:string,body:Record<string,unknown>,method='POST',explicitId?:string):Promise<T>=>{const key=JSON.stringify([path,method,body]),id=explicitId||pendingMutations.current.get(key)||uuid();pendingMutations.current.set(key,id);localReviewOpIdsRef.current.add(id);if(localReviewOpIdsRef.current.size>256){const oldest=localReviewOpIdsRef.current.values().next().value;if(oldest!==undefined)localReviewOpIdsRef.current.delete(oldest)};try{const result=await api<T>(path,{method,body:JSON.stringify({...body,operation_id:id})});pendingMutations.current.delete(key);return result}catch(error:any){if(!error.resultUnknown)pendingMutations.current.delete(key);else error.message+=`（操作 ${id}，重试将查询同一回执）`;throw error}}
  const refreshFocus=useRef<HTMLElement|null>(null)
  const returnFocus=useRef<HTMLElement|null>(null),session=useRef<string|undefined>(undefined),pendingSessionResetRef=useRef(false),noteBaseline=useRef<Record<string,string>>({}),notesRef=useRef(notes),polling=useRef(false),scrollAnchor=useRef<{vid:string;offset:number}[]|null>(null)
  notesRef.current=notes
  const [updates,setUpdates]=useState<{has_updates:boolean;refreshing:number;blocked:number;waiting?:number}|null>(null),[refreshConfirm,setRefreshConfirm]=useState(false),[sessionExpired,setSessionExpired]=useState(false)
  const [autoMergePreview,setAutoMergePreview]=useState<AutoWatermarkPreview|null>(null),[autoMergeOpen,setAutoMergeOpen]=useState(false),[autoMergeLoading,setAutoMergeLoading]=useState(false)
  const autoMergeTriggerRef=useRef<HTMLButtonElement>(null),autoMergeSeqRef=useRef(0)
  const [deleteDialog,setDeleteDialog]=useState<{open:boolean;mode:'single'|'batch';targets:ProcessingTask[]}|null>(null)
  const deleteTriggerRef=useRef<HTMLElement|null>(null)
  const pendingDeleteTasksRef=useRef(new Map<string,{pendingId:string;vids:string[]}>())
  const loadAutoMergePreview=useCallback(async()=>{const seq=++autoMergeSeqRef.current;setAutoMergeLoading(true);try{const next=await fetchAutoMergePreview();if(seq===autoMergeSeqRef.current)setAutoMergePreview(next)}catch{}finally{if(seq===autoMergeSeqRef.current)setAutoMergeLoading(false)}},[])
  const [groupsCollapsed,setGroupsCollapsed]=useState(false)
  const groupsManualRef=useRef(false)
  const [listWidth,setListWidth]=useState(356)
  const [listBounds,setListBounds]=useState<{min:number;max:number}>({min:280,max:500})
  const [resizing,setResizing]=useState(false)
  const workareaRef=useRef<HTMLDivElement>(null),queueRef=useRef<HTMLElement>(null),stageExpandBtnRef=useRef<HTMLButtonElement>(null)
  const [mobileSearchOpen,setMobileSearchOpen]=useState(false)
  const mobileSearchTriggerRef=useRef<HTMLButtonElement>(null),mobileToolbarRef=useRef<HTMLDivElement>(null)
  const [stageOpen,setStageOpen]=useState(true)
  const [stageTab,setStageTab]=useState<'current'|'reference'>('current')
  const [explicitStageRefName,setExplicitStageRefName]=useState<string|null>(null)
  const [stageRefIndex,setStageRefIndex]=useState(0)
  const [lightbox,setLightbox]=useState<{open:boolean;tab:'current'|'reference';initialTime:number}>({open:false,tab:'current',initialTime:0})
  const lightboxTriggerRef=useRef<HTMLElement|null>(null)
  const reviewDesktop=useReviewDesktop(),sequence=useRef(0),selectionHeading=useRef<HTMLHeadingElement>(null)
  const hasManualSelections=hasProtectedReviewSelections(selections,autoSelectedScopes),hasDrafts=Object.values(drafts).some(d=>!!d.creator||!!d.watermarkKey)||Object.entries(notes).some(([id,note])=>note!==noteBaseline.current[id])
  const hasManualSelectionsRef=useRef(hasManualSelections)
  hasManualSelectionsRef.current=hasManualSelections
  const hasDraftsRef=useRef(hasDrafts)
  hasDraftsRef.current=hasDrafts
  const protectedWork=busy||pendingVids.size>0||loading||hasManualSelections||hasDrafts||previewOpen||lightbox.open||autoMergeOpen||Boolean(deleteDialog?.open)||!!reference||(reviewDesktop&&stageOpen&&stageTab==='reference')||refreshConfirm||detailMode==='selection'
  const protectedWorkRef=useRef(protectedWork)
  protectedWorkRef.current=protectedWork
  const {version:activityVersion,canApply:canAutoApply,markAutoRefreshed}=useReviewActivity(protectedWork)
  const canAutoApplyRef=useRef(canAutoApply)
  canAutoApplyRef.current=canAutoApply
  const rawDataRef=useRef(rawData)
  rawDataRef.current=rawData
  const syncedQueryKeyRef=useRef<string|null>(null)
  const queryFetchKey=JSON.stringify([view,group,search,folder,page,linkFocus,personScope,clusterScope])
  const isSessionExpiredError=(message?:string)=>/核对已过期|核对会话|本次核对已过期/.test(message||'')
  const clearReviewSessionCaches=useCallback((resetRef=true)=>{if(resetRef)session.current=undefined;deleteWorkbenchCache('review:session');deleteWorkbenchCache('review:page:');deleteWorkbenchCache('review:lastData')},[])

  const activeGroup=group||data?.group?.key||'',groupMatched=!group||data?.group?.key===group,scope=reviewSelectionKey(activeGroup,view==='deferred'),chosen=selections[scope]||[],rows=(groupMatched?data?.rows||[]:[]).filter(r=>!pendingVids.has(r.vid))
  const activeGroupInfo=data?.groups.find(g=>g.key===activeGroup)??(groupMatched?data?.group??null:null)
  const activeGroupRemaining=groupMatched?(data?(data.remaining??data.total):null):(activeGroupInfo?.count??null)
  const blockedIds=new Set((data?.blocked||[]).map(item=>item.vid))
  const selected=new Set(chosen.map(t=>t.vid)),originalFocus=(inspected[scope]&&!pendingVids.has(inspected[scope].vid)?inspected[scope]:undefined)||rows.find(t=>t.vid===linkFocus)||rows[0],focused=originalFocus?{...originalFocus,review_blocked:data?.blocked?.find(item=>item.vid===originalFocus.vid)?.message}:undefined,hiddenCount=chosen.filter(t=>!rows.some(r=>r.vid===t.vid)).length
  const actionTargets=chosen.length>0?chosen:(focused?[focused]:[])
  const preferredTaskCreator=(task?:ProcessingTask)=>task?.data.suggested||task?.data.candidates?.find(c=>c.sources?.includes('watermark'))?.name||(task?.data.candidates?.length===1?task.data.candidates[0].name:undefined)
  const defaultCreator=data?.group?.kind==='suggested'?data.group.label:(preferredTaskCreator(focused)||'')
  const creator=drafts[scope]?.creator!==undefined?drafts[scope].creator!:defaultCreator,watermarkKey=drafts[scope]?.watermarkKey||''
  const setCreator=(creator:string)=>setDrafts(current=>({...current,[scope]:{...current[scope],creator,watermarkKey:''}}))
  const setWatermarkKey=(watermarkKey:string)=>setDrafts(current=>({...current,[scope]:{creator:current[scope]?.creator,watermarkKey}}))
  const disabled=loading||query!==search||!!error||sessionExpired
  const currentPage=data?.page??page
  const activeGroupRef=useRef(activeGroup)
  activeGroupRef.current=activeGroup
  const currentPageRef=useRef(currentPage)
  currentPageRef.current=currentPage
  const listKey=JSON.stringify([scope,search,folder,currentPage])
  const listScroll=useWorkspaceScroll('review:'+listKey,!!data&&groupMatched&&!loading&&query===search)
  useEffect(()=>{clearReviewSessionCaches(true)},[clearReviewSessionCaches])
  useEffect(()=>{const timer=setTimeout(()=>{setSearch(query)},250);return()=>clearTimeout(timer)},[query])
  const findNextAvailableGroup=useCallback((groupsList:Group[],currentKey:string)=>{
    const idx=groupsList.findIndex(g=>g.key===currentKey)
    return (idx>=0?groupsList.slice(idx+1).find(g=>g.key!==currentKey&&g.count>0)||groupsList.slice(0,idx).find(g=>g.key!==currentKey&&g.count>0):groupsList.find(g=>g.key!==currentKey&&g.count>0))||null
  },[])
  const applyGroupDefaultSelection=useCallback((pageData:Data)=>{
    const targetGroupKey=pageData.group?.key||group
    if(!targetGroupKey)return
    const targetScope=reviewSelectionKey(targetGroupKey,view==='deferred')
    const pageScopeKey=`${targetScope}:${pageData.page??page}`
    const visible=pageData.rows.filter(r=>!pendingVidsRef.current.has(r.vid))
    if(!visible.length)return
    const shouldSelectAll=forceSelectAllScopeRef.current===targetScope||(!initializedScopesRef.current.has(pageScopeKey)&&(pageData.group?.kind==='suggested'||view==='suggested'))
    if(!shouldSelectAll)return
    initializedScopesRef.current.add(pageScopeKey)
    if(forceSelectAllScopeRef.current===targetScope)forceSelectAllScopeRef.current=null
    const pageBlocked=new Set((pageData.blocked||[]).map(item=>item.vid))
    const selectable=visible.filter(r=>!pageBlocked.has(r.vid)&&!lockedVidsRef.current.has(r.vid))
    const first=selectable[0]||visible[0]
    setInspected(current=>({...current,[targetScope]:first}))
    if(selectable.length>0){
      if(selectionsRef.current[targetScope]?.length&&!autoSelectedScopesRef.current.has(targetScope))return
      setAutoSelectedScopes(prev=>{
        const next=new Set(prev)
        next.add(targetScope)
        autoSelectedScopesRef.current=next
        return next
      })
      setSelections(current=>{
        if(current[targetScope]?.length&&!autoSelectedScopesRef.current.has(targetScope))return current
        return {...current,[targetScope]:selectable}
      })
    }
  },[group,page,view])
  const load=useCallback(async(options:{refresh?:boolean;automatic?:boolean;preservePosition?:boolean;silent?:boolean;recovered?:boolean}={}):Promise<Data|null>=>{
    if(!initialized)return null
    if(options.preservePosition&&listScroll.ref.current){const area=listScroll.ref.current,top=area.getBoundingClientRect().top;scrollAnchor.current=[...area.querySelectorAll<HTMLElement>('[data-vid]')].filter(e=>e.getBoundingClientRect().bottom>top).map(e=>({vid:e.dataset.vid!,offset:e.getBoundingClientRect().top-top}))}
    const token=++sequence.current,activityAtStart=activityVersion(),startedAtSeq=syncSeqRef.current
    if(!options.automatic&&!options.silent&&(options.refresh||!rawDataRef.current||!!group&&rawDataRef.current.payload.group?.key!==group))setLoading(true)
    try{
      const replacedSession=session.current
      const shouldCreateFresh=Boolean(options.refresh)||!replacedSession||(pendingSessionResetRef.current&&!hasManualSelectionsRef.current&&!hasDraftsRef.current&&inFlightCountRef.current===0)
      if(shouldCreateFresh)pendingSessionResetRef.current=false
      const params=new URLSearchParams({view,group,q:search,folder,page:String(page),vid:linkFocus,person:personScope,cluster:clusterScope,...(!shouldCreateFresh&&replacedSession?{session:replacedSession}:{snapshot:'1',...(replacedSession?{replace_session:replacedSession}:{})})})
      const next=await api<Data>('/api/review?'+params)
      if(token!==sequence.current)return null
      if(shouldDiscardReviewRefreshResponse({automatic:options.automatic,refresh:options.refresh,recovered:options.recovered,canAutoApply:canAutoApply(activityAtStart),activityChanged:activityAtStart!==activityVersion()})){setUpdates({has_updates:true,refreshing:0,blocked:0});return null}
      if(options.automatic)markAutoRefreshed(activityAtStart)
      session.current=next.session
      if(shouldCreateFresh||options.preservePosition||options.silent){deleteWorkbenchCache('review:page:');deleteWorkbenchCache('review:lastData')}
      const remainingPending=pruneSettledReviewMutations(pendingOpsRef.current,startedAtSeq)
      pendingOpsRef.current=remainingPending
      setPendingOps(remainingPending)
      setRawData({payload:next,fetchedAtSeq:startedAtSeq})
      const projectedNext=applyPendingReviewMutations(next,startedAtSeq,remainingPending,view)
      setUpdates(previous=>shouldCreateFresh?null:({has_updates:projectedNext.has_updates??previous?.has_updates??false,refreshing:previous?.refreshing||0,blocked:previous?.blocked||0,waiting:projectedNext.waiting}));setSelections(current=>Object.fromEntries(Object.entries(current).map(([key,tasks])=>[key,tasks.map(t=>projectedNext.rows.find(r=>r.vid===t.vid)||t)])));setInspected(current=>Object.fromEntries(Object.entries(current).map(([key,t])=>[key,projectedNext.rows.find(r=>r.vid===t.vid)||t])));setError('');setSessionExpired(false)
      for(const row of projectedNext.rows)if(notesRef.current[row.vid]===undefined)noteBaseline.current[row.vid]=row.note
      if(options.refresh){initializedScopesRef.current.clear();autoSelectedScopesRef.current=new Set();setAutoSelectedScopes(new Set());setSelections({});setDrafts({});setNotes({});setPickerSearch({});noteBaseline.current={};setInspected({});setReference(null);setDetailMode('video');setPreviewOpen(false);setSubmitError('');setUpdates(null);void loadAutoMergePreview()}
      const visibleNextRows=projectedNext.rows.filter(r=>!pendingVidsRef.current.has(r.vid))
      const currentGroupKey=projectedNext.group?.key||group
      if(!options.silent&&currentGroupKey&&(!projectedNext.group||(!search&&!folder&&(projectedNext.remaining??visibleNextRows.length)===0))){
        const fallbackGroup=findNextAvailableGroup(projectedNext.groups,currentGroupKey)
        if(fallbackGroup){
          const fallbackScope=reviewSelectionKey(fallbackGroup.key,view==='deferred')
          initializedScopesRef.current.delete(`${fallbackScope}:0`)
          forceSelectAllScopeRef.current=fallbackScope
          updateFilters({group:fallbackGroup.key,page:'0',vid:''})
          return projectedNext
        }
      }
      const emptyPageTarget=!options.silent&&currentGroupKey&&projectedNext.group?resolveEmptyReviewPageTarget(projectedNext,{visibleRowCount:visibleNextRows.length,hasFilter:Boolean(search||folder)}):null
      if(emptyPageTarget!=null){
        const targetScope=reviewSelectionKey(currentGroupKey,view==='deferred')
        initializedScopesRef.current.delete(`${targetScope}:${emptyPageTarget}`)
        forceSelectAllScopeRef.current=targetScope
        updateFilters({group:currentGroupKey,page:String(emptyPageTarget),vid:''})
        return projectedNext
      }
      applyGroupDefaultSelection(projectedNext)
      const syncedGroup=projectedNext.group?.key||group,syncedPage=projectedNext.page
      if((!group&&projectedNext.group)||page!==syncedPage||options.refresh&&projectedNext.group?.key!==group){
        syncedQueryKeyRef.current=JSON.stringify([view,syncedGroup,search,folder,syncedPage,linkFocus,personScope,clusterScope])
        updateFilters({group:syncedGroup,page:String(syncedPage)})
      }
      return projectedNext
    }catch(e:any){if(token===sequence.current){if(isSessionExpiredError(e?.message)){clearReviewSessionCaches(true);if(shouldAutoRecoverExpiredReviewSession({hasManualSelections:hasManualSelectionsRef.current,hasDrafts:hasDraftsRef.current,inFlightCount:inFlightCountRef.current,alreadyRecovered:options.recovered}))return await latestLoadRef.current({refresh:true,recovered:true});setSessionExpired(true)}if(options.automatic)setUpdates(previous=>({...previous,has_updates:true,refreshing:previous?.refreshing||0,blocked:previous?.blocked||0}));else if(!options.silent)setError(e.message)}return null}
    finally{if(token===sequence.current&&!options.automatic&&!options.silent)setLoading(false)}
  },[initialized,view,group,search,folder,page,linkFocus,personScope,clusterScope,updateFilters,activityVersion,canAutoApply,markAutoRefreshed,findNextAvailableGroup,applyGroupDefaultSelection,clearReviewSessionCaches,loadAutoMergePreview])
  const latestLoadRef=useRef(load)
  latestLoadRef.current=load
  useEffect(()=>{if(data&&(!group||data.group?.key===group))applyGroupDefaultSelection(data)},[data,group,applyGroupDefaultSelection])
  useLayoutEffect(()=>{
    if(loading||!scrollAnchor.current||!listScroll.ref.current)return
    const area=listScroll.ref.current,top=area.getBoundingClientRect().top,anchors=scrollAnchor.current;scrollAnchor.current=null
    for(const anchor of anchors){const row=[...area.querySelectorAll<HTMLElement>('[data-vid]')].find(e=>e.dataset.vid===anchor.vid);if(row){area.scrollTop+=row.getBoundingClientRect().top-top-anchor.offset;break}}
  },[data,loading,listScroll.ref])
  const loadPeople=useCallback(async()=>{const cachedPeople=readPersistedWorkbenchCache<PersonChoice[]>('review:people',REVIEW_CACHE_TTL_MS);if(cachedPeople?.length)setPeople(prev=>prev.length?prev:cachedPeople);try{const list=(await api<{persons:PersonChoice[]}>('/api/persons?picker=1')).persons;writeWorkbenchCache('review:people',list,{persist:true});setPeople(list);setPeopleError('')}catch(e:any){setPeopleError(e.message)}},[])
  useEffect(()=>{
    if(!initialized)return
    if(syncedQueryKeyRef.current===queryFetchKey){syncedQueryKeyRef.current=null;return}
    syncedQueryKeyRef.current=null
    void load()
    return()=>{sequence.current++}
  },[initialized,queryFetchKey,load])
  useEffect(()=>{void loadPeople()},[loadPeople])
  useEffect(()=>{if(initialized)void loadAutoMergePreview()},[initialized,loadAutoMergePreview])
  useEffect(()=>{
    return subscribe(event=>{
      if(event.type==='archive-settled'||event.type==='auto-merge-settled'){
        void loadAutoMergePreview()
        if(!protectedWorkRef.current){
          clearReviewSessionCaches(true)
          void latestLoadRef.current({refresh:true})
        }else{
          pendingSessionResetRef.current=true
          clearReviewSessionCaches(false)
          setUpdates(prev=>({has_updates:true,refreshing:prev?.refreshing||0,blocked:prev?.blocked||0,waiting:prev?.waiting}))
        }
      }else if(event.type==='person-file-settled'&&event.task.action==='review-delete'){
        const tracked=pendingDeleteTasksRef.current.get(event.task.id)
        if(tracked){
          pendingDeleteTasksRef.current.delete(event.task.id)
          const ok=event.task.status==='complete'
          settlePendingMutation(tracked.pendingId,ok)
          clearReviewSessionCaches(false)
          void loadAutoMergePreview()
          if(ok)toast.success(`已移入回收站（${tracked.vids.length} 个视频）`)
          else toast.error(event.task.error||'移入回收站未完成，视频已保留')
          if(inFlightCountRef.current===0)void latestLoadRef.current({preservePosition:true,silent:true})
        }else{
          void loadAutoMergePreview()
          if(!protectedWorkRef.current){
            clearReviewSessionCaches(false)
            void latestLoadRef.current({preservePosition:true,silent:true})
          }else{
            clearReviewSessionCaches(false)
            setUpdates(prev=>({has_updates:true,refreshing:prev?.refreshing||0,blocked:prev?.blocked||0,waiting:prev?.waiting}))
          }
        }
      }else if(event.type==='person-batch-settled'||event.type==='person-file-settled'){
        deleteWorkbenchCache('review:people')
        void loadPeople()
        void loadAutoMergePreview()
        if(!protectedWorkRef.current){
          clearReviewSessionCaches(true)
          void latestLoadRef.current({refresh:true})
        }else{
          pendingSessionResetRef.current=true
          clearReviewSessionCaches(false)
          setUpdates(prev=>({has_updates:true,refreshing:prev?.refreshing||0,blocked:prev?.blocked||0,waiting:prev?.waiting}))
        }
      }else if(event.type==='server-op-settled'){
        const {settled}=event
        if(localReviewOpIdsRef.current.has(settled.op_id))return
        void loadAutoMergePreview()
        if(isDestructivePersonAction(settled.action)||settled.people.length>0){
          deleteWorkbenchCache('review:people')
          void loadPeople()
        }
        if(!protectedWorkRef.current&&inFlightCountRef.current===0){
          clearReviewSessionCaches(true)
          void latestLoadRef.current({refresh:true})
        }else{
          pendingSessionResetRef.current=true
          clearReviewSessionCaches(false)
          setUpdates(prev=>({has_updates:true,refreshing:prev?.refreshing||0,blocked:prev?.blocked||0,waiting:prev?.waiting}))
        }
      }
    })
  },[subscribe,loadPeople,clearReviewSessionCaches,loadAutoMergePreview])

  useEffect(()=>{setReference(null);setSubmitError('')},[scope])
  useEffect(()=>{if(reviewDesktop)setPreviewOpen(false)},[reviewDesktop])
  useEffect(()=>{
    const mq=window.matchMedia('(min-width: 1100px) and (max-width: 1360px)')
    const sync=()=>{if(!groupsManualRef.current)setGroupsCollapsed(mq.matches)}
    sync()
    mq.addEventListener('change',sync)
    return()=>mq.removeEventListener('change',sync)
  },[])
  useEffect(()=>{
    if(!reviewDesktop)return
    const el=workareaRef.current
    if(!el)return
    const recompute=()=>{
      const workareaW=el.clientWidth
      if(workareaW<=0)return
      const vw=window.innerWidth
      const inspectorW=vw<1280?284:vw<=1360?288:308
      const groupsW=groupsCollapsed?56:(vw<1280?180:196)
      const minStageW=vw<1280?340:400
      const max=Math.max(200,Math.min(500,Math.floor(workareaW-groupsW-7-minStageW-inspectorW)))
      const min=Math.min(280,max)
      setListBounds(prev=>prev.min===min&&prev.max===max?prev:{min,max})
    }
    recompute()
    const observer=new ResizeObserver(recompute)
    observer.observe(el)
    window.addEventListener('resize',recompute)
    return()=>{observer.disconnect();window.removeEventListener('resize',recompute)}
  },[reviewDesktop,groupsCollapsed])
  const clampedListWidth=Math.max(listBounds.min,Math.min(listBounds.max,listWidth))
  useEffect(()=>{
    if(!initialized)return
    let cancelled=false
    const poll=async()=>{if(polling.current||!session.current||document.visibilityState!=='visible')return;polling.current=true;const id=session.current,requestSequence=sequence.current
      try{const next=await api<{has_updates:boolean;refreshing:number;blocked:number;waiting?:number}>('/api/review?status=1&session='+encodeURIComponent(id));if(cancelled||session.current!==id||requestSequence!==sequence.current)return;setUpdates(next);if(next.has_updates&&canAutoApplyRef.current())await latestLoadRef.current({refresh:true,automatic:true})}
      catch(e:any){if(!cancelled&&session.current===id&&isSessionExpiredError(e?.message)){clearReviewSessionCaches(true);if(!protectedWorkRef.current&&shouldAutoRecoverExpiredReviewSession({hasManualSelections:hasManualSelectionsRef.current,hasDrafts:hasDraftsRef.current,inFlightCount:inFlightCountRef.current})){void latestLoadRef.current({refresh:true,recovered:true})}else{setSessionExpired(true);if(canAutoApplyRef.current())await latestLoadRef.current({refresh:true,automatic:true})}}}
      finally{polling.current=false}}
    const onVisibility=()=>{if(document.visibilityState==='visible')void poll()}
    const timer=setInterval(poll,10000);document.addEventListener('visibilitychange',onVisibility);void poll();return()=>{cancelled=true;clearInterval(timer);document.removeEventListener('visibilitychange',onVisibility)}
  },[initialized,clearReviewSessionCaches])
  const evidenceTokens=(vids:string[])=>Object.fromEntries(vids.map(id=>[id,[...rows,...Object.values(selections).flat(),...Object.values(inspected)].find(t=>t.vid===id)?.review_token]));
  useEffect(()=>{if(!loading&&!refreshConfirm&&refreshFocus.current){const target=refreshFocus.current;requestAnimationFrame(()=>{if(!target.matches(':disabled')){target.focus({preventScroll:true});refreshFocus.current=null}})}},[loading,refreshConfirm])
  const requestRefresh=(event:{currentTarget:HTMLElement})=>{if(busy||loading)return;refreshFocus.current=event.currentTarget;if(hasManualSelections||hasDrafts)setRefreshConfirm(true);else void load({refresh:true})}
  const commonMarks=actionTargets.length ? (actionTargets[0].data?.watermark?.candidates||[]).filter(c=>c.kind==='account'&&c.platform!=='Telegram'&&actionTargets.every(t=>(t.data?.watermark?.candidates||[]).some(other=>other.key===c.key))) : []
  const selectionSuggestions:PersonChoice[]=actionTargets.length?(()=>{
    const map=new Map<string,{count:number;sim?:number;thumb:string|null;sources:Set<string>}>()
    for(const t of actionTargets){
      const faceThumb=t.data?.face?.thumbs?.[0]?`/thumb?p=${encodeURIComponent(t.data.face.thumbs[0])}`:null
      for(const c of t.data?.candidates||[]){
        const prev=map.get(c.name)||{count:0,sim:undefined,thumb:faceThumb,sources:new Set<string>()}
        prev.count++
        if(!prev.thumb&&faceThumb)prev.thumb=faceThumb
        for(const s of c.sources||[])prev.sources.add(s)
        for(const e of c.evidence||[])if(e.source==='face'&&typeof e.score==='number')prev.sim=prev.sim===undefined?e.score:Math.max(prev.sim,e.score)
        if(typeof (c as {score?:unknown}).score==='number'){const sc=(c as {score:number}).score;prev.sim=prev.sim===undefined?sc:Math.max(prev.sim,sc)}
        map.set(c.name,prev)
      }
    }
    return [...map.entries()].sort((a,b)=>Number(b[1].sources.has('watermark'))-Number(a[1].sources.has('watermark'))||b[1].count-a[1].count||(b[1].sim||0)-(a[1].sim||0)).map(([name,info])=>{
      const existing=people.find(p=>p.name===name)
      const parts=[`${info.count}/${actionTargets.length} 命中`]
      if(info.sources.has('watermark'))parts.push('水印线索')
      if(info.sim!==undefined)parts.push(`相似度 ${info.sim.toFixed(2)}`)
      return {name,rep_thumb:existing?.rep_thumb||info.thumb||null,aliases:existing?.aliases,sim:info.sim,match_count:info.count,match_reason:parts.join(' · ')}
    })
  })():[]
  const selectionSampleFaces=actionTargets.length?[...new Set(actionTargets.flatMap(t=>(t.data.face?.thumbs||[]).slice(0,2).map(p=>`/thumb?p=${encodeURIComponent(p)}`)))].slice(0,6):[]
  const selectionNameHints:PersonNameHint[]=actionTargets.length?(()=>{
    const map=new Map<string,string>()
    for(const t of actionTargets){
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
  const selectionPreviewTask=selectionPreviewVid?actionTargets.find(t=>t.vid===selectionPreviewVid)||null:null
  const changeQuery=(value:string)=>{advanceSequenceRef.current++;if(!group&&data?.group)setGroup(data.group.key);updateFilters({q:value,page:'0',vid:''})}
  const chooseGroup=(key:string)=>{advanceSequenceRef.current++;const targetScope=reviewSelectionKey(key,view==='deferred');if(!(selections[targetScope]?.length)){initializedScopesRef.current.delete(`${targetScope}:0`);forceSelectAllScopeRef.current=targetScope}setGroup(key);setPage(0);setLinkFocus('');setPreviewOpen(false)}
  const toggle=(tasks:ProcessingTask[])=>{const currentIds=new Set((selectionsRef.current[scope]||[]).map(t=>t.vid));const allowed=tasks.filter(t=>!lockedVidsRef.current.has(t.vid)||currentIds.has(t.vid));if(!allowed.length)return;markScopeManual(scope);setSelections(current=>({...current,[scope]:toggleReviewSelection(current[scope]||[],allowed)}));setWatermarkKey('')}
  const clear=()=>{markScopeManual(scope);setSelections(current=>({...current,[scope]:[]}))}
  const inspect=(task:ProcessingTask)=>{setReference(null);setSelectionPreviewVid(null);if(!reviewDesktop&&!previewOpen){returnFocus.current=document.activeElement as HTMLElement;setInstantEntry(!!document.activeElement?.matches(':focus-visible')||matchMedia('(prefers-reduced-motion: reduce)').matches)}setInspected(current=>({...current,[scope]:task}));if(data?.group?.kind!=='suggested'&&chosen.length<=1)setDrafts(current=>({...current,[scope]:{creator:preferredTaskCreator(task),watermarkKey:''}}));setDetailMode('video');setExplicitStageRefName(null);setStageTab('current');if(reviewDesktop)setStageOpen(true);else setPreviewOpen(true)}
  const openSelection=()=>{if(!previewOpen){returnFocus.current=document.activeElement as HTMLElement;setInstantEntry(!!document.activeElement?.matches(':focus-visible')||matchMedia('(prefers-reduced-motion: reduce)').matches)}if(!chosen.length&&focused&&!lockedVidsRef.current.has(focused.vid)){markScopeManual(scope);setSelections(current=>({...current,[scope]:[focused]}))}if(!creator&&defaultCreator)setCreator(defaultCreator);setReference(null);setSubmitError('');setDetailMode('selection');if(!reviewDesktop)setPreviewOpen(true);else requestAnimationFrame(()=>selectionHeading.current?.focus())}
  const stepFocus=(delta:-1|1)=>{
    if(!rows.length)return
    const currentIdx=focused?rows.findIndex(t=>t.vid===focused.vid):-1
    const nextIdx=currentIdx<0?0:Math.max(0,Math.min(rows.length-1,currentIdx+delta))
    const nextTask=rows[nextIdx]
    if(!nextTask)return
    setSelectionPreviewVid(null)
    setExplicitStageRefName(null)
    if(reviewDesktop)setStageTab('current')
    setInspected(current=>({...current,[scope]:nextTask}))
    requestAnimationFrame(()=>{document.querySelector<HTMLElement>(`[data-vid="${CSS.escape(nextTask.vid)}"]`)?.scrollIntoView({block:'nearest'})})
  }
  const stepFocusRef=useRef(stepFocus)
  stepFocusRef.current=stepFocus
  useEffect(()=>{
    if(!reviewDesktop||lightbox.open||previewOpen||refreshConfirm||autoMergeOpen||Boolean(deleteDialog?.open))return
    const onKey=(e:KeyboardEvent)=>{
      if(e.defaultPrevented||e.metaKey||e.ctrlKey||e.altKey)return
      const target=e.target as HTMLElement|null
      if(target&&(['INPUT','TEXTAREA','SELECT'].includes(target.tagName)||target.isContentEditable||target.closest('[role="dialog"],[role="alertdialog"]')))return
      if(e.key==='['){e.preventDefault();stepFocusRef.current(-1)}
      else if(e.key===']'){e.preventDefault();stepFocusRef.current(1)}
    }
    window.addEventListener('keydown',onKey)
    return()=>window.removeEventListener('keydown',onKey)
  },[reviewDesktop,lightbox.open,previewOpen,refreshConfirm,autoMergeOpen,deleteDialog])
  const advanceAfterComplete=(vids:string[])=>{
    const completedSet=new Set(vids)
    const remainingRows=rows.filter(r=>!completedSet.has(r.vid))
    const anchorVid=vids[vids.length-1]||focused?.vid
    const anchorIdx=anchorVid?rows.findIndex(r=>r.vid===anchorVid):-1
    const isNextUnlocked=(r:ProcessingTask)=>!completedSet.has(r.vid)&&!lockedVidsRef.current.has(r.vid)
    const nextUnlockedRow=(anchorIdx>=0?rows.slice(anchorIdx+1).find(isNextUnlocked)||rows.slice(0,anchorIdx).reverse().find(isNextUnlocked):remainingRows.find(r=>!lockedVidsRef.current.has(r.vid)))||null
    const nextRow=nextUnlockedRow||(anchorIdx>=0?rows.slice(anchorIdx+1).find(r=>!completedSet.has(r.vid))||rows.slice(0,anchorIdx).reverse().find(r=>!completedSet.has(r.vid)):remainingRows[0])||null
    const nextRemainingInGroup=Math.max(0,(data?.remaining??rows.length)-vids.length)
    setSelectionPreviewVid(prev=>prev&&completedSet.has(prev)?null:prev)
    if(nextRow){
      setInspected(current=>{const next=Object.fromEntries(Object.entries(current).filter(([,task])=>!completedSet.has(task.vid)));next[scope]=nextRow;return next})
      setSelections(current=>{const cleaned=removeReviewSelection(current,vids);const remainingChosen=cleaned[scope]||[];cleaned[scope]=remainingChosen.length>0?remainingChosen:(lockedVidsRef.current.has(nextRow.vid)?[]:[nextRow]);return cleaned})
      setDrafts(current=>({...current,[scope]:{creator:data?.group?.kind==='suggested'?data.group.label:preferredTaskCreator(nextRow),watermarkKey:''}}))
      setLinkFocus('')
      requestAnimationFrame(()=>{document.querySelector<HTMLElement>(`[data-vid="${CSS.escape(nextRow.vid)}"]`)?.scrollIntoView({block:'nearest'})})
      return
    }
    setSelections(current=>removeReviewSelection(current,vids))
    setInspected(current=>Object.fromEntries(Object.entries(current).filter(([,task])=>!completedSet.has(task.vid))))
    setDrafts(current=>({...current,[scope]:{creator:undefined,watermarkKey:''}}))
    if(nextRemainingInGroup>0&&data?.next_page!=null){
      initializedScopesRef.current.delete(`${scope}:${data.next_page}`)
      forceSelectAllScopeRef.current=scope
      setPage(data.next_page)
      setLinkFocus('')
      return
    }
    const groupsAfter=(data?.groups||[]).map(g=>g.key===activeGroup?{...g,count:Math.max(0,g.count-vids.length)}:g)
    const nextGroup=findNextAvailableGroup(groupsAfter,activeGroup)
    if(nextGroup){
      const nextScope=reviewSelectionKey(nextGroup.key,view==='deferred')
      initializedScopesRef.current.delete(`${nextScope}:0`)
      forceSelectAllScopeRef.current=nextScope
      setPreviewOpen(false)
      updateFilters({group:nextGroup.key,page:'0',vid:''})
    }else{
      setPreviewOpen(false)
      setLinkFocus('')
    }
  }
  const countBuckets=(targets:ProcessingTask[])=>{
    const buckets:Record<string,number>={}
    for(const t of targets){
      const bucket=reviewTaskBucket(t)
      buckets[bucket]=(buckets[bucket]||0)+1
    }
    return buckets
  }
  const beginPendingMutation=(input:Omit<PendingReviewMutation,'id'|'settledAtSeq'>)=>{
    const id=uuid()
    const entry:PendingReviewMutation={...input,id,settledAtSeq:null}
    const next=[...pendingOpsRef.current,entry]
    pendingOpsRef.current=next
    setPendingOps(next)
    pendingVidsRef.current=new Set(next.flatMap(op=>op.vids))
    inFlightCountRef.current+=1
    return id
  }
  const settlePendingMutation=(id:string,ok:boolean)=>{
    inFlightCountRef.current=Math.max(0,inFlightCountRef.current-1)
    let next:PendingReviewMutation[]
    if(ok){
      const seq=++syncSeqRef.current
      next=pendingOpsRef.current.map(op=>op.id===id?{...op,settledAtSeq:seq}:op)
    }else{
      next=pendingOpsRef.current.filter(op=>op.id!==id)
    }
    pendingOpsRef.current=next
    setPendingOps(next)
    pendingVidsRef.current=new Set(next.flatMap(op=>op.vids))
  }
  const action=async(actionKind:string,vids:string[],note?:string)=>{
    if(!vids.length)return
    setSubmitError('')
    if(actionKind==='note'){
      setBusy(true)
      try{
        await mutate('/api/review',{action:actionKind,vids,note,review_tokens:evidenceTokens(vids),review_session:session.current,review_group:{key:activeGroup,deferred:view==='deferred'}})
        for(const id of vids)noteBaseline.current[id]=note||''
        setNotes(current=>Object.fromEntries(Object.entries(current).filter(([id])=>!vids.includes(id))))
        setInspected(current=>Object.fromEntries(Object.entries(current).map(([key,t])=>[key,vids.includes(t.vid)?{...t,note:note||''}:t])))
        try{
          const renewed=(await Promise.all(vids.filter(id=>!rows.some(t=>t.vid===id)).map(async vid=>{
            const page=await api<Data>('/api/review?'+new URLSearchParams({session:session.current||'',view,vid}))
            return page.rows.find(t=>t.vid===vid)
          }))).filter((t):t is ProcessingTask=>!!t)
          const replace=(t:ProcessingTask)=>renewed.find(r=>r.vid===t.vid)||t
          setSelections(current=>Object.fromEntries(Object.entries(current).map(([key,tasks])=>[key,tasks.map(replace)])))
          setInspected(current=>Object.fromEntries(Object.entries(current).map(([key,t])=>[key,replace(t)])))
        }catch{setSubmitError('备注已保存；核对凭据暂未刷新，请重新打开该视频后继续。')}
        await load({preservePosition:true,silent:true})
        toast.success('备注已保存')
      }catch(e:any){setSubmitError(e.message);if(isSessionExpiredError(e?.message)){clearReviewSessionCaches(true);setSessionExpired(true)}toast.error(e.message)}finally{setBusy(false)}
      return
    }
    if(vids.some(id=>lockedVidsRef.current.has(id)))return
    clearReviewSessionCaches(false)
    ++advanceSequenceRef.current
    const tokens=evidenceTokens(vids),confirmationSession=session.current,currentGroupKey=activeGroup,isDeferredLane=view==='deferred'
    const taskLookup=new Map([...rows,...Object.values(selections).flat(),...Object.values(inspected)].map(t=>[t.vid,t]))
    const resolvedTargets=vids.map(id=>taskLookup.get(id)).filter((t):t is ProcessingTask=>!!t)
    const pendingAction:PendingReviewMutation['action']=actionKind==='restore'?'restore':'defer'
    const pendingId=beginPendingMutation({action:pendingAction,groupKey:currentGroupKey,isDeferred:isDeferredLane,vids,buckets:countBuckets(resolvedTargets)})
    advanceAfterComplete(vids)
    if(!reviewDesktop)setPreviewOpen(false)
    const opId=uuid()
    const promise=mutate('/api/review',{action:actionKind,vids,note,review_tokens:tokens,review_session:confirmationSession,review_group:{key:currentGroupKey,deferred:isDeferredLane}},'POST',opId)
    void trackReviewMutation({
      action:actionKind==='restore'?'restore':'defer',
      title:actionKind==='restore'?'恢复待确认':'移至暂缓核对',
      subtitle:`${vids.length} 个视频`,
      vids,
      operationId:opId,
      promise,
    }).catch(()=>{})
    let succeeded=false
    try{
      await promise
      succeeded=true
      toast.success(actionKind==='restore'?'已恢复核对':'已移至暂缓')
    }catch(e:any){
      setSubmitError(e.message)
      if(isSessionExpiredError(e?.message)){clearReviewSessionCaches(true);setSessionExpired(true)}
      toast.error(e.message)
    }finally{
      clearReviewSessionCaches(false)
      settlePendingMutation(pendingId,succeeded)
      if(inFlightCountRef.current===0)void latestLoadRef.current({preservePosition:true,silent:true})
    }
  }
  const sourceOnly=async(key:string)=>{
    setBusy(true)
    try{await api('/api/watermarks',{method:'PUT',body:JSON.stringify({key,kind:'source'})});clearReviewSessionCaches(false);setPreviewOpen(false);setSelections({});setDetailMode('video');await load({preservePosition:true});toast.success('已标为来源，相关人物建议将重新分析');return true}
    catch(e:any){setSubmitError(e.message);if(isSessionExpiredError(e?.message)){clearReviewSessionCaches(true);setSessionExpired(true)}toast.error(e.message);return false}finally{setBusy(false)}
  }
  const confirmTargets=async(targets:ProcessingTask[],targetCreator:string)=>{
    const name=targetCreator.trim()
    const personLock=name?lockedPeopleRef.current.get(name):undefined
    if(!session.current||!targets.length||!name||sessionExpired||targets.some(t=>blockedIds.has(t.vid)||pendingVidsRef.current.has(t.vid)||lockedVidsRef.current.has(t.vid))||(personLock&&isDestructivePersonAction(personLock.action)))return
    setSubmitError('')
    clearReviewSessionCaches(false)
    const vids=targets.map(t=>t.vid),confirmationSession=session.current,tokens=evidenceTokens(vids),currentGroupKey=activeGroup,isDeferredLane=view==='deferred',currentWatermarkKey=watermarkKey||undefined
    const originGroup=currentGroupKey,originPage=currentPage,advanceToken=++advanceSequenceRef.current
    const fingerprints=Object.fromEntries(targets.map(t=>[t.vid,{path:t.path,size:t.size,mtime_ns:t.mtime_ns}]))
    const pendingId=beginPendingMutation({action:'confirm',groupKey:currentGroupKey,isDeferred:isDeferredLane,vids,buckets:countBuckets(targets)})
    advanceAfterComplete(vids)
    if(!reviewDesktop)setPreviewOpen(false)
    const opId=uuid()
    const promise=mutate<{vids?:string[];rule_warning?:string}>('/api/plan/queue',{vids,creator:name,review_tokens:tokens,review_session:confirmationSession,source:'review',source_id:currentGroupKey,review_group:{key:currentGroupKey,deferred:isDeferredLane},watermark_key:currentWatermarkKey,fingerprints},'POST',opId)
    void trackReviewMutation({
      action:'confirm',
      title:`归给 ${name}`,
      subtitle:`${vids.length} 个视频 · 加入待归档`,
      vids,
      creator:name,
      operationId:opId,
      promise,
    }).catch(()=>{})
    let succeeded=false
    try{
      const result=await promise
      succeeded=true
      const completed=result.vids||vids
      setReceipt({vids:completed,creator:name,session:confirmationSession})
      void loadPeople()
      result.rule_warning?toast.warning(result.rule_warning):toast.success('已加入待归档')
    }catch(e:any){
      clearReviewSessionCaches(false)
      const msg=e?.message||'提交失败'
      const isConflict=e?.status===409||/已变更|已变化|未在本次会话|不属于本次/.test(msg)||isSessionExpiredError(msg)
      setSubmitError(msg)
      if(isSessionExpiredError(msg)){clearReviewSessionCaches(true);setSessionExpired(true)}
      if(isConflict)setUpdates(prev=>({has_updates:true,refreshing:prev?.refreshing||0,blocked:(prev?.blocked||0)+vids.length,waiting:prev?.waiting}))
      toast.error(isConflict?`${name}（${vids.length} 个视频）未入队：${msg}，请点右上角「有更新」刷新列表`:`${name}（${vids.length} 个视频）未入队：${msg}`)
      if(advanceSequenceRef.current===advanceToken&&inFlightCountRef.current===1&&(originGroup!==activeGroupRef.current||originPage!==currentPageRef.current)){
        updateFilters({group:originGroup,page:String(originPage),vid:''})
      }
    }finally{
      clearReviewSessionCaches(false)
      settlePendingMutation(pendingId,succeeded)
      if(inFlightCountRef.current===0)void latestLoadRef.current({preservePosition:true,silent:true})
    }
  }
  const confirm=()=>confirmTargets(actionTargets,creator)
  const openDeleteDialog=(mode:'single'|'batch',targets:ProcessingTask[],trigger?:HTMLElement|null)=>{
    if(!targets.length||sessionExpired||targets.some(t=>blockedIds.has(t.vid)||pendingVidsRef.current.has(t.vid)||lockedVidsRef.current.has(t.vid)))return
    deleteTriggerRef.current=trigger??(document.activeElement as HTMLElement|null)
    if(lightbox.open)setLightbox(prev=>({...prev,open:false}))
    setDeleteDialog({open:true,mode,targets})
  }
  const confirmDeletePlan=(plan:PersonFilePlan,targets:ProcessingTask[])=>{
    setDeleteDialog(prev=>prev?{...prev,open:false}:null)
    if(!reviewDesktop)setPreviewOpen(false)
    clearReviewSessionCaches(false)
    ++advanceSequenceRef.current
    const vids=targets.map(t=>t.vid)
    const pendingId=beginPendingMutation({action:'delete',groupKey:activeGroup,isDeferred:view==='deferred',vids,buckets:countBuckets(targets)})
    advanceAfterComplete(vids)
    const taskId=enqueuePersonFile({plan,person:'',label:activeGroupInfo?.label||'人工确认'})
    pendingDeleteTasksRef.current.set(taskId,{pendingId,vids})
    toast.success(`已提交后台移入回收站（${vids.length} 个视频）`)
  }
  const undo=async()=>{
    if(!receipt)return
    const currentReceipt=receipt
    setReceipt(null)
    const opId=uuid()
    const promise=mutate<{warning?:string}>('/api/plan/queue',{vids:currentReceipt.vids,review_session:currentReceipt.session},'DELETE',opId)
    void trackReviewMutation({
      action:'undo',
      title:`撤销归给 ${currentReceipt.creator}`,
      subtitle:`${currentReceipt.vids.length} 个视频`,
      vids:currentReceipt.vids,
      creator:currentReceipt.creator,
      operationId:opId,
      promise,
    }).catch(()=>{})
    try{
      const result=await promise
      clearReviewSessionCaches(false)
      await load({preservePosition:true,silent:true})
      result.warning?toast.warning(result.warning):toast.success('已撤销本次归名')
    }catch(e:any){
      setReceipt(currentReceipt)
      toast.error(e.message)
    }
  }
  const openReference=(name:string)=>{
    if(reviewDesktop){
      setReference(null)
      setExplicitStageRefName(name)
      setStageRefIndex(0)
      setStageOpen(true)
      setStageTab('reference')
      return
    }
    setReference({name,returnTo:detailMode,scroll:document.querySelector('.ws-detail-content')?.scrollTop||0})
    requestAnimationFrame(()=>selectionHeading.current?.focus())
  }
  const backReference=()=>{const previous=reference;setReference(null);if(previous)requestAnimationFrame(()=>{const area=document.querySelector('.ws-detail-content');if(area)area.scrollTop=previous.scroll;const trigger=[...document.querySelectorAll<HTMLButtonElement>('.ws-detail-content button')].find(b=>b.getAttribute('aria-label')===`看 ${previous.name} 的对照视频`);trigger?.focus({preventScroll:true})})}
  const focusedIndex=focused?rows.findIndex(t=>t.vid===focused.vid):-1
  const nextAvailableGroup=data?findNextAvailableGroup(data.groups,activeGroup):null

  const stageCurrentTask=selectionPreviewTask||focused||null
  const [stagePosterState,setStagePosterState]=useState<{key:string;src?:string}|null>(null)
  const stageCurrentSampleTime=stageCurrentTask?reviewListSummary(stageCurrentTask).sampleTime:0.1
  const stageCurrentStamp=stageCurrentTask?JSON.stringify([stageCurrentTask.size,stageCurrentTask.mtime_ns||undefined]):''
  const stageCurrentPosterKey=stageCurrentTask?JSON.stringify([stageCurrentTask.path,stageCurrentStamp,stageCurrentSampleTime]):''
  useEffect(()=>{
    if(!stageCurrentTask||!stageCurrentPosterKey)return
    const abort=new AbortController()
    requestVideoPoster(stageCurrentTask.path,stageCurrentStamp,stageCurrentSampleTime,abort.signal)
      .then(poster=>{if(!abort.signal.aborted)setStagePosterState({key:stageCurrentPosterKey,src:poster.src})})
      .catch(()=>{if(!abort.signal.aborted)setStagePosterState({key:stageCurrentPosterKey,src:undefined})})
    return()=>abort.abort()
  },[stageCurrentTask?.path,stageCurrentStamp,stageCurrentSampleTime,stageCurrentPosterKey])
  const stageCurrentPoster=stagePosterState?.key===stageCurrentPosterKey?stagePosterState.src:undefined
  const stageRefName=explicitStageRefName||reference?.name||creator.trim()||defaultCreator||focused?.data.candidates?.[0]?.name||null
  const {current:stageRefData,loading:stageRefLoading,retry:retryStageRef}=usePersonReferences(reviewDesktop||lightbox.open?stageRefName:null)
  const refVideos=stageRefData?.videos||[]
  const clampedRefIdx=refVideos.length?Math.min(stageRefIndex,refVideos.length-1):0
  const activeRefVideo=refVideos[clampedRefIdx]||null
  const activeStageTab=stageTab==='reference'&&stageRefName?'reference':'current'
  const activeStageVideoPath=activeStageTab==='reference'?(activeRefVideo?activeRefVideo.path||activeRefVideo.vid:''):(stageCurrentTask?.path||'')
  const activeStageTitle=activeStageTab==='reference'?(activeRefVideo?.name||`${stageRefName||'人物'} · 对照视频`):(stageCurrentTask?.name||'选择视频查看')
  const formatStageSize=(size?:number,sizeGb?:number)=>typeof size==='number'&&size>0?(size>=2**30?`${(size/2**30).toFixed(2)} GB`:`${(size/2**20).toFixed(1)} MB`):typeof sizeGb==='number'?`${sizeGb} GB`:null
  const activeStageSize=activeStageTab==='reference'?formatStageSize(undefined,activeRefVideo?.size_gb):(stageCurrentTask?formatStageSize(stageCurrentTask.size):null)
  const activeLightboxTab=lightbox.tab==='reference'&&stageRefName?'reference':'current'
  const lightboxVideoPath=activeLightboxTab==='reference'?(activeRefVideo?activeRefVideo.path||activeRefVideo.vid:''):(stageCurrentTask?.path||'')
  const lightboxVideoName=activeLightboxTab==='reference'?(activeRefVideo?.name||`${stageRefName||'人物'} · 对照视频`):(stageCurrentTask?.name||'')
  const lightboxPoster=activeLightboxTab==='reference'?(activeRefVideo?.thumb||undefined):stageCurrentPoster
  const lightboxHasPrev=activeLightboxTab==='reference'?clampedRefIdx>0:focusedIndex>0
  const lightboxHasNext=activeLightboxTab==='reference'?clampedRefIdx<refVideos.length-1:(focusedIndex>=0&&focusedIndex<rows.length-1)
  const lightboxStep=(delta:-1|1)=>{
    if(activeLightboxTab==='reference'){
      setStageRefIndex(i=>Math.max(0,Math.min(refVideos.length-1,i+delta)))
    }else{
      stepFocus(delta)
    }
  }
  const targetLock=actionTargets.map(t=>lockedVids.get(t.vid)).find(Boolean)
  const hasLockedTarget=Boolean(targetLock)
  const focusedLock=focused?lockedVids.get(focused.vid):undefined
  const stageCurrentLock=stageCurrentTask?lockedVids.get(stageCurrentTask.vid):undefined
  const trimmedCreator=creator.trim()
  const creatorLock=trimmedCreator?lockedPeople.get(trimmedCreator):undefined
  const creatorDestructiveLock=creatorLock&&isDestructivePersonAction(creatorLock.action)?creatorLock:undefined
  const chosenLocked=chosen.some(t=>lockedVids.has(t.vid))
  const unlockedRows=rows.filter(t=>!lockedVids.has(t.vid))
  const activeLocalAutoMerge=tasks.find(t=>t.kind==='auto-merge'&&(t.status==='running'||t.status==='queued'||t.status==='recovery_needed'))
  const activeServerAutoMerge=serverActiveOps.find(op=>isAutoMergeAction(op.action)&&(op.status==='running'||op.status==='queued'||op.status==='recovery_needed'))
  const autoMergeActive=Boolean(activeLocalAutoMerge||activeServerAutoMerge)
  const autoMergeActiveStatus=activeLocalAutoMerge?.status??activeServerAutoMerge?.status
  const autoMergeActiveLabel=activeLocalAutoMerge?.status==='recovery_needed'?'待恢复':activeLocalAutoMerge?.status==='queued'?'排队合入':activeLocalAutoMerge?'合入中':(activeServerAutoMerge?.label||(activeServerAutoMerge?.status==='recovery_needed'?'待恢复':'合入中'))
  const detail=<>

    <div className="ws-detail-heading"><h2 ref={selectionHeading} tabIndex={-1}>{reference?`${reference.name} · 对照视频`:detailMode==='selection'?<>核对所选 · <DigitSwap value={actionTargets.length}/></>:'证据与归名'}</h2><div className="ws-actions">{detailMode==='video'&&!reference&&focused&&<Button className="ws-icon ws-danger-icon" variant="ghost" size="sm" aria-label="将当前视频移入回收站" title={focusedLock?`${focusedLock.label}，暂不可删除`:'删除当前视频（移入回收站）'} disabled={disabled||Boolean(focusedLock)||blockedIds.has(focused.vid)||pendingVids.has(focused.vid)} onClick={e=>openDeleteDialog('single',[focused],e.currentTarget)}><Trash2 size={14} aria-hidden="true"/></Button>}{reference?<Button variant="ghost" onClick={backReference}><ArrowLeft size={14} aria-hidden="true"/>返回{reference.returnTo==='selection'?'人物选择':'证据'}</Button>:detailMode==='selection'?<Button variant="ghost" size="sm" onClick={()=>setDetailMode('video')}><ArrowLeft size={14} aria-hidden="true"/>返回证据</Button>:<Button variant="ghost" size="sm" disabled={!actionTargets.length} onClick={openSelection}><ListChecks size={14} aria-hidden="true"/>改选人物 · <DigitSwap value={actionTargets.length}/></Button>}{reviewDesktop&&!stageOpen&&!reference&&focused&&rows.length>0&&<div className="ws-actions"><Button className="ws-icon" variant="ghost" size="sm" aria-label="上一条视频" title="上一条视频" disabled={focusedIndex<=0} onClick={()=>stepFocus(-1)}><ChevronLeft size={14} aria-hidden="true"/></Button><span className="ws-muted tabular-nums">{focusedIndex>=0?`${focusedIndex+1} / ${rows.length}`:'已查看'}</span><Button className="ws-icon" variant="ghost" size="sm" aria-label="下一条视频" title="下一条视频" disabled={focusedIndex<0||focusedIndex>=rows.length-1} onClick={()=>stepFocus(1)}><ChevronRight size={14} aria-hidden="true"/></Button></div>}</div></div>
    <div className="ws-detail-content" data-sheet-scroll>
      {reference?<ReferencePanel name={reference.name}/>:detailMode==='video'?(focused?<EvidencePanel key={focused.vid} hideMedia={reviewDesktop&&stageOpen} activeReferenceName={reviewDesktop&&stageOpen?stageRefName:null} poster={stageCurrentTask?.vid===focused.vid?stageCurrentPoster:undefined} task={focused} note={notes[focused.vid]??focused.note} onNoteChange={note=>{if(noteBaseline.current[focused.vid]===undefined)noteBaseline.current[focused.vid]=focused.note;setNotes(current=>{const next={...current};if(note===focused.note)delete next[focused.vid];else next[focused.vid]=note;return next})}} onSaveNote={()=>action('note',[focused.vid],notes[focused.vid]??focused.note)} onSource={sourceOnly} busy={busy} locked={hasLockedTarget||Boolean(focusedLock)} lockedPeople={lockedPeople} creator={creator} onChoose={name=>{setCreator(name);if(!chosen.length&&!lockedVidsRef.current.has(focused.vid)){markScopeManual(scope);setSelections(current=>({...current,[scope]:[focused]}))}}} onConfirmCandidate={name=>{setCreator(name);void confirmTargets(chosen.length>0?chosen:[focused],name)}} onReference={openReference}/>:<p className="ws-empty">选择视频查看证据</p>):<div className="ws-decision">
        <div>
          <p className="mb-2 text-xs text-muted-foreground">{data?.group?.label||'当前确认组'} · <DigitSwap value={actionTargets.length}/> 个视频{hiddenCount>0?`，其中 ${hiddenCount} 个不在当前页`:''}</p>
          {(!reviewDesktop||!stageOpen)&&<Collapsible open={Boolean(selectionPreviewTask)}>
            <CollapsibleContent>
              {selectionPreviewTask&&<Card size="compact" variant="muted" className="ws-confirm-preview" role="region" aria-label={`原位预览 ${selectionPreviewTask.name}`}>
                <div className="ws-confirm-preview-head">
                  <strong title={selectionPreviewTask.name}>{selectionPreviewTask.name}</strong>
                  <div className="ws-confirm-preview-actions">
                    <Button variant="ghost" size="sm" onClick={()=>inspect(selectionPreviewTask)}>完整证据</Button>
                    <Button variant="ghost" size="icon" aria-label="收起原位预览" title="收起预览" onClick={()=>setSelectionPreviewVid(null)}><X size={14} aria-hidden="true"/></Button>
                  </div>
                </div>
                <WorkspaceMedia key={selectionPreviewTask.vid} path={selectionPreviewTask.path} name={selectionPreviewTask.name} poster={stageCurrentTask?.vid===selectionPreviewTask.vid?stageCurrentPoster:undefined} compact autoPlay/>
              </Card>}
            </CollapsibleContent>
          </Collapsible>}
          <div className="ws-confirm-list" aria-label="本次完整归名范围">{actionTargets.map(t=>{
            const summary=reviewListSummary(t),faceThumbs=t.data.face?.thumbs?.slice(0,3)||[],isPreviewing=selectionPreviewTask?.vid===t.vid,itemLock=lockedVids.get(t.vid)
            return <div key={t.vid} className="ws-confirm-file" data-active={isPreviewing||undefined} data-locked={Boolean(itemLock)||undefined}>
              <button type="button" className="ws-confirm-thumb" aria-label={`预览 ${t.name}`} title="在舞台预览此视频" onClick={()=>{setInspected(current=>({...current,[scope]:t}));if(reviewDesktop){setSelectionPreviewVid(t.vid);setStageTab('current');setStageOpen(true)}else{setSelectionPreviewVid(prev=>prev===t.vid?null:t.vid)}}}>
                <VideoCover video={{path:t.path,thumb:faceThumbs[0]?`/thumb?p=${encodeURIComponent(faceThumbs[0])}`:null,size:t.size,mtime_ns:t.mtime_ns}} sampleTime={summary.sampleTime}/>
                <span className="ws-confirm-thumb-play"><Play size={11} aria-hidden="true"/></span>
              </button>
              <div className="ws-confirm-body">
                <div className="ws-confirm-top">
                  <Button variant="ghost" className="ws-confirm-name" onClick={()=>{setInspected(current=>({...current,[scope]:t}));if(reviewDesktop){setSelectionPreviewVid(t.vid);setStageTab('current');setStageOpen(true)}else{setSelectionPreviewVid(prev=>prev===t.vid?null:t.vid)}}} aria-label={`核对所选 ${t.name}`}><Play size={12} aria-hidden="true"/><span>{t.name}</span></Button>
                  {itemLock&&<AnimatedBadge size="sm" status={lockBadgeStatus(itemLock.status)} className="h-5 px-1.5 text-[10px]">{itemLock.label}</AnimatedBadge>}
                  {faceThumbs.length>0&&<AvatarGroup size="sm" max={3} label="检出人脸" className="ws-confirm-faces" members={faceThumbs.map((path,idx)=>({id:`${t.vid}-${idx}`,name:`${t.name} 人脸 ${idx+1}`,src:`/thumb?p=${encodeURIComponent(path)}`}))}/>}
                  {chosen.length>0&&<Button className="ws-icon" variant="ghost" size="sm" aria-label={`取消选择 ${t.name}`} title="取消选择" onClick={()=>toggle([t])}><X size={14} aria-hidden="true"/></Button>}
                </div>
                <WorkspacePath value={t.path} compact/>
              </div>
            </div>
          })}</div>
        </div>
        <PersonPicker search={pickerSearch[scope]||''} onSearchChange={value=>setPickerSearch(current=>({...current,[scope]:value}))} onReference={openReference} compact people={people} suggestions={selectionSuggestions} sampleFaces={selectionSampleFaces} nameHints={selectionNameHints} value={creator} onChange={setCreator} disabled={false} error={peopleError} onRetry={loadPeople}/>
        {commonMarks.length>0&&<WorkspaceField label="共同账号规则"><WorkspaceSelect label="保存共同账号规则" value={watermarkKey} disabled={false} onChange={setWatermarkKey} options={[{value:'',label:'仅归名，不保存账号规则'},...commonMarks.map(c=>({value:c.key,label:`${c.platform} · ${c.token}`}))]}/></WorkspaceField>}
        <p className="text-xs text-muted-foreground">确认后自动选中下一条并在后台加入待归档，文件暂不移动。</p>
      </div>}
      {targetLock&&<div role="status" className="ws-lock-notice"><AnimatedBadge size="sm" status={lockBadgeStatus(targetLock.status)}>{targetLock.label}</AnimatedBadge><span>当前视频正在处理，暂不可操作</span></div>}
      {creatorDestructiveLock&&<div role="status" className="ws-lock-notice"><AnimatedBadge size="sm" status={lockBadgeStatus(creatorDestructiveLock.status)}>{creatorDestructiveLock.label}</AnimatedBadge><span>「{trimmedCreator}」暂不可归名</span></div>}
      {actionTargets.some(t=>blockedIds.has(t.vid))&&<p role="alert" className="ws-error">部分已选项的文件或规则已变化，请更新列表后重新核对。</p>}
      {focused?.review_blocked&&detailMode==='video'&&<p role="alert" className="ws-error">{focused.review_blocked}</p>}
      {submitError&&<p role="alert" className="ws-error">{submitError}</p>}
    </div>
    <div className="ws-detail-actions">{reference?<><Button variant="outline" onClick={backReference}>返回{reference.returnTo==='selection'?'人物选择':'证据'}</Button><Button className="ws-action-primary" onClick={()=>{setCreator(reference.name);backReference()}}><span className="ws-btn-truncate">选择 {reference.name}</span></Button></>:detailMode==='selection'?<><Button variant="outline" disabled={!actionTargets.length||sessionExpired||hasLockedTarget} onClick={()=>void action(view==='deferred'?'restore':'defer',actionTargets.map(t=>t.vid))}>{view==='deferred'?'恢复':'暂缓'}</Button><Button className="ws-danger-btn" variant="outline" disabled={!actionTargets.length||disabled||hasLockedTarget||actionTargets.some(t=>blockedIds.has(t.vid)||pendingVids.has(t.vid))} aria-label={`将所选 ${actionTargets.length} 个视频移入回收站`} onClick={e=>openDeleteDialog('batch',actionTargets,e.currentTarget)}><Trash2 size={14} aria-hidden="true"/><span>移入回收站</span></Button><Button className="ws-action-primary" disabled={!actionTargets.length||!trimmedCreator||sessionExpired||hasLockedTarget||Boolean(creatorDestructiveLock)||actionTargets.some(t=>blockedIds.has(t.vid)||pendingVids.has(t.vid))} title={creatorDestructiveLock?`${trimmedCreator} ${creatorDestructiveLock.label}`:undefined} onClick={()=>void confirm()}><span className="ws-btn-truncate">归给 {trimmedCreator||'所选人物'}</span><span className="ws-btn-badge">· <DigitSwap value={actionTargets.length}/></span></Button></>:<><Button variant="outline" disabled={!focused||(Boolean(focusedLock)&&!selected.has(focused.vid))||(!!focused?.review_blocked&&!selected.has(focused.vid))} onClick={()=>focused&&toggle([focused])}>{focused&&selected.has(focused.vid)?'取消选择':'选择此项'}</Button><Button variant="outline" disabled={!actionTargets.length||sessionExpired||hasLockedTarget} onClick={()=>void action(view==='deferred'?'restore':'defer',actionTargets.map(t=>t.vid))}>{view==='deferred'?'恢复':'暂缓'}</Button>{trimmedCreator?<Button className="ws-action-primary" disabled={!actionTargets.length||sessionExpired||hasLockedTarget||Boolean(creatorDestructiveLock)||actionTargets.some(t=>blockedIds.has(t.vid)||pendingVids.has(t.vid))} title={creatorDestructiveLock?`${trimmedCreator} ${creatorDestructiveLock.label}`:undefined} onClick={()=>void confirm()}><span className="ws-btn-truncate">归给 {trimmedCreator}</span><span className="ws-btn-badge">· <DigitSwap value={actionTargets.length}/></span></Button>:<Button className="ws-action-primary" disabled={!actionTargets.length} onClick={openSelection}><span>核对所选</span><span className="ws-btn-badge">· <DigitSwap value={actionTargets.length}/></span></Button>}</>}</div>
  </>
  const midScopeSegment:WorkbenchBreadcrumbSegment|null=personScope?{key:`person:${personScope}`,label:personScope}:clusterScope?{key:`cluster:${clusterScope}`,label:`相似组 ${clusterScope}`}:view!=='all'?{key:`view:${view}`,label:views.find(v=>v.value===view)?.label||view}:null
  const leafGroupSegment:WorkbenchBreadcrumbSegment|null=data?.group?.label?{key:`group:${data.group.key}`,label:data.group.label}:null
  const hasDeeperBreadcrumb=Boolean(midScopeSegment||leafGroupSegment)
  const midScopeHref=personScope?`/review?person=${encodeURIComponent(personScope)}`:clusterScope?`/review?cluster=${encodeURIComponent(clusterScope)}`:`/review?view=${encodeURIComponent(view)}`
  const breadcrumbItems:WorkbenchBreadcrumbSegment[]=[
    {key:'home',label:'首页',href:'/',icon:'home'},
    {
      key:'review',
      label:'人工确认',
      ...(hasDeeperBreadcrumb?{
        href:'/review',
        onClick:()=>{advanceSequenceRef.current++;updateFilters({view:'all',group:'',person:'',cluster:'',page:'0',vid:''})},
      }:{}),
    },
    ...(midScopeSegment?[{
      ...midScopeSegment,
      ...(leafGroupSegment?{href:midScopeHref,onClick:()=>{advanceSequenceRef.current++;updateFilters({group:'',page:'0',vid:''})}}:{}),
    }]:[]),
    ...(leafGroupSegment?[leafGroupSegment]:[]),
  ]
  useSyncWorkbenchBreadcrumb(breadcrumbItems)
  const mobileSearchActive=mobileSearchOpen||Boolean(query)
  const isSinglePage=Boolean(data&&groupMatched&&Math.ceil(data.total/data.limit)<=1&&data.previous_page==null&&data.next_page==null)
  const renderFilterPopover=(trigger:React.ReactNode)=>(
    <Popover.Root>
      <Popover.Trigger asChild>{trigger}</Popover.Trigger>
      <Popover.Portal>
        <Popover.Content className="ws-filter-content" align="end" sideOffset={6}>
          <div className="ws-mobile-filter">
            <WorkspaceField label="确认组">
              <WorkspaceSelect label="选择确认组" value={activeGroup} onChange={chooseGroup} options={(data?.groups||[]).map(g=>({value:g.key,label:`${g.label} · ${g.count}`}))} placeholder="暂无确认组"/>
            </WorkspaceField>
          </div>
          <WorkspaceField label="原文件夹">
            <WorkspaceSelect label="按原文件夹筛选" value={folder} onChange={value=>{setFolder(value);if(!group&&data?.group)setGroup(data.group.key)}} options={[{value:'',label:'全部文件夹'},...(data?.folders||[]).map(f=>({value:f,label:f}))]}/>
          </WorkspaceField>
          {folder&&<Button className="mt-3" variant="ghost" size="sm" onClick={()=>{setFolder('');setPage(0)}}>清除筛选</Button>}
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  )
  return <main className="ws-page" aria-label="人工确认工作区" data-review-session={data?.session}>
    <WorkbenchHeaderActions>{((autoMergePreview?.eligible_count??0)>0||autoMergeActive)&&<Button ref={autoMergeTriggerRef} variant="outline" size="sm" aria-label={autoMergeActive?`自动合入（${autoMergeActiveLabel}）`:`自动合入 ${autoMergePreview!.eligible_count} 个已匹配视频`} title={autoMergeActive?`自动合入：${autoMergeActiveLabel}`:`可直接合入 ${autoMergePreview!.eligible_count} 个视频（${autoMergePreview!.group_count} 位人物）`} onClick={()=>setAutoMergeOpen(true)}><Sparkles size={14} aria-hidden="true"/><span>自动合入</span>{autoMergeActive?<AnimatedBadge size="sm" status={lockBadgeStatus(autoMergeActiveStatus)} className="ws-topbar-lock-badge">{autoMergeActiveLabel}</AnimatedBadge>:<span className="ws-count">{autoMergePreview!.eligible_count}</span>}</Button>}<NavigationButton variant="ghost" size="sm" asChild className="ws-review-status-link"><Link href="/operations"><span className="ws-link-label">分析中</span><span className="ws-count">{updates?.waiting??data?.waiting??'—'}</span><ChevronRight size={14} aria-hidden="true"/></Link></NavigationButton><Button variant={updates?.has_updates||sessionExpired?"outline":"ghost"} aria-label="更新人工确认列表" title={updates?.refreshing?`后台正在更新 ${updates.refreshing} 条建议，当前列表保持不变`:'更新建议和分组'} disabled={loading} onClick={requestRefresh}><RefreshCw size={15} aria-hidden="true"/>{updates?.has_updates||sessionExpired?<span>有更新</span>:updates?.refreshing?<span className="ws-muted">后台更新中</span>:null}</Button></WorkbenchHeaderActions>
    {receipt&&<div className="ws-receipt"><QueueReceipt count={receipt.vids.length} creator={receipt.creator} onUndo={undo} onDismiss={()=>setReceipt(null)} busy={false}/></div>}
    {error&&<p role="alert" className="ws-error">{error}</p>}
    {sessionExpired&&!error&&<p role="status" className="ws-error">本次核对已过期，勾选和草稿仍保留。请更新列表后继续。</p>}
    {data?.focus_missing&&<p className="ws-error">链接中的视频尚未就绪或已归名。</p>}
    {(personScope||clusterScope)&&<div className="ws-toolbar"><span className="ws-muted">{personScope||`相似组 ${clusterScope}`}</span><Button size="sm" variant="ghost" onClick={()=>{advanceSequenceRef.current++;updateFilters({person:'',cluster:'',group:'',page:'0',vid:''})}}>查看全部</Button></div>}
    <div className="ws-view-tabs"><WorkspaceTabs value={view} onChange={setView} label="确认状态" disabled={false} items={views.map(v=>({...v,count:data?.counts[v.value]}))}/></div>
    <div ref={workareaRef} className="ws-workarea ws-workarea-review" data-stage-open={reviewDesktop&&stageOpen?'true':'false'}>
      <aside className="ws-groups" data-collapsed={reviewDesktop&&groupsCollapsed?'true':'false'} aria-label="确认组">
        <div className="ws-panel-heading">
          <div className="ws-panel-heading-left">
            <h2 className="ws-groups-title">确认组</h2>
            <span className="ws-groups-total tabular-nums">{data?.groups.length??'—'}</span>
          </div>
          {reviewDesktop&&<Button className="ws-icon" variant="ghost" size="sm" aria-label={groupsCollapsed?'展开确认组':'收起确认组'} aria-expanded={!groupsCollapsed} title={groupsCollapsed?'展开确认组':'收起确认组'} onClick={()=>{groupsManualRef.current=true;setGroupsCollapsed(v=>!v)}}>{groupsCollapsed?<ChevronsRight size={14} aria-hidden="true"/>:<ChevronsLeft size={14} aria-hidden="true"/>}</Button>}
        </div>
        <div className="ws-group-list">
          {!data&&loading&&!error?(
            <div role="status" aria-busy="true" className="space-y-1.5 py-1">
              <span className="sr-only">正在读取确认组</span>
              {Array.from({length:6},(_,i)=>reviewDesktop&&groupsCollapsed?(
                <div key={i} className="flex min-h-[46px] flex-col items-center justify-center gap-1 py-1.5">
                  <Skeleton className="size-6 rounded-[6px]"/>
                  <Skeleton className="h-2.5 w-6 rounded-[4px]"/>
                </div>
              ):(
                <div key={i} className="flex min-h-[40px] items-center justify-between gap-2 rounded-[8px] px-2.5 py-2">
                  <Skeleton className="h-3.5 flex-1"/>
                  <Skeleton className="h-4 w-7 shrink-0 rounded-[4px]"/>
                </div>
              ))}
            </div>
          ):data?.groups.map(g=>{
            const personLock=g.key.startsWith('person:')?(lockedPeople.get(g.label)??lockedPeople.get(g.key.slice('person:'.length))):undefined
            return <button key={g.key} type="button" className="ws-group" data-locked={Boolean(personLock)||undefined} title={`${g.label} (${g.count})${personLock?` · ${personLock.label}`:''}`} aria-pressed={activeGroup===g.key} onClick={()=>chooseGroup(g.key)}><span className="ws-group-pill" aria-hidden="true">{Array.from(g.label).slice(0,2).join('')}</span><span className="ws-group-label">{g.label}</span>{personLock&&<AnimatedBadge size="sm" status={lockBadgeStatus(personLock.status)} className="ws-group-lock">{personLock.label}</AnimatedBadge>}<span className="ws-count">{g.count}</span>{(selections[reviewSelectionKey(g.key,view==='deferred')]||[]).length>0&&<><Check size={12} className="ws-group-check" aria-hidden="true"/><span className="sr-only">（有已选视频）</span></>}</button>
          })}
        </div>
      </aside>
      <section ref={queueRef} className="ws-queue" aria-label="待确认视频" style={reviewDesktop&&stageOpen?{width:`${clampedListWidth}px`}:undefined}>

        <div
          ref={mobileToolbarRef}
          className="ws-toolbar"
          data-mobile-open={mobileSearchActive?'true':'false'}
          onKeyDown={e=>{if(e.key==='Escape'&&!query&&mobileSearchOpen&&!e.nativeEvent.isComposing){e.preventDefault();setMobileSearchOpen(false);requestAnimationFrame(()=>mobileSearchTriggerRef.current?.focus({preventScroll:true}))}}}
        >
          <WorkspaceSearch value={query} onChange={changeQuery} label="搜索当前组视频" placeholder="搜索文件名或路径…" disabled={false}>
            {renderFilterPopover(<InputGroupButton size="icon-sm" aria-label="分组与筛选" title="分组与筛选" data-active={!!folder}><SlidersHorizontal size={15} aria-hidden="true"/>{folder&&<span className="size-1.5 rounded-full bg-current"/>}</InputGroupButton>)}
          </WorkspaceSearch>
          {mobileSearchActive&&<Button className="ws-mobile-search-close" variant="ghost" size="sm" aria-label="收起搜索" onClick={()=>{if(query)changeQuery('');setMobileSearchOpen(false);requestAnimationFrame(()=>mobileSearchTriggerRef.current?.focus({preventScroll:true}))}}>取消</Button>}
          {reviewDesktop&&!stageOpen&&<Button variant="outline" size="sm" aria-label="展开中间视频舞台" title="展开视频舞台" onClick={()=>setStageOpen(true)}><Film size={14} aria-hidden="true"/><span>展开舞台</span></Button>}
        </div>
        <div className="ws-selection">
          <Checkbox className="ws-check" aria-label="选择当前页视频" motionPreset="instant" checked={groupMatched&&unlockedRows.length>0&&unlockedRows.every(t=>selected.has(t.vid))} indeterminate={groupMatched&&unlockedRows.some(t=>selected.has(t.vid))&&!unlockedRows.every(t=>selected.has(t.vid))} disabled={disabled||!groupMatched||!unlockedRows.length} onCheckedChange={()=>toggle(unlockedRows)}/>
          <div className="ws-selection-summary"><strong>{chosen.length?`已选 ${chosen.length}`:activeGroupInfo?.label||'待确认'}</strong><span> · {hiddenCount?`${hiddenCount} 个在其他页或被筛选隐藏`:activeGroupRemaining!==null?`${activeGroupRemaining} 个待核对`:'读取中'}</span></div>
          {chosen.length>0&&<Button className="ws-selection-clear" variant="ghost" size="sm" title="清空当前组选择" aria-label="清空当前组选择" onClick={clear}><X size={14} aria-hidden="true"/><span>清空</span></Button>}
          {!mobileSearchActive&&<div className="ws-selection-tools">
            <Button ref={mobileSearchTriggerRef} className="ws-icon ws-selection-tool" variant="ghost" size="icon" aria-label="搜索文件名或路径" title="搜索" onClick={()=>{setMobileSearchOpen(true);requestAnimationFrame(()=>mobileToolbarRef.current?.querySelector<HTMLInputElement>('input[type="search"]')?.focus())}}><Search size={16} aria-hidden="true"/></Button>
            {renderFilterPopover(<Button className="ws-icon ws-selection-tool" variant="ghost" size="icon" aria-label="分组与筛选" title="分组与筛选" data-active={!!folder}><SlidersHorizontal size={16} aria-hidden="true"/>{folder&&<span className="size-1.5 rounded-full bg-current"/>}</Button>)}
          </div>}
        </div>
        <div ref={listScroll.ref} className="ws-list" aria-busy={loading||!groupMatched||query!==search} onScroll={listScroll.onScroll}>
          {(!data||!groupMatched)&&!error?<ReviewQueueSkeleton count={6} label="正在读取待确认视频"/>:!rows.length?<div className="ws-empty"><p>{group&&!data?.group?'该组已处理完，选择另一组继续。':query||folder?'当前筛选下没有视频。':data?.remaining===0?'本组已完成，可选择其他组继续。':'本批已处理完，可继续下一批。'}</p>{(query||folder)&&<Button variant="ghost" onClick={()=>{changeQuery('');setFolder('')}}>清除筛选</Button>}{!query&&!folder&&(data?.remaining??0)>0&&(data?.next_page!=null||data?.previous_page!=null)&&<Button variant="outline" onClick={()=>{advanceSequenceRef.current++;setPage((data.next_page??data.previous_page)!);setLinkFocus('')}}>继续剩余批次</Button>}{!query&&!folder&&nextAvailableGroup&&<Button variant="outline" onClick={()=>chooseGroup(nextAvailableGroup.key)}>下一待确认组 · {nextAvailableGroup.label}（{nextAvailableGroup.count}）</Button>}</div>:rows.map(t=><ReviewSummaryRow key={t.vid} task={t} selected={selected.has(t.vid)} active={focused?.vid===t.vid} disabled={disabled||!!t.review_blocked&&!selected.has(t.vid)} lock={lockedVids.get(t.vid)} onToggle={()=>toggle([t])} onInspect={()=>inspect(t)}/>)}
        </div>
        {chosen.length>0&&<div className="ws-bulk" aria-label="所选视频操作">
          <span className="ws-muted ws-bulk-count tabular-nums">已选 {chosen.length}</span>
          {reviewDesktop&&stageOpen?(
            <>
              <Button className="ws-icon ws-bulk-clear" variant="ghost" size="sm" title="清空当前组选择" aria-label="清空当前组选择" onClick={clear}><X size={14} aria-hidden="true"/></Button>
              <Button className="ws-bulk-delete ws-danger-btn" variant="outline" size="sm" aria-label={`将已选 ${chosen.length} 个视频移入回收站`} title={`移入回收站（${chosen.length} 个视频）`} disabled={disabled||!chosen.length||chosenLocked||chosen.some(t=>blockedIds.has(t.vid)||pendingVids.has(t.vid))} onClick={e=>openDeleteDialog('batch',chosen,e.currentTarget)}><Trash2 size={14} aria-hidden="true"/><span>删除</span></Button>
              <Button className="ws-bulk-inspect" variant="outline" size="sm" disabled={disabled||!chosen.length} onClick={openSelection}><ListChecks size={14} aria-hidden="true" className="shrink-0"/><span>核对所选</span></Button>
            </>
          ):(
            <>
              <Button className="ws-icon ws-bulk-clear" variant="ghost" size="sm" title="清空当前组选择" aria-label="清空当前组选择" onClick={clear}><X size={14} aria-hidden="true"/></Button>
              <Button className="ws-bulk-defer" variant="outline" size="sm" disabled={disabled||!chosen.length||chosenLocked} onClick={()=>void action(view==='deferred'?'restore':'defer',chosen.map(t=>t.vid))}>{view==='deferred'?'恢复':'暂缓'}</Button>
              <Button className="ws-bulk-delete ws-danger-btn" variant="outline" size="sm" aria-label={`将已选 ${chosen.length} 个视频移入回收站`} title={`移入回收站（${chosen.length} 个视频）`} disabled={disabled||!chosen.length||chosenLocked||chosen.some(t=>blockedIds.has(t.vid)||pendingVids.has(t.vid))} onClick={e=>openDeleteDialog('batch',chosen,e.currentTarget)}><Trash2 size={14} aria-hidden="true"/><span>删除</span></Button>
              {trimmedCreator&&<Button className="ws-bulk-confirm" size="sm" disabled={disabled||!chosen.length||chosenLocked||Boolean(creatorDestructiveLock)||chosen.some(t=>blockedIds.has(t.vid)||pendingVids.has(t.vid))} title={creatorDestructiveLock?`${trimmedCreator} ${creatorDestructiveLock.label}`:undefined} onClick={()=>void confirmTargets(chosen,creator)}><span className="ws-btn-truncate">归给 {trimmedCreator}</span><span className="ws-btn-badge">· <DigitSwap value={chosen.length}/></span></Button>}
              <Button className="ws-bulk-inspect" variant={trimmedCreator?'outline':'default'} size="sm" disabled={disabled||!chosen.length} onClick={openSelection}><ListChecks size={14} aria-hidden="true" className="shrink-0"/><span>核对所选</span></Button>
            </>
          )}
        </div>}
        <div className="ws-pagination" data-single-page={isSinglePage?'true':undefined}><span className="tabular-nums">{data&&groupMatched?`${data.remaining??data.total} 个待核对 · 第 ${currentPage+1} / ${Math.max(1,Math.ceil(data.total/data.limit))} 批`:activeGroupInfo?`${activeGroupInfo.count} 个待核对 · 读取中`:'读取中'}</span><div className="ws-actions"><Button className="ws-icon" variant="ghost" size="sm" aria-label="上一页" title="上一页" disabled={disabled||!groupMatched||(data?.session?data.previous_page==null:currentPage===0)} onClick={()=>{advanceSequenceRef.current++;setPage(data?.previous_page??currentPage-1);setLinkFocus('')}}><ChevronLeft size={15} aria-hidden="true"/></Button><Button className="ws-icon" variant="ghost" size="sm" aria-label="下一页" title="下一页" disabled={disabled||!groupMatched||!data||(data.session?data.next_page==null:(currentPage+1)*data.limit>=data.total)} onClick={()=>{advanceSequenceRef.current++;setPage(data?.next_page??currentPage+1);setLinkFocus('')}}><ChevronRight size={15} aria-hidden="true"/></Button></div></div>
      </section>
      {reviewDesktop&&stageOpen&&<>
        <div
          className="ws-resizer"
          role="separator"
          aria-orientation="vertical"
          aria-label="拖拽调整视频列表与舞台宽度"
          aria-valuemin={listBounds.min}
          aria-valuemax={listBounds.max}
          aria-valuenow={clampedListWidth}
          data-dragging={resizing||undefined}
          tabIndex={0}
          title="拖拽或使用 ←/→/Home/End 调整列表宽度"
          onPointerDown={e=>{
            if(e.button!==0)return
            e.preventDefault()
            const resizerEl=e.currentTarget
            const activePointerId=e.pointerId
            const startX=e.clientX,startW=clampedListWidth
            const minW=listBounds.min,maxW=listBounds.max
            let latestW=startW,rafId=0,finished=false
            try{resizerEl.setPointerCapture(activePointerId)}catch{}
            setResizing(true)
            const applyDom=()=>{
              rafId=0
              if(queueRef.current)queueRef.current.style.width=`${latestW}px`
              resizerEl.setAttribute('aria-valuenow',String(latestW))
            }
            const onMove=(ev:PointerEvent)=>{
              if(ev.pointerId!==activePointerId)return
              latestW=Math.max(minW,Math.min(maxW,Math.round(startW+(ev.clientX-startX))))
              if(!rafId)rafId=requestAnimationFrame(applyDom)
            }
            const finish=(ev?:Event)=>{
              if(finished)return
              if(ev&&'pointerId' in ev&&(ev as PointerEvent).pointerId!==activePointerId)return
              finished=true
              if(rafId){cancelAnimationFrame(rafId);rafId=0}
              applyDom()
              try{if(resizerEl.hasPointerCapture(activePointerId))resizerEl.releasePointerCapture(activePointerId)}catch{}
              window.removeEventListener('pointermove',onMove)
              window.removeEventListener('pointerup',finish)
              window.removeEventListener('pointercancel',finish)
              window.removeEventListener('blur',finish)
              resizerEl.removeEventListener('lostpointercapture',finish)
              setResizing(false)
              setListWidth(latestW)
            }
            window.addEventListener('pointermove',onMove)
            window.addEventListener('pointerup',finish)
            window.addEventListener('pointercancel',finish)
            window.addEventListener('blur',finish)
            resizerEl.addEventListener('lostpointercapture',finish)
          }}
          onKeyDown={e=>{
            if(e.key==='ArrowLeft'){e.preventDefault();setListWidth(Math.max(listBounds.min,clampedListWidth-16))}
            else if(e.key==='ArrowRight'){e.preventDefault();setListWidth(Math.min(listBounds.max,clampedListWidth+16))}
            else if(e.key==='Home'){e.preventDefault();setListWidth(listBounds.min)}
            else if(e.key==='End'){e.preventDefault();setListWidth(listBounds.max)}
          }}
        ><span className="ws-resizer-grip" aria-hidden="true"/></div>
        <section className="ws-stage-pane" aria-label="中间视频预览舞台">
          <div className="ws-stage-header">
            <div className="ws-stage-head-left">
              <h2 className="ws-stage-title" title={activeStageTitle}>{activeStageTitle}</h2>
              {activeStageSize&&<span className="ws-count ws-stage-size-badge">{activeStageSize}</span>}
            </div>
            {stageRefName&&<div className="ws-stage-tabs"><WorkspaceTabs value={activeStageTab} onChange={v=>{if(v==='reference')setStageRefIndex(0);setStageTab(v==='reference'?'reference':'current')}} label="预览模式" variant="segment" instant items={[{value:'current',label:'当前视频'},{value:'reference',label:'对照视频',count:stageRefData?stageRefData.total:undefined}]}/></div>}
            <div className="ws-stage-head-right">
              {activeStageTab==='reference'&&refVideos.length>0?<><Button className="ws-icon" variant="ghost" size="sm" aria-label="上一条对照视频" title="上一条对照视频" disabled={clampedRefIdx<=0} onClick={()=>setStageRefIndex(i=>Math.max(0,i-1))}><ChevronLeft size={14} aria-hidden="true"/></Button><span className="ws-muted tabular-nums">{`${clampedRefIdx+1} / ${refVideos.length}`}</span><Button className="ws-icon" variant="ghost" size="sm" aria-label="下一条对照视频" title="下一条对照视频" disabled={clampedRefIdx>=refVideos.length-1} onClick={()=>setStageRefIndex(i=>Math.min(refVideos.length-1,i+1))}><ChevronRight size={14} aria-hidden="true"/></Button></>:activeStageTab==='current'&&focused&&rows.length>0&&<><Button className="ws-icon" variant="ghost" size="sm" aria-label="上一条视频" title="上一条 ([)" disabled={focusedIndex<=0} onClick={()=>stepFocus(-1)}><ChevronLeft size={14} aria-hidden="true"/></Button><span className="ws-muted tabular-nums">{focusedIndex>=0?`${focusedIndex+1} / ${rows.length}`:'已查看'}</span><Button className="ws-icon" variant="ghost" size="sm" aria-label="下一条视频" title="下一条 (])" disabled={focusedIndex<0||focusedIndex>=rows.length-1} onClick={()=>stepFocus(1)}><ChevronRight size={14} aria-hidden="true"/></Button></>}
              {activeStageTab==='current'&&stageCurrentTask&&<Button className="ws-icon ws-danger-icon" variant="ghost" size="sm" aria-label="将当前视频移入回收站" title={stageCurrentLock?`${stageCurrentLock.label}，暂不可删除`:'删除当前视频（移入回收站）'} disabled={disabled||Boolean(stageCurrentLock)||blockedIds.has(stageCurrentTask.vid)||pendingVids.has(stageCurrentTask.vid)} onClick={e=>openDeleteDialog('single',[stageCurrentTask],e.currentTarget)}><Trash2 size={14} aria-hidden="true"/></Button>}
              <Button ref={stageExpandBtnRef} className="ws-icon" variant="ghost" size="sm" aria-label="居中放大预览视频" title="居中放大预览" disabled={!activeStageVideoPath} onClick={()=>{if(!activeStageVideoPath)return;lightboxTriggerRef.current=stageExpandBtnRef.current;setLightbox({open:true,tab:activeStageTab,initialTime:getMediaPosition(activeStageVideoPath)})}}><Maximize2 size={14} aria-hidden="true"/></Button>
              <Button className="ws-icon" variant="ghost" size="sm" aria-label="收起中间视频舞台" title="收起视频舞台" onClick={()=>setStageOpen(false)}><X size={15} aria-hidden="true"/></Button>
            </div>
          </div>
          <div className="ws-stage-player-slot">
            {lightbox.open?<div className="ws-stage-empty">正在居中放大窗口中预览…</div>:activeStageTab==='reference'?(activeRefVideo?<WorkspaceMedia key={`ref:${activeRefVideo.vid}`} path={activeRefVideo.path||activeRefVideo.vid} name={activeRefVideo.name} poster={activeRefVideo.thumb||undefined} fill showVolumeSlider initialTime={getMediaPosition(activeStageVideoPath)} onExpand={t=>{rememberMediaPosition(activeStageVideoPath,t);lightboxTriggerRef.current=stageExpandBtnRef.current;setLightbox({open:true,tab:'reference',initialTime:t})}}/>:stageRefLoading?<StageMediaLoader label="正在读取对照视频"/>:<div className="ws-stage-empty">{stageRefData?.error?<span>{stageRefData.error} <Button variant="outline" size="sm" onClick={retryStageRef}>重试</Button></span>:`${stageRefName||'当前人物'} 暂无已归档对照视频`}</div>):stageCurrentTask?<WorkspaceMedia key={`cur:${stageCurrentTask.vid}`} path={stageCurrentTask.path} name={stageCurrentTask.name} poster={stageCurrentPoster} fill showVolumeSlider initialTime={getMediaPosition(stageCurrentTask.path)} onExpand={t=>{rememberMediaPosition(stageCurrentTask.path,t);lightboxTriggerRef.current=stageExpandBtnRef.current;setLightbox({open:true,tab:'current',initialTime:t})}}/>:(!data||!groupMatched)&&!error?<StageMediaLoader label="正在读取视频预览"/>:<div className="ws-stage-empty">点击左侧列表中的视频开始预览</div>}
          </div>
          {activeStageTab==='reference'&&<div className="ws-stage-ref-strip">
            <div className="ws-stage-ref-head"><span>{stageRefName} · 已归档对照样本</span><span className="tabular-nums">{refVideos.length?`${clampedRefIdx+1} / ${refVideos.length}`:'0 个'}</span></div>
            {refVideos.length>0&&<div className="ws-stage-ref-row" role="group" aria-label="对照视频样本">{refVideos.map((v,idx)=><button key={v.vid} type="button" aria-pressed={idx===clampedRefIdx} className="ws-stage-ref-card" onClick={()=>setStageRefIndex(idx)} title={v.name}><span className="ws-stage-ref-thumb">{v.thumb?<img src={v.thumb} width={108} height={68} data-private-media draggable={false} alt="" loading="lazy"/>:<Play size={14} aria-hidden="true"/>}</span><span className="ws-stage-ref-name">{v.name}</span></button>)}</div>}
          </div>}
          {activeStageVideoPath&&<div className="ws-stage-footer">
            <div className="ws-stage-footer-path"><WorkspacePath value={activeStageVideoPath} compact/></div>
            <div className="ws-stage-footer-badges">{activeStageTab==='current'&&focused&&<span className="ws-stage-pill">{reviewListSummary(focused).status}</span>}{activeStageTab==='reference'&&stageRefName&&<span className="ws-stage-pill">对照 · {stageRefName}</span>}</div>
          </div>}
        </section>
      </>}
      {reviewDesktop&&<aside className="ws-inspector" aria-label="视频与归名">{detail}</aside>}
    </div>
    {!reviewDesktop&&<Dialog open={previewOpen} onOpenChange={open=>setPreviewOpen(open)}><ResponsiveDialogContent open={previewOpen} className="ws-detail-sheet" style={{animationDuration:instantEntry?'0ms':'180ms'}} overlayProps={{style:{animationDuration:instantEntry?'0ms':'140ms'}}} inert={!previewOpen} onCloseAutoFocus={e=>{e.preventDefault();const target=returnFocus.current;if(target?.isConnected&&!target.matches(':disabled')&&!target.closest('[inert]'))target.focus();else document.querySelector<HTMLButtonElement>('[aria-label="更新人工确认列表"]')?.focus()}} onEscapeKeyDown={e=>{if(reference){e.preventDefault();backReference()}}}><DialogTitle className="sr-only">{reference?`${reference.name} 的对照视频`:detailMode==='selection'?'核对所选视频':'视频与证据'}</DialogTitle><DialogDescription className="sr-only">关闭后保留筛选、勾选与列表位置。</DialogDescription>{detail}</ResponsiveDialogContent></Dialog>}
    <VideoPreviewDialog
      open={lightbox.open&&!!lightboxVideoPath}
      onOpenChange={open=>{if(!open)setLightbox(prev=>({...prev,open:false}))}}
      title={lightboxVideoName}
      badge={activeLightboxTab==='reference'&&stageRefName?<span className="ws-count">对照 · {stageRefName}</span>:undefined}
      src={`/api/video?p=${encodeURIComponent(lightboxVideoPath)}`}
      name={lightboxVideoName}
      poster={lightboxPoster}
      path={lightboxVideoPath}
      initialTime={lightbox.initialTime}
      onTimeRemember={t=>{if(lightboxVideoPath)rememberMediaPosition(lightboxVideoPath,t)}}
      headerTabs={stageRefName?<WorkspaceTabs value={activeLightboxTab} onChange={v=>{setLightbox(prev=>({...prev,tab:v==='reference'?'reference':'current',initialTime:0}));setStageTab(v==='reference'?'reference':'current')}} label="预览模式" variant="segment" instant items={[{value:'current',label:'当前视频'},{value:'reference',label:'对照视频',count:stageRefData?stageRefData.total:undefined}]}/>:undefined}
      onPrev={()=>lightboxStep(-1)}
      onNext={()=>lightboxStep(1)}
      hasPrev={lightboxHasPrev}
      hasNext={lightboxHasNext}
      counterText={activeLightboxTab==='reference'?(refVideos.length?`${clampedRefIdx+1} / ${refVideos.length}`:undefined):(focusedIndex>=0?`${focusedIndex+1} / ${rows.length}`:undefined)}
      returnFocus={lightboxTriggerRef}
      footerActions={activeLightboxTab==='current'&&stageCurrentTask?<Button className="ws-danger-btn" variant="outline" size="sm" disabled={disabled||Boolean(stageCurrentLock)||blockedIds.has(stageCurrentTask.vid)||pendingVids.has(stageCurrentTask.vid)} onClick={()=>openDeleteDialog('single',[stageCurrentTask],stageExpandBtnRef.current)}><Trash2 size={14} aria-hidden="true"/><span>移入回收站</span></Button>:undefined}
      stripSlot={activeLightboxTab==='reference'&&refVideos.length>1?<div className="ws-stage-ref-strip"><div className="ws-stage-ref-row" role="group" aria-label="对照视频样本">{refVideos.map((v,idx)=><button key={v.vid} type="button" aria-pressed={idx===clampedRefIdx} className="ws-stage-ref-card" onClick={()=>setStageRefIndex(idx)} title={v.name}><span className="ws-stage-ref-thumb">{v.thumb?<img src={v.thumb} width={108} height={68} data-private-media draggable={false} alt="" loading="lazy"/>:<Play size={14} aria-hidden="true"/>}</span><span className="ws-stage-ref-name">{v.name}</span></button>)}</div></div>:undefined}
    />
    <AutoMergeDialog open={autoMergeOpen} onOpenChange={setAutoMergeOpen} preview={autoMergePreview} loading={autoMergeLoading} onRefreshPreview={loadAutoMergePreview} returnFocus={autoMergeTriggerRef}/>
    <ReviewDeleteDialog
      open={Boolean(deleteDialog?.open)}
      onOpenChange={open=>setDeleteDialog(prev=>prev?{...prev,open}:null)}
      mode={deleteDialog?.mode??'single'}
      targets={deleteDialog?.targets??[]}
      groupLabel={activeGroupInfo?.label}
      isAutoSelected={autoSelectedScopes.has(scope)}
      otherSelectedCount={deleteDialog?.mode==='single'?chosen.filter(t=>!deleteDialog.targets.some(target=>target.vid===t.vid)).length:0}
      reviewSession={data?.session??session.current}
      reviewGroup={{key:activeGroup,deferred:view==='deferred'}}
      onConfirm={confirmDeletePlan}
      onSessionExpired={()=>{clearReviewSessionCaches(true);setSessionExpired(true)}}
      returnFocus={deleteTriggerRef}
    />
    <AlertDialog open={refreshConfirm} onOpenChange={setRefreshConfirm}><AlertDialogContent onCloseAutoFocus={e=>{e.preventDefault();refreshFocus.current?.focus({preventScroll:true})}}><AlertDialogHeader><AlertDialogTitle>更新当前核对列表？</AlertDialogTitle><AlertDialogDescription>后台的新建议可能调整分组。更新会清空勾选、未保存的备注和人物输入。取消可继续当前核对。</AlertDialogDescription></AlertDialogHeader><AlertDialogFooter><AlertDialogCancel>继续当前核对</AlertDialogCancel><AlertDialogAction onClick={()=>void load({refresh:true})}>更新列表</AlertDialogAction></AlertDialogFooter></AlertDialogContent></AlertDialog>
  </main>
}
