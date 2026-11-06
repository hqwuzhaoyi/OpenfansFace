'use client'
import { useEffect, useState } from 'react'
import { useParams } from 'next/navigation'
import Link from 'next/link'
import { ArrowLeft, Check } from 'lucide-react'
import { toast } from 'sonner'
import { api } from '@/lib/face-api'
import { decodedName } from '@/lib/review-logic'
import { type ReviewVideo, videoGroups } from '@/lib/review-ui'
import { ReviewFlow, QueueReceipt } from '@/components/review/workflow'
import { VideoRow, VideoPlayer } from '@/components/review/video'
import { PersonReference } from '@/components/review/person-reference'
import { Button } from '@/components/ui/button'

export default function PersonDetail() {
  const params=useParams<{name:string}>(), name=decodedName(params.name)
  const [data,setData]=useState<any>(null),[loading,setLoading]=useState(true),[error,setError]=useState('')
  const [selected,setSelected]=useState<Set<string>>(new Set()),[playing,setPlaying]=useState<ReviewVideo|null>(null)
  const [busy,setBusy]=useState(false),[receipt,setReceipt]=useState<string[]>([])
  const load=async()=>{const d=await api<{identities:any[]}>('/api/identities');const person=d.identities.find(i=>i.name===name);setData(person||null);return person}
  useEffect(()=>{let active=true;setLoading(true);setError('');setData(null);setSelected(new Set());setReceipt([]);api<{identities:any[]}>('/api/identities').then(d=>{if(!active)return;const p=d.identities.find(i=>i.name===name);setData(p||null);setSelected(new Set((p?.vids||[]).filter((v:ReviewVideo)=>v.state==='pending').map((v:ReviewVideo)=>v.vid)))}).catch(e=>{if(active)setError(e.message)}).finally(()=>{if(active)setLoading(false)});return()=>{active=false}},[name])
  const pending:ReviewVideo[]=(data?.vids||[]).filter((v:ReviewVideo)=>v.state==='pending')
  const handled:ReviewVideo[]=(data?.vids||[]).filter((v:ReviewVideo)=>v.state!=='pending')
  const toggle=(ids:string[])=>setSelected(prev=>{const n=new Set(prev),all=ids.every(v=>n.has(v));ids.forEach(v=>all?n.delete(v):n.add(v));return n})
  const enqueue=async()=>{if(!selected.size||busy)return;setBusy(true);const vids=pending.filter(v=>selected.has(v.vid)).map(v=>v.vid);try{await api('/api/plan/queue',{method:'POST',body:JSON.stringify({creator:name,vids,source:'person',source_id:name})});setReceipt(vids);setSelected(new Set());setData((p:any)=>p?{...p,vids:p.vids.map((v:any)=>vids.includes(v.vid)?{...v,state:'queued',queued_creator:name}:v)}:p);await load().catch(()=>toast.warning('归名已保存，人物列表暂未刷新，无需重复提交'));toast.success('已加入待归档，文件尚未移动')}catch(e:any){toast.error(e.message)}finally{setBusy(false)}}
  const undo=async()=>{setBusy(true);try{await api('/api/plan/queue',{method:'DELETE',body:JSON.stringify({vids:receipt})});setSelected(new Set(receipt));setReceipt([]);await load().catch(()=>toast.warning('撤销已保存，列表暂未刷新，无需重复操作'));toast.success('已撤销，视频回到待确认')}catch(e:any){toast.error(e.message)}finally{setBusy(false)}}
  return <div className="space-y-5 p-4 md:p-6"><ReviewFlow/><Link href="/persons" className="inline-flex items-center gap-1 text-sm text-muted-foreground"><ArrowLeft className="h-4 w-4"/>全部人物</Link><div><h1 className="break-all text-2xl font-semibold">{name}</h1><p className="mt-2 text-sm leading-relaxed text-muted-foreground">这里仅需核对尚未归类的视频。creators 内已归类的视频仅作对照，不参与勾选。</p><div className="mt-3"><PersonReference name={name}/></div></div>
    {receipt.length>0&&<QueueReceipt count={receipt.length} creator={name} onUndo={undo} busy={busy}/>}
    {loading?<p role="status" className="py-12 text-muted-foreground">加载人物视频…</p>:error?<p role="alert" className="text-amber-300">加载失败：{error}</p>:!data?<p className="rounded-xl border p-6 text-muted-foreground">暂未找到这个人物的识别报告，请返回人物列表核对名字。</p>:<>
      <p className="text-sm text-muted-foreground">待确认 {pending.length} 个 · 已在待归档或已归档 {handled.length} 个 · 模板脸 {data.tpl_faces??0} 张</p>
      {!!pending.length&&<><div className="flex flex-wrap items-center gap-3"><span className="text-sm">已选 {selected.size} 个</span><Button variant="outline" size="sm" disabled={busy} onClick={()=>setSelected(new Set(pending.map(v=>v.vid)))}>全选</Button><Button variant="ghost" size="sm" disabled={busy} onClick={()=>setSelected(new Set())}>清空选择</Button></div>{videoGroups(pending).map(([dir,vs])=><section key={dir} className="space-y-2"><label className="flex cursor-pointer items-start gap-2 rounded-lg border bg-muted/30 p-3"><input type="checkbox" className="mt-0.5 h-4 w-4 accent-white" checked={vs.every(v=>selected.has(v.vid))} disabled={busy} onChange={()=>toggle(vs.map(v=>v.vid))} aria-label={`选择文件夹 ${dir}`}/><span className="min-w-0 break-all text-xs leading-relaxed">{dir} · {vs.length} 个</span></label>{vs.map(v=><VideoRow key={v.vid} video={v} selected={selected.has(v.vid)} onToggle={()=>toggle([v.vid])} onPlay={setPlaying} disabled={busy}/>)}</section>)}</>}
      {!pending.length&&<p className="rounded-xl border border-dashed p-8 text-muted-foreground">这个人物当前没有待确认的视频。已确认的可到待归档查看，已归类的视频可用上方“看对照”查看。</p>}
      {!!handled.length&&<details className="rounded-xl border p-3"><summary className="cursor-pointer text-sm text-muted-foreground">查看已处理的 {handled.length} 个视频</summary><div className="mt-3 space-y-3">{handled.map(v=><VideoRow key={v.vid} video={v} selected={false} onToggle={()=>{}} disabled onPlay={setPlaying}><p className="text-xs text-emerald-300">{v.organized?'已归类（creators）':v.state==='archived'?'已归档':`待归档 → ${v.queued_creator}`} {v.state==='queued'&&<Link href="/integration" className="ml-2 underline">去查看或撤销</Link>}</p></VideoRow>)}</div></details>}
      {!!pending.length&&<div className="review-actionbar sticky bottom-20 z-10 flex flex-wrap items-center gap-3 rounded-xl border bg-background/95 p-4 backdrop-blur md:bottom-4"><p className="min-w-0 flex-1 text-xs text-muted-foreground">下一步：待归档。确认后文件仍在原处。</p><Button disabled={busy||!selected.size} onClick={enqueue} className="h-auto min-h-10 whitespace-normal"><Check className="h-4 w-4"/>{busy?'正在加入…':`确认选中的 ${selected.size} 个视频是 ${name}，加入待归档`}</Button></div>}
    </>}<VideoPlayer video={playing} onClose={()=>setPlaying(null)}/>
  </div>
}
