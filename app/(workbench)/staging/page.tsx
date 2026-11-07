'use client'
import { useCallback, useEffect, useRef, useState } from 'react'
import Link from 'next/link'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription } from '@/components/ui/dialog'
import { VideoHealthPanel } from '@/components/review/video-health-panel'
import type { StagedFile, StagingList, RestorePlan } from '@/lib/staging-backend'
import type { PersonFileResult } from '@/lib/person-file-model'
async function api(url:string,body?:unknown){const response=await fetch(url,{cache:'no-store',...(body?{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)}:{})});const data=await response.json();if(!response.ok)throw new Error(data.error||'暂存请求失败');return data}
function phaseLabel(phase:string){
  if(phase==='complete')return '已完成'
  if(phase==='rolled_back')return '已回退'
  if(phase==='recovery_needed')return '需要核对'
  if(/ledger/.test(phase))return '同步账本'
  if(/faces|jobs|json/.test(phase))return '同步文件记录'
  if(/file|source|destination|copy|directory/.test(phase))return '移动文件'
  return '正在核对'
}
export default function Staging(){
  const [list,setList]=useState<StagingList|null>(null),[error,setError]=useState(''),[loading,setLoading]=useState(false)
  const [selected,setSelected]=useState<StagedFile|null>(null),[target,setTarget]=useState(''),[folder,setFolder]=useState(''),[original,setOriginal]=useState(true)
  const [plan,setPlan]=useState<RestorePlan|null>(null),[busy,setBusy]=useState(false),[operationError,setOperationError]=useState(''),[result,setResult]=useState<PersonFileResult|null>(null),[phase,setPhase]=useState('')
  const trigger=useRef<HTMLButtonElement|null>(null),refreshButton=useRef<HTMLButtonElement|null>(null),player=useRef<HTMLVideoElement|null>(null)
  const load=useCallback(async(offset=0)=>{setLoading(true);setError('');try{setList(await api('/api/staging?offset='+offset))}catch(e){setError((e as Error).message)}finally{setLoading(false)}},[])
  useEffect(()=>{void load()},[load])
  useEffect(()=>{if(!busy||!plan)return;const timer=setInterval(()=>{void api('/api/staging?id='+plan.id).then(s=>setPhase(`${phaseLabel(s.phase||s.status)} · ${s.committed||0}/${s.total||plan.files.length}`)).catch(()=>{})},700);return()=>clearInterval(timer)},[busy,plan])
  function choose(file:StagedFile,button:HTMLButtonElement){trigger.current=button;setSelected(file);setTarget('');setFolder('');setOriginal(!!file.origin?.original_path);setPlan(null);setResult(null);setOperationError('');setPhase('')}
  async function preview(){if(!selected)return;setBusy(true);setOperationError('');try{setPlan(await api('/api/staging',{mode:'preview',request:{action:'restore-files',person:'',files:[{vid:selected.vid,path:selected.path,version:selected.version}],...(!original?{target_person:target.trim(),...(folder.trim()?{target_folder:folder.trim()}:{})}:{})}}))}catch(e){setOperationError((e as Error).message)}finally{setBusy(false)}}
  async function finish(value:PersonFileResult){setResult(value);await load(list?.offset||0)}
  async function apply(){if(!plan)return;player.current?.pause();setBusy(true);setOperationError('');try{await finish(await api('/api/staging',{mode:'apply',request:plan.request,version:plan.version}))}catch(e){setOperationError((e as Error).message)}finally{setBusy(false)}}
  async function check(){if(!plan)return;setBusy(true);setOperationError('');try{const value=await api('/api/staging',{mode:'reconcile',request:plan.request,version:plan.version});setPhase(phaseLabel(value.status==='complete'||value.status==='rolled_back'?value.status:value.phase||value.status));if(value.status==='complete')await finish(value.result);else if(value.status==='rolled_back'){setOperationError('操作已回退，文件仍在暂存区。请关闭后重新预览。');await load(list?.offset||0)}else setOperationError(value.error||'结果尚未确认，请保留此操作编号继续核对。')}catch(e){setOperationError((e as Error).message)}finally{setBusy(false)}}
  const video=selected&&/\.(mp4|m4v|mov|webm|mkv)$/i.test(selected.name)
  return <main className="wb-page space-y-4">
    <header className="wb-header flex items-center justify-between gap-3"><h1 className="wb-title">回收站</h1><Button ref={refreshButton} variant="outline" disabled={loading} onClick={()=>void load(list?.offset||0)}>{loading?'读取中…':'刷新'}</Button></header>
    {error&&<p role="alert" className="text-sm text-destructive">{error}</p>}
    <div className="grid gap-3 sm:grid-cols-2"><Card className="p-4"><CardHeader className="p-0 pb-1"><CardTitle className="text-xs font-medium text-muted-foreground">暂存文件数</CardTitle></CardHeader><CardContent className="p-0 text-2xl font-semibold tabular-nums">{list?.files??'—'}</CardContent></Card><Card className="p-4"><CardHeader className="p-0 pb-1"><CardTitle className="text-xs font-medium text-muted-foreground">暂存占用</CardTitle></CardHeader><CardContent className="p-0"><div className="text-2xl font-semibold tabular-nums">{list?.gb??'—'} GB</div>{list?.path&&<p className="mt-1 break-all text-xs text-muted-foreground">{list.path}</p>}</CardContent></Card></div>
    {list?.files===0&&<p className="text-xs text-muted-foreground">暂存目录当前没有文件。</p>}
    {list?.rows?.map(file=><Card key={file.path} className="flex flex-col gap-3 p-4 sm:flex-row sm:items-center"><div className="min-w-0 flex-1 space-y-1"><p className="break-all text-sm font-medium">{file.name} <span className="text-xs text-muted-foreground">{(file.size/2**20).toFixed(1)} MB</span></p><p className="break-all text-xs text-muted-foreground">{file.path}</p><p className="break-all text-xs">{file.origin?.original_path?`原位置：${file.origin.original_path}`:'来源未知'}</p>{file.origin?.operation&&<p className="break-all text-xs text-muted-foreground">操作 {file.origin.operation}</p>}</div><Button variant="outline" onClick={event=>choose(file,event.currentTarget)}>恢复</Button></Card>)}
    {!!list?.rows&&<div className="flex items-center justify-between"><Button variant="outline" disabled={loading||list.offset===0} onClick={()=>void load(Math.max(0,list.offset-100))}>上一页</Button><span className="text-xs text-muted-foreground">{list.files?list.offset+1:0}–{list.offset+list.rows.length} / {list.files}</span><Button variant="outline" disabled={loading||list.next_offset===null} onClick={()=>void load(list.next_offset!)}>下一页</Button></div>}
    <Dialog open={!!selected} onOpenChange={open=>{if(!open&&!busy){player.current?.pause();setSelected(null)}}}><DialogContent style={{animation:'none'}} overlayProps={{style:{animation:'none'}}} className="max-h-[90dvh] overflow-y-auto motion-reduce:transition-none" onCloseAutoFocus={event=>{event.preventDefault();(trigger.current?.isConnected?trigger.current:refreshButton.current)?.focus()}} onInteractOutside={event=>{if(busy)event.preventDefault()}}><DialogHeader><DialogTitle>{result?'恢复完成':'恢复文件'}</DialogTitle><DialogDescription className="break-all">{selected?.name}</DialogDescription></DialogHeader>
      {video&&!busy&&!result&&<video ref={player} controls preload="metadata" playsInline className="aspect-video w-full rounded-lg bg-muted" src={'/api/video?p='+encodeURIComponent(selected.vid)}/>}
      {result?<><p role="status">已恢复 {result.changed} 个文件</p><p className="break-all text-xs">{plan?.files[0]?.dst}</p>{result.person&&<Button asChild><Link href={'/persons/'+encodeURIComponent(result.person)}>查看人物视频</Link></Button>}</>:plan?<><div className="space-y-2 break-all text-xs"><p>{plan.files[0].src}</p><p>→ {plan.files[0].dst}</p><p className="text-muted-foreground">操作 {plan.id}</p></div>{phase&&<p role="status" className="break-all text-xs">{phase}</p>}<div className="flex flex-wrap justify-end gap-2"><Button variant="outline" disabled={busy} onClick={()=>{setPlan(null);setOperationError('')}}>返回修改</Button><Button disabled={busy} onClick={()=>void apply()}>{busy?'正在恢复…':'确认恢复'}</Button></div></>:<><div className="flex flex-wrap gap-4 text-sm"><label className="flex items-center gap-2"><input type="radio" name="destination" checked={original} disabled={!selected?.origin?.original_path} onChange={()=>setOriginal(true)}/>原位置</label><label className="flex items-center gap-2"><input type="radio" name="destination" checked={!original} onChange={()=>setOriginal(false)}/>指定人物</label></div>{original?<p className="break-all text-xs">{selected?.origin?.original_path}</p>:<div className="space-y-3"><label className="grid gap-1 text-sm">目标人物<Input value={target} onChange={e=>setTarget(e.target.value)} /></label><label className="grid gap-1 text-sm">子目录（可选）<Input value={folder} onChange={e=>setFolder(e.target.value)} /></label></div>}<Button disabled={busy||(!original&&!target.trim())} onClick={()=>void preview()}>{busy?'核对中…':'预览恢复路径'}</Button></>}
      {operationError&&<div className="space-y-2"><p role="alert" className="break-all text-sm text-destructive">{operationError}</p>{plan&&<Button variant="outline" disabled={busy} onClick={()=>void check()}>核对原操作回执</Button>}</div>}
    </DialogContent></Dialog>
    <VideoHealthPanel />
  </main>
}
