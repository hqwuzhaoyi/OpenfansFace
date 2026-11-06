"use client"

import Link from "next/link"
import { useEffect } from "react"
import { usePathname } from "next/navigation"
import {
  LayoutDashboard, Layers, Users, ShieldCheck, Package, Trash2, Clapperboard,
} from "lucide-react"

const NAV = [
  { href: "/", label: "总览", icon: LayoutDashboard },
  { href: "/unclassified", label: "待分类", icon: Layers },
  { href: "/persons", label: "人物", icon: Users },
  { href: "/approvals", label: "识别待确认", icon: ShieldCheck },
  { href: "/integration", label: "待归档", icon: Package },
  { href: "/staging", label: "回收站", icon: Trash2 },
]

export default function WorkbenchLayout({ children }: { children: React.ReactNode }) {
  const pathname = usePathname()
  useEffect(() => {
    const keyboard = () => { document.documentElement.dataset.input = "keyboard" }
    const pointer = () => { document.documentElement.dataset.input = "pointer" }
    window.addEventListener("keydown", keyboard, true)
    window.addEventListener("pointerdown", pointer, true)
    return () => { window.removeEventListener("keydown", keyboard, true); window.removeEventListener("pointerdown", pointer, true) }
  }, [])
  if (pathname === "/integration") return (
    <div className="flex h-dvh overflow-hidden bg-background text-foreground">
      <aside aria-label="工作台导航" className="hidden w-14 shrink-0 flex-col items-center gap-4 border-r bg-sidebar py-3 md:flex">
        <Link href="/" title="OpenfansFace" aria-label="OpenfansFace总览" className="grid size-9 place-items-center rounded-lg bg-foreground text-background"><Clapperboard className="size-5" /></Link>
        <nav className="flex flex-col gap-1">
          {NAV.map(n => <Link key={n.href} href={n.href} title={n.label} aria-label={n.label} aria-current={pathname === n.href ? "page" : undefined} className={`grid size-10 place-items-center rounded-md outline-none focus-visible:ring-2 focus-visible:ring-ring ${pathname === n.href ? "bg-accent text-foreground" : "text-muted-foreground hover:bg-accent hover:text-foreground"}`}><n.icon className="size-[18px]" /></Link>)}
        </nav>
      </aside>
      <div className="flex min-h-0 min-w-0 flex-1 flex-col">
        <div className="min-h-0 flex-1">{children}</div>
        <nav aria-label="工作台导航" className="flex shrink-0 border-t bg-sidebar pb-[env(safe-area-inset-bottom)] md:hidden">
          {NAV.map(n => <Link key={n.href} href={n.href} aria-current={pathname === n.href ? "page" : undefined} className={`flex min-h-12 min-w-0 flex-1 flex-col items-center justify-center gap-1 text-[10px] ${pathname === n.href ? "text-foreground" : "text-muted-foreground"}`}><n.icon className="size-[18px]" />{n.label}</Link>)}
        </nav>
      </div>
    </div>
  )
  return (
    <div className="flex min-h-dvh bg-background text-foreground">
      <aside className="fixed inset-y-0 left-0 z-10 hidden w-60 flex-col border-r bg-sidebar md:flex">
        <div className="flex h-14 items-center gap-2 border-b px-4">
          <Clapperboard className="h-5 w-5" />
          <span className="font-bold">OpenfansFace</span>
        </div>
        <nav className="flex-1 space-y-1 p-2">
          <p className="px-2 pb-1 text-xs font-medium text-muted-foreground">策展</p>
          {NAV.map((n) => {
            const active = pathname === n.href || (n.href !== "/" && pathname.startsWith(n.href))
            return (
              <Link
                key={n.href}
                href={n.href}
                className={`flex items-center gap-2 rounded-md px-2 py-1.5 text-sm ${
                  active ? "bg-accent text-accent-foreground font-medium" : "text-muted-foreground hover:text-foreground"
                }`}
              >
                <n.icon className="h-4 w-4" />
                {n.label}
              </Link>
            )
          })}
        </nav>
      </aside>
      <div className="flex-1 md:pl-60">
        <header className="flex h-14 items-center gap-3 border-b px-4 md:hidden">
          <Clapperboard className="h-5 w-5" />
          <span className="font-bold">OpenfansFace</span>
        </header>
        <div className="pb-24 md:pb-0">{children}</div>
      </div>
      {/* 移动端底部导航：≥768px 隐藏，桌面侧栏接管 */}
      <nav className="fixed inset-x-0 bottom-0 z-20 flex pb-[env(safe-area-inset-bottom)] border-t bg-sidebar/95 backdrop-blur md:hidden">
        {NAV.map((n) => {
          const active = pathname === n.href || (n.href !== "/" && pathname.startsWith(n.href))
          return (
            <Link
              key={n.href}
              href={n.href}
              className={`flex flex-1 flex-col items-center gap-0.5 py-2 text-[10px] leading-none ${
                active ? "text-accent-foreground font-medium" : "text-muted-foreground"
              }`}
            >
              <n.icon className={`h-5 w-5 ${active ? "text-foreground" : ""}`} />
              {n.label}
            </Link>
          )
        })}
      </nav>
    </div>
  )
}
