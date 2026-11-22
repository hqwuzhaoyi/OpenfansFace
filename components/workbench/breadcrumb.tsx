'use client'

import Link from 'next/link'
import { createPortal } from 'react-dom'
import { Home } from 'lucide-react'
import { MotionConfig } from 'motion/react'
import {
  createContext,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type MouseEvent,
  type ReactNode,
} from 'react'
import {
  Breadcrumb,
  BreadcrumbItem,
  BreadcrumbLink,
  BreadcrumbList,
  BreadcrumbPage,
  BreadcrumbSeparator,
} from '@/components/beui/motion/breadcrumb'
import { cn } from '@/lib/beui/utils'

export interface WorkbenchBreadcrumbSegment {
  key: string
  label: string
  href?: string
  onClick?: () => void
  icon?: 'home' | ReactNode
  title?: string
}

export interface WorkbenchBreadcrumbProps {
  items: WorkbenchBreadcrumbSegment[]
  maxItems?: number
  compact?: boolean
  asHeading?: boolean
  ariaLabel?: string
  className?: string
}

function useKeyboardInput() {
  const [isKeyboard, setIsKeyboard] = useState(false)

  useEffect(() => {
    setIsKeyboard(document.documentElement.dataset.input === 'keyboard')
    const onKeyDown = () => setIsKeyboard(true)
    const onPointerDown = () => setIsKeyboard(false)
    window.addEventListener('keydown', onKeyDown, true)
    window.addEventListener('pointerdown', onPointerDown, true)
    return () => {
      window.removeEventListener('keydown', onKeyDown, true)
      window.removeEventListener('pointerdown', onPointerDown, true)
    }
  }, [])

  return isKeyboard
}

function renderIcon(icon?: 'home' | ReactNode) {
  if (!icon) return null
  if (icon === 'home') return <Home aria-hidden="true" />
  return icon
}

function isPlainLeftClick(event: MouseEvent<HTMLAnchorElement>) {
  return !(
    event.metaKey ||
    event.ctrlKey ||
    event.shiftKey ||
    event.altKey ||
    event.button !== 0
  )
}

export function WorkbenchBreadcrumb({
  items,
  maxItems,
  compact = false,
  asHeading = !compact,
  ariaLabel = '页面路径',
  className,
}: WorkbenchBreadcrumbProps) {
  const isKeyboard = useKeyboardInput()
  const effectiveMaxItems = maxItems ?? (compact ? 3 : 4)

  return (
    <MotionConfig reducedMotion={isKeyboard ? 'always' : 'user'}>
      <Breadcrumb
        labels={{ label: ariaLabel, showHidden: '展开隐藏路径' }}
        className={cn('wb-breadcrumb', !compact && '-ml-2', className)}
      >
        <BreadcrumbList
          maxItems={effectiveMaxItems}
          overflowLabel="展开隐藏路径"
          className={cn('flex-nowrap', compact ? 'gap-x-0.5 text-[13px]' : 'gap-x-0.5 text-sm')}
        >
          {items.map((item, index) => {
            const isLeaf = index === items.length - 1
            const iconEl = renderIcon(item.icon)

            if (isLeaf) {
              return (
                <BreadcrumbItem key={item.key} className="min-w-0">
                  {index > 0 && <BreadcrumbSeparator />}
                  <BreadcrumbPage
                    title={item.title ?? item.label}
                    className={cn(
                      'min-w-0 max-w-full',
                      compact ? 'min-h-7 px-1.5' : 'px-2',
                    )}
                  >
                    {iconEl}
                    {asHeading ? (
                      <h1
                        className={cn(
                          'wb-breadcrumb-heading m-0 min-w-0 truncate font-semibold tracking-[-0.01em]',
                          compact ? 'max-w-[140px]' : 'max-w-[min(36vw,300px)]',
                        )}
                      >
                        {item.label}
                      </h1>
                    ) : (
                      <span
                        className={cn(
                          'min-w-0 truncate font-medium',
                          compact ? 'max-w-[140px]' : 'max-w-[min(36vw,300px)]',
                        )}
                      >
                        {item.label}
                      </span>
                    )}
                  </BreadcrumbPage>
                </BreadcrumbItem>
              )
            }

            const hasNav = Boolean(item.href || item.onClick)
            const linkClassName = cn(compact && 'min-h-7 px-1.5')
            const handleClick = item.onClick
              ? (event: MouseEvent<HTMLAnchorElement>) => {
                  if (!isPlainLeftClick(event)) return
                  event.preventDefault()
                  item.onClick?.()
                }
              : undefined

            return (
              <BreadcrumbItem key={item.key} className="shrink-0">
                {index > 0 && <BreadcrumbSeparator />}
                {hasNav ? (
                  <BreadcrumbLink
                    href={item.href ?? '#'}
                    title={item.title}
                    className={linkClassName}
                    onClick={handleClick}
                    render={
                      item.href
                        ? (linkProps) => <Link {...linkProps} href={item.href!} />
                        : undefined
                    }
                  >
                    {iconEl}
                    <span className="truncate max-w-[160px]">{item.label}</span>
                  </BreadcrumbLink>
                ) : (
                  <span
                    title={item.title}
                    className={cn(
                      'inline-flex min-h-8 min-w-0 items-center gap-1.5 px-2 font-medium text-muted-foreground [&>svg]:size-3.5 [&>svg]:shrink-0',
                      compact && 'min-h-7 px-1.5',
                    )}
                  >
                    {iconEl}
                    <span className="truncate max-w-[160px]">{item.label}</span>
                  </span>
                )}
              </BreadcrumbItem>
            )
          })}
        </BreadcrumbList>
      </Breadcrumb>
    </MotionConfig>
  )
}

interface SyncedBreadcrumbRecord {
  owner: symbol
  fingerprint: string
  items: WorkbenchBreadcrumbSegment[]
}

interface BreadcrumbActionsValue {
  setSynced: React.Dispatch<React.SetStateAction<SyncedBreadcrumbRecord | null>>
  activeOwnerRef: React.MutableRefObject<symbol | null>
  handlersRef: React.MutableRefObject<Map<string, () => void>>
}

const BreadcrumbItemsContext = createContext<SyncedBreadcrumbRecord | null>(null)
const BreadcrumbActionsContext = createContext<BreadcrumbActionsValue | null>(null)
const WorkbenchHeaderSlotContext = createContext<HTMLDivElement | null>(null)

export function WorkbenchHeaderSlotProvider({
  target,
  children,
}: {
  target: HTMLDivElement | null
  children: ReactNode
}) {
  return (
    <WorkbenchHeaderSlotContext.Provider value={target}>
      {children}
    </WorkbenchHeaderSlotContext.Provider>
  )
}

export function WorkbenchHeaderActions({ children }: { children: ReactNode }) {
  const target = useContext(WorkbenchHeaderSlotContext)
  if (!target) return null
  return createPortal(children, target)
}

export function WorkbenchBreadcrumbProvider({ children }: { children: ReactNode }) {
  const [synced, setSynced] = useState<SyncedBreadcrumbRecord | null>(null)
  const activeOwnerRef = useRef<symbol | null>(null)
  const handlersRef = useRef<Map<string, () => void>>(new Map())

  const actions = useMemo<BreadcrumbActionsValue>(
    () => ({ setSynced, activeOwnerRef, handlersRef }),
    [],
  )

  return (
    <BreadcrumbActionsContext.Provider value={actions}>
      <BreadcrumbItemsContext.Provider value={synced}>
        {children}
      </BreadcrumbItemsContext.Provider>
    </BreadcrumbActionsContext.Provider>
  )
}

export function useWorkbenchBreadcrumbItems(): WorkbenchBreadcrumbSegment[] | null {
  return useContext(BreadcrumbItemsContext)?.items ?? null
}

function buildFingerprint(items: WorkbenchBreadcrumbSegment[]) {
  return items
    .map((item) =>
      [
        item.key,
        item.label,
        item.href ?? '',
        item.onClick ? '1' : '0',
        typeof item.icon === 'string' ? item.icon : '',
        item.title ?? '',
      ].join('\u0000'),
    )
    .join('\u0001')
}

export function useSyncWorkbenchBreadcrumb(items: WorkbenchBreadcrumbSegment[]) {
  const actions = useContext(BreadcrumbActionsContext)
  const [owner] = useState(() => Symbol('wb-breadcrumb'))
  const fingerprint = buildFingerprint(items)
  const itemsRef = useRef(items)
  itemsRef.current = items

  useEffect(() => {
    if (!actions) return
    if (actions.activeOwnerRef.current === owner) {
      for (const item of items) {
        if (item.onClick) actions.handlersRef.current.set(item.key, item.onClick)
        else actions.handlersRef.current.delete(item.key)
      }
    }
  })

  useEffect(() => {
    if (!actions) return
    const currentItems = itemsRef.current
    actions.activeOwnerRef.current = owner
    actions.handlersRef.current.clear()
    for (const item of currentItems) {
      if (item.onClick) actions.handlersRef.current.set(item.key, item.onClick)
    }

    const proxiedItems: WorkbenchBreadcrumbSegment[] = currentItems.map((item) => ({
      key: item.key,
      label: item.label,
      href: item.href,
      icon: typeof item.icon === 'string' ? item.icon : undefined,
      title: item.title,
      onClick: item.onClick
        ? () => {
            actions.handlersRef.current.get(item.key)?.()
          }
        : undefined,
    }))

    actions.setSynced((prev) =>
      prev && prev.owner === owner && prev.fingerprint === fingerprint
        ? prev
        : { owner, fingerprint, items: proxiedItems },
    )
  }, [actions, owner, fingerprint])

  useEffect(() => {
    if (!actions) return
    return () => {
      if (actions.activeOwnerRef.current === owner) {
        actions.activeOwnerRef.current = null
        actions.handlersRef.current.clear()
        actions.setSynced(null)
      }
    }
  }, [actions, owner])
}

export const useSyncMobileBreadcrumb = useSyncWorkbenchBreadcrumb
