'use client'
import { WorkspacePath } from '@/components/workbench/inner'
import { ExternalPlayerBar } from '@/components/workbench/video-player'
import { VideoPreviewDialog } from '@/components/workbench/video-preview-dialog'
import { useEffect, useRef, useState } from 'react'
import Link from 'next/link'
import { api } from '@/lib/face-api'
import { cn } from '@/lib/utils'
import { Play, AlertCircle } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { fileName, type ReviewVideo } from '@/lib/review-ui'

export function PathText({value,label='原路径'}:{value:string;label?:string}){return <WorkspacePath value={value} label={label}/>}
export function VideoThumbnail({ video, onPlay, className }: { video: ReviewVideo; onPlay: (v: ReviewVideo) => void; className?:string }) {
  return <button type="button" className={cn("video-thumb group relative aspect-video w-28 shrink-0 overflow-hidden rounded-lg border bg-black sm:w-36",className)} onClick={() => onPlay(video)} aria-label={`播放 ${video.name || fileName(video.vid)}`}>
    {video.thumb && <img data-private-media draggable={false} src={video.thumb} width={144} height={81} alt="" loading="lazy" className="h-full w-full object-contain" />}
    <span className="absolute inset-0 flex items-center justify-center bg-black/25"><span className="flex h-9 w-9 items-center justify-center rounded-full bg-black/65 text-white"><Play className="ml-0.5 h-4 w-4" fill="currentColor" aria-hidden="true" /></span></span>
    <span className="absolute bottom-1 right-1 rounded bg-black/75 px-1.5 py-0.5 text-[11px] text-white">播放</span>
  </button>
}
export function VideoPlayer({ video, onClose, returnFocus, instantEntry }: { video: ReviewVideo | null; onClose: () => void; returnFocus?: React.RefObject<HTMLButtonElement | null>; instantEntry?: boolean }) {
  const [failed, setFailed] = useState(false)
  const [reported,setReported]=useState(false),reportSequence=useRef(0)
  const guard=useRef<Promise<{fingerprint:string;path:string;material?:string}>|null>(null)
  useEffect(()=>{
    guard.current=video?api<{fingerprint:string;path:string;material?:string}>('/api/video-health?vid='+encodeURIComponent(video.vid)+(video.path?'&path='+encodeURIComponent(video.path):'')):null
    guard.current?.catch(()=>{})
    setFailed(false);setReported(false);reportSequence.current++
  },[video?.vid,video?.path])
  const close=()=>{reportSequence.current++;onClose();setFailed(false);setReported(false)}
  const playbackError=(event:React.SyntheticEvent<HTMLVideoElement>)=>{
    setFailed(true)
    const code=event.currentTarget.error?.code
    if(!video||![2,3,4].includes(code||0))return
    const sequence=++reportSequence.current
    if(!guard.current)return
    guard.current.then(identity=>api('/api/video-health/report',{method:'POST',body:JSON.stringify({vid:video.vid,path:identity.path,fingerprint:identity.fingerprint,material:identity.material,code})}))
      .then(()=>{if(reportSequence.current===sequence)setReported(true)}).catch(()=>{})
  }
  const src = video ? `/api/video?p=${encodeURIComponent(video.path || video.vid)}` : ''
  const name = video ? (video.name || fileName(video.vid)) : ''
  return (
    <VideoPreviewDialog
      open={!!video}
      onOpenChange={open => { if (!open) close() }}
      title={name || '视频预览'}
      src={src}
      name={name}
      poster={video?.thumb || undefined}
      path={video ? (video.path || video.vid) : undefined}
      onError={playbackError}
      returnFocus={returnFocus}
      instantEntry={instantEntry}
      errorSlot={failed ? (
        <div role="alert" className="space-y-3 rounded-lg border border-amber-500/30 p-3 text-sm text-amber-300">
          <p className="flex items-start gap-2"><AlertCircle className="mt-0.5 h-4 w-4 shrink-0" aria-hidden="true" />暂时无法播放，可能是路径变化、网络中断或不支持此编码，可直接调用外部播放器。</p>
          <ExternalPlayerBar src={src} name={name} />
          {reported && <p className="text-xs">已记下本次失败，下一次自动体检会优先复查。原文件保留。</p>}
          <Button asChild variant="outline" className="min-h-11"><Link href="/staging#video-health" onClick={close}>查看视频体检与失效记录</Link></Button>
        </div>
      ) : undefined}
      footerActions={<Button variant="outline" size="sm" onClick={close}>关闭播放</Button>}
    />
  )
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
