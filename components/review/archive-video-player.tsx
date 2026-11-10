'use client'

import { useEffect, useRef, useState } from 'react'
import { ExternalPlayerBar, WorkbenchVideoPlayer } from '@/components/workbench/video-player'
import Link from 'next/link'
import { AlertCircle, RefreshCw } from 'lucide-react'
import { api } from '@/lib/face-api'
import type { ReviewVideo } from '@/lib/review-ui'
import { Button } from '@/components/ui/button'

/** Key this component by the source path so switching rows tears down the previous media. */
export function ArchiveVideoPlayer({ video, detailVisibleOnMobile }: { video: ReviewVideo; detailVisibleOnMobile: boolean }) {
  const [failed, setFailed] = useState(false)
  const [reported, setReported] = useState(false)
  const [attempt, setAttempt] = useState(0)
  const reportSequence = useRef(0)
  const player = useRef<HTMLVideoElement>(null)
  const source = `/api/video?p=${encodeURIComponent(video.path || video.vid)}`
  useEffect(() => {
    const element = player.current
    // React Strict Mode replays effects; restore the source after its cleanup pass.
    if (element && !element.getAttribute('src')) element.setAttribute('src', source)
    return () => {
      reportSequence.current++
      element?.pause()
      element?.removeAttribute('src')
      element?.load()
    }
  }, [attempt, source])
  useEffect(() => {
    const mobile = window.matchMedia('(max-width: 767px)')
    const pauseHidden = () => { if (mobile.matches && !detailVisibleOnMobile) player.current?.pause() }
    pauseHidden()
    mobile.addEventListener('change', pauseHidden)
    return () => mobile.removeEventListener('change', pauseHidden)
  }, [detailVisibleOnMobile, attempt])
  const playbackError = (event: React.SyntheticEvent<HTMLVideoElement>) => {
    setFailed(true)
    const code = event.currentTarget.error?.code
    if (![2, 3, 4].includes(code || 0)) return
    const sequence = ++reportSequence.current
    void api('/api/video-health/report', { method: 'POST', body: JSON.stringify({ vid: video.vid, path: video.path || video.vid, code }) })
      .then(() => { if (sequence === reportSequence.current) setReported(true) }).catch(() => {})
  }
  return <div className="archive-inlinePlayer">
    <WorkbenchVideoPlayer videoRef={player} key={attempt} src={source} name={video.name} poster={video.thumb || undefined} preload="none" onError={playbackError} />
    {failed && <div role="alert" className="archive-playerError"><p><AlertCircle size={15} />暂时无法播放，请检查文件路径、网络或视频编码，或使用外部播放器打开。</p><ExternalPlayerBar src={source} name={video.name} />{reported && <small>已记录播放失败，原文件保留。</small>}<div><Button variant="outline" size="sm" onClick={() => { reportSequence.current++; setFailed(false); setReported(false); setAttempt(value => value + 1) }}><RefreshCw size={14} />重新加载</Button><Link href="/staging#video-health">查看视频体检</Link></div></div>}
  </div>
}
