'use client'

import { memo } from 'react'
import { Radio, Cpu, Activity, Server, WifiOff } from 'lucide-react'
import { AnimatedBadge } from '@/components/beui/motion/animated-badge'
import type { NodeLifecycleStatus } from '@/lib/processing-node-model'

export interface NodeStatusBadgeProps {
  status: NodeLifecycleStatus
  size?: 'sm' | 'md'
}

export const NodeStatusBadge = memo(function NodeStatusBadge({
  status,
  size = 'sm',
}: NodeStatusBadgeProps) {
  switch (status) {
    case 'accepting':
      return (
        <AnimatedBadge
          status="success"
          size={size}
          pulse={true}
          icon={<Radio size={12} className="animate-pulse" />}
        >
          可接单
        </AnimatedBadge>
      )
    case 'model_ready':
      return (
        <AnimatedBadge
          status="info"
          size={size}
          icon={<Cpu size={12} />}
        >
          模型就绪
        </AnimatedBadge>
      )
    case 'online':
      return (
        <AnimatedBadge
          status="warning"
          size={size}
          icon={<Activity size={12} />}
        >
          在线
        </AnimatedBadge>
      )
    case 'registered':
      return (
        <AnimatedBadge
          status="neutral"
          size={size}
          icon={<Server size={12} />}
        >
          已登记
        </AnimatedBadge>
      )
    case 'offline':
    default:
      return (
        <AnimatedBadge
          status="danger"
          size={size}
          icon={<WifiOff size={12} />}
        >
          已失联
        </AnimatedBadge>
      )
  }
})
