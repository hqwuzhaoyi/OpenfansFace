'use client'

import { useCallback, useEffect, useRef, useState } from 'react'
import * as Popover from '@radix-ui/react-popover'
import {
  Check,
  FolderOpen,
  Keyboard,
  LayoutList,
  X,
} from 'lucide-react'
import { Dialog, DialogDescription, DialogTitle } from '@/components/ui/dialog'
import { Kbd, KbdGroup } from '@/components/ui/kbd'
import { ActionSwapText } from '@/components/beui/motion/action-swap'
import { Collapsible, CollapsibleContent } from '@/components/beui/motion/collapsible'
import { ExternalPlayerBar, WorkbenchVideoPlayer } from '@/components/workbench/video-player'
import { VideoCover } from '@/components/review/video-cover'
import { api } from '@/lib/face-api'
import type { PersonVideo } from '@/lib/person-library-model'
import type { PersonFileAction } from '@/lib/person-file-model'
import {
  WorkbenchButton as Button,
  WorkbenchCount as Count,
  WorkbenchDialogContent as DialogContent,
} from './ui'

export type PlayerContextKind = 'owned' | 'queued' | 'recommendation'

const sizeText = (size?: number) =>
  !size
    ? ''
    : size >= 2 ** 30
      ? `${(size / 2 ** 30).toFixed(2)} GB`
      : size >= 2 ** 20
        ? `${(size / 2 ** 20).toFixed(1)} MB`
        : size >= 1024
          ? `${(size / 1024).toFixed(0)} KB`
          : `${size} B`

export interface PersonVideoDialogProps {
  video: PersonVideo | null
  playlist: PersonVideo[]
  context: PlayerContextKind
  personName: string
  personRoot?: string
  selectedIds: string[]
  busy?: boolean
  theme: 'light' | 'dark'
  onClose: () => void
  onSelectVideo: (video: PersonVideo) => void
  onToggleCheck?: (vid: string) => void
  onManageFile?: (action: PersonFileAction, video: PersonVideo) => void
  onReturnQueued?: (vid: string) => void
  onAcceptRecommendation?: (video: PersonVideo) => void
  onRejectRecommendation?: (video: PersonVideo) => void
  onLaterRecommendation?: (video: PersonVideo) => void
  notify: (message: string) => void
}

export function PersonVideoDialog({
  video,
  playlist,
  context,
  personName,
  selectedIds,
  busy = false,
  theme,
  onClose,
  onSelectVideo,
  onToggleCheck,
  onAcceptRecommendation,
  onRejectRecommendation,
  notify,
}: PersonVideoDialogProps) {
  const videoRef = useRef<HTMLVideoElement | null>(null)
  const copyTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const wasDialogOpenRef = useRef(false)

  const [failed, setFailed] = useState(false)
  const [showEpisodes, setShowEpisodes] = useState(false)
  const [showShortcuts, setShowShortcuts] = useState(false)
  const [copied, setCopied] = useState(false)
  const [hudSignal, setHudSignal] = useState<{ key: number; label: string } | null>(null)

  const rawVideoSrc = video ? `/api/video?p=${encodeURIComponent(video.path || video.vid)}` : ''
  const index = video ? playlist.findIndex(item => item.vid === video.vid) : -1
  const hasPrev = index > 0
  const hasNext = index >= 0 && index < playlist.length - 1
  const checked = video ? selectedIds.includes(video.vid) : false

  const flashHud = useCallback((label: string) => {
    setHudSignal({ key: Date.now(), label })
  }, [])

  useEffect(() => {
    return () => {
      if (copyTimer.current) clearTimeout(copyTimer.current)
    }
  }, [])

  useEffect(() => {
    const isOpen = Boolean(video)
    if (isOpen && !wasDialogOpenRef.current) {
      setShowEpisodes(typeof window !== 'undefined' ? window.innerWidth >= 768 && window.innerHeight >= 780 : false)
      setShowShortcuts(false)
    }
    wasDialogOpenRef.current = isOpen
  }, [video])

  useEffect(() => {
    setFailed(false)
    setCopied(false)
  }, [video?.vid])

  const copyPath = useCallback(async () => {
    if (!video) return
    const targetPath = video.path || video.vid
    try {
      await navigator.clipboard.writeText(targetPath)
      setCopied(true)
      if (copyTimer.current) clearTimeout(copyTimer.current)
      copyTimer.current = setTimeout(() => setCopied(false), 1600)
      flashHud('路径已复制')
    } catch {
      notify('复制失败，请选中路径文字复制。')
    }
  }, [video, flashHud, notify])

  const goPrev = useCallback(() => {
    if (hasPrev) {
      onSelectVideo(playlist[index - 1])
      flashHud(`上一集 (${index} / ${playlist.length})`)
    }
  }, [hasPrev, index, playlist, onSelectVideo, flashHud])

  const goNext = useCallback(() => {
    if (hasNext) {
      onSelectVideo(playlist[index + 1])
      flashHud(`下一集 (${index + 2} / ${playlist.length})`)
    }
  }, [hasNext, index, playlist, onSelectVideo, flashHud])

  const toggleSelectCurrent = useCallback(() => {
    if (!video || !onToggleCheck || context === 'recommendation') return
    onToggleCheck(video.vid)
    flashHud(checked ? '已取消勾选' : '已勾选')
  }, [video, onToggleCheck, context, checked, flashHud])

  useEffect(() => {
    if (!video) return
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.defaultPrevented || e.metaKey || e.ctrlKey || e.altKey) return
      const target = e.target as HTMLElement | null
      if (target && (['INPUT', 'TEXTAREA', 'SELECT'].includes(target.tagName) || target.isContentEditable)) return

      switch (e.key) {
        case '[':
          if (hasPrev) {
            e.preventDefault()
            goPrev()
          }
          break
        case ']':
          if (hasNext) {
            e.preventDefault()
            goNext()
          }
          break
        case 'x':
        case 'X':
          if (context !== 'recommendation' && onToggleCheck) {
            e.preventDefault()
            toggleSelectCurrent()
          }
          break
        case 'c':
        case 'C':
          e.preventDefault()
          void copyPath()
          break
        case '?':
        case 'h':
        case 'H':
          e.preventDefault()
          setShowShortcuts(v => !v)
          break
        case 'e':
        case 'E':
          if (playlist.length > 1) {
            e.preventDefault()
            setShowEpisodes(v => !v)
          }
          break
      }
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [
    video,
    context,
    hasPrev,
    hasNext,
    playlist.length,
    goPrev,
    goNext,
    toggleSelectCurrent,
    copyPath,
    onToggleCheck,
  ])

  const folderLabel =
    context === 'recommendation'
      ? '推荐加入'
      : context === 'queued'
        ? '待归档'
        : video?.folder === '.' || !video?.folder
          ? '根目录'
          : video.folder

  const railStart = Math.max(0, Math.min(index - 3, playlist.length - 8))
  const railItems = playlist.slice(railStart, railStart + 8)

  const theaterTopOverlay = video ? (
    <div className="pw-theater-top">
      <div className="pw-theater-title-group">
        <span className="pw-theater-tag">
          {personName ? `${personName} · ${folderLabel}` : folderLabel}
        </span>
        <strong className="pw-theater-title" title={video.name}>
          {video.name}
        </strong>
        {sizeText(video.size) && <span className="pw-theater-size">{sizeText(video.size)}</span>}
      </div>

      <div className="pw-theater-top-actions">
        {playlist.length > 1 && index >= 0 && (
          <span className="pw-theater-counter">
            <Count value={index + 1} /> / {playlist.length}
          </span>
        )}
        {playlist.length > 1 && (
          <button
            type="button"
            className="pw-theater-header-pill"
            aria-expanded={showEpisodes}
            aria-label={showEpisodes ? '收起选集' : '展开选集'}
            title="选集 (E)"
            onClick={() => setShowEpisodes(v => !v)}
          >
            <LayoutList size={14} aria-hidden="true" />
            <span>选集</span>
          </button>
        )}
        <Popover.Root open={showShortcuts} onOpenChange={setShowShortcuts}>
          <Popover.Trigger asChild>
            <button
              type="button"
              className="pw-theater-icon-btn pw-theater-shortcuts-btn"
              aria-label="快捷键说明"
              aria-expanded={showShortcuts}
              title="快捷键 (?)"
            >
              <Keyboard size={15} aria-hidden="true" />
            </button>
          </Popover.Trigger>
          <Popover.Portal>
            <Popover.Content
              className="pw-theater-shortcuts-sheet"
              align="end"
              sideOffset={8}
              collisionPadding={12}
              aria-label="键盘快捷键说明"
            >
              <div className="pw-theater-shortcuts-head">
                <strong>键盘快捷键</strong>
                <Popover.Close asChild>
                  <button type="button" aria-label="关闭快捷键说明">
                    <X size={14} aria-hidden="true" />
                  </button>
                </Popover.Close>
              </div>
              <div className="pw-theater-shortcuts-grid">
                <div>
                  <span>播放 / 暂停</span>
                  <Kbd>Space</Kbd>
                </div>
                <div>
                  <span>快退 / 快进 5 秒</span>
                  <KbdGroup>
                    <Kbd>←</Kbd>
                    <Kbd>→</Kbd>
                  </KbdGroup>
                </div>
                <div>
                  <span>长按 2.0x 倍速</span>
                  <Kbd>长按 →</Kbd>
                </div>
                <div>
                  <span>调节音量</span>
                  <KbdGroup>
                    <Kbd>↑</Kbd>
                    <Kbd>↓</Kbd>
                  </KbdGroup>
                </div>
                <div>
                  <span>上一集 / 下一集</span>
                  <KbdGroup>
                    <Kbd>[</Kbd>
                    <Kbd>]</Kbd>
                  </KbdGroup>
                </div>
                <div>
                  <span>打开 / 收起选集</span>
                  <Kbd>E</Kbd>
                </div>
                <div>
                  <span>勾选当前视频</span>
                  <Kbd>X</Kbd>
                </div>
                <div>
                  <span>复制文件路径</span>
                  <Kbd>C</Kbd>
                </div>
                <div>
                  <span>全屏 / 静音</span>
                  <KbdGroup>
                    <Kbd>F</Kbd>
                    <Kbd>M</Kbd>
                  </KbdGroup>
                </div>
              </div>
            </Popover.Content>
          </Popover.Portal>
        </Popover.Root>
        <button
          type="button"
          className="pw-theater-icon-btn"
          aria-label="关闭视频预览"
          title="关闭 (Esc)"
          onClick={onClose}
        >
          <X size={16} aria-hidden="true" />
        </button>
      </div>
    </div>
  ) : null

  return (
    <Dialog open={!!video} onOpenChange={open => !open && onClose()}>
      <DialogContent
        className="pw-modal pw-theater-modal top-0 left-0 translate-x-0 translate-y-0 md:top-[50%] md:left-[50%] md:translate-x-[-50%] md:translate-y-[-50%]"
        data-theme={theme}
        showCloseButton={false}
      >
        {video && (
          <>
            <DialogTitle className="sr-only">{video.name}</DialogTitle>
            <DialogDescription className="sr-only">
              沉浸式视频播放器，支持空格播放暂停、左右快进快退、长按右方向键二倍速、方括号切换上下集。
            </DialogDescription>

            <div className="pw-theater-stage-wrap">
              <WorkbenchVideoPlayer
                key={video.path || video.vid}
                videoRef={videoRef}
                src={rawVideoSrc}
                name={video.name}
                poster={video.thumb || undefined}
                fill
                autoPlay
                showVolumeSlider
                enableGlobalHotkeys
                onPrev={playlist.length > 1 ? goPrev : undefined}
                onNext={playlist.length > 1 ? goNext : undefined}
                hasPrev={hasPrev}
                hasNext={hasNext}
                topOverlay={theaterTopOverlay}
                hudSignal={hudSignal}
                onError={() => {
                  setFailed(true)
                  const code = videoRef.current?.error?.code
                  if ([2, 3, 4].includes(code || 0)) {
                    void api('/api/video-health/report', {
                      method: 'POST',
                      body: JSON.stringify({ vid: video.vid, path: video.path || video.vid, code }),
                    }).catch(() => {})
                  }
                }}
              />
            </div>

            {failed && (
              <div role="alert" className="pw-error pw-theater-error">
                <p>暂时无法播放，请核对文件路径、网络或浏览器编码支持，或使用外部播放器打开。</p>
                <ExternalPlayerBar src={rawVideoSrc} name={video.name} />
              </div>
            )}

            <div className="pw-theater-footer">
              <button
                type="button"
                className="pw-theater-path-btn"
                title="点击复制完整路径 (快捷键 C)"
                onClick={() => void copyPath()}
              >
                <FolderOpen size={13} aria-hidden="true" />
                <code translate="no">{video.path || video.vid}</code>
                <span className="pw-theater-copy-state">
                  <ActionSwapText value={copied ? 'copied' : 'copy'} animation="roll">
                    {copied ? '已复制' : '复制'}
                  </ActionSwapText>
                </span>
              </button>

              {context === 'recommendation' && (onAcceptRecommendation || onRejectRecommendation) && (
                <div className="pw-theater-actions">
                  {onAcceptRecommendation && (
                    <Button
                      disabled={busy}
                      className="pw-primary"
                      onClick={() => onAcceptRecommendation(video)}
                    >
                      <Check size={13} aria-hidden="true" />
                      <span>去人工确认</span>
                    </Button>
                  )}
                  {onRejectRecommendation && (
                    <Button disabled={busy} onClick={() => onRejectRecommendation(video)}>
                      不是此人
                    </Button>
                  )}
                </div>
              )}
            </div>

            {railItems.length > 1 && (
              <Collapsible open={showEpisodes} onOpenChange={setShowEpisodes} className="pw-theater-episodes-collapse">
                <CollapsibleContent className="pw-theater-episodes-body">
                  <div className="pw-player-rail" role="region" aria-label="同目录选集">
                    {railItems.map(item => {
                      const active = item.vid === video.vid
                      const itemChecked = selectedIds.includes(item.vid)
                      return (
                        <button
                          key={item.vid}
                          type="button"
                          aria-current={active ? 'true' : undefined}
                          className="pw-player-rail-item"
                          data-active={active || undefined}
                          onClick={() => onSelectVideo(item)}
                          title={item.name}
                        >
                          <VideoCover video={item} className="pw-player-rail-cover" />
                          {itemChecked && (
                            <span className="pw-player-rail-check" aria-label="已勾选">
                              <Check size={10} aria-hidden="true" />
                            </span>
                          )}
                          <span className="pw-player-rail-name">{item.name}</span>
                        </button>
                      )
                    })}
                  </div>
                </CollapsibleContent>
              </Collapsible>
            )}
          </>
        )}
      </DialogContent>
    </Dialog>
  )
}
