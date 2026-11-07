'use client'

import { useCallback, useLayoutEffect, useRef, useState, type ComponentProps } from 'react'
import { usePathname } from 'next/navigation'
import { Eye, EyeOff } from 'lucide-react'
import { Button } from '@/components/beui/motion/button/base'
import { usePrivacy } from '@/components/privacy-provider'

// Thin application policy around the existing BEUI button and native media.
// source + route + revision ensure permission cannot transfer to another item.
function useMediaPrivacy(source: string | undefined) {
  const { enabled, ready, revision } = usePrivacy()
  const pathname = usePathname()
  const token = JSON.stringify([source, pathname, revision])
  const [grant, setGrant] = useState<string | null>(null)
  const container = useRef<HTMLSpanElement>(null)
  const revealed = ready && enabled && grant === token
  const blocked = !ready || (enabled && !revealed)

  useLayoutEffect(() => { setGrant(null) }, [source, pathname, revision])

  useLayoutEffect(() => {
    const node = container.current
    const dialog = node?.closest('[role="dialog"], [role="alertdialog"]')
    if (!node || !dialog) return
    const observer = new MutationObserver(() => {
      if (dialog.getAttribute('data-state') === 'closed') {
        node.querySelector('[data-privacy-revealed]')?.removeAttribute('data-privacy-revealed')
        node.querySelector('video')?.pause()
        setGrant(null)
      }
    })
    observer.observe(dialog, { attributes: true, attributeFilter: ['data-state'] })
    return () => observer.disconnect()
  }, [])

  return { container, enabled, ready, revealed, blocked, toggle: () => setGrant(revealed ? null : token) }
}

function RevealButton({ revealed, toggle, kind }: { revealed: boolean; toggle: () => void; kind: '视频' | '图片' }) {
  const label = revealed ? `遮挡${kind}` : `显示${kind}`
  return <Button variant="secondary" size="sm" whileTap={undefined} className="privacy-reveal" aria-label={label} title={label} aria-pressed={revealed} onClick={toggle}>
    {revealed ? <EyeOff size={15} /> : <Eye size={15} />}<span>{label}</span>
  </Button>
}

export function PrivateImage({ src, alt, compact = false, ...props }: ComponentProps<'img'> & { compact?: boolean }) {
  const privacy = useMediaPrivacy(typeof src === 'string' ? src : undefined)
  return <span ref={privacy.container} className="privacy-image" data-compact={compact || undefined} data-revealed={privacy.revealed}>
    <img {...props} src={src} alt={alt} data-private-media data-privacy-revealed={privacy.revealed || undefined} draggable={false} onContextMenu={event => { if (privacy.enabled) event.preventDefault() }} />
    {privacy.ready && privacy.enabled && <RevealButton revealed={privacy.revealed} toggle={privacy.toggle} kind="图片" />}
  </span>
}

export function PrivateVideo({ ref, src, autoPlay, controls, muted, controlsList, disablePictureInPicture, disableRemotePlayback, onPlay, onContextMenu, ...props }: Omit<ComponentProps<'video'>, 'src'> & { src?: string }) {
  const privacy = useMediaPrivacy(src)
  const player = useRef<HTMLVideoElement | null>(null)
  useLayoutEffect(() => { if (privacy.blocked) player.current?.pause() }, [privacy.blocked])
  useLayoutEffect(() => {
    const video = player.current
    const exitPictureInPicture = () => {
      if (privacy.enabled && document.pictureInPictureElement) void document.exitPictureInPicture().catch(() => {})
    }
    video?.addEventListener('enterpictureinpicture', exitPictureInPicture)
    return () => video?.removeEventListener('enterpictureinpicture', exitPictureInPicture)
  }, [privacy.enabled])
  const assignRef = useCallback((node: HTMLVideoElement | null) => {
    player.current = node
    if (typeof ref === 'function') return ref(node)
    if (ref) ref.current = node
  }, [ref])
  return <span ref={privacy.container} className="privacy-video" data-revealed={privacy.revealed}>
    <video {...props} ref={assignRef} src={src} data-private-media data-privacy-revealed={privacy.revealed || undefined}
      autoPlay={privacy.ready && !privacy.enabled && autoPlay} controls={!privacy.blocked && controls}
      muted={privacy.blocked || muted} tabIndex={privacy.blocked ? -1 : props.tabIndex}
      controlsList={privacy.enabled ? 'nofullscreen nodownload noremoteplayback' : controlsList}
      disablePictureInPicture={privacy.enabled || disablePictureInPicture} disableRemotePlayback={privacy.enabled || disableRemotePlayback}
      onContextMenu={event => { if (privacy.enabled) event.preventDefault(); else onContextMenu?.(event) }}
      onPlay={event => {
        if (document.documentElement.dataset.privacy !== 'off' && event.currentTarget.dataset.privacyRevealed !== 'true') event.currentTarget.pause()
        else onPlay?.(event)
      }}
    />
    {privacy.ready && privacy.enabled && <RevealButton revealed={privacy.revealed} toggle={privacy.toggle} kind="视频" />}
  </span>
}
