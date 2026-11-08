'use client'

import { PrivateImage, PrivateVideo } from '@/components/private-media'
import { useCallback, useEffect, useRef, useState } from 'react'
import Link from 'next/link'
import { AnimatePresence } from 'motion/react'
import { WorkbenchMotion, WorkbenchNotice, WorkbenchStep } from './motion'
import { useTheme } from '@/components/theme-provider'
import { Archive, ArrowDownAZ, ArrowRight, Check, ChevronRight, Copy, FileVideo, Folder, FolderOpen, Grid2X2, Pencil, Play, RefreshCw, Search, ShieldCheck, Sparkles, Trash2, Undo2, X } from 'lucide-react'
import { Dialog, DialogDescription, DialogTitle } from '@/components/ui/dialog'
import { api } from '@/lib/face-api'
import { decodedName } from '@/lib/review-logic'
import type { PlanRow } from '@/lib/review-plan'
import type { ReviewVideo } from '@/lib/review-ui'
import type { LibraryPerson, PersonLibrary, PersonVideo, PersonDirectoryStats } from '@/lib/person-library-model'
import { ExternalProfiles } from './external-profiles'
import { PeachPersonLink } from './peach-link'
import { AccountAssociation } from './account-association'
import { CircleHelp, Link2, Unlink2 } from 'lucide-react'
import { WorkbenchButton as Button, WorkbenchInput as Input, WorkbenchBadge as Badge, WorkbenchCheckbox as Checkbox, WorkbenchCount as Count, WorkbenchDialogContent as DialogContent } from './ui'
import { PersonSearch } from './person-search'
import { PeopleActionDialog } from './people-actions'
import type { PersonBatchAction, PersonBatchResult } from '@/lib/person-batch-model'
import { FileActionDialog, FILE_OPERATION_STORAGE_KEY, type FileActionSelection } from './file-actions'
import type { PersonFileAction, PersonFileResult, PersonFilePlan } from '@/lib/person-file-model'
import { FolderInput, Merge } from 'lucide-react'
import { sortedLibraryPeople, type PersonSort } from '@/lib/person-list-model'
import { readWorkbenchCache, writeWorkbenchCache, deleteWorkbenchCache } from '@/components/workbench/state'
import './tokens.css'
import './workbench.css'

interface Plan {rows:PlanRow[];version:string}
type Undo = {kind:'queue';name:string;vid:string}|{kind:'reject';name:string;vid:string;id:string}
const sizeText=(size:number)=>size>=2**40?`${(size/2**40).toFixed(2)} TB`:size>=2**30?`${(size/2**30).toFixed(2)} GB`:size>=2**20?`${(size/2**20).toFixed(1)} MB`:size>=1024?`${(size/1024).toFixed(1)} KB`:`${size} B`
const timeText=(duration?:number)=>duration&&Number.isFinite(duration)?`${Math.floor(duration/60)}:${String(Math.floor(duration%60)).padStart(2,'0')}`:'时长未记录'
const queueVideo=(row:PlanRow):PersonVideo=>({vid:row.vid,path:row.src||row.vid,name:row.name,folder:'.',size:row.size,size_gb:row.size_gb,modified_at:'',thumb:row.thumb,evidence:[],state:'queued'})

export function PersonWorkbench({initialName=''}:{initialName?:string}) {return <WorkbenchMotion><PersonWorkbenchContent initialName={initialName}/></WorkbenchMotion>}
function PersonWorkbenchContent({initialName=''}:{initialName?:string}) {
  const [selectedPeople,setSelectedPeople]=useState<string[]>([]),[peopleAction,setPeopleAction]=useState<PersonBatchAction|null>(null)
  const [accountPerson,setAccountPerson]=useState<string|null>(null),[accountsRevision,setAccountsRevision]=useState(0)
  const [people,setPeople]=useState<LibraryPerson[]>(()=>readWorkbenchCache<LibraryPerson[]>('persons:list')??[]),[peopleError,setPeopleError]=useState(''),[peopleLoading,setPeopleLoading]=useState(()=>!readWorkbenchCache<LibraryPerson[]>('persons:list'))
  const [peopleStats,setPeopleStats]=useState<Record<string,PersonDirectoryStats>>(()=>readWorkbenchCache<Record<string,PersonDirectoryStats>>('persons:stats')??{}),[statsLoaded,setStatsLoaded]=useState(()=>!!readWorkbenchCache<Record<string,PersonDirectoryStats>>('persons:stats')),[statsLoading,setStatsLoading]=useState(false),[statsError,setStatsError]=useState(''),[peopleSort,setPeopleSort]=useState<PersonSort>('recent')
  const statsSequence=useRef(0),statsAbort=useRef<AbortController|null>(null)
  const [name,setName]=useState(()=>decodedName(initialName)||readWorkbenchCache<string>('persons:selected')||readWorkbenchCache<LibraryPerson[]>('persons:list')?.[0]?.name||''),[peopleQuery,setPeopleQuery]=useState('')
  const [data,setData]=useState<PersonLibrary|null>(()=>{const initial=decodedName(initialName)||readWorkbenchCache<string>('persons:selected')||readWorkbenchCache<LibraryPerson[]>('persons:list')?.[0]?.name||'';return initial?readWorkbenchCache<PersonLibrary>('persons:library:'+initial)??null:null}),[loading,setLoading]=useState(false),[error,setError]=useState('')
  const [plan,setPlan]=useState<Plan|null>(()=>readWorkbenchCache<Plan>('integration:plan')??null),[planError,setPlanError]=useState(''),[planLoading,setPlanLoading]=useState(false)
  const [tab,setTab]=useState<'owned'|'queued'>('owned'),[folder,setFolder]=useState('*'),[query,setQuery]=useState(''),[descending,setDescending]=useState(false)
  const [selected,setSelected]=useState<string[]>([]),[limit,setLimit]=useState(60),[recommendationLimit,setRecommendationLimit]=useState(20)
  const [later,setLater]=useState<string[]>([]),[mobile,setMobile]=useState<'people'|'library'|'suggestions'>('library')
  const [fileAction,setFileAction]=useState<FileActionSelection|null>(null),[pendingFilePlan,setPendingFilePlan]=useState<PersonFilePlan|null>(null)
  useEffect(()=>{
    try{const raw=localStorage.getItem(FILE_OPERATION_STORAGE_KEY);if(raw){const plan=JSON.parse(raw) as PersonFilePlan;if(plan.backend==='rust'&&plan.id&&plan.request?.id===plan.id){setPendingFilePlan(plan);setFileAction({action:plan.action,files:[],resumePlan:plan})}}}catch{/* A malformed browser record never triggers a submission. */}
  },[])
  const [playing,setPlaying]=useState<ReviewVideo|null>(null),[busy,setBusy]=useState(false),[message,setMessage]=useState(''),[undo,setUndo]=useState<Undo|null>(null)
  const [confirmArchive,setConfirmArchive]=useState<{rows:PlanRow[];version:string;name:string}|null>(null)
  const { resolved: theme } = useTheme()
  const nameRef=useRef(name),sequence=useRef(0),planSequence=useRef(0),mounted=useRef(true)
  nameRef.current=name
  useEffect(()=>{if(name)writeWorkbenchCache('persons:selected',name)},[name])
  useEffect(()=>{mounted.current=true;return()=>{mounted.current=false}},[])
  const loadStats=useCallback(async(force=false)=>{
    statsAbort.current?.abort();const controller=new AbortController();statsAbort.current=controller;const token=++statsSequence.current
    if(force||!readWorkbenchCache('persons:stats'))setStatsLoading(true)
    setStatsError('')
    try{const result=await api<{stats:Record<string,PersonDirectoryStats>}>(`/api/persons/library?stats=1${force?'&refresh=1':''}`,{signal:controller.signal});if(mounted.current&&token===statsSequence.current){writeWorkbenchCache('persons:stats',result.stats);setPeopleStats(result.stats);setStatsLoaded(true)}}
    catch(e){if(mounted.current&&!controller.signal.aborted&&token===statsSequence.current)setStatsError((e as Error).message)}
    finally{if(mounted.current&&!controller.signal.aborted&&token===statsSequence.current)setStatsLoading(false)}
  },[])
  useEffect(()=>{void loadStats();return()=>statsAbort.current?.abort()},[loadStats])
  const loadPeople=useCallback(async()=>{
    if(!readWorkbenchCache('persons:list'))setPeopleLoading(true)
    setPeopleError('')
    try {const result=await api<{persons:LibraryPerson[]}>('/api/persons/library');if(mounted.current){writeWorkbenchCache('persons:list',result.persons);setPeople(result.persons);setSelectedPeople(current=>current.filter(name=>result.persons.some(p=>p.name===name)));setName(current=>current||result.persons[0]?.name||'')}return result.persons}
    catch(e){if(mounted.current)setPeopleError((e as Error).message)}finally{if(mounted.current)setPeopleLoading(false)}
  },[])
  useEffect(()=>{void loadPeople()},[loadPeople])
  useEffect(()=>{if(initialName&&decodedName(initialName)!==nameRef.current){const nextName=decodedName(initialName);setData(readWorkbenchCache<PersonLibrary>('persons:library:'+nextName)??null);setName(nextName)}},[initialName])
  const refresh=useCallback(async(target:string,signal?:AbortSignal)=>{
    const token=++sequence.current
    const cached=readWorkbenchCache<PersonLibrary>('persons:library:'+target)
    if(!cached)setLoading(true)
    setError('')
    try {const next=await api<PersonLibrary>(`/api/persons/library?name=${encodeURIComponent(target)}`,{signal});if(mounted.current&&nameRef.current===target&&token===sequence.current){writeWorkbenchCache('persons:library:'+target,next);writeWorkbenchCache('persons:library:'+next.name,next);setData(next);if(next.name!==target){setName(next.name);history.replaceState(null,'',`/persons/${encodeURIComponent(next.name)}`)};setSelected(prev=>prev.filter(id=>next.files.some(v=>v.vid===id)));setPeople(prev=>{const updated=prev.map(p=>p.name===target?{...p,queued:next.queued}:p);writeWorkbenchCache('persons:list',updated);return updated})}return next}
    catch(e){if(!signal?.aborted&&mounted.current&&nameRef.current===target&&token===sequence.current)setError((e as Error).message);throw e}
    finally{if(mounted.current&&token===sequence.current&&!signal?.aborted)setLoading(false)}
  },[])
  useEffect(()=>{
    setData(name?readWorkbenchCache<PersonLibrary>('persons:library:'+name)??null:null);setSelected([]);setQuery('');setFolder('*');setLater([]);setLimit(60);setRecommendationLimit(20);setUndo(null);setError('');setPlanError('')
    if(!name)return
    const controller=new AbortController();void refresh(name,controller.signal).catch(()=>{});return()=>controller.abort()
  },[name,refresh])
  const loadPlan=useCallback(async(target:string,signal?:AbortSignal)=>{
    const token=++planSequence.current
    if(!readWorkbenchCache('integration:plan'))setPlanLoading(true)
    setPlanError('')
    try{const next=await api<Plan>('/api/plan/preview',{signal});writeWorkbenchCache('integration:plan',next);if(mounted.current&&nameRef.current===target&&!signal?.aborted&&token===planSequence.current)setPlan(next);return next}
    catch(e){if(mounted.current&&nameRef.current===target&&!signal?.aborted&&token===planSequence.current)setPlanError((e as Error).message);throw e}
    finally{if(mounted.current&&nameRef.current===target&&!signal?.aborted&&token===planSequence.current)setPlanLoading(false)}
  },[])
  useEffect(()=>{if(tab!=='queued'||!name)return;const controller=new AbortController();void loadPlan(name,controller.signal).catch(()=>{});return()=>controller.abort()},[name,tab,loadPlan])
  const person=people.find(p=>p.name===name)
  const visiblePeople=sortedLibraryPeople(people.map(p=>({...p,...(Object.hasOwn(peopleStats,p.name)?peopleStats[p.name]:statsLoaded&&!statsLoading&&!statsError?{video_count:0,total_bytes:0}:{}),...(p.name===name&&data?{video_count:data.files.length,total_bytes:data.files.reduce((sum,file)=>sum+file.size,0),stats_error:undefined}:{})})),peopleQuery,peopleSort)
  const queueRows=(plan?.rows||[]).filter(r=>r.creator===name),queueMap=new Map(queueRows.map(row=>[row.vid,row]))
  const allFiles=tab==='owned'?(data?.files||[]):queueRows.map(queueVideo)
  const filtered=allFiles.filter(v=>(tab==='queued'||folder==='*'||v.folder===folder)&&`${v.name} ${v.path}`.toLowerCase().includes(query.toLowerCase())).sort((a,b)=>(descending?-1:1)*a.name.localeCompare(b.name,'zh-CN'))
  const visible=filtered.slice(0,limit),chosen=queueRows.filter(r=>selected.includes(r.vid)),canArchive=chosen.length>0&&chosen.every(r=>r.status==='ok')
  const candidates=[...(data?.recommendations||[])].sort((a,b)=>Number(later.includes(a.vid))-Number(later.includes(b.vid))||later.indexOf(a.vid)-later.indexOf(b.vid))
  function selectPerson(next:string){if(busy||next===name)return;setMessage('');setData(readWorkbenchCache<PersonLibrary>('persons:library:'+next)??null);setSelected([]);setName(next);setTab('owned');setMobile('library');history.replaceState(null,'',`/persons/${encodeURIComponent(next)}`)}
  function switchTab(next:'owned'|'queued'){setTab(next);setSelected([]);setQuery('');setFolder('*');setLimit(60)}
  function toggle(vid:string){setSelected(prev=>prev.includes(vid)?prev.filter(v=>v!==vid):[...prev,vid])}
  function notify(text:string){setMessage(text)}
  async function reloadAfterSave(target:string){await refresh(target).catch(()=>notify('操作已保存，列表刷新失败，请点刷新，不要重复提交。'));if(tab==='queued')await loadPlan(target).catch(()=>{})}
  async function enqueue(video:PersonVideo){
    if(!busy)window.location.assign('/review?vid='+encodeURIComponent(video.vid))
  }
  async function reject(video:PersonVideo){
    if(busy)return;const target=name;setBusy(true);setUndo(null)
    try{const result=await api<{id:string}>('/api/persons/recommendations',{method:'POST',body:JSON.stringify({name:target,vid:video.vid})});setUndo({kind:'reject',name:target,vid:video.vid,id:result.id});notify('已记为“不是此人”，不再向当前人物推荐。');await reloadAfterSave(target)}catch(e){notify((e as Error).message)}finally{setBusy(false)}
  }
  async function undoLast(){
    if(!undo||busy)return;const action=undo;setBusy(true)
    try{await api(action.kind==='queue'?'/api/plan/queue':'/api/persons/recommendations',{method:'DELETE',body:JSON.stringify(action.kind==='queue'?{vids:[action.vid]}:action)});setUndo(null);notify('已撤销，视频可重新核对。');await reloadAfterSave(action.name)}catch(e){notify((e as Error).message)}finally{setBusy(false)}
  }
  async function returnToReview(vids:string[]){
    if(busy||!vids.length)return;const target=name;setBusy(true);setUndo(null)
    try{await api('/api/plan/queue',{method:'DELETE',body:JSON.stringify({vids})});notify('已撤销人物归名，文件没有移动。');setSelected([]);await reloadAfterSave(target)}catch(e){notify((e as Error).message)}finally{setBusy(false)}
  }
  async function applyArchive(){
    if(!confirmArchive||busy)return;const snapshot=confirmArchive;setConfirmArchive(null);setBusy(true);setUndo(null)
    try{const result=await api<{moved:number;errors?:string[]}>('/api/plan/apply',{method:'POST',body:JSON.stringify({vids:snapshot.rows.map(v=>v.vid),version:snapshot.version})});notify(`已归档 ${result.moved} 个。${result.errors?.length?result.errors.join('；'):'Peach 账本已同步。'}`);setSelected([]);await reloadAfterSave(snapshot.name)}catch(e){notify((e as Error).message);await loadPlan(snapshot.name).catch(()=>{})}finally{setBusy(false)}
  }
  async function rolledBackFileAction(){
    const current=nameRef.current
    // Refresh observations after compensation without applying success-only selection or notice changes.
    await Promise.allSettled([loadPeople(),loadStats(true),...(current?[refresh(current)]:[]),...(current&&tab==='queued'?[loadPlan(current)]:[])])
  }
  async function savedFileAction(result:PersonFileResult){
    setSelected([]);setUndo(null)
    deleteWorkbenchCache('persons:library:'+name)
    notify(result.warning|| (fileAction?.action==='merge-person'?`已将 ${name} 合并到 ${result.person}`:fileAction?.action==='rename-person'?`人物已重命名为 ${result.person}`:result.staged?`已将 ${result.changed} 个视频移入全局暂存区`:`已更新 ${result.changed} 个文件，路径和记录已同步`))
    await loadPeople();void loadStats(true)
    if(result.person!==name){deleteWorkbenchCache('persons:library:'+result.person);setData(null);setPlan(null);setName(result.person);setTab('owned');history.replaceState(null,'',`/persons/${encodeURIComponent(result.person)}`)}
    else await reloadAfterSave(name)
  }
  async function savedPeopleAction(result:PersonBatchResult){
    for(const item of result.completed)deleteWorkbenchCache('persons:library:'+item)
    if(result.target)deleteWorkbenchCache('persons:library:'+result.target)
    setSelectedPeople(result.remaining);setSelected([]);setUndo(null);setData(null);setPlan(null)
    const currentPeople=await loadPeople();void loadStats(true);setAccountsRevision(n=>n+1)
    if(result.target&&result.ok)setMobile('library')
    const next=result.target||(currentPeople?.some(p=>p.name===name)?name:currentPeople?.[0]?.name||'')
    if(next){setName(next);history.replaceState(null,'',`/persons/${encodeURIComponent(next)}`);await reloadAfterSave(next)}
    else {setName('');history.replaceState(null,'','/persons')}
    notify(result.ok?peopleAction==='merge'?`已合并 ${result.completed.length} 个人物，保留 ${result.target}`:`已删除 ${result.completed.length} 个人物，目录文件已移入暂存区`:`已完成 ${result.completed.length} 项，剩余 ${result.remaining.length} 项未完成`)
  }
  const manageFiles=(action:PersonFileAction,files:PersonVideo[])=>{if(pendingFilePlan){setFileAction({action:pendingFilePlan.action,files:[],resumePlan:pendingFilePlan});return};setMessage('');setUndo(null);setFileAction({action,files})}
  const count=tab==='owned'?data?.files.length:plan?queueRows.length:data?.queued??person?.queued
  return <div className="pw pw-directory pw-live" data-theme={theme}>
    <div className="pw-surface"><header className="pw-top"><div className="pw-brand"><strong>人物库</strong>{name&&<><span>/</span><span className="pw-current-person">{name}</span></>}</div>{pendingFilePlan&&<Button onClick={()=>setFileAction({action:pendingFilePlan.action,files:[],resumePlan:pendingFilePlan})}>查看文件操作</Button>}<Button className="pw-icon pw-refresh" aria-label="刷新人物和视频" disabled={busy||loading||peopleLoading} onClick={()=>{void loadPeople();void loadStats(true);if(name){void refresh(name).catch(()=>{});if(tab==='queued')void loadPlan(name).catch(()=>{})}}}><RefreshCw size={15}/></Button></header>
      <nav className="pw-mobile-nav" aria-label="人物工作区">{(['people','library','suggestions'] as const).map((section,i)=><Button key={section} data-active={mobile===section} onClick={()=>setMobile(section)}>{['人物','视频目录','推荐加入'][i]}</Button>)}</nav>
      <div className="pw-layout" data-mobile={mobile}>
        <aside className="pw-people"><div className="pw-panel-title"><h2>人物目录</h2><span><Count value={people.length}/></span></div><PersonSearch value={peopleQuery} onChange={setPeopleQuery} sort={peopleSort} onSortChange={setPeopleSort} theme={theme}/><div className="pw-people-select-all"><Checkbox label="全选" aria-label="选择当前搜索结果" checked={visiblePeople.length>0&&visiblePeople.every(p=>selectedPeople.includes(p.name))} indeterminate={visiblePeople.some(p=>selectedPeople.includes(p.name))&&!visiblePeople.every(p=>selectedPeople.includes(p.name))} disabled={busy||!visiblePeople.length} onCheckedChange={checked=>setSelectedPeople(current=>checked?[...new Set([...current,...visiblePeople.map(p=>p.name)])]:current.filter(name=>!visiblePeople.some(p=>p.name===name)))}/></div><div className="pw-person-list">
          {peopleLoading&&!people.length&&<p className="pw-load" role="status">正在加载人物…</p>}
          {peopleError&&<ErrorBox text={peopleError} retry={()=>void loadPeople()}/>}
          {visiblePeople.map(p=><div key={p.name} className="pw-person-row" data-selected={p.name===name} data-checked={selectedPeople.includes(p.name)}><Checkbox className="pw-person-checkbox" aria-label={`选择人物 ${p.name}`} checked={selectedPeople.includes(p.name)} disabled={busy} onCheckedChange={checked=>setSelectedPeople(current=>checked?[...new Set([...current,p.name])]:current.filter(n=>n!==p.name))}/><Button className="pw-person" aria-pressed={p.name===name} disabled={busy} onClick={()=>selectPerson(p.name)}><span className="pw-avatar">{p.rep_thumb?<img data-private-media draggable={false} src={p.rep_thumb} alt="" loading="lazy"/>:p.name.slice(0,1)}</span><span className="pw-person-copy"><strong title={p.name}>{p.name}</strong><small className="pw-person-stat" title={p.stats_error||undefined}>{p.video_count!=null&&p.total_bytes!=null?<><span>{p.video_count.toLocaleString('zh-CN')} 个视频</span><span>{sizeText(p.total_bytes)}</span></>:<span>{p.stats_error?'统计不可用':statsError?'统计未加载':'统计中…'}</span>}</small></span>{p.queued>0&&<Badge className="pw-counter" title={`${p.queued} 个待归档`} aria-label={`${p.queued} 个待归档`}>{p.queued}</Badge>}</Button><Button className="pw-account-link" data-linked={(p.account_count||0)>0} disabled={busy} title={p.account_error||(p.account_count==null?'关联状态未读取':p.account_count?`已手动关联 ${p.account_count} 个账号`:'关联账号')} aria-label={`${p.name}：${p.account_count==null?'关联状态未读取':p.account_count?`已关联 ${p.account_count} 个账号`:'未关联账号'}，管理关联`} onClick={()=>setAccountPerson(p.name)}><span>{p.account_count==null?<CircleHelp size={15}/>:p.account_count?<Link2 size={15}/>:<Unlink2 size={15}/>}{p.account_count!=null&&p.account_count>0&&<b>{p.account_count}</b>}</span></Button></div>)}
          {!peopleLoading&&!peopleError&&!visiblePeople.length&&<p className="pw-load">{peopleQuery?'没有匹配的人物。':'暂无人物，先确认视频归属。'}</p>}
        </div><div className="pw-person-foot">
          {selectedPeople.length>0?<WorkbenchStep phase="selected" className="pw-people-bulk"><div className="pw-people-selection"><span className="pw-selection-count" aria-live="polite" aria-atomic="true">已选 <Count value={selectedPeople.length}/> 人</span><Button className="pw-clear-people" disabled={busy} aria-label="清空人物选择" title="清空选择" onClick={()=>{setSelectedPeople([]);document.querySelector<HTMLButtonElement>('.pw-people [aria-label="选择当前搜索结果"]')?.focus()}}><X size={14}/></Button></div><div className="pw-people-bulk-actions" role="group" aria-label="人物批量操作"><Button aria-label="删除所选人物" disabled={busy||selectedPeople.length>50} onClick={()=>setPeopleAction('delete')}><Trash2 size={14}/>删除</Button><Button aria-label="合并所选人物" disabled={busy||selectedPeople.length<2||selectedPeople.length>50} title="勾选至少两个人物后合并" onClick={()=>setPeopleAction('merge')}><Merge size={14}/>合并</Button></div>{selectedPeople.length>50&&<small>单次最多操作 50 个人物</small>}</WorkbenchStep>:<div className="pw-foot-note">{visiblePeople.length} / {people.length} 人</div>}
          {(statsLoading||statsError)&&<div className="pw-foot-note">{statsLoading?<p role="status">正在统计视频数量和体积…</p>:<p role="alert">统计未更新<Button onClick={()=>void loadStats(true)}>重试</Button></p>}</div>}
        </div></aside>
        <main className="pw-library"><div className="pw-library-head"><div><h1>{name||'选择人物'}<span>的影像库</span><Button className="pw-person-rename" aria-label="重命名人物" title="重命名人物" disabled={busy||loading||!name} onClick={()=>manageFiles('rename-person',[])}><Pencil size={14}/></Button></h1>{name&&<p><FolderOpen size={13}/><span className="pw-library-location" title={data?.root}>creators / {name}</span><span>·</span>{data?`${data.files.length} 个视频`:'读取中'}</p>}</div><PeachPersonLink name={name} refreshKey={data}/></div>
          {name&&<ExternalProfiles name={name} compact refreshKey={accountsRevision} onManage={()=>setAccountPerson(name)}/>}
          <div className="pw-tabs" role="group" aria-label="文件状态"><Button data-active={tab==='owned'} onClick={()=>switchTab('owned')} disabled={busy}><Grid2X2 size={14}/>已有视频<Badge>{data?.files.length??'—'}</Badge></Button><Button data-active={tab==='queued'} onClick={()=>switchTab('queued')} disabled={busy}><Archive size={14}/>待归档<Badge>{plan?queueRows.length:data?.queued??person?.queued??'—'}</Badge></Button></div>
          <div className="pw-toolbar"><label className="pw-search"><Search size={14}/><Input aria-label="搜索视频" placeholder="搜索文件名或路径…" value={query} onChange={e=>{setQuery(e.target.value);setLimit(60);setSelected([])}}/></label><Button className="pw-icon" aria-label={descending?'按名称正序':'按名称倒序'} onClick={()=>setDescending(v=>!v)}><ArrowDownAZ size={16}/></Button>{<Button className="pw-text-button" disabled={busy||!visible.length} onClick={()=>setSelected(visible.every(v=>selected.includes(v.vid))?selected.filter(id=>!visible.some(v=>v.vid===id)):[...new Set([...selected,...visible.map(v=>v.vid)])])}>选择本页</Button>}</div>
          {tab==='owned'&&data&&<div className="pw-folders"><Button data-active={folder==='*'} onClick={()=>{setFolder('*');setLimit(60);setSelected([])}}><Folder size={13}/>全部视频</Button>{data.folders.some(f=>f.name!=='.')&&data.folders.map(f=><Button key={f.name} data-active={folder===f.name} onClick={()=>{setFolder(f.name);setLimit(60);setSelected([])}}><Folder size={13}/>{f.name==='.'?'根目录':f.name}<span>{f.count}</span></Button>)}</div>}
          <div className="pw-library-scroll" key={`${name}-${tab}`}>
            {(tab==='owned'?loading:planLoading)&&<p role="status" className="pw-load">{tab==='owned'?'读取人物目录…':'检查待归档路径…'}</p>}
            {(tab==='owned'?error:planError)&&<ErrorBox text={tab==='owned'?error:planError} retry={()=>{if(tab==='owned')void refresh(name).catch(()=>{});else void loadPlan(name).catch(()=>{})}}/>}
            <div className="pw-section-heading"><span>{tab==='owned'?(folder==='*'?'全部视频':folder==='.'?'根目录':folder):'已确认，尚未归档'}</span><small>{filtered.length} 个{filtered.length!==(count||0)?'匹配结果':''}</small></div>
            <div className="pw-clips">{visible.map(video=><VideoCard key={video.vid} video={video} queue={queueMap.get(video.vid)} queued={tab==='queued'} selected={selected.includes(video.vid)} busy={busy} onToggle={()=>toggle(video.vid)} onPlay={()=>setPlaying(video)} onReturn={()=>void returnToReview([video.vid])} onManage={action=>manageFiles(action,[video])} notify={notify}/>)}</div>
            {filtered.length>visible.length&&<Button className="pw-load-more" onClick={()=>setLimit(n=>n+60)}>再显示 60 个（已显示 {visible.length} / {filtered.length}）</Button>}
            {!(tab==='owned'?loading||error:planLoading||planError)&&!filtered.length&&<div className="pw-empty pw-empty-library"><FolderOpen size={30}/><h3>{query?'没有匹配的视频':tab==='queued'?'当前人物没有待归档任务':'这个人物目录暂时没有视频'}</h3><p>{tab==='queued'?'核对右侧候选后，确认归名即可进入这里。':data&&!data.directory_exists?'第一次归档时会创建人物目录。':'右侧推荐经确认和归档后会出现在这里。'}</p></div>}
          </div>
          {tab==='owned'&&selected.length>0&&<div className="pw-archive-actions"><span>已选 {selected.length} 个{selected.length>500?' · 单次最多 500 个':''}</span><div><Button disabled={busy||selected.length>500} className="pw-outline" onClick={()=>manageFiles('move-files',allFiles.filter(file=>selected.includes(file.vid)))}><FolderInput size={14}/>移动所选</Button><Button disabled={busy||selected.length>500} className="pw-danger" onClick={()=>manageFiles('delete-files',allFiles.filter(file=>selected.includes(file.vid)))}><Trash2 size={14}/>删除所选</Button></div></div>}
          {tab==='queued'&&<div className="pw-archive-actions"><span>已选 {chosen.length} 个{chosen.some(r=>r.status!=='ok')?' · 含需处理的项目':''}</span><div><Button disabled={busy||!chosen.length} className="pw-outline" onClick={()=>void returnToReview(chosen.map(r=>r.vid))}>撤销归名</Button><Button className="pw-primary" disabled={busy||planLoading||!canArchive||!plan} onClick={()=>setConfirmArchive({rows:chosen,version:plan!.version,name})}>归档所选<ArrowRight size={14}/></Button></div></div>}
        </main>
        <aside className="pw-recommendations"><div className="pw-rec-title"><div><Sparkles size={17}/><h2>推荐加入</h2><Badge>{candidates.length}</Badge></div></div><div className="pw-rec-scroll">
          {loading&&!data&&<p className="pw-load" role="status">读取推荐依据…</p>}
          {error&&<ErrorBox text={error} retry={()=>void refresh(name).catch(()=>{})}/>}
          {candidates.slice(0,recommendationLimit).map(video=><Recommendation key={video.vid} video={video} name={name} busy={busy} notify={notify} play={()=>setPlaying(video)} add={()=>void enqueue(video)} reject={()=>void reject(video)} later={()=>{setLater(prev=>[...prev.filter(id=>id!==video.vid),video.vid]);notify('已放到本次推荐末尾，稍后核对。')}}/>)}
          {candidates.length>recommendationLimit&&<Button className="pw-load-more" onClick={()=>setRecommendationLimit(n=>n+20)}>再显示 20 条推荐</Button>}
          {!loading&&!error&&!candidates.length&&<div className="pw-empty"><Check size={26}/><h3>暂时没有待核对推荐</h3><p>已确认的可到“待归档”查看。</p></div>}
        </div><div className="pw-rec-footer">确认归属 → 待归档 → 执行文件归档</div></aside>
      </div>
    </div>
    <AnimatePresence initial={false}>{message&&<WorkbenchNotice><span>{message}</span>{undo&&<Button disabled={busy} onClick={()=>void undoLast()}><Undo2 size={13}/>撤销这次</Button>}<Button aria-label="关闭提示" onClick={()=>setMessage('')}><X size={14}/></Button></WorkbenchNotice>}</AnimatePresence>
    {peopleAction&&<PeopleActionDialog action={peopleAction} people={selectedPeople} theme={theme} onClose={()=>setPeopleAction(null)} onSaved={savedPeopleAction} onBusyChange={setBusy}/>}
    {fileAction&&<FileActionDialog key={`${fileAction.action}:${fileAction.files.map(v=>v.vid).join()}`} selection={fileAction} person={fileAction.resumePlan?.person||name} people={people} theme={theme} onClose={()=>setFileAction(null)} onSaved={savedFileAction} onRolledBack={rolledBackFileAction} onBusyChange={setBusy} onSubmitted={setPendingFilePlan} onForget={()=>{localStorage.removeItem(FILE_OPERATION_STORAGE_KEY);setPendingFilePlan(null)}}/>}
    {accountPerson&&<AccountAssociation key={accountPerson} name={accountPerson} theme={theme} onClose={()=>setAccountPerson(null)} onChanged={()=>{setAccountsRevision(n=>n+1);void loadPeople()}}/>}
    <Player video={playing} close={()=>setPlaying(null)} theme={theme}/>
    <Dialog open={!!confirmArchive} onOpenChange={open=>!open&&setConfirmArchive(null)}><DialogContent className="pw-modal" data-theme={theme}><DialogTitle>归档选中的 {confirmArchive?.rows.length} 个视频？</DialogTitle><DialogDescription>这一步会移动文件到已确认人物的目录，并同步 Peach 账本。未选中的任务继续保留。</DialogDescription><div className="pw-archive-confirm">{confirmArchive?.rows.map(row=><div key={row.vid}><strong>{row.name}</strong><CopyPath value={row.dst||''} label="目标路径" notify={notify}/></div>)}</div><div className="pw-modal-actions"><Button onClick={()=>setConfirmArchive(null)}>返回检查</Button><Button className="pw-primary" onClick={()=>void applyArchive()}>确认移动并归档</Button></div></DialogContent></Dialog>
  </div>
}

function ErrorBox({text,retry}:{text:string;retry:()=>void}){return <div className="pw-error" role="alert"><p>{text}</p><Button className="pw-outline" onClick={retry}>重新加载</Button></div>}
function CopyPath({value,label='原文件路径',notify}:{value:string;label?:string;notify:(message:string)=>void}){
  return <div className="pw-source-path"><div><span>{label}</span><Button aria-label={`复制${label}`} onClick={async()=>{try{await navigator.clipboard.writeText(value);notify('完整文件路径已复制。')}catch{notify('复制失败，请选中路径文字复制。')}}}><Copy size={12}/>复制</Button></div><p>{value}</p></div>
}
function Thumbnail({video,onPlay}:{video:ReviewVideo;onPlay:()=>void}) {return <Button className="pw-image-button" aria-label={`播放 ${video.name}`} onClick={onPlay}>{video.thumb?<img data-private-media draggable={false} src={video.thumb} alt="" loading="lazy"/>:<span className="pw-no-thumb"><FileVideo size={34}/><span>点击播放视频</span></span>}<span className="pw-play"><Play size={18} fill="currentColor"/></span></Button>}
function VideoCard({video,queue,queued,selected,busy,onToggle,onPlay,onReturn,onManage,notify}:{video:PersonVideo;queue?:PlanRow;queued:boolean;selected:boolean;busy:boolean;onToggle:()=>void;onPlay:()=>void;onReturn:()=>void;onManage:(action:PersonFileAction)=>void;notify:(message:string)=>void}) {
  const [pathOpen,setPathOpen]=useState(false)
  return <article className="pw-card" data-selected={selected}><div className="pw-thumb"><Thumbnail video={video} onPlay={onPlay}/>{<div className="pw-check"><Checkbox aria-label={`选择 ${video.name}`} checked={selected} disabled={busy} onCheckedChange={onToggle}/></div>}<span className="pw-duration">{timeText(video.duration)}</span></div><div className="pw-card-body"><h3 title={video.name}>{video.name}</h3><p><span>{video.folder==='.'?'根目录':video.folder}</span><span>{sizeText(video.size)}</span></p>
    {queue&&<div className={`pw-queue-status ${queue.status==='ok'?'ready':'issue'}`}><span>{queue.reason}</span>{queue.status!=='ok'&&<Link href="/integration">去待归档处理<ChevronRight size={12}/></Link>}</div>}
    <div className="pw-card-actions"><Button onClick={()=>setPathOpen(v=>!v)} aria-expanded={pathOpen}><FolderOpen size={13}/>路径</Button>{queued?<Button disabled={busy} onClick={onReturn}><Undo2 size={13}/>撤销归名</Button>:<span className="pw-file-actions"><Button disabled={busy} title="重命名" aria-label={`重命名 ${video.name}`} onClick={()=>onManage('rename-file')}><Pencil size={13}/></Button><Button disabled={busy} title="移动" aria-label={`移动 ${video.name}`} onClick={()=>onManage('move-files')}><FolderInput size={13}/></Button><Button disabled={busy} title="删除" aria-label={`删除 ${video.name}`} onClick={()=>onManage('delete-files')}><Trash2 size={13}/></Button></span>}</div>
    {pathOpen&&<><CopyPath value={video.path} label={queued?'原文件路径':'文件路径'} notify={notify}/>{queue?.dst&&<CopyPath value={queue.dst} label="归档目标" notify={notify}/>}</>}
  </div></article>
}
function Recommendation({video,name,busy,notify,play,add,reject,later}:{video:PersonVideo;name:string;busy:boolean;notify:(message:string)=>void;play:()=>void;add:()=>void;reject:()=>void;later:()=>void}) {
  const [expanded,setExpanded]=useState(false)
  return <article className="pw-recommendation"><div className="pw-rec-heading"><Button className="pw-mini-thumb" aria-label={`播放推荐 ${video.name}`} onClick={play}>{video.thumb?<img data-private-media draggable={false} src={video.thumb} alt=""/>:<FileVideo size={20}/>}<Play size={15}/></Button><div><h3 title={video.name}>{video.name}</h3><p>{timeText(video.duration)} · {sizeText(video.size)}</p></div></div><CopyPath value={video.path} notify={notify}/>
    {video.conflict&&<div className="pw-evidence conflict"><strong>证据冲突，需要逐项核对</strong><p>不同线索指向不同人物，不能仅凭推荐确认。</p></div>}
    {video.evidence.map((e,index)=><div key={`${e.kind}-${index}`} className="pw-evidence"><div><ShieldCheck size={14}/><strong>{e.title}</strong></div><p>{e.detail}</p>{e.score!==undefined&&<p>相似度 {e.score.toFixed(3)}{e.faces!==undefined?` · 检出 ${e.faces} 张脸`:''}</p>}{expanded&&<div className="pw-proof-detail">{e.timestamp!==undefined&&<span>采样时间 <b>{timeText(e.timestamp)}</b></span>}{e.image&&<PrivateImage className="pw-evidence-image" src={e.image} alt={`水印证据：${e.detail}`} loading="lazy"/>}{e.kind==='face'&&<p>相似度是模型分数，不是身份正确概率。</p>}</div>}</div>)}
    <Button className="pw-proof-toggle" aria-expanded={expanded} onClick={()=>setExpanded(v=>!v)}>{expanded?'收起依据':'展开依据'}<ChevronRight size={12}/></Button><div className="pw-candidate-actions"><Button className="pw-primary" disabled={busy} aria-label={`去人工确认 ${video.name}`} onClick={add}><Check size={13}/>去人工确认</Button><Button disabled={busy} onClick={reject}>不是此人</Button><Button disabled={busy} onClick={later}>稍后</Button></div>
  </article>
}
function Player({video,close,theme}:{video:ReviewVideo|null;close:()=>void;theme:'light'|'dark'}){
  const [failed,setFailed]=useState(false)
  useEffect(()=>setFailed(false),[video?.vid])
  return <Dialog open={!!video} onOpenChange={open=>!open&&close()}><DialogContent className="pw-modal pw-player-modal" data-theme={theme}><DialogTitle>{video?.name}</DialogTitle><DialogDescription>关闭后回到刚才的位置，勾选保持不变。</DialogDescription>{video&&<PrivateVideo key={video.path||video.vid} src={`/api/video?p=${encodeURIComponent(video.path||video.vid)}`} controls autoPlay playsInline onError={()=>{setFailed(true);const element=document.querySelector<HTMLVideoElement>('.pw-player-modal video');const code=element?.error?.code;if([2,3,4].includes(code||0))void api('/api/video-health/report',{method:'POST',body:JSON.stringify({vid:video.vid,path:video.path||video.vid,code})}).catch(()=>{})}}/>}{failed&&<p role="alert" className="pw-error">暂时无法播放，请核对文件路径、网络或浏览器编码支持。</p>}<p className="pw-player-path">{video?.path||video?.vid}</p></DialogContent></Dialog>
}
