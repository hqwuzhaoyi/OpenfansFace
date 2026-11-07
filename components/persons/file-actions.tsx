'use client'
import { useState } from 'react'
import { ArrowRight } from 'lucide-react'
import { Dialog, DialogDescription, DialogTitle } from '@/components/ui/dialog'
import { api } from '@/lib/face-api'
import type { LibraryPerson, PersonVideo } from '@/lib/person-library-model'
import type { PersonFileAction, PersonFilePlan, PersonFileRequest, PersonFileResult } from '@/lib/person-file-model'
import { WorkbenchDialogContent as DialogContent, WorkbenchButton as Button, WorkbenchInput as Input } from './ui'

export interface FileActionSelection {action:PersonFileAction;files:PersonVideo[]}
const labels:Record<PersonFileAction,string>={'rename-file':'重命名文件','move-files':'移动视频','delete-files':'删除视频','rename-person':'人物重命名','merge-person':'合并人物','delete-person':'删除人物'}
export function FileActionDialog({selection,person,people,theme,onClose,onSaved,onBusyChange}:{selection:FileActionSelection;person:string;people:LibraryPerson[];theme:'light'|'dark';onClose:()=>void;onSaved:(result:PersonFileResult)=>Promise<void>;onBusyChange:(busy:boolean)=>void}) {
  const [name,setName]=useState(selection.action==='rename-person'?person:selection.files[0]?.name||'')
  const [target,setTarget]=useState(''),[folder,setFolder]=useState(''),[plan,setPlan]=useState<PersonFilePlan|null>(null),[error,setError]=useState(''),[busy,setBusy]=useState(false)
  const [targetQuery,setTargetQuery]=useState('')
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
    if(!plan||busy)return;updateBusy(true);setError('')
    try{
      const result=await api<PersonFileResult>('/api/persons/manage',{method:'POST',body:JSON.stringify({mode:'apply',request:plan.request,version:plan.version})})
      await onSaved(result);onClose()
    }catch(e){setError((e as Error).message)}finally{updateBusy(false)}
  }
  const valid=selection.action==='delete-files'||(selection.action==='move-files'||merging?!!target:!!name.trim()&&name.trim()!==(selection.action==='rename-person'?person:selection.files[0]?.name))
  return <Dialog open onOpenChange={open=>!open&&!busy&&onClose()}><DialogContent className={`pw-modal pw-file-dialog${merging?' pw-merge-dialog':''}`} data-theme={theme} showCloseButton={!busy} onEscapeKeyDown={e=>busy&&e.preventDefault()} onInteractOutside={e=>busy&&e.preventDefault()}>
    <DialogTitle>{plan?'确认操作 · ':''}{labels[selection.action]}</DialogTitle>
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
      <div className="pw-modal-actions"><Button disabled={busy} onClick={()=>{setPlan(null);setError('')}}>返回修改</Button><Button className={plan.action==='delete-files'?'pw-danger':'pw-primary'} disabled={busy} onClick={()=>void apply()}>{busy?'正在同步，请稍候…':plan.action==='delete-files'?'确认移入暂存':`确认${labels[plan.action]}`}</Button></div>
    </>}
  </DialogContent></Dialog>
}
