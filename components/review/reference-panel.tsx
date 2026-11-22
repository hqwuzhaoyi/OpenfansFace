'use client'
import { useEffect, useState } from 'react'
import { Play } from 'lucide-react'
import { api } from '@/lib/face-api'
import type { ReviewVideo } from '@/lib/review-ui'
import { WorkspaceButton as Button, WorkspaceMessage, WorkspaceMedia, WorkspacePath, WorkspaceSection } from '@/components/workbench/inner'
import { ExternalProfiles } from '@/components/persons/external-profiles'

export function usePersonReferences(name: string | null) {
  const [state, setState] = useState<{ name: string; videos: ReviewVideo[]; total: number; error?: string } | null>(null)
  const [retryToken, setRetryToken] = useState(0)

  useEffect(() => {
    if (!name) {
      setState(null)
      return
    }
    const abort = new AbortController()
    setState(null)
    api<{ vids: ReviewVideo[]; total: number }>(`/api/persons/reference?name=${encodeURIComponent(name)}`, { signal: abort.signal })
      .then(data => {
        if (!abort.signal.aborted) setState({ name, videos: data.vids, total: data.total })
      })
      .catch(error => {
        if (!abort.signal.aborted) setState({ name, videos: [], total: 0, error: (error as Error).message })
      })
    return () => abort.abort()
  }, [name, retryToken])

  const current = name && state?.name === name ? state : null
  return {
    current,
    loading: Boolean(name && !current),
    retry: () => setRetryToken(n => n + 1),
  }
}

/** Inline content shared by the inspector and legacy reference entry; never opens another player dialog. */
export function ReferencePanel({ name }: { name: string }) {
  const { current, retry } = usePersonReferences(name)
  const [playing, setPlaying] = useState<ReviewVideo | null>(null)

  useEffect(() => {
    setPlaying(null)
  }, [name])

  return (
    <div className="ws-inner-body" aria-label={`${name} 的对照视频`}>
      {playing && (
        <>
          <WorkspaceMedia key={playing.vid} path={playing.path || playing.vid} name={playing.name} autoPlay />
          <WorkspacePath value={playing.path || playing.vid} />
        </>
      )}
      {!current ? (
        <WorkspaceMessage>读取对照视频…</WorkspaceMessage>
      ) : current.error ? (
        <WorkspaceMessage error>
          {current.error}
          <Button variant="outline" onClick={retry}>
            重新读取
          </Button>
        </WorkspaceMessage>
      ) : (
        <WorkspaceSection title="已归类视频" count={current.total}>
          {current.videos.length ? (
            <div className="ws-reference-list">
              {current.videos.map(video => (
                <Button
                  key={video.vid}
                  variant="ghost"
                  className="ws-reference-row"
                  aria-label={`播放对照 ${video.name}`}
                  aria-pressed={playing?.vid === video.vid}
                  onClick={() => setPlaying(video)}
                >
                  <span className="ws-reference-thumb">
                    {video.thumb ? <img src={video.thumb} data-private-media draggable={false} alt="" loading="lazy" /> : <Play size={16} />}
                  </span>
                  <span className="ws-person-name">
                    {video.name}
                    <small>{video.size_gb == null ? '大小未记录' : `${video.size_gb} GB`}</small>
                  </span>
                  <Play size={14} />
                </Button>
              ))}
            </div>
          ) : (
            <WorkspaceMessage>该人物暂时没有已扫描的对照视频。</WorkspaceMessage>
          )}
          {current.total > current.videos.length && (
            <p className="ws-muted">
              显示前 {current.videos.length} 个，共 {current.total} 个。
            </p>
          )}
        </WorkspaceSection>
      )}
      <ExternalProfiles name={name} compact />
    </div>
  )
}
