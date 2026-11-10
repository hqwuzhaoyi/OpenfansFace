'use client'

import { useCallback, useEffect, useRef, useState } from 'react'
import { AnimatePresence, motion, useReducedMotion } from 'motion/react'
import {
  FastForward,
  Maximize2,
  Minimize2,
  Pause,
  Play,
  RotateCcw,
  RotateCw,
  Volume2,
  VolumeX,
} from 'lucide-react'
import { PrivateVideo } from '@/components/private-media'
import { ActionSwapIcon } from '@/components/beui/motion/action-swap'
import { EASE_OUT } from '@/lib/beui/ease'

const SPEEDS = [1, 1.25, 1.5, 2] as const

export const formatVideoClock = (seconds?: number) => {
  if (seconds === undefined || !Number.isFinite(seconds) || seconds < 0) return '00:00'
  const total = Math.floor(seconds)
  const mins = Math.floor(total / 60)
  const secs = total % 60
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
  enableGlobalHotkeys?: boolean
  className?: string
  videoRef?: React.RefObject<HTMLVideoElement | null>
  onTimeRemember?: (time: number) => void
  onError?: (event: React.SyntheticEvent<HTMLVideoElement>) => void
}

export function WorkbenchVideoPlayer({
  src,
  name,
  poster,
  autoPlay = false,
  preload = 'metadata',
  initialTime = 0,
  compact = false,
  enableGlobalHotkeys = false,
  className = '',
  videoRef: externalVideoRef,
  onTimeRemember,
  onError,
}: WorkbenchVideoPlayerProps) {
  const reduce = useReducedMotion()
  const stageRef = useRef<HTMLDivElement>(null)
  const internalVideoRef = useRef<HTMLVideoElement | null>(null)
  const hudTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const idleTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const keyHoldTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const touchHoldTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const singleTapTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const holdingRight = useRef(false)
  const holdingTouch = useRef(false)
  const preHoldSpeed = useRef(1)
  const lastTapRef = useRef<{ time: number; x: number }>({ time: 0, x: 0 })
  const pointerDownPos = useRef<{ x: number; y: number; type: string } | null>(null)

  const [paused, setPaused] = useState(!autoPlay)
  const [muted, setMuted] = useState(false)
  const [volume, setVolume] = useState(1)
  const [speed, setSpeed] = useState<number>(1)
  const [holdingBoost, setHoldingBoost] = useState(false)
  const [currentTime, setCurrentTime] = useState(0)
  const [duration, setDuration] = useState(0)
  const [isFullscreen, setIsFullscreen] = useState(false)
  const [controlsVisible, setControlsVisible] = useState(true)
  const [speedMenuOpen, setSpeedMenuOpen] = useState(false)
  const [isTouchDevice, setIsTouchDevice] = useState(false)
  const [hud, setHud] = useState<{ key: number; label: string } | null>(null)

  const assignVideoRef = useCallback(
    (node: HTMLVideoElement | null) => {
      internalVideoRef.current = node
      if (externalVideoRef) externalVideoRef.current = node
    },
    [externalVideoRef]
  )

  const flashHud = useCallback((label: string) => {
    if (hudTimer.current) clearTimeout(hudTimer.current)
    setHud({ key: Date.now(), label })
    hudTimer.current = setTimeout(() => setHud(null), 850)
  }, [])

  const scheduleHideControls = useCallback(() => {
    if (idleTimer.current) clearTimeout(idleTimer.current)
    idleTimer.current = setTimeout(() => {
      if (internalVideoRef.current && !internalVideoRef.current.paused && !speedMenuOpen) {
        setControlsVisible(false)
      }
    }, 2600)
  }, [speedMenuOpen])

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
  }, [src])

  useEffect(() => {
    setPaused(!autoPlay)
    setCurrentTime(0)
    setDuration(0)
    setSpeedMenuOpen(false)
    revealControls()
  }, [src, autoPlay, revealControls])

  const togglePlay = useCallback(() => {
    const el = internalVideoRef.current
    if (!el) return
    // If privacy mode is blocking the video, click the reveal button first
    const revealBtn = stageRef.current?.querySelector<HTMLButtonElement>(
      '.privacy-video:not([data-revealed="true"]) .privacy-reveal'
    )
    if (revealBtn) {
      revealBtn.click()
      requestAnimationFrame(() => {
        void internalVideoRef.current?.play().catch(() => {})
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
      const el = internalVideoRef.current
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
    [flashHud, revealControls]
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
    const next = !muted
    setMuted(next)
    if (internalVideoRef.current) internalVideoRef.current.muted = next
    flashHud(next ? '静音' : `音量 ${Math.round(volume * 100)}%`)
  }, [muted, volume, flashHud])

  const toggleFullscreen = useCallback(() => {
    const container = stageRef.current
    const video = internalVideoRef.current as (HTMLVideoElement & {
      webkitEnterFullscreen?: () => void
      webkitExitFullscreen?: () => void
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
  }, [])

  const handleKeyDown = useCallback(
    (e: KeyboardEvent | React.KeyboardEvent) => {
      if (e.defaultPrevented || e.metaKey || e.ctrlKey || e.altKey) return
      const target = e.target as HTMLElement | null
      if (target && ['INPUT', 'TEXTAREA', 'SELECT'].includes(target.tagName)) return

      if (e.key === 'ArrowRight') {
        e.preventDefault()
        if (e.repeat) return
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
      }
    },
    [speed, togglePlay, seekBy, adjustVolume, toggleMute, toggleFullscreen]
  )

  const handleKeyUp = useCallback(
    (e: KeyboardEvent | React.KeyboardEvent) => {
      if (e.key === 'ArrowRight') {
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
        } else {
          seekBy(5)
        }
      }
    },
    [seekBy]
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

  const endTouchBoost = useCallback(() => {
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

  const progressRatio = duration > 0 ? Math.min(1, Math.max(0, currentTime / duration)) : 0

  return (
    <div
      ref={stageRef}
      tabIndex={0}
      role="region"
      aria-label={`视频播放器 ${name}`}
      className={`wb-vstage ${compact ? 'wb-vstage-compact' : ''} ${className}`}
      data-controls={controlsVisible || paused || holdingBoost ? 'visible' : 'hidden'}
      data-touch={isTouchDevice || undefined}
      onKeyDown={enableGlobalHotkeys ? undefined : handleKeyDown}
      onKeyUp={enableGlobalHotkeys ? undefined : handleKeyUp}
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
        if ((e.target as HTMLElement).closest('.wb-vstage-bottom, .wb-vstage-center, .privacy-reveal')) {
          return
        }
        pointerDownPos.current = { x: e.clientX, y: e.clientY, type: e.pointerType }
        if (isTouch) {
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
      onPointerCancel={() => {
        pointerDownPos.current = null
        endTouchBoost()
      }}
      onPointerUp={e => {
        if ((e.target as HTMLElement).closest('.wb-vstage-bottom, .wb-vstage-center, .privacy-reveal')) {
          pointerDownPos.current = null
          endTouchBoost()
          return
        }
        const wasHoldingBoost = endTouchBoost()
        const down = pointerDownPos.current
        pointerDownPos.current = null
        if (wasHoldingBoost) return

        if (e.pointerType !== 'mouse') {
          if ( down && (Math.abs(e.clientX - down.x) > 14 || Math.abs(e.clientY - down.y) > 14) ) {
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
                if (next) scheduleHideControls()
                return next
              })
            }, 220)
          }
        }
      }}
      onClick={e => {
        if ((e.target as HTMLElement).closest('.wb-vstage-bottom, .wb-vstage-center, .privacy-reveal')) {
          return
        }
        // Desktop mouse click toggles play/pause; touch is handled in onPointerUp
        if (!isTouchDevice) {
          togglePlay()
        }
      }}
      onDoubleClick={e => {
        if ((e.target as HTMLElement).closest('.wb-vstage-bottom, .wb-vstage-center, .privacy-reveal')) {
          return
        }
        if (!isTouchDevice) {
          toggleFullscreen()
        }
      }}
    >
      <PrivateVideo
        ref={assignVideoRef}
        src={src}
        poster={poster}
        aria-label={`播放 ${name}`}
        controls={false}
        autoPlay={autoPlay}
        playsInline
        preload={preload}
        className="ws-player wb-vstage-video"
        onLoadedMetadata={e => {
          const player = e.currentTarget
          setDuration(player.duration || 0)
          player.playbackRate = speed
          player.muted = muted
          player.volume = volume
          if (initialTime > 0 && initialTime < (player.duration || 0) - 0.25) {
            player.currentTime = initialTime
            setCurrentTime(initialTime)
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
          setMuted(e.currentTarget.muted)
          setVolume(e.currentTarget.volume)
        }}
        onRateChange={e => {
          if (!holdingRight.current && !holdingTouch.current) {
            setSpeed(e.currentTarget.playbackRate)
          }
        }}
        onError={onError}
      />

      {/* Bilibili-style Top HUD Pill (2.0x boost or Seek/Play feedback) */}
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
            <FastForward size={13} />
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

      {/* Center Touch & Paused Controls (-5s / Play-Pause / +5s) */}
      <div className="wb-vstage-center" data-visible={paused || (isTouchDevice && controlsVisible) || undefined}>
        <button
          type="button"
          className="wb-vstage-center-seek"
          aria-label="快退 5 秒"
          title="快退 5 秒 (←)"
          onClick={() => seekBy(-5)}
        >
          <RotateCcw size={15} />
          <span>5s</span>
        </button>
        <button
          type="button"
          className="wb-vstage-center-play"
          aria-label={paused ? '播放视频' : '暂停视频'}
          title={paused ? '播放 (Space)' : '暂停 (Space)'}
          onClick={togglePlay}
        >
          <ActionSwapIcon value={paused ? 'play' : 'pause'} animation="roll">
            {paused ? <Play size={20} fill="currentColor" /> : <Pause size={20} fill="currentColor" />}
          </ActionSwapIcon>
        </button>
        <button
          type="button"
          className="wb-vstage-center-seek"
          aria-label="快进 5 秒"
          title="快进 5 秒 (→)"
          onClick={() => seekBy(5)}
        >
          <RotateCw size={15} />
          <span>5s</span>
        </button>
      </div>

      {/* Bottom Gradient Control Bar */}
      <div className="wb-vstage-bottom">
        <div className="wb-vstage-timeline">
          <input
            type="range"
            aria-label="播放进度"
            min={0}
            max={duration || 100}
            step={0.05}
            value={currentTime}
            onChange={e => {
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
              type="button"
              className="wb-vstage-btn"
              aria-label={paused ? '播放' : '暂停'}
              title={paused ? '播放 (Space)' : '暂停 (Space)'}
              onClick={togglePlay}
            >
              <ActionSwapIcon value={paused ? 'play' : 'pause'} animation="roll">
                {paused ? <Play size={15} fill="currentColor" /> : <Pause size={15} fill="currentColor" />}
              </ActionSwapIcon>
            </button>

            <button
              type="button"
              className="wb-vstage-btn wb-vstage-hide-xs"
              aria-label="快退 5 秒"
              title="快退 5 秒 (←)"
              onClick={() => seekBy(-5)}
            >
              <RotateCcw size={14} />
            </button>

            <button
              type="button"
              className="wb-vstage-btn wb-vstage-hide-xs"
              aria-label="快进 5 秒"
              title="快进 5 秒 (→)"
              onClick={() => seekBy(5)}
            >
              <RotateCw size={14} />
            </button>

            <button
              type="button"
              className="wb-vstage-btn"
              aria-label={muted ? '取消静音' : '静音'}
              title="静音 (M)"
              onClick={toggleMute}
            >
              <ActionSwapIcon value={muted ? 'muted' : 'unmuted'} animation="roll">
                {muted || volume === 0 ? <VolumeX size={15} /> : <Volume2 size={15} />}
              </ActionSwapIcon>
            </button>

            <span className="wb-vstage-time">
              {formatVideoClock(currentTime)} <em>/</em> {formatVideoClock(duration)}
            </span>
          </div>

          <div className="wb-vstage-controls-right">
            <div className="wb-vstage-speed-wrap">
              <button
                type="button"
                className="wb-vstage-pill"
                aria-label={`播放倍速 ${speed}x`}
                aria-expanded={speedMenuOpen}
                onClick={() => setSpeedMenuOpen(v => !v)}
              >
                {speed === 1 ? '倍速' : `${speed}x`}
              </button>
              <AnimatePresence initial={false}>
                {speedMenuOpen && (
                  <motion.div
                    role="menu"
                    aria-label="选择倍速"
                    className="wb-vstage-speed-menu"
                    initial={reduce ? false : { opacity: 0, transform: 'translateY(4px)' }}
                    animate={{ opacity: 1, transform: 'translateY(0px)' }}
                    exit={reduce ? undefined : { opacity: 0, transform: 'translateY(4px)' }}
                    transition={{ duration: reduce ? 0 : 0.14, ease: EASE_OUT }}
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

            <button
              type="button"
              className="wb-vstage-btn"
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
  )
}
