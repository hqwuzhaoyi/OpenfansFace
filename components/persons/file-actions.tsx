'use client'
import { useEffect, useRef, useState } from 'react'
import { ArrowRight } from 'lucide-react'
import { Dialog, DialogDescription, DialogTitle } from '@/components/ui/dialog'
import { api } from '@/lib/face-api'
import type { LibraryPerson, PersonVideo } from '@/lib/person-library-model'
import type { PersonFileAction, PersonFilePlan, PersonFileRequest, PersonFileResult } from '@/lib/person-file-model'
import { WorkbenchDialogContent as DialogContent, WorkbenchButton as Button, WorkbenchInput as Input } from './ui'

export interface FileActionSelection {action:PersonFileAction;files:PersonVideo[];resumePlan?:PersonFilePlan}
export const FILE_OPERATION_STORAGE_KEY='person-file-operation-v1'
const labels:Record<PersonFileAction,string>={'rename-file':'重命名文件','move-files':'移动视频','delete-files':'删除视频','rename-person':'人物重命名','merge-person':'合并人物','delete-person':'删除人物'}
export function FileActionDialog({selection,person,people,theme,onClose,onSaved,onBusyChange,onSubmitted,onForget}:{selection:FileActionSelection;person:string;people:LibraryPerson[];theme:'light'|'dark';onClose:()=>void;onSaved:(result:PersonFileResult)=>Promise<void>;onBusyChange:(busy:boolean)=>void;onSubmitted:(plan:PersonFilePlan)=>void;onForget:()=>void}) {
  const [name,setName]=useState(selection.action==='rename-person'?person:selection.files[0]?.name||'')
  const [target,setTarget]=useState(''),[folder,setFolder]=useState(''),[plan,setPlan]=useState<PersonFilePlan|null>(selection.resumePlan||null),[error,setError]=useState(''),[busy,setBusy]=useState(false)
  const [targetQuery,setTargetQuery]=useState('')
  const [submitted,setSubmitted]=useState(!!selection.resumePlan),[status,setStatus]=useState(''),[phase,setPhase]=useState(''),[checking,setChecking]=useState(false)
  const revision=useRef(-1),finished=useRef(false),alive=useRef(true),saved=useRef(onSaved)
  saved.current=onSaved
  useEffect(()=>{alive.current=true;return()=>{alive.current=false}},[])
  type Receipt={id:string;status:string;revision?:number;phase?:string;result?:PersonFileResult;error?:string;failure?:{path?:string;step?:string}}
  async function receive(receipt:Receipt){
    if(!alive.current||receipt.id!==plan?.id||(receipt.revision??0)<revision.current||finished.current)return
    revision.current=receipt.revision??0;setStatus(receipt.status);setPhase(receipt.phase||'')
    const detail=[receipt.error,receipt.failure?.step,receipt.failure?.path].filter(Boolean).join(' · ')
    if(detail||receipt.status!=='not_started')setError(detail)
    if(['complete','rolled_back','cancelled','failed'].includes(receipt.status)){
      finished.current=true;updateBusy(false)
      if(receipt.result?.ok)await saved.current(receipt.result)
    }
  }
  useEffect(()=>{
    if(!submitted||!plan||plan.backend!=='rust')return
    let active=true,timer:ReturnType<typeof setTimeout>
    const poll=async()=>{
      try{const receipt=await api<Receipt>(`/api/persons/manage?id=${encodeURIComponent(plan.id)}`);if(active)await receive(receipt)}
      catch(e){if(active&&!finished.current)setError((e as Error).message)}
      if(active&&!finished.current)timer=setTimeout(poll,1000)
    }
    void poll();return()=>{active=false;clearTimeout(timer)}
  // The submitted immutable plan owns this polling lifetime; callback reads current handlers.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  },[submitted,plan?.id])
  async function reconcile(){
    if(!plan||checking)return;setChecking(true)
    try{await receive(await api<Receipt>('/api/persons/manage',{method:'POST',body:JSON.stringify({mode:'reconcile',request:plan.request,version:plan.version})}))}
    catch(e){setError((e as Error).message)}finally{setChecking(false)}
  }
  const terminal=['complete','rolled_back','cancelled','failed'].includes(status)
  const phaseText=phase.startsWith('ledger')?'正在核对账本…':phase.startsWith('faces')||phase.startsWith('jobs')||phase.startsWith('json')?'正在同步记录…':phase.startsWith('compensation')?'正在回退…':'正在处理文件…'
  function close(){updateBusy(false);onClose()}

  const merging=selection.action==='merge-person'
  const targets=people.filter(p=>(!merging||p.name!==person)&&[p.name,...(p.aliases||[])].some(value=>value.toLowerCase().includes(targetQuery.toLowerCase())))
  const updateBusy=(value:boolean)=>{setBusy(value);onBusyChange(value)}
  async function preview(){
    updateBusy(true);setError('')
    try{
      const request:PersonFileRequest={action:selection.action,person,
        ...(selection.action==='rename-person'?{new_name:name.trim()}:merging?{}:{files:selection.files.map(file=>({vid:file.vid,path:file.path,version:file.version||''}))}),
        ...(selection.action==='rename-file'?{filename:name.trim()}:{}),...(selection.action==='move-files'?{target_person:target,target_folder:folder.trim()}:merging?{target_person:target}:{})}
      setPlan(await api<PersonFilePlan>('/api/persons/manage',{method:'POST',body:JSON.stringify({mode:'preview',request})}))
    }catch(e){setError((e as Error).message)}finally{updateBusy(false)}
  }
  async function apply(){
    if(!plan||busy||submitted)return;updateBusy(true);setError('')
    if(plan.backend==='rust'){
      try{localStorage.setItem(FILE_OPERATION_STORAGE_KEY,JSON.stringify(plan));onSubmitted(plan);setSubmitted(true);setStatus('submitting')}
      catch{setError('无法保存操作编号，请允许浏览器本地存储后再提交。');updateBusy(false);return}
    }
    try{
      const result=await api<PersonFileResult>('/api/persons/manage',{method:'POST',body:JSON.stringify({mode:'apply',request:plan.request,version:plan.version})})
      if(plan.backend==='rust'){
        if(alive.current&&!finished.current){finished.current=true;setStatus('complete');setError('');await saved.current(result)}
      }else{await onSaved(result);onClose()}
    }catch(e){if(alive.current&&!finished.current)setError((e as Error).message)}finally{if(alive.current)updateBusy(false)}
  }
  const valid=selection.action==='delete-files'||(selection.action==='move-files'||merging?!!target:!!name.trim()&&name.trim()!==(selection.action==='rename-person'?person:selection.files[0]?.name))
  return <Dialog open onOpenChange={open=>!open&&(!busy||submitted)&&close()}><DialogContent className={`pw-modal pw-file-dialog${merging?' pw-merge-dialog':''}`} data-theme={theme} showCloseButton={!busy||submitted} onEscapeKeyDown={e=>busy&&!submitted&&e.preventDefault()} onInteractOutside={e=>busy&&!submitted&&e.preventDefault()}>
    <DialogTitle>{submitted?'操作结果 · ':plan?'确认操作 · ':''}{labels[selection.action]}</DialogTitle>
    <DialogDescription>{merging?`把「${person}」的文件、待归档任务和账号关联并入目标人物。来源人物不再单独显示，同名文件保留两份。`:selection.action==='delete-files'?'视频会移入全局暂存区，原人物目录不再显示。文件内容保留。':selection.action==='rename-person'?'同时修改人物目录、人物资料、推荐归属、水印映射和待归档任务。不会与已有同名人物合并。':selection.action==='move-files'?'将选中的视频移动到目标人物目录，并同步主归属与文件记录。':'保留原扩展名，同步视频路径和相关记录。'}</DialogDescription>
    {!plan?<form onSubmit={e=>{e.preventDefault();void preview()}}>
      {selection.action==='rename-file'&&<label className="pw-edit-label">新文件名<Input aria-label="新文件名" value={name} onChange={e=>setName(e.target.value)} autoFocus disabled={busy}/></label>}
      {selection.action==='rename-person'&&<label className="pw-edit-label">新人物名<Input aria-label="新人物名" value={name} onChange={e=>setName(e.target.value)} autoFocus disabled={busy}/></label>}
      {(selection.action==='move-files'||merging)&&<div className="pw-edit-fields"><label className="pw-edit-label">搜索目标人物<Input aria-label="搜索目标人物" placeholder="按姓名或别名筛选" value={targetQuery} onChange={e=>{setTargetQuery(e.target.value);setTarget('')}} disabled={busy}/></label><label className="pw-edit-label">{merging?'保留哪个人物':'目标人物'}<select aria-label="目标人物" value={target} onChange={e=>setTarget(e.target.value)} disabled={busy}><option value="">选择目标人物…</option>{targets.map(p=><option key={p.name} value={p.name}>{p.name}{p.name===person?'（当前人物）':''}</option>)}</select></label>{!merging&&<label className="pw-edit-label">目标子文件夹（可选）<Input aria-label="目标子文件夹" placeholder="留空放到人物根目录" value={folder} onChange={e=>setFolder(e.target.value)} disabled={busy}/></label>}</div>}
      {selection.files.length>0&&<div className="pw-file-selection"><strong>选中 {selection.files.length} 个视频</strong><ul>{selection.files.map(file=><li key={file.vid}>{file.name}<small>{file.path}</small></li>)}</ul></div>}
      {error&&<p role="alert" className="pw-error">{error}</p>}
      <div className="pw-modal-actions"><Button disabled={busy} onClick={onClose}>取消</Button><Button className="pw-primary" type="submit" disabled={busy||!valid}>{busy?'检查文件和账本…':'预览变更'}<ArrowRight size={14}/></Button></div>
    </form>:<>
      <div className="pw-operation-summary">{['rename-person','merge-person'].includes(plan.action)?<strong>{plan.person} → {plan.target_person}</strong>:<strong>{plan.files.length} 个视频 · {(plan.total_bytes/2**20).toFixed(1)} MB</strong>}<p>{merging?`合并 ${plan.files.length} 个文件（含附属资料），保留目标人物 ${plan.target_person}`:plan.action==='delete-files'?'将移入全局暂存区':plan.action==='move-files'?`移动到 ${plan.target_person}`:'文件路径及相关记录将一起更新'}</p>{plan.merge_summary&&<><p>迁移 {plan.merge_summary.pending_tasks} 条待归档任务 · {plan.merge_summary.manual_accounts} 个手动账号关联</p>{plan.merge_summary.renamed_files>0&&<p>{plan.merge_summary.renamed_files} 个文件的目标名或路径已调整，保留双方文件。</p>}{plan.merge_summary.keep_target_template&&<p>双方都有模板设置，保留目标人物的选择；来源设置会留档。</p>}</>}</div>
      <div className="pw-move-preview">{!plan.files.length&&<p>没有需要搬动的文件，将更新人物资料与归属记录。</p>}{plan.files.map(file=><div key={file.src}><span>原路径</span><code>{file.src}</code><span>新路径{file.renamed?' · 已调整以保留两份':''}</span><code>{file.dst}</code></div>)}</div>
      {error&&<p role="alert" className="pw-error">{error}</p>}
      {submitted?<>
        <p role="status" aria-live="polite">{status==='complete'?'文件与记录已同步':status==='rolled_back'?'操作已回退，原文件与记录已恢复':status==='cancelled'?'未开始执行，迟到提交已阻止':status==='recovery_needed'?'需要核对恢复，写入已暂停':status==='not_started'?'提交尚未受理，可核对并停止此次提交':status==='failed'?'操作未完成，请核对错误依据':phaseText}</p>
        <p className="pw-player-path">操作编号：{plan.id}</p>
        {!terminal&&<p>关闭窗口只停止等待，文件操作会继续。刷新后可按此编号查看结果。</p>}
        <div className="pw-modal-actions">
          {!terminal&&<Button disabled={checking} onClick={()=>void reconcile()}>{checking?'正在核对…':'核对原操作'}</Button>}
          <Button onClick={close}>{terminal?'关闭':'停止等待'}</Button>
          {terminal&&<Button onClick={()=>{onForget();close()}}>关闭并清除结果入口</Button>}
        </div>
      </>:<div className="pw-modal-actions"><Button disabled={busy} onClick={()=>{setPlan(null);setError('')}}>返回修改</Button><Button className={plan.action==='delete-files'?'pw-danger':'pw-primary'} disabled={busy} onClick={()=>void apply()}>{busy?'正在同步，请稍候…':plan.action==='delete-files'?'确认移入暂存':`确认${labels[plan.action]}`}</Button></div>}

    </>}
  </DialogContent></Dialog>
}
