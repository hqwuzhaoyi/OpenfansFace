import { House, LayoutDashboard, ShieldCheck, Users, Package, Trash2 } from 'lucide-react'

export const workbenchNavigation = [
  { href: '/', label: '首页', Icon: House },
  { href: '/operations', label: '处理操作台', Icon: LayoutDashboard },
  { href: '/review', label: '人工确认', Icon: ShieldCheck },
  { href: '/persons', label: '人物库', Icon: Users },
  { href: '/integration', label: '待归档', Icon: Package },
  { href: '/staging', label: '回收站', Icon: Trash2 },
]
