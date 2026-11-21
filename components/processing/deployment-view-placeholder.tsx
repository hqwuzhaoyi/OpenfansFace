'use client'

import { WorkspaceCard } from '@/components/workbench/inner'
import { Package, ShieldCheck, Cpu } from 'lucide-react'

export default function DeploymentViewPlaceholder() {
  return (
    <div style={{ maxWidth: 1700, margin: '0 auto', padding: 24 }}>
      <WorkspaceCard style={{ padding: 32, textAlign: 'center' }}>
        <div
          style={{
            display: 'inline-flex',
            padding: 16,
            borderRadius: 12,
            background: 'var(--muted)',
            marginBottom: 16,
            color: 'var(--foreground)',
          }}
        >
          <Package size={32} />
        </div>
        <h3 style={{ fontSize: 18, fontWeight: 600, marginBottom: 8 }}>
          受限 CUDA 运行包部署视图
        </h3>
        <p style={{ fontSize: 13, color: 'var(--muted-foreground)', maxWidth: 500, margin: '0 auto 16px' }}>
          Milestone 2 (#22) 运行包分发、非特权沙箱与启动基准自检视图。当前阶段：Milestone 1 Agent 注册与节点视图已就绪。
        </p>
        <div
          style={{
            display: 'inline-flex',
            alignItems: 'center',
            gap: 16,
            fontSize: 12,
            color: 'var(--muted-foreground)',
          }}
        >
          <span style={{ display: 'inline-flex', alignItems: 'center', gap: 4 }}>
            <Cpu size={14} /> 模型就绪探测
          </span>
          <span>·</span>
          <span style={{ display: 'inline-flex', alignItems: 'center', gap: 4 }}>
            <ShieldCheck size={14} /> SHA-256 清单核对
          </span>
        </div>
      </WorkspaceCard>
    </div>
  )
}
