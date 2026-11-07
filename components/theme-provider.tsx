'use client'

import { createContext, useCallback, useContext, useEffect, useState } from 'react'
import { Toaster } from 'sonner'

export type ThemeMode = 'light' | 'dark' | 'system'
export type ResolvedTheme = 'light' | 'dark'

interface ThemeContextValue {
  mode: ThemeMode
  resolved: ResolvedTheme
  setMode: (mode: ThemeMode) => void
  cycleMode: () => void
}

const STORAGE_KEY = 'wb-theme'
const LEGACY_KEY = 'persons-theme'

const ThemeContext = createContext<ThemeContextValue>({
  mode: 'dark',
  resolved: 'dark',
  setMode: () => {},
  cycleMode: () => {},
})

function resolveTheme(mode: ThemeMode): ResolvedTheme {
  if (mode === 'light' || mode === 'dark') return mode
  if (typeof window !== 'undefined' && window.matchMedia('(prefers-color-scheme: light)').matches) {
    return 'light'
  }
  return 'dark'
}

function applyThemeToDOM(mode: ThemeMode, resolved: ResolvedTheme) {
  if (typeof document === 'undefined') return
  const root = document.documentElement
  root.dataset.theme = resolved
  root.dataset.themeMode = mode
  root.classList.toggle('dark', resolved === 'dark')
  root.classList.toggle('light', resolved === 'light')
  root.style.colorScheme = resolved
}

export function ThemeProvider({ children }: { children: React.ReactNode }) {
  const [mode, setModeState] = useState<ThemeMode>('dark')
  const [resolved, setResolved] = useState<ResolvedTheme>('dark')

  useEffect(() => {
    let initial: ThemeMode = 'dark'
    try {
      const stored = localStorage.getItem(STORAGE_KEY) as ThemeMode | null
      if (stored === 'light' || stored === 'dark' || stored === 'system') {
        initial = stored
      } else {
        const legacy = localStorage.getItem(LEGACY_KEY)
        if (legacy === 'light' || legacy === 'dark') {
          initial = legacy
          localStorage.setItem(STORAGE_KEY, legacy)
        }
      }
    } catch {}
    const nextResolved = resolveTheme(initial)
    setModeState(initial)
    setResolved(nextResolved)
    applyThemeToDOM(initial, nextResolved)
  }, [])

  useEffect(() => {
    const media = window.matchMedia('(prefers-color-scheme: light)')
    const onMediaChange = () => {
      if (mode === 'system') {
        const nextResolved = resolveTheme('system')
        setResolved(nextResolved)
        applyThemeToDOM('system', nextResolved)
      }
    }
    const onStorage = (event: StorageEvent) => {
      if (event.key === STORAGE_KEY && event.newValue) {
        const nextMode = event.newValue as ThemeMode
        if (nextMode === 'light' || nextMode === 'dark' || nextMode === 'system') {
          const nextResolved = resolveTheme(nextMode)
          setModeState(nextMode)
          setResolved(nextResolved)
          applyThemeToDOM(nextMode, nextResolved)
        }
      }
    }
    media.addEventListener('change', onMediaChange)
    window.addEventListener('storage', onStorage)
    return () => {
      media.removeEventListener('change', onMediaChange)
      window.removeEventListener('storage', onStorage)
    }
  }, [mode])

  const setMode = useCallback((nextMode: ThemeMode) => {
    const nextResolved = resolveTheme(nextMode)
    setModeState(nextMode)
    setResolved(nextResolved)
    applyThemeToDOM(nextMode, nextResolved)
    try {
      localStorage.setItem(STORAGE_KEY, nextMode)
      localStorage.setItem(LEGACY_KEY, nextResolved)
    } catch {}
  }, [])

  const cycleMode = useCallback(() => {
    const order: ThemeMode[] = ['light', 'dark', 'system']
    const next = order[(order.indexOf(mode) + 1) % order.length]
    setMode(next)
  }, [mode, setMode])

  return (
    <ThemeContext.Provider value={{ mode, resolved, setMode, cycleMode }}>
      {children}
      <Toaster richColors position="bottom-right" theme={resolved} />
    </ThemeContext.Provider>
  )
}

export function useTheme() {
  return useContext(ThemeContext)
}
