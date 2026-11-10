'use client'

import { useCallback, useEffect, useRef, useState } from 'react'
import { AnimatePresence, motion } from 'motion/react'
import {
  Check,
  ExternalLink,
  FolderInput,
  FolderOpen,
  FastForward,
  Keyboard,
  LayoutList,
  Maximize2,
  Minimize2,
  Pause,
  Pencil,
  Play,
  RotateCcw,
  RotateCw,
  SkipBack,
  SkipForward,
  Trash2,
  Undo2,
  Volume2,
  VolumeX,
  X,
} from 'lucide-react'
import { PrivateVideo } from '@/components/private-media'
import {
  buildExternalPlayerLinks,
  ExternalPlayerBar,
  normalizeVideoApiPath,
} from '@/components/workbench/video-player'
import { VideoCover } from '@/components/review/video-cover'
import { Dialog, DialogDescription, DialogTitle } from '@/components/ui/dialog'
import { Kbd, KbdGroup } from '@/components/ui/kbd'
import { ActionSwapIcon, ActionSwapText } from '@/components/beui/motion/action-swap'
import { Collapsible, CollapsibleContent } from '@/components/beui/motion/collapsible'
import { EASE_OUT, SPRING_LAYOUT } from '@/lib/beui/ease'
import { api } from '@/lib/face-api'
import type { PersonVideo } from '@/lib/person-library-model'
import type { PersonFileAction } from '@/lib/person-file-model'
import {
  WorkbenchButton as Button,
  WorkbenchCount as Count,
  WorkbenchDialogContent as DialogContent,
} from './ui'
import { useWorkbenchMotion } from './motion'

export type PlayerContextKind = 'owned' | 'queued' | 'recommendation'

const SPEEDS = [1, 1.25, 1.5, 2] as const

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

const clockText = (seconds?: number) => {
  if (seconds === undefined || !Number.isFinite(seconds) || seconds < 0) return '00:00'
  const total = Math.floor(seconds)
  const mins = Math.floor(total / 60)
  const secs = total % 60
  return `${String(mins).padStart(2, '0')}:${String(secs).padStart(2, '0')}`
}

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
  onManageFile,
  onReturnQueued,
  onAcceptRecommendation,
  onRejectRecommendation,
  onLaterRecommendation,
  notify,
}: PersonVideoDialogProps) {
  const { instant } = useWorkbenchMotion()
  const stageRef = useRef<HTMLDivElement>(null)
  const videoRef = useRef<HTMLVideoElement>(null)
  const hudTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const copyTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const idleTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const holdTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const touchHoldTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const singleTapTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const holdingRight = useRef(false)
  const holdingTouch = useRef(false)
  const preHoldSpeed = useRef(1)
  const lastTapRef = useRef<{ time: number; x: number }>({ time: 0, x: 0 })
  const pointerDownPos = useRef<{ x: number; y: number } | null>(null)

  const [failed, setFailed] = useState(false)
  const [paused, setPaused] = useState(false)
  const [muted, setMuted] = useState(false)
  const [volume, setVolume] = useState(1)
  const [speed, setSpeed] = useState<number>(1)
  const [holdingBoost, setHoldingBoost] = useState(false)
  const [currentTime, setCurrentTime] = useState(0)
  const [duration, setDuration] = useState<number>(0)
  const [isFullscreen, setIsFullscreen] = useState(false)
  const [controlsVisible, setControlsVisible] = useState(true)
  const [isTouchDevice, setIsTouchDevice] = useState(false)
  const [showEpisodes, setShowEpisodes] = useState(false)
  const [showShortcuts, setShowShortcuts] = useState(false)
  const [speedMenuOpen, setSpeedMenuOpen] = useState(false)
  const [extMenuOpen, setExtMenuOpen] = useState(false)
  const [copied, setCopied] = useState(false)
  const [hud, setHud] = useState<{ key: number; label: string } | null>(null)

  const rawVideoSrc = video ? `/api/video?p=${encodeURIComponent(video.path || video.vid)}` : ''
  const streamSrc = video ? normalizeVideoApiPath(rawVideoSrc, video.name) : ''
  const extLinks = video ? buildExternalPlayerLinks(rawVideoSrc, video.name) : null

  const index = video ? playlist.findIndex(item => item.vid === video.vid) : -1
  const hasPrev = index > 0
  const hasNext = index >= 0 && index < playlist.length - 1
  const checked = video ? selectedIds.includes(video.vid) : false
  const totalDuration = duration > 0 ? duration : video?.duration || 0
  const progressRatio = totalDuration > 0 ? Math.min(1, Math.max(0, currentTime / totalDuration)) : 0

  const flashHud = useCallback((label: string) => {
    if (hudTimer.current) clearTimeout(hudTimer.current)
    setHud({ key: Date.now(), label })
    hudTimer.current = setTimeout(() => setHud(null), 850)
  }, [])

  const revealControls = useCallback(() => {
    setControlsVisible(true)
    if (idleTimer.current) clearTimeout(idleTimer.current)
    idleTimer.current = setTimeout(() => {
      if (
        videoRef.current &&
        !videoRef.current.paused &&
        !speedMenuOpen &&
        !extMenuOpen &&
        !showShortcuts
      ) {
        setControlsVisible(false)
      }
    }, 2600)
  }, [speedMenuOpen, extMenuOpen, showShortcuts])

  useEffect(() => {
    return () => {
      if (hudTimer.current) clearTimeout(hudTimer.current)
      if (copyTimer.current) clearTimeout(copyTimer.current)
      if (idleTimer.current) clearTimeout(idleTimer.current)
      if (holdTimer.current) clearTimeout(holdTimer.current)
      if (touchHoldTimer.current) clearTimeout(touchHoldTimer.current)
      if (singleTapTimer.current) clearTimeout(singleTapTimer.current)
    }
  }, [])

  useEffect(() => {
    const onFsChange = () => setIsFullscreen(!!document.fullscreenElement)
    const el = videoRef.current
    const onWebkitBegin = () => setIsFullscreen(true)
    const onWebkitEnd = () => setIsFullscreen(false)
    document.addEventListener('fullscreenchange', onFsChange)
    el?.addEventListener('webkitbeginfullscreen', onWebkitBegin)
    el?.addEventListener('webkitendfullscreen', onWebkitEnd)
    return () => {
      document.removeEventListener('fullscreenchange', onFsChange)
      el?.removeEventListener('webkitbeginfullscreen', onWebkitBegin)
      el?.removeEventListener('webkitendfullscreen', onWebkitEnd)
    }
  }, [video?.vid])

  useEffect(() => {
    setFailed(false)
    setPaused(false)
    setCurrentTime(0)
    setDuration(video?.duration || 0)
    setCopied(false)
    setSpeedMenuOpen(false)
    setExtMenuOpen(false)
    revealControls()
  }, [video?.vid, video?.duration, revealControls])

  const syncMediaState = useCallback(() => {
    const el = videoRef.current
    if (!el) return
    el.playbackRate = speed
    el.muted = muted
    el.volume = volume
  }, [speed, muted, volume])

  const togglePlay = useCallback(() => {
    const el = videoRef.current
    if (!el) return
    const revealBtn = stageRef.current?.querySelector<HTMLButtonElement>(
      '.privacy-video:not([data-revealed="true"]) .privacy-reveal'
    )
    if (revealBtn) {
      revealBtn.click()
      requestAnimationFrame(() => {
        void videoRef.current?.play().catch(() => {})
      })
      return
    }
    revealControls()
    if (el.paused) {
      void el.play().catch(() => {})
      flashHud('播放')
    } else {
      el.pause()
      setControlsVisible(true)
      flashHud('暂停')
    }
  }, [flashHud, revealControls])

  const seekBy = useCallback(
    (delta: number) => {
      const el = videoRef.current
      if (!el) return
      revealControls()
      const max = Number.isFinite(el.duration) ? el.duration : Infinity
      el.currentTime = Math.max(0, Math.min(max, el.currentTime + delta))
      setCurrentTime(el.currentTime)
      flashHud(delta > 0 ? `快进 +${delta}s` : `快退 ${delta}s`)
    },
    [flashHud, revealControls]
  )

  const adjustVolume = useCallback(
    (delta: number) => {
      const el = videoRef.current
      if (!el) return
      revealControls()
      const next = Math.round(Math.max(0, Math.min(1, (el.muted ? 0 : el.volume) + delta)) * 100) / 100
      el.volume = next
      el.muted = next === 0
      setVolume(next)
      setMuted(next === 0)
      flashHud(next === 0 ? '静音' : `音量 ${Math.round(next * 100)}%`)
    },
    [flashHud, revealControls]
  )

  const selectSpeed = useCallback(
    (next: number) => {
      setSpeed(next)
      setSpeedMenuOpen(false)
      if (videoRef.current) videoRef.current.playbackRate = next
      flashHud(`${next}x 倍速`)
    },
    [flashHud]
  )

  const toggleMute = useCallback(() => {
    const next = !muted
    setMuted(next)
    if (videoRef.current) videoRef.current.muted = next
    flashHud(next ? '静音' : `音量 ${Math.round(volume * 100)}%`)
  }, [muted, volume, flashHud])

  const toggleFullscreen = useCallback(() => {
    const container = stageRef.current
    const el = videoRef.current as (HTMLVideoElement & {
      webkitEnterFullscreen?: () => void
    }) | null
    if (document.fullscreenElement) {
      void document.exitFullscreen().catch(() => {})
      return
    }
    if (container && typeof container.requestFullscreen === 'function') {
      container.requestFullscreen().catch(() => {
        el?.webkitEnterFullscreen?.()
      })
    } else if (el?.webkitEnterFullscreen) {
      el.webkitEnterFullscreen()
    }
  }, [])

  const endTouchBoost = useCallback(() => {
    if (touchHoldTimer.current) {
      clearTimeout(touchHoldTimer.current)
      touchHoldTimer.current = null
    }
    if (holdingTouch.current) {
      holdingTouch.current = false
      setHoldingBoost(false)
      if (videoRef.current) videoRef.current.playbackRate = preHoldSpeed.current
      return true
    }
    return false
  }, [])

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
      if (target && ['INPUT', 'TEXTAREA', 'SELECT'].includes(target.tagName)) return

      if (e.key === 'ArrowRight') {
        e.preventDefault()
        if (e.repeat) return
        preHoldSpeed.current = speed
        holdTimer.current = setTimeout(() => {
          holdingRight.current = true
          setHoldingBoost(true)
          if (videoRef.current) {
            if (videoRef.current.paused) void videoRef.current.play().catch(() => {})
            videoRef.current.playbackRate = 2.0
          }
        }, 220)
        return
      }

      switch (e.key) {
        case ' ':
        case 'k':
        case 'K':
          e.preventDefault()
          togglePlay()
          break
        case 'ArrowLeft':
          e.preventDefault()
          seekBy(-5)
          break
        case 'ArrowUp':
          e.preventDefault()
          adjustVolume(0.1)
          break
        case 'ArrowDown':
          e.preventDefault()
          adjustVolume(-0.1)
          break
        case 'm':
        case 'M':
          e.preventDefault()
          toggleMute()
          break
        case 'f':
        case 'F':
          e.preventDefault()
          toggleFullscreen()
          break
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
          setControlsVisible(true)
          break
      }
    }

    const onKeyUp = (e: KeyboardEvent) => {
      if (e.key === 'ArrowRight') {
        if (holdTimer.current) {
          clearTimeout(holdTimer.current)
          holdTimer.current = null
        }
        if (holdingRight.current) {
          holdingRight.current = false
          setHoldingBoost(false)
          if (videoRef.current) videoRef.current.playbackRate = preHoldSpeed.current
        } else {
          seekBy(5)
        }
      }
    }

    window.addEventListener('keydown', onKeyDown)
    window.addEventListener('keyup', onKeyUp)
    return () => {
      window.removeEventListener('keydown', onKeyDown)
      window.removeEventListener('keyup', onKeyUp)
    }
  }, [
    video,
    speed,
    context,
    hasPrev,
    hasNext,
    togglePlay,
    seekBy,
    adjustVolume,
    toggleMute,
    toggleFullscreen,
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

  return (
    <Dialog open={!!video} onOpenChange={open => !open && onClose()}>
      <DialogContent
        className="pw-modal pw-theater-modal"
        data-theme={theme}
        showCloseButton={false}
      >
        {video && (
          <>
            <DialogTitle className="sr-only">{video.name}</DialogTitle>
            <DialogDescription className="sr-only">
              沉浸式视频播放器，支持空格播放暂停、左右快进快退、长按右方向键二倍速、方括号切换上下集。
            </DialogDescription>

            <div
              ref={stageRef}
              className="pw-theater-stage"
              data-controls={controlsVisible || paused || holdingBoost ? 'visible' : 'hidden'}
              data-touch={isTouchDevice || undefined}
              onPointerMove={e => {
                if (e.pointerType === 'mouse') revealControls()
                else if (pointerDownPos.current) {
                  const dx = Math.abs(e.clientX - pointerDownPos.current.x)
                  const dy = Math.abs(e.clientY - pointerDownPos.current.y)
                  if (dx > 12 || dy > 12) {
                    if (touchHoldTimer.current) {
                      clearTimeout(touchHoldTimer.current)
                      touchHoldTimer.current = null
                    }
                  }
                }
              }}
              onPointerDown={e => {
                const isTouch = e.pointerType !== 'mouse'
                setIsTouchDevice(isTouch)
                if (
                  (e.target as HTMLElement).closest(
                    '.pw-theater-top, .pw-theater-bottom, .pw-theater-center, .pw-theater-shortcuts-sheet, .privacy-reveal'
                  )
                ) {
                  return
                }
                pointerDownPos.current = { x: e.clientX, y: e.clientY }
                if (isTouch) {
                  preHoldSpeed.current = speed
                  if (touchHoldTimer.current) clearTimeout(touchHoldTimer.current)
                  touchHoldTimer.current = setTimeout(() => {
                    holdingTouch.current = true
                    setHoldingBoost(true)
                    if (videoRef.current) {
                      if (videoRef.current.paused) void videoRef.current.play().catch(() => {})
                      videoRef.current.playbackRate = 2.0
                    }
                  }, 300)
                }
              }}
              onPointerCancel={() => {
                pointerDownPos.current = null
                endTouchBoost()
              }}
              onPointerUp={e => {
                if (
                  (e.target as HTMLElement).closest(
                    '.pw-theater-top, .pw-theater-bottom, .pw-theater-center, .pw-theater-shortcuts-sheet, .privacy-reveal'
                  )
                ) {
                  pointerDownPos.current = null
                  endTouchBoost()
                  return
                }
                const wasHolding = endTouchBoost()
                const down = pointerDownPos.current
                pointerDownPos.current = null
                if (wasHolding) return

                if (e.pointerType !== 'mouse') {
                  if (down && (Math.abs(e.clientX - down.x) > 14 || Math.abs(e.clientY - down.y) > 14)) {
                    return
                  }
                  const now = Date.now()
                  const rect = stageRef.current?.getBoundingClientRect()
                  const relX = rect ? (e.clientX - rect.left) / Math.max(1, rect.width) : 0.5
                  if (now - lastTapRef.current.time < 280) {
                    if (singleTapTimer.current) {
                      clearTimeout(singleTapTimer.current)
                      singleTapTimer.current = null
                    }
                    lastTapRef.current = { time: 0, x: 0 }
                    if (relX < 0.35) seekBy(-5)
                    else if (relX > 0.65) seekBy(5)
                    else togglePlay()
                  } else {
                    lastTapRef.current = { time: now, x: relX }
                    if (singleTapTimer.current) clearTimeout(singleTapTimer.current)
                    singleTapTimer.current = setTimeout(() => {
                      setControlsVisible(v => {
                        const next = !v
                        if (next) revealControls()
                        return next
                      })
                    }, 220)
                  }
                }
              }}
              onClick={e => {
                if (
                  (e.target as HTMLElement).closest(
                    '.pw-theater-top, .pw-theater-bottom, .pw-theater-center, .pw-theater-shortcuts-sheet, .privacy-reveal'
                  )
                ) {
                  return
                }
                if (!isTouchDevice) {
                  togglePlay()
                }
              }}
              onDoubleClick={e => {
                if (
                  (e.target as HTMLElement).closest(
                    '.pw-theater-top, .pw-theater-bottom, .pw-theater-center, .pw-theater-shortcuts-sheet, .privacy-reveal'
                  )
                ) {
                  return
                }
                if (!isTouchDevice) {
                  toggleFullscreen()
                }
              }}
            >
              <PrivateVideo
                ref={videoRef}
                key={video.path || video.vid}
                src={streamSrc}
                poster={video.thumb || undefined}
                controls={false}
                autoPlay
                playsInline
                onLoadedMetadata={e => {
                  setDuration(e.currentTarget.duration || 0)
                  syncMediaState()
                }}
                onTimeUpdate={e => setCurrentTime(e.currentTarget.currentTime)}
                onPlay={() => setPaused(false)}
                onPause={() => {
                  setPaused(true)
                  setControlsVisible(true)
                }}
                onVolumeChange={e => {
                  setMuted(e.currentTarget.muted)
                  setVolume(e.currentTarget.volume)
                }}
                onRateChange={e => {
                  if (!holdingRight.current && !holdingTouch.current) setSpeed(e.currentTarget.playbackRate)
                }}
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

              {/* Top Gradient Overlay (Netflix / Bilibili header) */}
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
                  <button
                    type="button"
                    className="pw-theater-icon-btn"
                    aria-label="快捷键说明"
                    aria-pressed={showShortcuts}
                    title="快捷键 (?)"
                    onClick={() => setShowShortcuts(v => !v)}
                  >
                    <Keyboard size={15} />
                  </button>
                  <button
                    type="button"
                    className="pw-theater-icon-btn"
                    aria-label="关闭视频预览"
                    title="关闭 (Esc)"
                    onClick={onClose}
                  >
                    <X size={16} />
                  </button>
                </div>
              </div>

              {/* Bilibili-style Long-press 2x Speed Pill / Action HUD */}
              <AnimatePresence initial={false}>
                {holdingBoost ? (
                  <motion.div
                    key="boost"
                    role="status"
                    className="pw-theater-hud pw-theater-hud-boost"
                    initial={instant ? false : { opacity: 0, transform: 'translate(-50%, -6px) scale(0.95)' }}
                    animate={{ opacity: 1, transform: 'translate(-50%, 0px) scale(1)' }}
                    exit={instant ? undefined : { opacity: 0, transform: 'translate(-50%, -6px) scale(0.95)' }}
                    transition={{ duration: instant ? 0 : 0.15, ease: EASE_OUT }}
                  >
                    <FastForward size={13} />
                    <span>2.0x 倍速快进中</span>
                  </motion.div>
                ) : hud ? (
                  <motion.div
                    key={hud.key}
                    role="status"
                    aria-live="polite"
                    className="pw-theater-hud"
                    initial={instant ? false : { opacity: 0, transform: 'translate(-50%, -6px) scale(0.95)' }}
                    animate={{ opacity: 1, transform: 'translate(-50%, 0px) scale(1)' }}
                    exit={instant ? undefined : { opacity: 0, transform: 'translate(-50%, -6px) scale(0.95)' }}
                    transition={{ duration: instant ? 0 : 0.15, ease: EASE_OUT }}
                  >
                    {hud.label}
                  </motion.div>
                ) : null}
              </AnimatePresence>

              {/* Shortcuts Glass Popover */}
              <AnimatePresence initial={false}>
                {showShortcuts && (
                  <motion.div
                    role="dialog"
                    aria-label="键盘快捷键"
                    className="pw-theater-shortcuts-sheet"
                    initial={instant ? false : { opacity: 0, transform: 'scale(0.97)' }}
                    animate={{ opacity: 1, transform: 'scale(1)' }}
                    exit={instant ? undefined : { opacity: 0, transform: 'scale(0.97)' }}
                    transition={{ duration: instant ? 0 : 0.16, ease: EASE_OUT }}
                  >
                    <div className="pw-theater-shortcuts-head">
                      <strong>键盘快捷键</strong>
                      <button type="button" aria-label="关闭快捷键说明" onClick={() => setShowShortcuts(false)}>
                        <X size={14} />
                      </button>
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
                  </motion.div>
                )}
              </AnimatePresence>

              {/* Center Touch & Paused Controls (-5s / Play-Pause / +5s) */}
              <div
                className="pw-theater-center"
                data-visible={paused || (isTouchDevice && controlsVisible) || undefined}
              >
                <button
                  type="button"
                  className="pw-theater-center-seek"
                  aria-label="快退 5 秒"
                  title="快退 5 秒 (←)"
                  onClick={() => seekBy(-5)}
                >
                  <RotateCcw size={16} />
                  <span>5s</span>
                </button>
                <button
                  type="button"
                  className="pw-theater-center-play"
                  aria-label={paused ? '播放视频' : '暂停视频'}
                  title={paused ? '播放 (Space)' : '暂停 (Space)'}
                  onClick={togglePlay}
                >
                  <ActionSwapIcon value={paused ? 'play' : 'pause'} animation="roll">
                    {paused ? <Play size={22} fill="currentColor" /> : <Pause size={22} fill="currentColor" />}
                  </ActionSwapIcon>
                </button>
                <button
                  type="button"
                  className="pw-theater-center-seek"
                  aria-label="快进 5 秒"
                  title="快进 5 秒 (→)"
                  onClick={() => seekBy(5)}
                >
                  <RotateCw size={16} />
                  <span>5s</span>
                </button>
              </div>

              {/* Bottom Gradient Player Bar */}
              <div className="pw-theater-bottom">
                {/* Interactive Scrubber Timeline */}
                <div className="pw-theater-timeline">
                  <input
                    type="range"
                    aria-label="播放进度"
                    min={0}
                    max={totalDuration || 100}
                    step={0.05}
                    value={currentTime}
                    onChange={e => {
                      const next = Number(e.target.value)
                      setCurrentTime(next)
                      if (videoRef.current) videoRef.current.currentTime = next
                      revealControls()
                    }}
                  />
                  <div className="pw-theater-timeline-track">
                    <div
                      className="pw-theater-timeline-fill"
                      style={{ transform: `scaleX(${progressRatio})` }}
                    />
                  </div>
                  <span
                    className="pw-theater-timeline-thumb"
                    style={{ left: `${progressRatio * 100}%` }}
                  />
                </div>

                {/* Controls Row */}
                <div className="pw-theater-controls">
                  <div className="pw-theater-controls-left">
                    <button
                      type="button"
                      className="pw-theater-ctrl-btn"
                      aria-label={paused ? '播放' : '暂停'}
                      title={paused ? '播放 (Space)' : '暂停 (Space)'}
                      onClick={togglePlay}
                    >
                      <ActionSwapIcon value={paused ? 'play' : 'pause'} animation="roll">
                        {paused ? <Play size={17} fill="currentColor" /> : <Pause size={17} fill="currentColor" />}
                      </ActionSwapIcon>
                    </button>

                    {playlist.length > 1 && (
                      <>
                        <button
                          type="button"
                          className="pw-theater-ctrl-btn"
                          aria-label="上一个视频"
                          title="上一个视频 ([)"
                          disabled={!hasPrev}
                          onClick={goPrev}
                        >
                          <SkipBack size={15} />
                        </button>
                        <button
                          type="button"
                          className="pw-theater-ctrl-btn"
                          aria-label="下一个视频"
                          title="下一个视频 (])"
                          disabled={!hasNext}
                          onClick={goNext}
                        >
                          <SkipForward size={15} />
                        </button>
                      </>
                    )}

                    <div className="pw-theater-volume">
                      <button
                        type="button"
                        className="pw-theater-ctrl-btn"
                        aria-label={muted ? '取消静音' : '静音'}
                        title="静音 (M)"
                        onClick={toggleMute}
                      >
                        <ActionSwapIcon value={muted ? 'muted' : 'unmuted'} animation="roll">
                          {muted || volume === 0 ? <VolumeX size={16} /> : <Volume2 size={16} />}
                        </ActionSwapIcon>
                      </button>
                      <input
                        type="range"
                        aria-label="音量"
                        className="pw-theater-volume-slider"
                        min={0}
                        max={1}
                        step={0.05}
                        value={muted ? 0 : volume}
                        onChange={e => {
                          const next = Number(e.target.value)
                          setVolume(next)
                          setMuted(next === 0)
                          if (videoRef.current) {
                            videoRef.current.volume = next
                            videoRef.current.muted = next === 0
                          }
                        }}
                      />
                    </div>

                    <span className="pw-theater-time">
                      {clockText(currentTime)} <em>/</em> {clockText(totalDuration)}
                    </span>
                  </div>

                  <div className="pw-theater-controls-right">
                    {/* External Player Menu (Infuse / nPlayer / SenPlayer) */}
                    {extLinks && (
                      <div className="pw-theater-speed-wrap">
                        <button
                          type="button"
                          className="pw-theater-pill-btn"
                          aria-label="外部播放器打开"
                          title="在 Infuse / nPlayer / SenPlayer 中打开"
                          aria-expanded={extMenuOpen}
                          onClick={() => {
                            setSpeedMenuOpen(false)
                            setExtMenuOpen(v => !v)
                          }}
                        >
                          <ExternalLink size={13} />
                          <span>外部播放</span>
                        </button>
                        <AnimatePresence initial={false}>
                          {extMenuOpen && (
                            <motion.div
                              role="menu"
                              aria-label="选择外部播放器"
                              className="pw-theater-speed-menu pw-theater-ext-menu"
                              initial={instant ? false : { opacity: 0, transform: 'translateY(4px)' }}
                              animate={{ opacity: 1, transform: 'translateY(0px)' }}
                              exit={instant ? undefined : { opacity: 0, transform: 'translateY(4px)' }}
                              transition={{ duration: instant ? 0 : 0.14, ease: EASE_OUT }}
                            >
                              <a
                                href={extLinks.infuse}
                                role="menuitem"
                                onClick={() => setExtMenuOpen(false)}
                              >
                                Infuse
                              </a>
                              <a
                                href={extLinks.nplayer}
                                role="menuitem"
                                onClick={() => setExtMenuOpen(false)}
                              >
                                nPlayer
                              </a>
                              <a
                                href={extLinks.senplayer}
                                role="menuitem"
                                onClick={() => setExtMenuOpen(false)}
                              >
                                SenPlayer
                              </a>
                              <button
                                type="button"
                                role="menuitem"
                                onClick={() => {
                                  setExtMenuOpen(false)
                                  void navigator.clipboard
                                    ?.writeText(extLinks.streamUrl)
                                    .then(() => flashHud('流地址已复制'))
                                    .catch(() => {})
                                }}
                              >
                                复制流地址
                              </button>
                            </motion.div>
                          )}
                        </AnimatePresence>
                      </div>
                    )}

                    {/* Speed Pill + Popover Menu (Bilibili style) */}
                    <div className="pw-theater-speed-wrap">
                      <button
                        type="button"
                        className="pw-theater-pill-btn"
                        aria-label={`播放倍速 ${speed}x`}
                        aria-expanded={speedMenuOpen}
                        onClick={() => {
                          setExtMenuOpen(false)
                          setSpeedMenuOpen(v => !v)
                        }}
                      >
                        {speed === 1 ? '倍速' : `${speed}x`}
                      </button>
                      <AnimatePresence initial={false}>
                        {speedMenuOpen && (
                          <motion.div
                            role="menu"
                            aria-label="选择倍速"
                            className="pw-theater-speed-menu"
                            initial={instant ? false : { opacity: 0, transform: 'translateY(4px)' }}
                            animate={{ opacity: 1, transform: 'translateY(0px)' }}
                            exit={instant ? undefined : { opacity: 0, transform: 'translateY(4px)' }}
                            transition={{ duration: instant ? 0 : 0.14, ease: EASE_OUT }}
                          >
                            {[...SPEEDS].reverse().map(item => (
                              <button
                                key={item}
                                type="button"
                                role="menuitemradio"
                                aria-checked={speed === item}
                                data-active={speed === item || undefined}
                                onClick={() => selectSpeed(item)}
                              >
                                {item.toFixed(item === 1.25 ? 2 : 1)}x
                              </button>
                            ))}
                          </motion.div>
                        )}
                      </AnimatePresence>
                    </div>

                    {/* Episodes Toggle */}
                    {playlist.length > 1 && (
                      <button
                        type="button"
                        className="pw-theater-pill-btn"
                        aria-expanded={showEpisodes}
                        onClick={() => setShowEpisodes(v => !v)}
                      >
                        <LayoutList size={13} />
                        <span>选集</span>
                      </button>
                    )}

                    <button
                      type="button"
                      className="pw-theater-ctrl-btn"
                      aria-label={isFullscreen ? '退出全屏' : '全屏播放'}
                      title="全屏 (F / 双击画面)"
                      onClick={toggleFullscreen}
                    >
                      {isFullscreen ? <Minimize2 size={15} /> : <Maximize2 size={15} />}
                    </button>
                  </div>
                </div>
              </div>
            </div>

            {failed && (
              <div role="alert" className="pw-error pw-theater-error">
                <p>暂时无法播放，请核对文件路径、网络或浏览器编码支持，或使用外部播放器打开。</p>
                <ExternalPlayerBar src={rawVideoSrc} name={video.name} />
              </div>
            )}

            {/* Single Clean Footer Bar (Ponytail: no duplicate file CRUD buttons) */}
            <div className="pw-theater-footer">
              <button
                type="button"
                className="pw-theater-path-btn"
                title="点击复制完整路径 (快捷键 C)"
                onClick={() => void copyPath()}
              >
                <FolderOpen size={13} />
                <code>{video.path || video.vid}</code>
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
                      <Check size={13} />
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

            {/* Collapsible Episode Strip (only shown when user opens "选集") */}
            {railItems.length > 1 && (
              <Collapsible open={showEpisodes} onOpenChange={setShowEpisodes} className="pw-theater-episodes-collapse">
                <CollapsibleContent className="pw-theater-episodes-body">
                  <div className="pw-player-rail" role="listbox" aria-label="同目录选集">
                    {railItems.map(item => {
                      const active = item.vid === video.vid
                      const itemChecked = selectedIds.includes(item.vid)
                      return (
                        <button
                          key={item.vid}
                          type="button"
                          role="option"
                          aria-selected={active}
                          className="pw-player-rail-item"
                          data-active={active || undefined}
                          onClick={() => onSelectVideo(item)}
                          title={item.name}
                        >
                          {active && (
                            <motion.span
                              layoutId={instant ? undefined : 'pw-player-rail-active'}
                              transition={SPRING_LAYOUT}
                              className="pw-player-rail-ring"
                              aria-hidden="true"
                            />
                          )}
                          <VideoCover video={item} className="pw-player-rail-cover" />
                          {itemChecked && (
                            <span className="pw-player-rail-check" aria-label="已勾选">
                              <Check size={10} />
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
