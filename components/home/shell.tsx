'use client'

import Link from 'next/link'
import { usePathname } from 'next/navigation'
import { Clapperboard, Monitor, Moon, PanelLeft, Sun, X, Eye, EyeOff } from 'lucide-react'
import { Button } from '@/components/beui/motion/button/base'
import { Tabs, TabsList, TabsTrigger } from '@/components/beui/motion/tabs'
import { Dock, DockItem, DockSeparator } from '@/components/beui/motion/dock'
import { ThemeToggle } from '@/components/beui/motion/theme-toggle'
import { usePrivacy } from '@/components/privacy-provider'
import { MotionConfig } from 'motion/react'
import { workbenchNavigation } from '@/lib/workbench-navigation'
import { useTheme, type ThemeMode } from '@/components/theme-provider'
import { BackgroundOperationsDock, useBackgroundOperations } from '@/components/workbench/background-operations'
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

function MobileDrawerThemeTabs() {
  const { mode, setMode } = useTheme()
  return (
    <Tabs
      value={mode}
      onValueChange={(next) => setMode(next as ThemeMode)}
      variant="segment"
      className="home-theme-mobile-tabs"
    >
      <TabsList aria-label="界面主题" className="home-theme-mobile-list">
        {THEME_OPTIONS.map(({ mode: itemMode, label, Icon }) => (
          <TabsTrigger
            key={itemMode}
            value={itemMode}
            icon={<Icon size={14} />}
            className="home-theme-seg"
          >
            <span>{label}</span>
          </TabsTrigger>
        ))}
      </TabsList>
    </Tabs>
  )
}

function PrivacyControl({ mobile = false }: { mobile?: boolean }) {
  const { enabled, ready, setEnabled } = usePrivacy()
  const label = `隐私模式：${enabled ? '已开启' : '已关闭'}`
  return <Button variant="ghost" size="icon" whileTap={undefined} disabled={!ready} aria-label={label} title={`${label}，点击${enabled ? '关闭' : '开启'}`} aria-pressed={enabled} onClick={() => setEnabled(!enabled)} className={`privacy-control ${mobile ? 'privacy-control-mobile' : ''}`}>
    {enabled ? <EyeOff size={17} /> : <Eye size={17} />}
  </Button>
}

function SidebarUtilityDock() {
  const { isMobile } = useAnimatedSidebar()
  const { tasks, open: opsOpen } = useBackgroundOperations()

  if (isMobile) {
    return (
      <>
        <PrivacyControl />
        <MobileDrawerThemeTabs />
      </>
    )
  }

  return (
    <Dock orientation="vertical" size={36} className="home-footer-dock">
      {tasks.length > 0 && (
        <>
          <DockItem active={opsOpen}>
            <BackgroundOperationsDock />
          </DockItem>
          <DockSeparator orientation="vertical" />
        </>
      )}
      <DockItem>
        <PrivacyControl />
      </DockItem>
      <DockItem>
        <ThemeToggle
          variant="rectangle"
          start="bottom-left"
          labels={{ switchToLight: '切换为浅色主题', switchToDark: '切换为深色主题' }}
          className="home-theme-trigger"
          iconClassName="size-[17px]"
        />
      </DockItem>
    </Dock>
  )
}

function MobileTopBar({ pathname }: { pathname: string }) {
  const { isMobile } = useAnimatedSidebar()
  const { tasks, open: opsOpen } = useBackgroundOperations()
  if (!isMobile) return null
  const currentNav = navigation.find(n => isNavActive(pathname, n.href))

  return (
    <div className="home-mobile-nav">
      <AnimatedSidebarTrigger aria-label="打开工作台导航" className="home-mobile-trigger">
        <PanelLeft size={18} />
      </AnimatedSidebarTrigger>
      <Link href="/" className="home-mobile-brand" aria-label="返回首页" title="返回首页">
        <span className="home-brand-mark home-brand-mark-sm">
          <Clapperboard size={15} />
        </span>
      </Link>
      <span className="home-mobile-current">{currentNav ? currentNav.label : 'OpenfansFace'}</span>
      <div className="home-mobile-actions">
        <Dock size={36} className="home-mobile-dock">
          {tasks.length > 0 && (
            <>
              <DockItem active={opsOpen} className="w-auto">
                <BackgroundOperationsDock mobile />
              </DockItem>
              <DockSeparator />
            </>
          )}
          <DockItem>
            <PrivacyControl mobile />
          </DockItem>
          <DockItem>
            <ThemeToggle
              variant="rectangle"
              start="top-right"
              labels={{ switchToLight: '切换为浅色主题', switchToDark: '切换为深色主题' }}
              className="home-mobile-theme"
              iconClassName="size-[17px]"
            />
          </DockItem>
        </Dock>
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
            <SidebarUtilityDock />
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
