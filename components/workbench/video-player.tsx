'use client'

import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react'
import { AnimatePresence, motion, useReducedMotion } from 'motion/react'
import * as DropdownMenu from '@radix-ui/react-dropdown-menu'
import {
  Check,
  Copy,
  ExternalLink,
  EyeOff,
  FastForward,
  Maximize,
  Maximize2,
  Minimize2,
  MoreHorizontal,
  Pause,
  Play,
  RotateCcw,
  RotateCw,
  ShieldAlert,
  SkipBack,
  SkipForward,
  Volume2,
  VolumeX,
} from 'lucide-react'
import { PrivateVideo, useMediaPrivacy } from '@/components/private-media'
import { ActionSwapIcon } from '@/components/beui/motion/action-swap'
import { EASE_OUT } from '@/lib/beui/ease'

const SPEEDS = [0.5, 0.75, 1, 1.25, 1.5, 2] as const

export const strictEncodeURIComponent = (value: string) =>
  encodeURIComponent(value).replace(
    /[!'()*]/g,
    ch => `%${ch.charCodeAt(0).toString(16).toUpperCase()}`
  )

export function normalizeVideoApiPath(src: string, name?: string): string {
  try {
    const parsed = new URL(src, 'http://localhost')
    const rawP = parsed.searchParams.get('p')
    if (!rawP) return src
    const fileName = (name || rawP.split(/[\\/]/).pop() || 'video.mp4').trim() || 'video.mp4'
    return `/api/video/${strictEncodeURIComponent(fileName)}?p=${strictEncodeURIComponent(rawP)}`
  } catch {
    return src
  }
}

export interface ExternalPlayerLinks {
  streamUrl: string
  infuse: string
  nplayer: string
  senplayer: string
}

export function buildExternalPlayerLinks(src: string, name?: string, origin = 'http://127.0.0.1:8919'): ExternalPlayerLinks {
  const relative = normalizeVideoApiPath(src, name)
  const streamUrl = relative.startsWith('http') ? relative : `${origin}${relative}`
  const encodedStream = strictEncodeURIComponent(streamUrl)
  return {
    streamUrl,
    infuse: `infuse://x-callback-url/play?url=${encodedStream}`,
    nplayer: /^https?:\/\//i.test(streamUrl) ? `nplayer-${streamUrl}` : `nplayer://${encodedStream}`,
    senplayer: `senplayer://x-callback-url/play?url=${encodedStream}`,
  }
}

export function ExternalPlayerBar({
  src,
  name,
  className = '',
}: {
  src: string
  name?: string
  className?: string
}) {
  const [copied, setCopied] = useState(false)
  const [clientOrigin, setClientOrigin] = useState<string | undefined>(undefined)
  useEffect(() => {
    if (window.location?.origin) setClientOrigin(window.location.origin)
  }, [])
  const links = buildExternalPlayerLinks(src, name, clientOrigin)
  return (
    <div className={`wb-ext-player-bar ${className}`} role="group" aria-label="外部播放器打开">
      <span className="wb-ext-player-label">外部播放器：</span>
      <a
        href={links.infuse}
        className="wb-ext-player-chip"
        aria-label="在 Infuse 中打开"
        title="使用 Infuse 播放此视频流"
      >
        <ExternalLink size={12} aria-hidden="true" />
        <span>Infuse</span>
      </a>
      <a
        href={links.nplayer}
        className="wb-ext-player-chip"
        aria-label="在 nPlayer 中打开"
        title="使用 nPlayer 播放此视频流"
      >
        <ExternalLink size={12} aria-hidden="true" />
        <span>nPlayer</span>
      </a>
      <a
        href={links.senplayer}
        className="wb-ext-player-chip"
        aria-label="在 SenPlayer 中打开"
        title="使用 SenPlayer 播放此视频流"
      >
        <ExternalLink size={12} aria-hidden="true" />
        <span>SenPlayer</span>
      </a>
      <button
        type="button"
        className="wb-ext-player-chip"
        aria-label="复制视频流地址"
        title="复制 HTTP 流地址到剪贴板"
        onClick={() => {
          void navigator.clipboard
            ?.writeText(links.streamUrl)
            .then(() => {
              setCopied(true)
              setTimeout(() => setCopied(false), 1600)
            })
            .catch(() => {})
        }}
      >
        {copied ? <Check size={12} aria-hidden="true" /> : <Copy size={12} aria-hidden="true" />}
        <span>{copied ? '已复制流地址' : '复制流地址'}</span>
      </button>
    </div>
  )
}

export const formatVideoClock = (seconds?: number) => {
  if (seconds === undefined || !Number.isFinite(seconds) || seconds < 0) return '00:00'
  const total = Math.floor(seconds)
  const hrs = Math.floor(total / 3600)
  const mins = Math.floor((total % 3600) / 60)
  const secs = total % 60
  if (hrs > 0) {
    return `${String(hrs).padStart(2, '0')}:${String(mins).padStart(2, '0')}:${String(secs).padStart(2, '0')}`
  }
  return `${String(mins).padStart(2, '0')}:${String(secs).padStart(2, '0')}`
}

export interface WorkbenchVideoPlayerProps {
  src: string
  name: string
  poster?: string
  autoPlay?: boolean
  preload?: 'none' | 'metadata' | 'auto'
  initialTime?: number
  compact?: boolean
  fill?: boolean
  showVolumeSlider?: boolean
  enableGlobalHotkeys?: boolean
  className?: string
  videoRef?: React.RefObject<HTMLVideoElement | null>
  onTimeRemember?: (time: number) => void
  onError?: (event: React.SyntheticEvent<HTMLVideoElement>) => void
  onExpand?: (currentTime: number, triggerEl: HTMLElement | null) => void
  onPrev?: () => void
  onNext?: () => void
  hasPrev?: boolean
  hasNext?: boolean
  extraControls?: React.ReactNode
  topOverlay?: React.ReactNode
  hudSignal?: { key: number; label: string } | null
  onRevealChange?: (revealed: boolean) => void
}

export function WorkbenchVideoPlayer({
  src,
  name,
  poster,
  autoPlay = false,
  preload = 'metadata',
  initialTime = 0,
  compact = false,
  fill = false,
  showVolumeSlider = false,
  enableGlobalHotkeys = false,
  className = '',
  videoRef: externalVideoRef,
  onTimeRemember,
  onError,
  onExpand,
  onPrev,
  onNext,
  hasPrev = false,
  hasNext = false,
  extraControls,
  topOverlay,
  hudSignal,
  onRevealChange,
}: WorkbenchVideoPlayerProps) {
  const reduce = useReducedMotion()
  const stageRef = useRef<HTMLDivElement>(null)
  const internalVideoRef = useRef<HTMLVideoElement | null>(null)
  const shieldCtaRef = useRef<HTMLButtonElement | null>(null)
  const playBtnRef = useRef<HTMLButtonElement | null>(null)
  const pendingFocusRef = useRef<'play' | 'shield' | null>(null)
  const hudTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const idleTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const keyHoldTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const touchHoldTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const singleTapTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const rightKeyHeldRef = useRef(false)
  const holdingRight = useRef(false)
  const holdingTouch = useRef(false)
  const holdPointerIdRef = useRef<number | null>(null)
  const preHoldSpeed = useRef(1)
  const pendingSeekRef = useRef(initialTime)
  const lastTapRef = useRef<{ time: number; x: number }>({ time: 0, x: 0 })
  const pointerDownPos = useRef<{ x: number; y: number; type: string } | null>(null)

  const [clientOrigin, setClientOrigin] = useState<string | undefined>(undefined)
  useEffect(() => {
    if (window.location?.origin) setClientOrigin(window.location.origin)
  }, [])

  const streamSrc = normalizeVideoApiPath(src, name)
  const extLinks = buildExternalPlayerLinks(src, name, clientOrigin)
  const privacy = useMediaPrivacy(streamSrc)
  const concealed = privacy.blocked

  const [paused, setPaused] = useState(!autoPlay || concealed)
  const [muted, setMuted] = useState(false)
  const [volume, setVolume] = useState(1)
  const [speed, setSpeed] = useState<number>(1)
  const [holdingBoost, setHoldingBoost] = useState(false)
  const [currentTime, setCurrentTime] = useState(0)
  const [duration, setDuration] = useState(0)
  const [isFullscreen, setIsFullscreen] = useState(false)
  const [controlsVisible, setControlsVisible] = useState(true)
  const [speedMenuOpen, setSpeedMenuOpen] = useState(false)
  const [extMenuOpen, setExtMenuOpen] = useState(false)
  const [overflowMenuOpen, setOverflowMenuOpen] = useState(false)
  const [isTouchDevice, setIsTouchDevice] = useState(false)
  const [hud, setHud] = useState<{ key: number; label: string } | null>(null)

  const initialTimeRef = useRef(initialTime)
  initialTimeRef.current = initialTime
  const autoPlayRef = useRef(autoPlay)
  autoPlayRef.current = autoPlay
  const concealedRef = useRef(concealed)
  concealedRef.current = concealed
  const menuOpenRef = useRef(false)
  menuOpenRef.current = speedMenuOpen || extMenuOpen || overflowMenuOpen

  const assignVideoRef = useCallback(
    (node: HTMLVideoElement | null) => {
      internalVideoRef.current = node
      if (externalVideoRef) externalVideoRef.current = node
    },
    [externalVideoRef]
  )

  useEffect(() => {
    onRevealChange?.(privacy.revealed)
  }, [privacy.revealed, onRevealChange])

  const flashHud = useCallback((label: string) => {
    if (hudTimer.current) clearTimeout(hudTimer.current)
    setHud({ key: Date.now(), label })
    hudTimer.current = setTimeout(() => setHud(null), 850)
  }, [])

  useEffect(() => {
    if (!hudSignal) return
    if (hudTimer.current) clearTimeout(hudTimer.current)
    setHud(hudSignal)
    hudTimer.current = setTimeout(() => setHud(null), 850)
  }, [hudSignal])

  const scheduleHideControls = useCallback(() => {
    if (idleTimer.current) clearTimeout(idleTimer.current)
    idleTimer.current = setTimeout(() => {
      if (internalVideoRef.current && !internalVideoRef.current.paused && !menuOpenRef.current) {
        setControlsVisible(false)
      }
    }, 2600)
  }, [])

  const revealControls = useCallback(() => {
    setControlsVisible(true)
    scheduleHideControls()
  }, [scheduleHideControls])

  useEffect(() => {
    return () => {
      if (hudTimer.current) clearTimeout(hudTimer.current)
      if (idleTimer.current) clearTimeout(idleTimer.current)
      if (keyHoldTimer.current) clearTimeout(keyHoldTimer.current)
      if (touchHoldTimer.current) clearTimeout(touchHoldTimer.current)
      if (singleTapTimer.current) clearTimeout(singleTapTimer.current)
    }
  }, [])

  useEffect(() => {
    const onFsChange = () => setIsFullscreen(!!document.fullscreenElement)
    const video = internalVideoRef.current
    const onWebkitBegin = () => setIsFullscreen(true)
    const onWebkitEnd = () => setIsFullscreen(false)
    document.addEventListener('fullscreenchange', onFsChange)
    video?.addEventListener('webkitbeginfullscreen', onWebkitBegin)
    video?.addEventListener('webkitendfullscreen', onWebkitEnd)
    return () => {
      document.removeEventListener('fullscreenchange', onFsChange)
      video?.removeEventListener('webkitbeginfullscreen', onWebkitBegin)
      video?.removeEventListener('webkitendfullscreen', onWebkitEnd)
    }
  }, [streamSrc])

  useEffect(() => {
    const el = internalVideoRef.current
    pendingSeekRef.current = initialTimeRef.current
    setPaused(!autoPlayRef.current || concealedRef.current)
    setCurrentTime(el && Number.isFinite(el.currentTime) ? el.currentTime : 0)
    setDuration(el && Number.isFinite(el.duration) ? el.duration : 0)
    setSpeedMenuOpen(false)
    setExtMenuOpen(false)
    setOverflowMenuOpen(false)
    revealControls()
  }, [streamSrc, revealControls])

  useEffect(() => {
    if (concealed) {
      setSpeedMenuOpen(false)
      setExtMenuOpen(false)
      setOverflowMenuOpen(false)
      setHoldingBoost(false)
      rightKeyHeldRef.current = false
      holdPointerIdRef.current = null
      setPaused(true)
    }
  }, [concealed])

  useLayoutEffect(() => {
    const nextTarget = pendingFocusRef.current
    if (!nextTarget) return
    if (nextTarget === 'play' && !concealed) {
      pendingFocusRef.current = null
      ;(playBtnRef.current ?? stageRef.current)?.focus({ preventScroll: true })
    } else if (nextTarget === 'shield' && concealed) {
      pendingFocusRef.current = null
      shieldCtaRef.current?.focus({ preventScroll: true })
    }
  }, [concealed])

  const revealAndPlay = useCallback(() => {
    const shouldMoveFocus =
      document.activeElement === shieldCtaRef.current ||
      !stageRef.current?.contains(document.activeElement)
    if (shouldMoveFocus) pendingFocusRef.current = 'play'
    privacy.reveal()
    onRevealChange?.(true)
    const el = internalVideoRef.current
    if (el) {
      el.setAttribute('data-privacy-revealed', 'true')
      el.muted = muted
      el.volume = volume
      el.playbackRate = speed
      if (Number.isFinite(el.duration) && el.duration > 0) {
        setDuration(el.duration)
      }
      if (pendingSeekRef.current > 0 && Number.isFinite(el.duration) && pendingSeekRef.current < el.duration - 0.15) {
        el.currentTime = pendingSeekRef.current
        setCurrentTime(pendingSeekRef.current)
        pendingSeekRef.current = 0
      }
      void el.play().catch(() => {})
    }
    revealControls()
    flashHud('已解除遮挡并播放')
  }, [privacy, onRevealChange, muted, volume, speed, revealControls, flashHud])

  const concealVideo = useCallback(() => {
    const hadFocusInside = Boolean(stageRef.current?.contains(document.activeElement))
    if (hadFocusInside) pendingFocusRef.current = 'shield'
    const el = internalVideoRef.current
    if (el) {
      el.pause()
      el.removeAttribute('data-privacy-revealed')
      el.muted = true
    }
    if (document.fullscreenElement === stageRef.current) {
      void document.exitFullscreen().catch(() => {})
    }
    privacy.conceal()
    onRevealChange?.(false)
    setSpeedMenuOpen(false)
    setExtMenuOpen(false)
    setOverflowMenuOpen(false)
    flashHud('已重新遮挡')
  }, [privacy, onRevealChange, flashHud])

  const togglePlay = useCallback(() => {
    if (concealed) {
      revealAndPlay()
      return
    }
    const el = internalVideoRef.current
    if (!el) return
    revealControls()
    if (el.paused) {
      void el.play().catch(() => {})
      flashHud('播放')
    } else {
      el.pause()
      setControlsVisible(true)
      flashHud('暂停')
    }
  }, [concealed, revealAndPlay, revealControls, flashHud])

  const seekBy = useCallback(
    (delta: number) => {
      if (concealed) return
      const el = internalVideoRef.current
      if (!el) return
      revealControls()
      const max = Number.isFinite(el.duration) ? el.duration : Infinity
      el.currentTime = Math.max(0, Math.min(max, el.currentTime + delta))
      setCurrentTime(el.currentTime)
      flashHud(delta > 0 ? `快进 +${delta}s` : `快退 ${delta}s`)
    },
    [concealed, flashHud, revealControls]
  )

  const adjustVolume = useCallback(
    (delta: number) => {
      if (concealed) return
      const el = internalVideoRef.current
      if (!el) return
      revealControls()
      const next =
        Math.round(Math.max(0, Math.min(1, (el.muted ? 0 : el.volume) + delta)) * 100) / 100
      el.volume = next
      el.muted = next === 0
      setVolume(next)
      setMuted(next === 0)
      flashHud(next === 0 ? '静音' : `音量 ${Math.round(next * 100)}%`)
    },
    [concealed, flashHud, revealControls]
  )

  const selectSpeed = useCallback(
    (next: number) => {
      setSpeed(next)
      setSpeedMenuOpen(false)
      if (internalVideoRef.current) internalVideoRef.current.playbackRate = next
      flashHud(`${next}x 倍速`)
    },
    [flashHud]
  )

  const toggleMute = useCallback(() => {
    if (concealed) return
    const next = !muted
    setMuted(next)
    if (internalVideoRef.current) internalVideoRef.current.muted = next
    flashHud(next ? '静音' : `音量 ${Math.round(volume * 100)}%`)
  }, [concealed, muted, volume, flashHud])

  const toggleFullscreen = useCallback(() => {
    if (privacy.enabled || concealed) return
    const container = stageRef.current
    const video = internalVideoRef.current as (HTMLVideoElement & {
      webkitEnterFullscreen?: () => void
    }) | null
    if (document.fullscreenElement) {
      void document.exitFullscreen().catch(() => {})
      return
    }
    if (container && typeof container.requestFullscreen === 'function') {
      container.requestFullscreen().catch(() => {
        video?.webkitEnterFullscreen?.()
      })
    } else if (video?.webkitEnterFullscreen) {
      video.webkitEnterFullscreen()
    }
  }, [privacy.enabled, concealed])

  const handleKeyDown = useCallback(
    (e: KeyboardEvent | React.KeyboardEvent) => {
      if (e.defaultPrevented || e.metaKey || e.ctrlKey || e.altKey) return
      const target = e.target as HTMLElement | null
      if (target && (['INPUT', 'TEXTAREA', 'SELECT'].includes(target.tagName) || target.isContentEditable)) return
      const onInteractive = Boolean(
        target &&
          target !== stageRef.current &&
          target.closest('button, a, [role="tab"], [role="menuitem"], [role="menuitemradio"], [role="checkbox"], [role="option"]')
      )

      if ((e.key === ' ' || e.key === 'Enter') && onInteractive) return

      if (concealed) {
        if (e.key === ' ' || e.key === 'k' || e.key === 'K') {
          e.preventDefault()
          revealAndPlay()
        }
        return
      }

      if (e.key === 'ArrowRight') {
        if (target?.closest('[role="tab"], [role="separator"], [role="menuitem"], [role="menuitemradio"]')) return
        e.preventDefault()
        if (e.repeat) return
        rightKeyHeldRef.current = true
        preHoldSpeed.current = speed
        keyHoldTimer.current = setTimeout(() => {
          holdingRight.current = true
          setHoldingBoost(true)
          if (internalVideoRef.current) {
            if (internalVideoRef.current.paused) {
              void internalVideoRef.current.play().catch(() => {})
            }
            internalVideoRef.current.playbackRate = 2.0
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
          if (target?.closest('[role="tab"], [role="separator"], [role="menuitem"], [role="menuitemradio"]')) return
          e.preventDefault()
          seekBy(-5)
          break
        case 'ArrowUp':
          if (target?.closest('[role="menuitem"], [role="menuitemradio"]')) return
          e.preventDefault()
          adjustVolume(0.1)
          break
        case 'ArrowDown':
          if (target?.closest('[role="menuitem"], [role="menuitemradio"]')) return
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
      }
    },
    [concealed, revealAndPlay, speed, togglePlay, seekBy, adjustVolume, toggleMute, toggleFullscreen]
  )

  const handleKeyUp = useCallback(
    (e: KeyboardEvent | React.KeyboardEvent) => {
      if (e.key === 'ArrowRight') {
        if (!rightKeyHeldRef.current) return
        rightKeyHeldRef.current = false
        if (keyHoldTimer.current) {
          clearTimeout(keyHoldTimer.current)
          keyHoldTimer.current = null
        }
        if (holdingRight.current) {
          holdingRight.current = false
          setHoldingBoost(false)
          if (internalVideoRef.current) {
            internalVideoRef.current.playbackRate = preHoldSpeed.current
          }
        } else if (!concealed) {
          seekBy(5)
        }
      }
    },
    [concealed, seekBy]
  )

  useEffect(() => {
    if (!enableGlobalHotkeys) return
    window.addEventListener('keydown', handleKeyDown)
    window.addEventListener('keyup', handleKeyUp)
    return () => {
      window.removeEventListener('keydown', handleKeyDown)
      window.removeEventListener('keyup', handleKeyUp)
    }
  }, [enableGlobalHotkeys, handleKeyDown, handleKeyUp])

  const endTouchBoost = useCallback((pointerId?: number) => {
    if (pointerId !== undefined && holdPointerIdRef.current !== null && pointerId !== holdPointerIdRef.current) {
      return false
    }
    holdPointerIdRef.current = null
    if (touchHoldTimer.current) {
      clearTimeout(touchHoldTimer.current)
      touchHoldTimer.current = null
    }
    if (holdingTouch.current) {
      holdingTouch.current = false
      setHoldingBoost(false)
      if (internalVideoRef.current) {
        internalVideoRef.current.playbackRate = preHoldSpeed.current
      }
      return true
    }
    return false
  }, [])

  useEffect(() => {
    const onWindowBlur = () => {
      rightKeyHeldRef.current = false
      if (keyHoldTimer.current) {
        clearTimeout(keyHoldTimer.current)
        keyHoldTimer.current = null
      }
      if (holdingRight.current) {
        holdingRight.current = false
        setHoldingBoost(false)
        if (internalVideoRef.current) {
          internalVideoRef.current.playbackRate = preHoldSpeed.current
        }
      }
      pointerDownPos.current = null
      endTouchBoost()
    }
    window.addEventListener('blur', onWindowBlur)
    return () => window.removeEventListener('blur', onWindowBlur)
  }, [endTouchBoost])

  const progressRatio = duration > 0 ? Math.min(1, Math.max(0, currentTime / duration)) : 0
  const portalContainer = isFullscreen ? stageRef.current ?? undefined : undefined
  const showCenterPlay = paused && !concealed

  return (
    <div
      ref={stageRef}
      tabIndex={0}
      role="region"
      aria-label={`视频播放器 ${name}`}
      className={`wb-vstage ${fill ? 'wb-vstage-fill' : ''} ${compact ? 'wb-vstage-compact' : ''} ${className}`.trim()}
      data-concealed={concealed ? 'true' : 'false'}
      data-controls={controlsVisible || paused || holdingBoost ? 'visible' : 'hidden'}
      data-touch={isTouchDevice || undefined}
      onKeyDown={enableGlobalHotkeys ? undefined : handleKeyDown}
      onKeyUp={enableGlobalHotkeys ? undefined : handleKeyUp}
    >
      <div
        className="wb-vstage-canvas"
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
          if (concealed || (e.target as HTMLElement).closest('button, input, [role="dialog"], .wb-vstage-shield, .wb-vstage-center, .pw-theater-top, .pw-theater-shortcuts-sheet')) {
            return
          }
          if (isTouch && holdPointerIdRef.current !== null) return
          pointerDownPos.current = { x: e.clientX, y: e.clientY, type: e.pointerType }
          if (isTouch) {
            holdPointerIdRef.current = e.pointerId
            preHoldSpeed.current = speed
            if (touchHoldTimer.current) clearTimeout(touchHoldTimer.current)
            touchHoldTimer.current = setTimeout(() => {
              holdingTouch.current = true
              setHoldingBoost(true)
              if (internalVideoRef.current) {
                if (internalVideoRef.current.paused) {
                  void internalVideoRef.current.play().catch(() => {})
                }
                internalVideoRef.current.playbackRate = 2.0
              }
            }, 300)
          }
        }}
        onPointerCancel={e => {
          pointerDownPos.current = null
          endTouchBoost(e.pointerId)
        }}
        onLostPointerCapture={e => {
          pointerDownPos.current = null
          endTouchBoost(e.pointerId)
        }}
        onPointerUp={e => {
          if (concealed || (e.target as HTMLElement).closest('button, input, [role="dialog"], .wb-vstage-shield, .wb-vstage-center, .pw-theater-top, .pw-theater-shortcuts-sheet')) {
            pointerDownPos.current = null
            endTouchBoost(e.pointerId)
            return
          }
          const wasHoldingBoost = endTouchBoost(e.pointerId)
          const down = pointerDownPos.current
          pointerDownPos.current = null
          if (wasHoldingBoost) return

          if (e.pointerType !== 'mouse') {
            if (down && (Math.abs(e.clientX - down.x) > 14 || Math.abs(e.clientY - down.y) > 14)) {
              return
            }
            const now = Date.now()
            const rect = e.currentTarget.getBoundingClientRect()
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
                  if (next) scheduleHideControls()
                  return next
                })
              }, 220)
            }
          }
        }}
        onClick={e => {
          if (concealed || (e.target as HTMLElement).closest('button, input, [role="dialog"], .wb-vstage-shield, .wb-vstage-center, .pw-theater-top, .pw-theater-shortcuts-sheet')) {
            return
          }
          if (!isTouchDevice) {
            togglePlay()
          }
        }}
        onDoubleClick={e => {
          if (concealed || privacy.enabled || (e.target as HTMLElement).closest('button, input, [role="dialog"], .wb-vstage-shield, .wb-vstage-center, .pw-theater-top, .pw-theater-shortcuts-sheet')) {
            return
          }
          if (!isTouchDevice) {
            toggleFullscreen()
          }
        }}
      >
        <PrivateVideo
          ref={assignVideoRef}
          src={streamSrc}
          poster={poster}
          privacy={privacy}
          hideRevealButton
          aria-label={`播放 ${name}`}
          aria-hidden={concealed || undefined}
          tabIndex={concealed ? -1 : undefined}
          controls={false}
          autoPlay={autoPlay}
          playsInline
          preload={preload}
          className="ws-player wb-vstage-video"
          onDurationChange={e => setDuration(e.currentTarget.duration || 0)}
          onLoadedMetadata={e => {
            const player = e.currentTarget
            setDuration(player.duration || 0)
            player.playbackRate = speed
            player.muted = concealed ? true : muted
            player.volume = volume
            if (pendingSeekRef.current > 0 && pendingSeekRef.current < (player.duration || 0) - 0.15) {
              player.currentTime = pendingSeekRef.current
              setCurrentTime(pendingSeekRef.current)
              pendingSeekRef.current = 0
            }
          }}
          onTimeUpdate={e => {
            const t = e.currentTarget.currentTime
            setCurrentTime(t)
            onTimeRemember?.(t)
          }}
          onPlay={() => {
            setPaused(false)
            scheduleHideControls()
          }}
          onPause={() => {
            setPaused(true)
            setControlsVisible(true)
          }}
          onVolumeChange={e => {
            if (!concealed) {
              setMuted(e.currentTarget.muted)
              setVolume(e.currentTarget.volume)
            }
          }}
          onRateChange={e => {
            if (!holdingRight.current && !holdingTouch.current) {
              setSpeed(e.currentTarget.playbackRate)
            }
          }}
          onError={onError}
        />

        {topOverlay}

        {/* Dark Smoked-Glass Blurred Privacy Shield (stays mounted for composite reveal fade) */}
        <div
          className="wb-vstage-shield"
          role="region"
          aria-label="隐私模式已模糊遮挡视频"
          aria-hidden={!concealed || undefined}
          inert={!concealed || undefined}
        >
          <button
            ref={shieldCtaRef}
            type="button"
            className="wb-vstage-shield-cta"
            aria-label="显示并播放视频"
            tabIndex={concealed ? 0 : -1}
            onClick={e => {
              e.stopPropagation()
              revealAndPlay()
            }}
          >
            <Play size={14} fill="currentColor" className="translate-x-[1px]" aria-hidden="true" />
            <span>显示并播放</span>
          </button>
        </div>

        {/* Bilibili-style Top HUD Pill */}
        <AnimatePresence initial={false}>
          {holdingBoost ? (
            <motion.div
              key="boost"
              role="status"
              className="wb-vstage-hud wb-vstage-hud-boost"
              initial={reduce ? false : { opacity: 0, transform: 'translate(-50%, -6px) scale(0.95)' }}
              animate={{ opacity: 1, transform: 'translate(-50%, 0px) scale(1)' }}
              exit={reduce ? undefined : { opacity: 0, transform: 'translate(-50%, -6px) scale(0.95)' }}
              transition={{ duration: reduce ? 0 : 0.14, ease: EASE_OUT }}
            >
              <FastForward size={13} aria-hidden="true" />
              <span>2.0x 倍速快进中</span>
            </motion.div>
          ) : hud ? (
            <motion.div
              key={hud.key}
              role="status"
              aria-live="polite"
              className="wb-vstage-hud"
              initial={reduce ? false : { opacity: 0, transform: 'translate(-50%, -6px) scale(0.95)' }}
              animate={{ opacity: 1, transform: 'translate(-50%, 0px) scale(1)' }}
              exit={reduce ? undefined : { opacity: 0, transform: 'translate(-50%, -6px) scale(0.95)' }}
              transition={{ duration: reduce ? 0 : 0.14, ease: EASE_OUT }}
            >
              {hud.label}
            </motion.div>
          ) : null}
        </AnimatePresence>

        {/* Single Clean Paused Play Button (non-tabbable while playing or concealed) */}
        <div
          className="wb-vstage-center"
          data-visible={showCenterPlay || undefined}
          aria-hidden={!showCenterPlay || undefined}
        >
          <button
            type="button"
            className="wb-vstage-center-play"
            aria-label="播放视频"
            title="播放 (Space)"
            tabIndex={showCenterPlay ? 0 : -1}
            aria-hidden={!showCenterPlay || undefined}
            onClick={togglePlay}
          >
            <Play size={20} fill="currentColor" className="translate-x-[1px]" aria-hidden="true" />
          </button>
        </div>
      </div>

      {/* Dedicated Non-Overlapping Transport Bar */}
      <div className="wb-vstage-transport">
        <div className="wb-vstage-timeline">
          <input
            type="range"
            aria-label="播放进度"
            aria-valuetext={`${formatVideoClock(currentTime)} / ${formatVideoClock(duration)}`}
            min={0}
            max={duration || 100}
            step={0.05}
            value={currentTime}
            onChange={e => {
              if (concealed) return
              const next = Number(e.target.value)
              setCurrentTime(next)
              if (internalVideoRef.current) internalVideoRef.current.currentTime = next
              revealControls()
            }}
          />
          <div className="wb-vstage-timeline-track">
            <div
              className="wb-vstage-timeline-fill"
              style={{ transform: `scaleX(${progressRatio})` }}
            />
          </div>
          <span
            className="wb-vstage-timeline-thumb"
            style={{ left: `${progressRatio * 100}%` }}
          />
        </div>

        <div className="wb-vstage-controls">
          <div className="wb-vstage-controls-left">
            <button
              ref={playBtnRef}
              type="button"
              className="wb-vstage-btn"
              aria-label={paused ? '播放' : '暂停'}
              title={paused ? '播放 (Space)' : '暂停 (Space)'}
              onClick={togglePlay}
            >
              {paused ? (
                <Play size={15} fill="currentColor" className="translate-x-[1px]" aria-hidden="true" />
              ) : (
                <Pause size={15} fill="currentColor" aria-hidden="true" />
              )}
            </button>

            <button
              type="button"
              className="wb-vstage-btn wb-vstage-fold-narrow"
              aria-label="快退 5 秒"
              title="快退 5 秒 (←)"
              onClick={() => seekBy(-5)}
            >
              <RotateCcw size={14} aria-hidden="true" />
            </button>

            <button
              type="button"
              className="wb-vstage-btn wb-vstage-fold-narrow"
              aria-label="快进 5 秒"
              title="快进 5 秒 (→) · 长按 2x"
              onClick={() => seekBy(5)}
            >
              <RotateCw size={14} aria-hidden="true" />
            </button>

            {onPrev && (
              <button
                type="button"
                className="wb-vstage-btn"
                aria-label="上一条"
                title="上一条 ([)"
                disabled={!hasPrev}
                onClick={onPrev}
              >
                <SkipBack size={14} aria-hidden="true" />
              </button>
            )}

            {onNext && (
              <button
                type="button"
                className="wb-vstage-btn"
                aria-label="下一条"
                title="下一条 (])"
                disabled={!hasNext}
                onClick={onNext}
              >
                <SkipForward size={14} aria-hidden="true" />
              </button>
            )}

            <div className="wb-vstage-vol-wrap">
              <button
                type="button"
                className="wb-vstage-btn"
                aria-label={muted ? '取消静音' : '静音'}
                title="静音 (M)"
                onClick={toggleMute}
              >
                {muted || volume === 0 ? (
                  <VolumeX size={15} aria-hidden="true" />
                ) : (
                  <Volume2 size={15} aria-hidden="true" />
                )}
              </button>
              {showVolumeSlider && (
                <input
                  type="range"
                  aria-label="音量"
                  aria-valuetext={muted || volume === 0 ? '静音' : `${Math.round(volume * 100)}%`}
                  className="wb-vstage-vol-slider"
                  min={0}
                  max={1}
                  step={0.05}
                  value={muted ? 0 : volume}
                  onChange={e => {
                    const next = Number(e.target.value)
                    setVolume(next)
                    setMuted(next === 0)
                    if (internalVideoRef.current) {
                      internalVideoRef.current.volume = next
                      internalVideoRef.current.muted = next === 0
                    }
                  }}
                />
              )}
            </div>

            <span className="wb-vstage-time">
              {formatVideoClock(currentTime)} <em>/</em> {formatVideoClock(duration)}
            </span>
          </div>

          <div className="wb-vstage-controls-right">
            {privacy.enabled && privacy.revealed && (
              <button
                type="button"
                className="wb-vstage-pill"
                aria-label="遮挡视频"
                title="立即重新遮挡"
                onClick={concealVideo}
              >
                <EyeOff size={13} aria-hidden="true" />
                <span className="wb-vstage-btn-label">遮挡视频</span>
              </button>
            )}

            {/* External Player Portal Dropdown */}
            <DropdownMenu.Root open={extMenuOpen} onOpenChange={setExtMenuOpen}>
              <DropdownMenu.Trigger asChild>
                <button
                  type="button"
                  className="wb-vstage-pill wb-vstage-fold-narrow"
                  aria-label="外部播放器打开"
                  title="在 Infuse / nPlayer / SenPlayer 中打开"
                >
                  <ExternalLink size={12} aria-hidden="true" />
                  <span className="wb-vstage-btn-label">外部</span>
                </button>
              </DropdownMenu.Trigger>
              <DropdownMenu.Portal container={portalContainer}>
                <DropdownMenu.Content
                  className="wb-vstage-portal-menu"
                  side="top"
                  align="end"
                  sideOffset={6}
                  collisionPadding={12}
                >
                  <DropdownMenu.Label className="wb-vstage-portal-title">外部播放器</DropdownMenu.Label>
                  <DropdownMenu.Item asChild className="wb-vstage-portal-item">
                    <a href={extLinks.infuse}>
                      <span>Infuse</span>
                      <ExternalLink size={12} aria-hidden="true" />
                    </a>
                  </DropdownMenu.Item>
                  <DropdownMenu.Item asChild className="wb-vstage-portal-item">
                    <a href={extLinks.nplayer}>
                      <span>nPlayer</span>
                      <ExternalLink size={12} aria-hidden="true" />
                    </a>
                  </DropdownMenu.Item>
                  <DropdownMenu.Item asChild className="wb-vstage-portal-item">
                    <a href={extLinks.senplayer}>
                      <span>SenPlayer</span>
                      <ExternalLink size={12} aria-hidden="true" />
                    </a>
                  </DropdownMenu.Item>
                  <DropdownMenu.Separator className="wb-vstage-portal-sep" />
                  <DropdownMenu.Item
                    className="wb-vstage-portal-item"
                    onSelect={() => {
                      void navigator.clipboard
                        ?.writeText(extLinks.streamUrl)
                        .then(() => flashHud('流地址已复制'))
                        .catch(() => {})
                    }}
                  >
                    <span>复制流地址</span>
                    <Copy size={12} aria-hidden="true" />
                  </DropdownMenu.Item>
                </DropdownMenu.Content>
              </DropdownMenu.Portal>
            </DropdownMenu.Root>

            {/* Speed Portal Dropdown */}
            <DropdownMenu.Root open={speedMenuOpen} onOpenChange={setSpeedMenuOpen}>
              <DropdownMenu.Trigger asChild>
                <button
                  type="button"
                  className="wb-vstage-pill"
                  aria-label={`播放倍速 ${speed}x`}
                  title="切换播放倍速"
                >
                  {speed === 1 ? '倍速' : `${speed}x`}
                </button>
              </DropdownMenu.Trigger>
              <DropdownMenu.Portal container={portalContainer}>
                <DropdownMenu.Content
                  className="wb-vstage-portal-menu"
                  side="top"
                  align="end"
                  sideOffset={6}
                  collisionPadding={12}
                >
                  <DropdownMenu.Label className="wb-vstage-portal-title">播放倍速</DropdownMenu.Label>
                  <DropdownMenu.RadioGroup
                    value={String(speed)}
                    onValueChange={val => selectSpeed(Number(val))}
                  >
                    {[...SPEEDS].reverse().map(item => (
                      <DropdownMenu.RadioItem
                        key={item}
                        value={String(item)}
                        className="wb-vstage-portal-item"
                      >
                        <span>{item.toFixed(item === 0.75 || item === 1.25 ? 2 : 1)}x</span>
                        {speed === item && <Check size={12} aria-hidden="true" />}
                      </DropdownMenu.RadioItem>
                    ))}
                  </DropdownMenu.RadioGroup>
                </DropdownMenu.Content>
              </DropdownMenu.Portal>
            </DropdownMenu.Root>

            {extraControls}

            {onExpand && (
              <button
                type="button"
                className="wb-vstage-pill"
                aria-label="居中放大预览"
                title="居中放大并承接当前进度"
                onClick={e => {
                  const t = internalVideoRef.current?.currentTime || 0
                  internalVideoRef.current?.pause()
                  onExpand(t, e.currentTarget)
                }}
              >
                <Maximize size={12} aria-hidden="true" />
                <span className="wb-vstage-btn-label">居中放大</span>
              </button>
            )}

            {/* Narrow Overflow Portal Menu (<= 400px @container) */}
            <DropdownMenu.Root open={overflowMenuOpen} onOpenChange={setOverflowMenuOpen}>
              <DropdownMenu.Trigger asChild>
                <button
                  type="button"
                  className="wb-vstage-btn wb-vstage-overflow-trigger"
                  aria-label="更多播放控制"
                  title="更多控制"
                >
                  <MoreHorizontal size={15} aria-hidden="true" />
                </button>
              </DropdownMenu.Trigger>
              <DropdownMenu.Portal container={portalContainer}>
                <DropdownMenu.Content
                  className="wb-vstage-portal-menu"
                  side="top"
                  align="end"
                  sideOffset={6}
                  collisionPadding={12}
                >
                  <DropdownMenu.Item className="wb-vstage-portal-item" onSelect={() => seekBy(-5)}>
                    <span>快退 5 秒 (←)</span>
                    <RotateCcw size={12} aria-hidden="true" />
                  </DropdownMenu.Item>
                  <DropdownMenu.Item className="wb-vstage-portal-item" onSelect={() => seekBy(5)}>
                    <span>快进 5 秒 (→)</span>
                    <RotateCw size={12} aria-hidden="true" />
                  </DropdownMenu.Item>
                  <DropdownMenu.Item className="wb-vstage-portal-item" onSelect={toggleMute}>
                    <span>{muted ? '取消静音 (M)' : '静音 (M)'}</span>
                    {muted ? <Volume2 size={12} aria-hidden="true" /> : <VolumeX size={12} aria-hidden="true" />}
                  </DropdownMenu.Item>
                  <DropdownMenu.Separator className="wb-vstage-portal-sep" />
                  <DropdownMenu.Item asChild className="wb-vstage-portal-item">
                    <a href={extLinks.infuse}>
                      <span>在 Infuse 打开</span>
                      <ExternalLink size={12} aria-hidden="true" />
                    </a>
                  </DropdownMenu.Item>
                  <DropdownMenu.Item asChild className="wb-vstage-portal-item">
                    <a href={extLinks.nplayer}>
                      <span>在 nPlayer 打开</span>
                      <ExternalLink size={12} aria-hidden="true" />
                    </a>
                  </DropdownMenu.Item>
                  <DropdownMenu.Item asChild className="wb-vstage-portal-item">
                    <a href={extLinks.senplayer}>
                      <span>在 SenPlayer 打开</span>
                      <ExternalLink size={12} aria-hidden="true" />
                    </a>
                  </DropdownMenu.Item>
                  <DropdownMenu.Item
                    className="wb-vstage-portal-item"
                    onSelect={() => {
                      void navigator.clipboard
                        ?.writeText(extLinks.streamUrl)
                        .then(() => flashHud('流地址已复制'))
                        .catch(() => {})
                    }}
                  >
                    <span>复制流地址</span>
                    <Copy size={12} aria-hidden="true" />
                  </DropdownMenu.Item>
                </DropdownMenu.Content>
              </DropdownMenu.Portal>
            </DropdownMenu.Root>

            {!privacy.enabled && (
              <button
                type="button"
                className="wb-vstage-btn"
                aria-label={isFullscreen ? '退出全屏' : '全屏播放'}
                title="全屏 (F / 双击画面)"
                onClick={toggleFullscreen}
              >
                {isFullscreen ? <Minimize2 size={15} aria-hidden="true" /> : <Maximize2 size={15} aria-hidden="true" />}
              </button>
            )}
          </div>
        </div>
      </div>
    </div>
  )
}
