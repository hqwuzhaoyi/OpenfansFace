'use client'
import { useState } from 'react'
import { ResponsiveDialogContent } from './responsive-dialog'
import { Eye } from 'lucide-react'
import { api } from '@/lib/face-api'
import { type ReviewVideo } from '@/lib/review-ui'
import { Button } from '@/components/ui/button'
import { Dialog, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { VideoThumbnail, VideoPlayer } from './video'
export function PersonReference({ name, compact = false }: {name:string; compact?:boolean}) {
  const [open,setOpen]=useState(false),[loading,setLoading]=useState(false),[error,setError]=useState(''),[videos,setVideos]=useState<ReviewVideo[]>([]),[playing,setPlaying]=useState<ReviewVideo|null>(null)
  const load=async()=>{setOpen(true);setLoading(true);setError('');setVideos([]);try{const d=await api<{vids:ReviewVideo[]}>(`/api/persons/reference?name=${encodeURIComponent(name)}`);setVideos(d.vids||[])}catch(e:any){setError(e.message)}finally{setLoading(false)}}
  return <><Button variant={compact?"ghost":"outline"} size="sm" className="h-auto min-h-11 max-w-full whitespace-normal text-left" onClick={load} aria-label={`看 ${name} 的对照视频`}><Eye className="h-3.5 w-3.5 shrink-0"/><span className="min-w-0 break-all">{compact?"看对照":`看 ${name} 的对照视频`}</span></Button><Dialog open={open} onOpenChange={setOpen}><ResponsiveDialogContent open={open} className="flex max-h-[90dvh] flex-col gap-0 overflow-hidden p-0 sm:max-w-3xl sm:p-0"><DialogHeader className="shrink-0 border-b p-4 pr-14 text-left"><DialogTitle>{name} 的对照视频</DialogTitle><DialogDescription>来自该人物 creators 目录中已归类的视频，仅作对照。点缩略图可以播放。</DialogDescription></DialogHeader><div data-sheet-scroll className="min-h-0 flex-1 overflow-y-auto p-4">{loading?<p role="status">加载对照视频…</p>:error?<p role="alert">{error}</p>:videos.length?<div className="grid grid-cols-2 gap-3 sm:grid-cols-3">{videos.map(v=><div key={v.vid} className="min-w-0"><VideoThumbnail video={v} onPlay={setPlaying} className="w-full sm:w-full"/><p className="mt-1 break-all text-xs text-muted-foreground">{v.name}</p></div>)}</div>:<p className="py-8 text-sm text-muted-foreground">creators 目录中暂时没有这个人物的已扫描对照视频，拿不准可以先跳过。</p>}</div><div className="mobile-bottom-actions shrink-0 border-t p-3"><Button variant="outline" className="min-h-11 w-full" onClick={()=>setOpen(false)}>返回选择人物</Button></div></ResponsiveDialogContent></Dialog><VideoPlayer video={playing} onClose={()=>setPlaying(null)}/></>
}
