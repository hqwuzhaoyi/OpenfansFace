'use client'
import { useRef, useState } from 'react'
import Link from 'next/link'
import { api } from '@/lib/face-api'
import { ResponsiveDialogContent } from './responsive-dialog'
import { cn } from '@/lib/utils'
import { Copy, Play, AlertCircle } from 'lucide-react'
import { toast } from 'sonner'
import { Dialog, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { fileName, type ReviewVideo } from '@/lib/review-ui'

export function PathText({ value, label = '原路径' }: { value: string; label?: string }) {
  const copy = async () => {
    try {
      if (navigator.clipboard?.writeText) await navigator.clipboard.writeText(value)
      else {
        const previous=document.activeElement
        const input = document.createElement('textarea'); input.value = value; input.readOnly=true; input.tabIndex=-1; input.setAttribute('inputmode','none')
        input.style.position = 'fixed'; input.style.opacity = '0'; document.body.appendChild(input)
        input.select(); const ok = document.execCommand('copy'); input.remove(); if(previous instanceof HTMLElement)previous.focus({preventScroll:true}); if (!ok) throw new Error()
      }
      toast.success('路径已复制')
    } catch { toast.error('复制失败，可以直接选择路径文字复制') }
  }
  return <div className="min-w-0 text-xs text-muted-foreground">
    <div className="mb-1 flex items-center gap-2"><span>{label}</span><button type="button" className="inline-flex min-h-7 items-center gap-1 rounded px-1 text-muted-foreground hover:text-foreground" onClick={copy} aria-label={`复制${label}`}><Copy className="h-3 w-3" />复制</button></div>
    <p className="break-all font-mono leading-relaxed text-foreground/65">{value}</p>
  </div>
}
export function VideoThumbnail({ video, onPlay, className }: { video: ReviewVideo; onPlay: (v: ReviewVideo) => void; className?:string }) {
  return <button type="button" className={cn("video-thumb group relative aspect-video w-28 shrink-0 overflow-hidden rounded-lg border bg-muted sm:w-36",className)} onClick={() => onPlay(video)} aria-label={`播放 ${video.name || fileName(video.vid)}`}>
    {video.thumb && <img src={video.thumb} alt="" loading="lazy" className="h-full w-full object-cover" />}
    <span className="absolute inset-0 flex items-center justify-center bg-black/25"><span className="flex h-9 w-9 items-center justify-center rounded-full bg-black/65 text-white"><Play className="ml-0.5 h-4 w-4" fill="currentColor" /></span></span>
    <span className="absolute bottom-1 right-1 rounded bg-black/75 px-1.5 py-0.5 text-[10px] text-white">播放</span>
  </button>
}
export function VideoPlayer({ video, onClose }: { video: ReviewVideo | null; onClose: () => void }) {
  const [failed, setFailed] = useState(false)
  const [reported,setReported]=useState(false),reportSequence=useRef(0)
  const close=()=>{reportSequence.current++;onClose();setFailed(false);setReported(false)}
  const playbackError=(event:React.SyntheticEvent<HTMLVideoElement>)=>{
    setFailed(true)
    const code=event.currentTarget.error?.code
    if(!video||![2,3,4].includes(code||0))return
    const sequence=++reportSequence.current
    api('/api/video-health/report',{method:'POST',body:JSON.stringify({vid:video.vid,path:video.path||video.vid,code})})
      .then(()=>{if(reportSequence.current===sequence)setReported(true)}).catch(()=>{})
  }
  const src = video ? `/api/video?p=${encodeURIComponent(video.path || video.vid)}` : ''
  return <Dialog open={!!video} onOpenChange={o => { if (!o) close() }}>
    <ResponsiveDialogContent open={!!video} className="flex max-h-[92dvh] flex-col gap-0 overflow-hidden p-0 sm:max-w-4xl sm:p-0">
      <DialogHeader className="shrink-0 border-b p-4 pr-14 text-left"><DialogTitle>视频预览</DialogTitle><DialogDescription>关闭后回到刚才的位置，勾选保持不变。</DialogDescription></DialogHeader>
      {video&&<div data-sheet-scroll className="min-h-0 flex-1 space-y-3 overflow-y-auto p-4"><p className="break-all text-sm font-medium">{video.name}</p><video key={src} src={src} controls autoPlay playsInline preload="metadata" onError={playbackError} className="max-h-[55dvh] w-full rounded-lg bg-black"/>
        {failed&&<div role="alert" className="space-y-3 rounded-lg border border-amber-500/30 p-3 text-sm text-amber-300"><p className="flex items-start gap-2"><AlertCircle className="mt-0.5 h-4 w-4 shrink-0"/>暂时无法播放，可能是路径变化、网络中断或手机不支持此编码。</p>{reported&&<p className="text-xs">已记下本次失败，下一次自动体检会优先复查。原文件保留。</p>}<Button asChild variant="outline" className="min-h-11"><Link href="/staging#video-health" onClick={close}>查看视频体检与失效记录</Link></Button></div>}<PathText value={video.path||video.vid}/></div>}
      <div className="mobile-bottom-actions shrink-0 border-t p-3"><Button variant="outline" className="min-h-11 w-full" onClick={close}>关闭播放，继续核对</Button></div>
    </ResponsiveDialogContent>
  </Dialog>
}
export function VideoRow({ video, selected, onToggle, onPlay, children, disabled }: {
  video: ReviewVideo; selected: boolean; onToggle: () => void; onPlay: (v: ReviewVideo) => void; children?: React.ReactNode; disabled?: boolean
}) {
  return <article data-vid={video.vid} className={`min-w-0 rounded-xl border p-3 ${selected ? 'border-white/25 bg-white/[0.035]' : 'bg-card'}`}>
    <div className="flex items-start gap-3">
      <label className="flex min-h-11 min-w-11 shrink-0 cursor-pointer items-center justify-center"><input type="checkbox" checked={selected} disabled={disabled} onChange={onToggle} aria-label={`选择 ${video.name || fileName(video.vid)}`} className="h-4 w-4 accent-white" /></label>
      <div className="min-w-0 flex-1 space-y-2">
        <div className="flex flex-col items-start gap-3 sm:flex-row"><VideoThumbnail video={video} onPlay={onPlay} /><div className="min-w-0 flex-1 space-y-1"><p className="break-all text-sm font-medium leading-relaxed">{video.name || fileName(video.vid)}</p><p className="text-xs text-muted-foreground">{video.size_gb ?? 0} GB{video.duration ? ` · ${Math.floor(video.duration / 60)} 分钟` : ''}{video.conf === 'weak' && <span className="ml-2 text-amber-300">支持归组的人脸较少，请核对</span>}</p></div></div>
        <PathText value={video.path || video.vid} />{children}
      </div>
    </div>
  </article>
}
