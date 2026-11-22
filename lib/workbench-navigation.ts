import { House, LayoutDashboard, ShieldCheck, Users, Package, Trash2 } from 'lucide-react'

export interface WorkbenchNavChild {
  href: string
  label: string
  tab?: string
  view?: string
}

export interface WorkbenchNavItem {
  href: string
  label: string
  Icon: typeof House
  children?: WorkbenchNavChild[]
}

export const workbenchNavigation: WorkbenchNavItem[] = [
  { href: '/', label: '首页', Icon: House },
  {
    href: '/operations',
    label: '处理操作台',
    Icon: LayoutDashboard,
    children: [
      { href: '/operations?tab=tasks&view=dashboard', label: '批次总览', tab: 'tasks', view: 'dashboard' },
      { href: '/operations?tab=tasks&view=queue', label: '任务队列', tab: 'tasks', view: 'queue' },
      { href: '/operations?tab=nodes', label: '节点视图', tab: 'nodes' },
      { href: '/operations?tab=deploy', label: '部署视图', tab: 'deploy' },
    ],
  },
  { href: '/review', label: '人工确认', Icon: ShieldCheck },
  { href: '/persons', label: '人物库', Icon: Users },
  { href: '/integration', label: '待归档', Icon: Package },
  { href: '/staging', label: '回收站', Icon: Trash2 },
]
