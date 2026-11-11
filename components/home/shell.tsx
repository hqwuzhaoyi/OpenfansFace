'use client'

import Link from 'next/link'
import { usePathname } from 'next/navigation'
import { useEffect, useRef, useState } from 'react'
import { Clapperboard, Monitor, Moon, PanelLeft, Sun, X, Eye, EyeOff } from 'lucide-react'
import { Button } from '@/components/beui/motion/button/base'
import { usePrivacy } from '@/components/privacy-provider'
import { MotionConfig } from 'motion/react'
import { workbenchNavigation } from '@/lib/workbench-navigation'
import { useTheme, type ThemeMode } from '@/components/theme-provider'
import { BackgroundOperationsDock } from '@/components/workbench/background-operations'
import {
  AnimatedSidebar,
  AnimatedSidebarClose,
  AnimatedSidebarContent,
  AnimatedSidebarFooter,
  AnimatedSidebarHeader,
  AnimatedSidebarInset,
  AnimatedSidebarMenu,
  AnimatedSidebarMenuButton,
  AnimatedSidebarMenuItem,
  AnimatedSidebarProvider,
  AnimatedSidebarTrigger,
  useAnimatedSidebar,
} from '@/components/beui/motion/animated-sidebar'
import './home.css'

const navigation = workbenchNavigation

const THEME_OPTIONS: { mode: ThemeMode; label: string; Icon: typeof Sun }[] = [
  { mode: 'light', label: '浅色', Icon: Sun },
  { mode: 'dark', label: '深色', Icon: Moon },
  { mode: 'system', label: '跟随系统', Icon: Monitor },
]

function isNavActive(pathname: string, href: string) {
  if (href === '/') return pathname === '/'
  return pathname === href || pathname.startsWith(href + '/')
}

function SidebarHeaderBrand() {
  const { isMobile } = useAnimatedSidebar()
  return (
    <div className="home-sidebar-head">
      <Link className="home-brand" href="/" aria-label="返回首页" title="返回首页">
        <span className="home-brand-mark">
          <Clapperboard size={17} />
        </span>
        {isMobile && <span className="home-brand-text">OpenfansFace</span>}
      </Link>
      {isMobile && (
        <AnimatedSidebarClose aria-label="关闭工作台导航" className="home-mobile-close">
          <X size={18} />
        </AnimatedSidebarClose>
      )}
    </div>
  )
}

function SidebarThemeControl() {
  const { isMobile } = useAnimatedSidebar()
  const { mode, resolved, setMode } = useTheme()
  const [open, setOpen] = useState(false)
  const wrapRef = useRef<HTMLDivElement>(null)
  const triggerRef = useRef<HTMLButtonElement>(null)

  useEffect(() => {
    if (!open) return
    const onPointerDown = (e: PointerEvent) => {
      if (!wrapRef.current?.contains(e.target as Node)) setOpen(false)
    }
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault()
        setOpen(false)
        triggerRef.current?.focus()
      }
    }
    window.addEventListener('pointerdown', onPointerDown)
    window.addEventListener('keydown', onKeyDown)
    return () => {
      window.removeEventListener('pointerdown', onPointerDown)
      window.removeEventListener('keydown', onKeyDown)
    }
  }, [open])

  if (isMobile) {
    return (
      <div className="home-theme-mobile" role="group" aria-label="界面主题">
        {THEME_OPTIONS.map(({ mode: itemMode, label, Icon }) => (
          <button
            key={itemMode}
            type="button"
            aria-pressed={mode === itemMode}
            onClick={() => setMode(itemMode)}
            className="home-theme-seg"
          >
            <Icon size={14} />
            <span>{label}</span>
          </button>
        ))}
      </div>
    )
  }

  const currentLabel = THEME_OPTIONS.find(o => o.mode === mode)?.label ?? '深色'
  const CurrentIcon = mode === 'system' ? Monitor : resolved === 'light' ? Sun : Moon

  return (
    <div ref={wrapRef} className="home-theme-rail">
      <button
        ref={triggerRef}
        type="button"
        aria-label={`主题：${currentLabel}`}
        aria-expanded={open}
        aria-haspopup="true"
        title={`主题：${currentLabel}`}
        onClick={() => setOpen(v => !v)}
        className="home-theme-trigger"
      >
        <CurrentIcon size={17} />
      </button>
      {open && (
        <div role="menu" aria-label="选择主题" className="home-theme-menu">
          {THEME_OPTIONS.map(({ mode: itemMode, label, Icon }) => (
            <button
              key={itemMode}
              type="button"
              role="menuitemradio"
              aria-checked={mode === itemMode}
              onClick={() => {
                setMode(itemMode)
                setOpen(false)
                triggerRef.current?.focus()
              }}
              className="home-theme-option"
            >
              <Icon size={14} />
              <span>{label}</span>
            </button>
          ))}
        </div>
      )}
    </div>
  )
}

function PrivacyControl({ mobile = false }: { mobile?: boolean }) {
  const { enabled, ready, setEnabled } = usePrivacy()
  const label = `隐私模式：${enabled ? '已开启' : '已关闭'}`
  return <Button variant="ghost" size="icon" whileTap={undefined} disabled={!ready} aria-label={label} title={`${label}，点击${enabled ? '关闭' : '开启'}`} aria-pressed={enabled} onClick={() => setEnabled(!enabled)} className={`privacy-control ${mobile ? 'privacy-control-mobile' : ''}`}>
    {enabled ? <EyeOff size={17} /> : <Eye size={17} />}
  </Button>
}

function MobileTopBar({ pathname }: { pathname: string }) {
  const { isMobile } = useAnimatedSidebar()
  const { mode, resolved, cycleMode } = useTheme()
  if (!isMobile) return null
  const currentNav = navigation.find(n => isNavActive(pathname, n.href))
  const modeLabel = THEME_OPTIONS.find(o => o.mode === mode)?.label ?? '深色'
  const Icon = mode === 'system' ? Monitor : resolved === 'light' ? Sun : Moon

  return (
    <div className="home-mobile-nav">
      <AnimatedSidebarTrigger aria-label="打开工作台导航" className="home-mobile-trigger">
        <PanelLeft size={18} />
      </AnimatedSidebarTrigger>
      <Link href="/" className="home-mobile-brand" aria-label="返回首页">
        <Clapperboard size={16} />
        <span>OpenfansFace</span>
      </Link>
      {currentNav && <span className="home-mobile-current">{currentNav.label}</span>}
      <div className="home-mobile-actions">
        <BackgroundOperationsDock mobile />
        <PrivacyControl mobile />
        <button
          type="button"
          aria-label={`切换主题（当前：${modeLabel}）`}
          title={`主题：${modeLabel}`}
          onClick={cycleMode}
          className="home-mobile-theme"
        >
          <Icon size={16} />
        </button>
      </div>
    </div>
  )
}

export function HomeShell({ children }: { children: React.ReactNode }) {
  const pathname = usePathname()
  const isSplitPane = pathname === '/review' || pathname === '/operations' || pathname === '/integration' || pathname === '/staging' || pathname === '/persons' || pathname.startsWith('/persons/')

  return (
    <MotionConfig reducedMotion="user">
      <AnimatedSidebarProvider
        open={false}
        className="home-shell"
        style={{
          '--sidebar-width': '56px',
          '--sidebar-width-icon': '56px',
          '--sidebar-width-mobile': '260px',
        }}
      >
        <AnimatedSidebar ariaLabel="工作台导航" panelClassName="home-sidebar">
          <AnimatedSidebarHeader className="home-sidebar-header">
            <SidebarHeaderBrand />
          </AnimatedSidebarHeader>
          <AnimatedSidebarContent className="home-sidebar-content">
            <AnimatedSidebarMenu>
              {navigation.map(n => (
                <AnimatedSidebarMenuItem key={n.href}>
                  <AnimatedSidebarMenuButton
                    href={n.href}
                    icon={<n.Icon size={18} />}
                    isActive={isNavActive(pathname, n.href)}
                  >
                    {n.label}
                  </AnimatedSidebarMenuButton>
                </AnimatedSidebarMenuItem>
              ))}
            </AnimatedSidebarMenu>
          </AnimatedSidebarContent>
          <AnimatedSidebarFooter className="home-sidebar-footer">
            <PrivacyControl />
            <SidebarThemeControl />
          </AnimatedSidebarFooter>
        </AnimatedSidebar>
        <AnimatedSidebarInset className={`home-inset ${isSplitPane ? 'home-inset-split' : ''}`}>
          <MobileTopBar pathname={pathname} />
          <div className={`home-content-area ${isSplitPane ? 'home-content-split' : ''}`}>
            {children}
          </div>
        </AnimatedSidebarInset>
      </AnimatedSidebarProvider>
    </MotionConfig>
  )
}
