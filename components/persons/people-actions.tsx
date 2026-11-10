'use client'
import { WorkbenchStep, useWorkbenchMotion } from './motion'
import { ArrowRight, FolderCheck, FolderOutput, LoaderCircle } from 'lucide-react'
import { useEffect, useMemo, useRef, useState } from 'react'
import { Avatar } from '@/components/beui/motion/avatar-group'
import { Collapsible, CollapsibleContent, CollapsibleIndicator, CollapsibleTrigger } from '@/components/beui/motion/collapsible'
import { Dialog, DialogDescription, DialogTitle } from '@/components/ui/dialog'
import { api } from '@/lib/face-api'
import type { LibraryPerson, PersonLibraryPreview } from '@/lib/person-library-model'
import type { PersonBatchAction, PersonBatchPlan, PersonBatchProgress, PersonBatchResult } from '@/lib/person-batch-model'
import { WorkbenchDialogContent as DialogContent, WorkbenchButton as Button } from './ui'
import { MergePersonVideos, usePersonPreviews, type MergePlaying } from './merge-videos'
import { MergeFlow } from './merge-flow'

export function PeopleActionDialog({
  action,people:selection,directoryPeople,theme,onClose,onSaved,onBusyChange,onEnqueueBackground
}:{
  action:PersonBatchAction;people:string[];directoryPeople?:LibraryPerson[];theme:'light'|'dark'
  onClose:()=>void;onSaved:(result:PersonBatchResult)=>Promise<void>;onBusyChange:(busy:boolean)=>void
  onEnqueueBackground?:(payload:{plan:PersonBatchPlan;people:string[];thumbs:Record<string,string|null>})=>void
}){
  const [people]=useState(selection),{mode}=useWorkbenchMotion(),merging=action==='merge'
  const [target,setTarget]=useState(''),[plan,setPlan]=useState<PersonBatchPlan|null>(null),[result,setResult]=useState<PersonBatchResult|null>(null)
  const [busy,setBusy]=useState(false),[applying,setApplying]=useState(false),[error,setError]=useState(''),[progress,setProgress]=useState<PersonBatchProgress|null>(null),[progressError,setProgressError]=useState('')
  const body=useRef<HTMLDivElement>(null),[finalCount,setFinalCount]=useState<number|null>(null)
  const [playing,setPlaying]=useState<MergePlaying|null>(null)
  const {states,load}=usePersonPreviews(people,merging)
  const thumbs=useMemo(()=>{
    const map:Record<string,string|null>={}
    for(const person of directoryPeople||[])map[person.name]=person.rep_thumb
    return map
  },[directoryPeople])
  const getThumb=(name:string)=>states[name]?.data?.files.find(f=>f.thumb)?.thumb||thumbs[name]||undefined
  const totalVideos=people.every(person=>states[person]?.data)?people.reduce((sum,person)=>sum+states[person].data!.total_videos,0):null
  const targetDirectory=target?(plan?.plans[0]?.target_directory||states[target]?.data?.root||`creators/${target}`):''
  useEffect(()=>{if(body.current)body.current.scrollTop=0},[plan,applying,result])
  const updateBusy=(value:boolean)=>{setBusy(value);onBusyChange(value)}
  useEffect(()=>{
    if(!applying||!plan)return
    const controller=new AbortController();let timer:ReturnType<typeof setTimeout>|undefined
    const poll=async()=>{
      try{const next=await api<PersonBatchProgress>(`/api/persons/batch?id=${encodeURIComponent(plan.id)}`,{signal:controller.signal});if(!controller.signal.aborted){setProgress(next);setProgressError('')}}
      catch{if(!controller.signal.aborted)setProgressError('进度暂时无法刷新，正在等待执行结果。')}
      if(!controller.signal.aborted)timer=setTimeout(()=>void poll(),600)
    }
    void poll();return()=>{controller.abort();clearTimeout(timer)}
  },[applying,plan])
  async function preview(){
    setBusy(true);setError('');setPlaying(null)
    try{setPlan(await api<PersonBatchPlan>('/api/persons/batch',{method:'POST',body:JSON.stringify({mode:'preview',request:{action,people,...(merging?{target}:{})}})}))}
    catch(e){setError((e as Error).message)}finally{setBusy(false)}
  }
  async function apply(){
    if(!plan||busy)return
    if(onEnqueueBackground){
      const resolvedThumbs:Record<string,string|null>={}
      for(const p of people)resolvedThumbs[p]=getThumb(p)||null
      onEnqueueBackground({plan,people,thumbs:resolvedThumbs})
      onClose()
      return
    }
    updateBusy(true);setApplying(true);setPlaying(null);setError('');setProgressError('')
    try{
      const saved=await api<PersonBatchResult>('/api/persons/batch',{method:'POST',body:JSON.stringify({mode:'apply',id:plan.id,version:plan.version})})
      setResult(saved);await onSaved(saved)
      if(saved.ok&&merging)try{const updated=await api<PersonLibraryPreview>(`/api/persons/library?preview=1&name=${encodeURIComponent(target)}`);setFinalCount(updated.total_videos)}catch{}
      if(saved.ok&&!merging)onClose()
    }catch(e){setError(`${(e as Error).message}。如连接中断，可再次确认以查询同一批次结果。`)}finally{setApplying(false);updateBusy(false)}
  }
  const selectTarget=(name:string)=>{setTarget(name);setPlan(null);setError('')}
  return <Dialog open onOpenChange={open=>!open&&!busy&&onClose()}><DialogContent className={`pw-modal pw-file-dialog pw-merge-dialog pw-people-dialog${merging?' pw-merge-rich-dialog':''}`} data-theme={theme} onCloseAutoFocus={event=>{if(result){event.preventDefault();document.querySelector<HTMLButtonElement>('[aria-label="刷新人物和视频"]')?.focus()}}} showCloseButton={!busy} onEscapeKeyDown={e=>busy&&e.preventDefault()} onInteractOutside={e=>busy&&e.preventDefault()}>
    <DialogTitle>{result?.ok&&merging?'合并完成':`${plan?'确认':''}${merging?'合并':'删除'}所选人物`} · {people.length} 人</DialogTitle>
    <DialogDescription>{merging?result?.ok?`已保留「${target}」。视频、待归档任务和账号关联已合入，同名文件保留两份。`:'先查看人物视频，再选择保留人物。同名文件保留两份，账号关联和待归档任务一起合并。':'删除所选人物，并将其整个目录（含视频和附属文件）移入全局暂存区。待归档任务撤回，账号关联和识别映射移除，文件内容保留。'}</DialogDescription>
    <div ref={body} className="pw-people-dialog-body">
      {merging&&<MergeFlow people={people} target={target} applying={applying} progress={progress} result={result} totalVideos={result?.ok?finalCount:totalVideos} plan={plan} states={states} thumbs={thumbs} disabled={busy} onSelectTarget={selectTarget}/>}
      {merging&&!result&&<>
        {!plan&&<label className="pw-edit-label">保留哪个人物<select aria-label="保留人物" value={target} disabled={busy} onChange={e=>selectTarget(e.target.value)}><option value="">从已选人物中选择…</option>{people.map(person=><option key={person} value={person}>{person}</option>)}</select></label>}
      </>}
      <WorkbenchStep phase={result?'result':plan?'confirm':'choose'}>
        {!plan&&!merging&&<div className="pw-file-selection"><strong>已选 {people.length} 个人物</strong><ul>{people.map(person=><li key={person}>{person}</li>)}</ul></div>}
        {plan&&!result&&<>
          <div className="pw-operation-summary">
            <strong>{merging?`保留 ${plan.request.target}，合入其余 ${plan.plans.length} 人`:`删除 ${plan.plans.length} 个人物`}</strong>
            <p>涉及 {plan.plans.reduce((sum,p)=>sum+p.files.length,0)} 个文件 · {(plan.plans.reduce((sum,p)=>sum+p.total_bytes,0)/2**20).toFixed(1)} MB</p>
            {merging&&targetDirectory&&<div className="pw-merge-target-path"><FolderOutput size={14}/><span>合并至文件夹</span><code>{targetDirectory}</code></div>}
            <p>{merging?'同名文件会自动改名以保留两份，最终路径以合并结果为准。':'目录文件移入暂存区，人物将从列表移除。'}</p>
          </div>
          <div className="pw-batch-summary">{plan.plans.map(p=>{
            const srcDir=p.source_directory||states[p.person]?.data?.root||`creators/${p.person}`
            const dstDir=p.target_directory||targetDirectory
            const renamed=p.merge_summary?.renamed_files||0
            return <div key={p.person} className="pw-batch-item">
              <div className="pw-batch-item-head">
                <Avatar name={p.person} src={getThumb(p.person)} size="sm" className="pw-batch-avatar"/>
                <div className="pw-batch-item-copy">
                  <strong>{p.person}{merging?` → ${plan.request.target}`:''}</strong>
                  <span>{p.files.length} 个文件 · {(p.total_bytes/2**20).toFixed(1)} MB{renamed>0?` · ${renamed} 个同名保留改名`:''}</span>
                  {(p.merge_summary||p.delete_summary)&&<small>{(p.merge_summary||p.delete_summary)!.pending_tasks} 条待归档 · {(p.merge_summary||p.delete_summary)!.manual_accounts} 个账号关联</small>}
                </div>
              </div>
              {merging?<div className="pw-batch-dirs"><code title={srcDir}>{srcDir}</code><ArrowRight size={12}/><code title={dstDir}>{dstDir}</code></div>:<code>{p.target_directory}</code>}
              {merging&&p.files.length>0&&<Collapsible className="pw-batch-files-collapse">
                <CollapsibleTrigger className="pw-batch-files-trigger">
                  <span>查看 {p.files.length} 个文件目标路径</span>
                  <CollapsibleIndicator/>
                </CollapsibleTrigger>
                <CollapsibleContent contentClassName="pw-batch-files-list">
                  {p.files.slice(0,20).map(file=><div key={file.src} className="pw-batch-file-row">
                    <span>{file.src.split('/').pop()}{file.renamed?<b>保留两份改名</b>:null}</span>
                    <code>{file.dst}</code>
                  </div>)}
                  {p.files.length>20&&<small>其余 {p.files.length-20} 个文件同样移入 {dstDir}</small>}
                </CollapsibleContent>
              </Collapsible>}
            </div>
          })}</div>
          <p className="pw-batch-note">按人物依次处理；如某项失败，会停止剩余操作并列出已完成的结果。</p>
        </>}
        {result?.ok&&merging&&<div className="pw-merge-result">
          <strong>{people.length} 个人物 → {target}</strong>
          {targetDirectory&&<div className="pw-merge-target-path"><FolderCheck size={14}/><span>已合入文件夹</span><code>{targetDirectory}</code></div>}
          <p>已合入 {result.completed.reduce((sum,item)=>sum+item.result.changed,0)} 个文件（含附属资料）。</p>
          {result.completed.map(item=><p key={item.person}>{item.person} · {item.result.changed} 个文件已合入{item.result.warning?` · ${item.result.warning}`:''}</p>)}
        </div>}
        {result&&!result.ok&&<div className="pw-error" role="alert"><p>{result.error}</p>{result.recovery_needed&&<p>当前人物结果待核对，写入已暂停；已完成人物保留。</p>}<p>已完成：{result.completed.map(p=>p.person).join('、')||'无'}</p><p>尚未完成：{result.remaining.join('、')}</p></div>}
        {applying&&!merging&&progress&&<div className="pw-batch-note" role="status"><strong>{progress.active_person?`正在处理 ${progress.active_person}`:'正在核对批次'}</strong><p>已完成：{progress.completed.join('、')||'无'}</p><p>尚未完成：{progress.remaining.join('、')}</p></div>}
        {progressError&&applying&&<p role="status" className="pw-batch-note">{progressError}</p>}
        {error&&<p className="pw-error" role="alert">{error}</p>}
      </WorkbenchStep>
        {merging&&!result&&!applying&&<div className="pw-merge-people">{people.map(person=><MergePersonVideos key={person} name={person} state={states[person]} repThumb={thumbs[person]} target={target} disabled={busy} playing={playing} onPlay={setPlaying} onLoad={(name,offset)=>void load(name,offset)} onTarget={selectTarget}/>)}</div>}
    </div>
    <div className="pw-modal-actions">{result?<Button disabled={busy} className="pw-primary" onClick={onClose}>{result.ok&&merging?'查看合并后的视频':'关闭并查看结果'}</Button>:<><Button disabled={busy} onClick={()=>plan?(setPlan(null),setError('')):onClose()}>{plan?'返回修改':'取消'}</Button><Button aria-busy={busy} className={plan&&!merging?'pw-danger':'pw-primary'} disabled={busy||(merging&&!target)} onClick={()=>void(plan?apply():preview())}>{busy&&<LoaderCircle size={14} aria-hidden className="pw-busy-icon" data-motion={mode}/>}<span>{busy?(plan?'正在处理所选人物…':'检查文件和关联…'):plan?(merging?'确认合并人物':'确认删除并移入暂存'):'预览变更'}</span></Button></>}</div>
  </DialogContent></Dialog>
}
