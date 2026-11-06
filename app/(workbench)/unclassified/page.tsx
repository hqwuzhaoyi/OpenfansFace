'use client'
import { useEffect, useState } from 'react'
import Link from 'next/link'
import { toast } from 'sonner'
import { api } from '@/lib/face-api'
import { type ReviewVideo, videoGroups } from '@/lib/review-ui'
import { FolderVideoAccordion } from '@/components/review/folder-video-accordion'
import { PersonPicker, type PersonChoice } from '@/components/review/person-picker'
import { VideoPlayer } from '@/components/review/video'
import { ReviewFlow, QueueReceipt } from '@/components/review/workflow'
import { ResponsiveDialogContent } from '@/components/review/responsive-dialog'
import { Dialog, DialogHeader, DialogTitle, DialogDescription } from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'

type Video=ReviewVideo&{uncovered:boolean;reason:string}
type Data={videos:Video[];total:number;uncovered:number}
export default function Unclassified(){
  const [data,setData]=useState<Data|null>(null),[error,setError]=useState(''),[loading,setLoading]=useState(true),[busy,setBusy]=useState(false)
  const [onlyNew,setOnlyNew]=useState(false),[query,setQuery]=useState(''),[folder,setFolder]=useState(''),[expanded,setExpanded]=useState<string[]>([])
  const [selected,setSelected]=useState<Set<string>>(new Set()),[playing,setPlaying]=useState<ReviewVideo|null>(null)
  const [people,setPeople]=useState<PersonChoice[]>([]),[peopleError,setPeopleError]=useState(''),[creator,setCreator]=useState(''),[dialog,setDialog]=useState(false)
  const [receipt,setReceipt]=useState<{vids:string[];creator:string}|null>(null)
  const load=async()=>{setLoading(true);setError('');try{const d=await api<Data>('/api/unclassified');setData(d);setSelected(new Set());return d}catch(e:any){setError(e.message)}finally{setLoading(false)}}
  const loadPeople=async()=>{setPeopleError('');try{setPeople((await api<{persons:PersonChoice[]}>('/api/persons?picker=1')).persons)}catch(e:any){setPeopleError(e.message)}}
  useEffect(()=>{load();loadPeople()},[])
  const videos=(data?.videos||[]).filter(v=>(!onlyNew||v.uncovered)&&(!query||[v.path||'',v.name].some(s=>s.toLowerCase().includes(query.toLowerCase()))))
  const groups=videoGroups(videos),index=Math.max(0,groups.findIndex(([p])=>p===folder)),current=groups[index],rows=current?.[1]||[]
  const selectedRows=rows.filter(v=>selected.has(v.vid))
  const choose=(path:string)=>{setFolder(path);setSelected(new Set());setExpanded([path])}
  const toggle=(ids:string[])=>setSelected(prev=>{const next=new Set(prev),all=ids.every(id=>next.has(id));ids.forEach(id=>all?next.delete(id):next.add(id));return next})
  const enqueue=async()=>{
    if(busy||!selectedRows.length||!creator.trim())return
    setBusy(true);const vids=selectedRows.map(v=>v.vid),name=creator.trim(),position=index
    try{
      await api('/api/plan/queue',{method:'POST',body:JSON.stringify({vids,creator:name,source:'all',source_id:''})})
      setDialog(false);setReceipt({vids,creator:name});setSelected(new Set())
      setData(d=>d?{...d,videos:d.videos.filter(v=>!vids.includes(v.vid)),total:d.total-vids.length,uncovered:d.uncovered-selectedRows.filter(v=>(v as Video).uncovered).length}:d)
      const next=await load();if(next){const filtered=next.videos.filter(v=>(!onlyNew||v.uncovered)&&(!query||[v.path||'',v.name].some(s=>s.toLowerCase().includes(query.toLowerCase()))));const folders=videoGroups(filtered);const same=folders.find(([p])=>p===current?.[0]);choose((same||folders[Math.min(position,folders.length-1)])?.[0]||'')}
      else toast.warning('归名已保存，列表刷新失败，请刷新后继续')
      loadPeople();toast.success('已加入待归档，下一步检查目标路径后归档')
    }catch(e:any){toast.error(e.message)}finally{setBusy(false)}
  }
  const undo=async()=>{if(!receipt||busy)return;setBusy(true);try{await api('/api/plan/queue',{method:'DELETE',body:JSON.stringify({vids:receipt.vids})});setReceipt(null);await load();toast.success('已撤销这波归名，视频回到待分类')}catch(e:any){toast.error(e.message)}finally{setBusy(false)}}
  return <div className="space-y-4 p-4 md:p-6"><ReviewFlow/><h1 className="text-2xl font-semibold">全部待分类</h1><p className="text-sm leading-relaxed text-muted-foreground">所有已收录、文件存在且尚未归名的视频都在这里，按实际路径去重。先打开文件夹核对视频，再勾选属于同一个人的视频，批量选择人物。</p>
    <div className="flex flex-wrap gap-2"><Button variant="outline" asChild><Link href="/clusters">按相似人脸分组查看</Link></Button><Button variant="outline" disabled={loading||busy} onClick={load}>刷新</Button></div>
    {receipt&&<QueueReceipt count={receipt.vids.length} creator={receipt.creator} onUndo={undo} busy={busy}/>}
    {data&&<div className="flex flex-wrap gap-2">{[{value:false,label:`全部 ${data.total}`},{value:true,label:`此前遗漏 ${data.uncovered}`}].map(t=><Button key={String(t.value)} disabled={busy} variant={onlyNew===t.value?'secondary':'ghost'} onClick={()=>{setOnlyNew(t.value);choose('')}}>{t.label}</Button>)}</div>}
    <Input className="h-11 text-base" placeholder="搜索文件夹、完整路径或文件名" aria-label="搜索待分类视频" disabled={busy} value={query} onChange={e=>{setQuery(e.target.value);choose('')}}/>
    {loading&&!data?<p role="status">正在核对实际路径，加载全部待分类视频…</p>:error?<p role="alert" className="text-amber-300">{error}</p>:current?<>
      <label className="block space-y-2 text-sm"><span>这一波：文件夹 {index+1}/{groups.length} · {rows.length} 个视频</span><select aria-label="选择待分类文件夹" disabled={busy} value={current[0]} onChange={e=>choose(e.target.value)} className="h-11 w-full min-w-0 rounded-lg border bg-background px-2 text-base">{groups.map(([p,vs])=><option key={p} value={p}>{vs.length} 个 · {p}</option>)}</select></label>
      <div className="flex gap-2"><Button variant="outline" disabled={busy||index===0} onClick={()=>choose(groups[index-1][0])}>上一文件夹</Button><Button variant="outline" disabled={busy||index===groups.length-1} onClick={()=>choose(groups[index+1][0])}>下一文件夹</Button></div>
      <p className="text-xs text-muted-foreground">这里只勾选当前文件夹中符合搜索条件的视频。切换文件夹或筛选会清空勾选。</p>
      <FolderVideoAccordion videos={rows} selected={selected} onToggle={toggle} onPlay={setPlaying} disabled={busy} value={expanded} onValueChange={setExpanded}/>
      <details className="rounded-lg border p-3"><summary className="min-h-11 cursor-pointer text-sm">查看这一波的识别情况</summary><div className="space-y-2">{rows.map(v=><p key={v.vid} className="break-all text-xs text-muted-foreground">{v.name}：{(v as Video).reason}</p>)}</div></details>
      <div className="review-actionbar sticky bottom-20 z-10 space-y-2 rounded-xl border bg-background/95 p-3 backdrop-blur md:bottom-4"><p className="text-sm">已选 {selectedRows.length} 个。下一步：选择人物，再加入待归档。</p><Button className="min-h-11 w-full" disabled={busy||loading||!selectedRows.length} onClick={()=>setDialog(true)}>为选中的 {selectedRows.length} 个视频选择人物</Button></div>
    </>:data&&<p className="rounded-xl border border-dashed p-8 text-sm text-muted-foreground">{query?'没有匹配的文件，可以修改搜索条件。':onlyNew?'此前遗漏的视频已处理完，可以切换到全部。':'当前没有待分类视频。'}</p>}
    <Dialog open={dialog} onOpenChange={o=>!busy&&setDialog(o)}><ResponsiveDialogContent open={dialog} showCloseButton={!busy} onEscapeKeyDown={e=>busy&&e.preventDefault()} onInteractOutside={e=>busy&&e.preventDefault()} className="flex max-h-[90dvh] flex-col gap-0 overflow-hidden p-0 sm:max-w-2xl sm:p-0"><DialogHeader className="shrink-0 border-b p-4 pr-14 text-left"><DialogTitle>这 {selectedRows.length} 个视频属于谁？</DialogTitle><DialogDescription>本次确认会加入待归档。之后仍可撤销这波归名。</DialogDescription></DialogHeader><div data-sheet-scroll className="min-h-0 flex-1 overflow-y-auto p-4"><PersonPicker people={people} value={creator} onChange={setCreator} disabled={busy} error={peopleError} onRetry={loadPeople}/></div><div className="mobile-bottom-actions shrink-0 border-t p-3"><Button className="min-h-11 w-full" disabled={busy||!creator.trim()||!selectedRows.length} onClick={enqueue}>{busy?'正在保存…':`确认归给 ${creator.trim()||'所选人物'}，加入待归档`}</Button></div></ResponsiveDialogContent></Dialog>
    <VideoPlayer video={playing} onClose={()=>setPlaying(null)}/>
  </div>
}
