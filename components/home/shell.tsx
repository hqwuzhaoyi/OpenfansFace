'use client'

import Link from 'next/link'
import { usePathname } from 'next/navigation'
import { useEffect, useState } from 'react'
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
  WorkbenchBreadcrumb,
  WorkbenchBreadcrumbProvider,
  WorkbenchHeaderSlotProvider,
  useWorkbenchBreadcrumbItems,
  type WorkbenchBreadcrumbSegment,
} from '@/components/workbench/breadcrumb'
import {
  AnimatedSidebar,
  AnimatedSidebarClose,
  AnimatedSidebarContent,
  AnimatedSidebarFooter,
  AnimatedSidebarGroup,
  AnimatedSidebarGroupContent,
  AnimatedSidebarHeader,
  AnimatedSidebarInset,
  AnimatedSidebarMenu,
  AnimatedSidebarMenuButton,
  AnimatedSidebarMenuItem,
  AnimatedSidebarMenuSub,
  AnimatedSidebarMenuSubButton,
  AnimatedSidebarMenuSubItem,
  AnimatedSidebarProvider,
  AnimatedSidebarRail,
  AnimatedSidebarTrigger,
  useAnimatedSidebar,
} from '@/components/beui/motion/animated-sidebar'
import './home.css'

const navigation = workbenchNavigation
const SIDEBAR_STORAGE_KEY = 'wb-sidebar-open'

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
  const { isMobile, open } = useAnimatedSidebar()
  return (
    <div className="home-sidebar-head">
      <Link className="home-brand" href="/" aria-label="返回首页" title="返回首页">
        <span className="home-brand-mark">
          <Clapperboard size={16} />
        </span>
        <span
          className="home-brand-text group-data-[state=collapsed]/sidebar:pointer-events-none group-data-[state=collapsed]/sidebar:opacity-0"
          aria-hidden={!isMobile && !open}
        >
          OpenfansFace
        </span>
      </Link>
      {isMobile ? (
        <AnimatedSidebarClose aria-label="关闭工作台导航" className="home-mobile-close ml-auto">
          <X size={18} />
        </AnimatedSidebarClose>
      ) : null}
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
  const { isMobile, open } = useAnimatedSidebar()
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
    <Dock
      orientation={open ? 'horizontal' : 'vertical'}
      size={36}
      className={open ? 'home-footer-dock home-footer-dock-expanded' : 'home-footer-dock'}
    >
      {tasks.length > 0 && (
        <>
          <DockItem active={opsOpen}>
            <BackgroundOperationsDock />
          </DockItem>
          <DockSeparator orientation={open ? 'horizontal' : 'vertical'} />
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

function buildFallbackBreadcrumb(pathname: string): WorkbenchBreadcrumbSegment[] {
  if (pathname === '/') {
    return [{ key: 'home', label: '首页', icon: 'home' }]
  }
  if (pathname === '/operations/tasks') {
    return [
      { key: 'home', label: '首页', href: '/', icon: 'home' },
      { key: 'operations', label: '处理操作台', href: '/operations?tab=tasks&view=dashboard' },
      { key: 'ops-queue', label: '任务列表' },
    ]
  }
  if (pathname.startsWith('/persons/')) {
    const raw = pathname.slice('/persons/'.length).split('/')[0] || ''
    let decoded = raw
    try {
      decoded = decodeURIComponent(raw)
    } catch {
      decoded = raw
    }
    return [
      { key: 'home', label: '首页', href: '/', icon: 'home' },
      { key: 'persons', label: '人物库', href: '/persons' },
      ...(decoded ? [{ key: `person:${decoded}`, label: decoded }] : []),
    ]
  }
  const currentNav = navigation.find(n => isNavActive(pathname, n.href))
  if (currentNav) {
    return [
      { key: 'home', label: '首页', href: '/', icon: 'home' },
      { key: currentNav.href, label: currentNav.label },
    ]
  }
  return [
    { key: 'home', label: '首页', href: '/', icon: 'home' },
    { key: 'app', label: 'OpenfansFace' },
  ]
}

function WorkbenchTopBar({
  pathname,
  desktopActionsRef,
  mobileInlineActionsRef,
}: {
  pathname: string
  desktopActionsRef: (node: HTMLDivElement | null) => void
  mobileInlineActionsRef: (node: HTMLDivElement | null) => void
}) {
  const { isMobile, open } = useAnimatedSidebar()
  const { tasks, open: opsOpen } = useBackgroundOperations()
  const syncedItems = useWorkbenchBreadcrumbItems()
  const breadcrumbItems = syncedItems ?? buildFallbackBreadcrumb(pathname)
  const displayBreadcrumbItems =
    isMobile &&
    breadcrumbItems.length > 1 &&
    breadcrumbItems[0]?.key === 'home'
      ? breadcrumbItems.slice(1)
      : breadcrumbItems

  return (
    <header className="home-topbar">
      <div className="home-topbar-left">
        <AnimatedSidebarTrigger
          aria-label={isMobile ? '打开工作台导航' : open ? '收起侧边栏' : '展开侧边栏'}
          title={isMobile ? '打开工作台导航' : open ? '收起侧边栏 (⌘B)' : '展开侧边栏 (⌘B)'}
          className="home-topbar-trigger"
        >
          <PanelLeft size={16} />
        </AnimatedSidebarTrigger>
        {pathname !== '/' && (
          <Link href="/" className="home-topbar-brand-mobile" aria-label="返回首页" title="返回首页">
            <span className="home-brand-mark home-brand-mark-sm">
              <Clapperboard size={15} />
            </span>
          </Link>
        )}
        <span className="home-topbar-divider" aria-hidden="true" />
        <WorkbenchBreadcrumb
          items={displayBreadcrumbItems}
          maxItems={isMobile ? 3 : 4}
          compact={isMobile}
          asHeading
          className="home-topbar-breadcrumb"
        />
      </div>
      <div ref={desktopActionsRef} className="home-topbar-actions" />
      <div className="home-topbar-mobile-dock">
        <div ref={mobileInlineActionsRef} className="home-topbar-mobile-inline" />
        <Dock size={36} className="home-mobile-dock">
          {tasks.length > 0 && (
            <>
              <DockItem active={opsOpen} autoWidth className="w-auto">
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
    </header>
  )
}

function WorkbenchShellFrame({
  pathname,
  isSplitPane,
  openSection,
  setOpenSection,
  operationsState,
  children,
}: {
  pathname: string
  isSplitPane: boolean
  openSection: string | null
  setOpenSection: React.Dispatch<React.SetStateAction<string | null>>
  operationsState: { tab: string; view: string } | null
  children: React.ReactNode
}) {
  const { isMobile } = useAnimatedSidebar()
  const [desktopActionsSlot, setDesktopActionsSlot] = useState<HTMLDivElement | null>(null)
  const [mobileActionsSlot, setMobileActionsSlot] = useState<HTMLDivElement | null>(null)
  const [mobileInlineSlot, setMobileInlineSlot] = useState<HTMLDivElement | null>(null)
  const activeActionsSlot = isMobile ? mobileActionsSlot : desktopActionsSlot

  return (
    <WorkbenchHeaderSlotProvider
      target={activeActionsSlot}
      mobileInlineTarget={mobileInlineSlot}
      isMobile={isMobile}
    >
      <AnimatedSidebar ariaLabel="工作台导航" panelClassName="home-sidebar">
        <AnimatedSidebarHeader className="home-sidebar-header">
          <SidebarHeaderBrand />
        </AnimatedSidebarHeader>
        <AnimatedSidebarContent className="home-sidebar-content">
          <AnimatedSidebarGroup>
            <AnimatedSidebarGroupContent>
              <AnimatedSidebarMenu>
                {navigation.map(n => {
                  const active = isNavActive(pathname, n.href)
                  if (!n.children?.length) {
                    return (
                      <AnimatedSidebarMenuItem key={n.href}>
                        <AnimatedSidebarMenuButton
                          href={n.href}
                          icon={<n.Icon size={17} />}
                          isActive={active}
                        >
                          {n.label}
                        </AnimatedSidebarMenuButton>
                      </AnimatedSidebarMenuItem>
                    )
                  }
                  const sectionOpen = openSection === n.href
                  return (
                    <AnimatedSidebarMenuItem key={n.href}>
                      <AnimatedSidebarMenuButton
                        icon={<n.Icon size={17} />}
                        isActive={active}
                        ariaExpanded={sectionOpen}
                        onSelect={() => setOpenSection(cur => (cur === n.href ? null : n.href))}
                      >
                        {n.label}
                      </AnimatedSidebarMenuButton>
                      <AnimatedSidebarMenuSub open={sectionOpen}>
                        {n.children.map(sub => {
                          const subActive = Boolean(
                            operationsState &&
                              operationsState.tab === (sub.tab || 'tasks') &&
                              (sub.tab !== 'tasks' || operationsState.view === (sub.view || 'dashboard')),
                          )
                          return (
                            <AnimatedSidebarMenuSubItem key={sub.href}>
                              <AnimatedSidebarMenuSubButton
                                href={sub.href}
                                isActive={subActive}
                                onClick={event => {
                                  if (
                                    pathname === '/operations' &&
                                    !event.metaKey &&
                                    !event.ctrlKey &&
                                    !event.shiftKey &&
                                    !event.altKey &&
                                    event.button === 0
                                  ) {
                                    event.preventDefault()
                                    window.history.replaceState(window.history.state, '', sub.href)
                                    window.dispatchEvent(new Event('wb-query-sync'))
                                    window.dispatchEvent(new Event('wb-url-change'))
                                  }
                                }}
                              >
                                {sub.label}
                              </AnimatedSidebarMenuSubButton>
                            </AnimatedSidebarMenuSubItem>
                          )
                        })}
                      </AnimatedSidebarMenuSub>
                    </AnimatedSidebarMenuItem>
                  )
                })}
              </AnimatedSidebarMenu>
            </AnimatedSidebarGroupContent>
          </AnimatedSidebarGroup>
        </AnimatedSidebarContent>
        <AnimatedSidebarFooter className="home-sidebar-footer">
          <SidebarUtilityDock />
        </AnimatedSidebarFooter>
        <AnimatedSidebarRail />
      </AnimatedSidebar>
      <AnimatedSidebarInset className={`home-inset ${isSplitPane ? 'home-inset-split' : ''}`}>
        <WorkbenchTopBar
          pathname={pathname}
          desktopActionsRef={setDesktopActionsSlot}
          mobileInlineActionsRef={setMobileInlineSlot}
        />
        <div ref={setMobileActionsSlot} className="home-mobile-actionbar" />
        <div className={`home-content-area ${isSplitPane ? 'home-content-split' : ''}`}>
          {children}
        </div>
      </AnimatedSidebarInset>
    </WorkbenchHeaderSlotProvider>
  )
}

export function HomeShell({ children }: { children: React.ReactNode }) {
  const pathname = usePathname()
  const isSplitPane =
    pathname === '/review' ||
    pathname === '/operations' ||
    pathname.startsWith('/operations/') ||
    pathname === '/integration' ||
    pathname === '/staging' ||
    pathname === '/persons' ||
    pathname.startsWith('/persons/')
  const [sidebarOpen, setSidebarOpen] = useState(true)
  const [openSection, setOpenSection] = useState<string | null>(() => (pathname.startsWith('/operations') ? '/operations' : null))
  const [search, setSearch] = useState('')

  useEffect(() => {
    try {
      const saved = localStorage.getItem(SIDEBAR_STORAGE_KEY)
      if (saved === '0') setSidebarOpen(false)
      else if (saved === '1') setSidebarOpen(true)
      else if (window.innerWidth < 1100) setSidebarOpen(false)
    } catch {}
  }, [])

  useEffect(() => {
    if (pathname.startsWith('/operations')) {
      setOpenSection('/operations')
    }
  }, [pathname])

  useEffect(() => {
    const syncSearch = () => setSearch(window.location.search)
    syncSearch()
    window.addEventListener('popstate', syncSearch)
    window.addEventListener('wb-url-change', syncSearch)
    window.addEventListener('wb-query-sync', syncSearch)
    return () => {
      window.removeEventListener('popstate', syncSearch)
      window.removeEventListener('wb-url-change', syncSearch)
      window.removeEventListener('wb-query-sync', syncSearch)
    }
  }, [pathname])

  const handleSidebarOpenChange = (next: boolean) => {
    setSidebarOpen(next)
    try {
      localStorage.setItem(SIDEBAR_STORAGE_KEY, next ? '1' : '0')
    } catch {}
  }

  const operationsState = (() => {
    if (!pathname.startsWith('/operations')) return null
    const params = new URLSearchParams(search)
    const tab = params.get('tab') || 'tasks'
    const hasQueueFilter = Boolean(
      params.get('stage') || params.get('batch') || params.get('node_id') || params.get('waiting_reason'),
    )
    const defaultView = pathname === '/operations/tasks' ? 'queue' : 'dashboard'
    const viewParam = params.get('view')
    const view =
      viewParam === 'queue' || viewParam === 'dashboard'
        ? viewParam
        : hasQueueFilter
          ? 'queue'
          : defaultView
    return { tab, view }
  })()

  return (
    <MotionConfig reducedMotion="user">
      <WorkbenchBreadcrumbProvider>
        <AnimatedSidebarProvider
          open={sidebarOpen}
          onOpenChange={handleSidebarOpenChange}
          labels={{
            label: '工作台导航',
            toggle: '展开或收起侧边栏',
            close: '关闭工作台导航',
          }}
          className="home-shell"
          style={{
            '--sidebar-width': '224px',
            '--sidebar-width-icon': '68px',
            '--sidebar-width-mobile': '272px',
          }}
        >
          <WorkbenchShellFrame
            pathname={pathname}
            isSplitPane={isSplitPane}
            openSection={openSection}
            setOpenSection={setOpenSection}
            operationsState={operationsState}
          >
            {children}
          </WorkbenchShellFrame>
        </AnimatedSidebarProvider>
      </WorkbenchBreadcrumbProvider>
    </MotionConfig>
  )
}
