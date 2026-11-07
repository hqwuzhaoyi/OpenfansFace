'use client'

import { createContext, useCallback, useContext, useEffect, useLayoutEffect, useRef, useState } from 'react'
import { usePathname } from 'next/navigation'
import { PRIVACY_STORAGE_KEY as STORAGE_KEY } from '@/lib/privacy'

const PrivacyContext = createContext({ enabled: true, ready: false, revision: 0, setEnabled: (_enabled: boolean) => {} })

function stopMedia() {
  document.querySelectorAll<HTMLVideoElement>('video').forEach(video => {
    video.pause()
    const native = video as HTMLVideoElement & { webkitDisplayingFullscreen?: boolean; webkitExitFullscreen?: () => void }
    if (native.webkitDisplayingFullscreen) native.webkitExitFullscreen?.()
  })
  if (document.fullscreenElement) void document.exitFullscreen().catch(() => {})
  if (document.pictureInPictureElement) void document.exitPictureInPicture().catch(() => {})
}

export function PrivacyProvider({ children }: { children: React.ReactNode }) {
  const [enabled, setState] = useState(true)
  const [ready, setReady] = useState(false)
  const [revision, setRevision] = useState(0)
  const enabledRef = useRef(true)
  const pathname = usePathname()
  const reset = useCallback(() => {
    // Revoke in the same event, before React's next render (also covers portals).
    document.querySelectorAll('[data-privacy-revealed]').forEach(node => node.removeAttribute('data-privacy-revealed'))
    stopMedia()
    setRevision(value => value + 1)
  }, [])
  const apply = useCallback((value: boolean) => {
    enabledRef.current = value
    document.documentElement.dataset.privacy = value ? 'on' : 'off'
    reset()
    setState(value)
  }, [reset])
  const setEnabled = useCallback((value: boolean) => {
    apply(value)
    try { localStorage.setItem(STORAGE_KEY, value ? 'on' : 'off') } catch {}
  }, [apply])

  useLayoutEffect(() => {
    apply(document.documentElement.dataset.privacy !== 'off')
    setReady(true)
  }, [apply])
  useLayoutEffect(() => { if (enabledRef.current) reset() }, [pathname, reset])
  useEffect(() => {
    const hide = () => { if (enabledRef.current) reset() }
    const visibility = () => { if (document.hidden) hide() }
    const storage = (event: StorageEvent) => {
      if (event.key === STORAGE_KEY || event.key === null) apply(event.newValue === 'on')
    }
    const fullscreen = () => { if (enabledRef.current && document.fullscreenElement) reset() }
    window.addEventListener('blur', hide)
    window.addEventListener('pagehide', hide)
    window.addEventListener('storage', storage)
    document.addEventListener('visibilitychange', visibility)
    document.addEventListener('fullscreenchange', fullscreen)
    return () => {
      window.removeEventListener('blur', hide)
      window.removeEventListener('pagehide', hide)
      window.removeEventListener('storage', storage)
      document.removeEventListener('visibilitychange', visibility)
      document.removeEventListener('fullscreenchange', fullscreen)
    }
  }, [apply, reset])

  return <PrivacyContext.Provider value={{ enabled, ready, revision, setEnabled }}>{children}</PrivacyContext.Provider>
}

export const usePrivacy = () => useContext(PrivacyContext)
