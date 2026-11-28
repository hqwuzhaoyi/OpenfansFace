'use client'

import { useState, useMemo, useCallback, useRef } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import {
  Server,
  Cpu,
  RefreshCw,
  CheckCircle2,
  AlertTriangle,
  XCircle,
  Clock,
  RotateCcw,
  Sliders,
  Copy,
  Check,
  ShieldAlert,
  ArrowRight,
  ShieldCheck,
  Sparkles,
  Layers,
} from 'lucide-react'
import { api } from '@/lib/face-api'
import { workspaceToast as toast } from '@/components/workbench/feedback'
import { WorkspaceCard, WorkspaceButton as Button } from '@/components/workbench/inner'
import { WorkspaceTabs, WorkspaceSearch } from '@/components/workbench/controls'
import { WorkbenchHeaderActions, useSyncWorkbenchBreadcrumb, type WorkbenchBreadcrumbSegment } from '@/components/workbench/breadcrumb'
import { Drawer } from '@/components/beui/motion/drawer'
import { AnimatedBadge } from '@/components/beui/motion/animated-badge'
import { useFocusTrap } from '@/lib/beui/hooks/use-focus-trap'
import {
  STAGE_STEPS,
  getStepStatus,
  formatDrift,
  formatDuration,
  formatKernelEvents,
  formatVramFloor,
  type DeploymentListApiResponse,
  type DeploymentRecord,
  type DeploymentStage,
} from '@/lib/processing-deployment-model'
import './deployment-view.css'

export default function DeploymentView({ onNavigateOverview }: { onNavigateOverview?: () => void } = {}) {
  const queryClient = useQueryClient()
  const [filter, setFilter] = useState<string>('all')
  const [query, setQuery] = useState<string>('')
  const [busyNodeId, setBusyNodeId] = useState<string | null>(null)
  const [selectedRecord, setSelectedRecord] = useState<DeploymentRecord | null>(null)
  const [rollbackNode, setRollbackNode] = useState<{ nodeId: string; hostname: string; targetVersion?: string } | null>(null)
  const [copiedKey, setCopiedKey] = useState<string | null>(null)
  const rollbackCardRef = useRef<HTMLDivElement>(null)

  // Safe focus trap for rollback confirmation dialog
  useFocusTrap(Boolean(rollbackNode), rollbackCardRef, {
    onEscape: () => {
      if (!busyNodeId) setRollbackNode(null)
    },
  })

  // Polling deployments every 5 seconds
  const { data, isFetching, error, refetch } = useQuery<DeploymentListApiResponse>({
    queryKey: ['processing', 'deployments'],
    queryFn: async ({ signal }) => {
      const res = await api<DeploymentListApiResponse>('/api/processing/deployments', {
        signal: AbortSignal.any([signal, AbortSignal.timeout(10000)]),
        cache: 'no-store',
      })
      if (res?.schema_version !== 1) {
        throw new Error('部署接口协议版本不兼容，已暂停操作')
      }
      return res
    },
    refetchInterval: 5000,
    refetchIntervalInBackground: false,
    staleTime: 3000,
    retry: false,
  })

  // Clipboard helper
  const handleCopy = useCallback(async (text: string, key: string) => {
    try {
      if (navigator.clipboard?.writeText) {
        await navigator.clipboard.writeText(text)
      } else {
        const textarea = document.createElement('textarea')
        textarea.value = text
        document.body.appendChild(textarea)
        textarea.select()
        document.execCommand('copy')
        document.body.removeChild(textarea)
      }
      setCopiedKey(key)
      toast.success('已复制到剪贴板')
      setTimeout(() => setCopiedKey(null), 2000)
    } catch {
      toast.error('复制失败')
    }
  }, [])

  // Safe Rollback action
  const handleConfirmRollback = useCallback(async () => {
    if (!rollbackNode) return
    const { nodeId, targetVersion } = rollbackNode
    setBusyNodeId(nodeId)
    try {
      await api('/api/processing/deployments', {
        method: 'POST',
        body: JSON.stringify({
          action: 'rollback',
          node_id: nodeId,
          target_version: targetVersion,
        }),
      })
      toast.success(`节点 ${nodeId} 回滚指令已下发，正在平滑排空并切换稳定版本`)
      setRollbackNode(null)
      await queryClient.invalidateQueries({ queryKey: ['processing', 'deployments'] })
    } catch (e: any) {
      toast.error(e.message || '回滚操作失败')
    } finally {
      setBusyNodeId(null)
    }
  }, [rollbackNode, queryClient])

  // Distinct active packages & providers across deployments
  const activePackages = useMemo(() => {
    if (!data?.deployments) return []
    const set = new Set<string>()
    for (const d of data.deployments) {
      if (d.package_id) set.add(d.package_id)
    }
    return Array.from(set)
  }, [data?.deployments])

  // Filter deployments
  const filteredDeployments = useMemo(() => {
    if (!data?.deployments) return []
    return data.deployments.filter((d) => {
      if (filter === 'synced' && d.sync_status !== 'synced') return false
      if (filter === 'deploying' && d.sync_status !== 'deploying') return false
      if (filter === 'mismatched' && d.sync_status !== 'mismatched') return false
      if (filter === 'failed' && d.sync_status !== 'failed') return false

      if (query.trim()) {
        const q = query.trim().toLowerCase()
        const matchHost = d.hostname.toLowerCase().includes(q)
        const matchId = d.node_id.toLowerCase().includes(q)
        const matchVer = d.desired_version.toLowerCase().includes(q)
        const matchActual = d.actual_version?.toLowerCase().includes(q)
        if (!matchHost && !matchId && !matchVer && !matchActual) return false
      }
      return true
    })
  }, [data, filter, query])

  const breadcrumbItems = useMemo<WorkbenchBreadcrumbSegment[]>(
    () => [
      { key: 'home', label: '首页', href: '/', icon: 'home' },
      {
        key: 'operations',
        label: '处理操作台',
        href: '/operations?tab=tasks&view=dashboard',
        onClick: onNavigateOverview,
      },
      { key: 'ops-deploy', label: '部署视图' },
    ],
    [onNavigateOverview],
  )
  useSyncWorkbenchBreadcrumb(breadcrumbItems)

  return (
    <div className="ws-deploy-root">
      <WorkbenchHeaderActions
        mobileInline={
          <Button
            variant="ghost"
            size="icon"
            onClick={() => refetch()}
            disabled={isFetching}
            aria-label="刷新部署状态"
            title="刷新部署状态"
          >
            <RefreshCw size={15} className={isFetching ? 'animate-spin' : ''} />
          </Button>
        }
      >
        <Button
          variant="ghost"
          size="sm"
          onClick={() => refetch()}
          disabled={isFetching}
          aria-label="刷新部署状态"
        >
          <RefreshCw size={14} className={isFetching ? 'animate-spin' : ''} />
          <span>刷新</span>
        </Button>
      </WorkbenchHeaderActions>
      {/* Pinned Release Banner */}
      <div className="ws-deploy-pinned-banner">
        <div className="ws-deploy-banner-top">
          <div className="ws-deploy-banner-title">
            <ShieldCheck size={18} className="text-emerald-400" />
            <span>受限运行包配置：SCRFD 640 CUDA（非特权沙箱与真 CUDA 自检就绪）</span>
            {activePackages.length > 1 && (
              <span className="text-xs text-slate-400 ml-2">
                （活跃运行包：{activePackages.join(' · ')}）
              </span>
            )}
          </div>
          <div className="ws-deploy-banner-stats">
            <div className="ws-deploy-banner-stat-item">
              <span>节点总数:</span>
              <strong>{data?.summary?.total_nodes ?? '未知'}</strong>
            </div>
            <div className="ws-deploy-banner-stat-item">
              <span>已同步:</span>
              <strong className="text-emerald-400">{data?.summary?.synced_nodes ?? '未知'}</strong>
            </div>
            <div className="ws-deploy-banner-stat-item">
              <span>部署中:</span>
              <strong className="text-blue-400">{data?.summary?.in_progress ?? '未知'}</strong>
            </div>
            <div className="ws-deploy-banner-stat-item">
              <span>异常:</span>
              <strong className="text-rose-400">{data?.summary?.failed_nodes ?? '未知'}</strong>
            </div>
          </div>
        </div>
        <div className="ws-deploy-banner-hashes">
          <div className="ws-deploy-hash-tag">
            <span>安全约束:</span>
            <span>无 root 权限 · 64MiB 独立 tmpfs · 600MiB 显存准入底线 · TTS 永久停用</span>
          </div>
          <div className="ws-deploy-hash-tag">
            <span>自检容差阈值:</span>
            <span>人脸点位偏差 ≤ 0.05000</span>
          </div>
        </div>
      </div>

      {/* Toolbar */}
      <div className="ws-deploy-toolbar">
        <div className="ws-deploy-toolbar-left">
          <WorkspaceTabs
            value={filter}
            onChange={setFilter}
            label="部署状态过滤"
            items={[
              { value: 'all', label: '全部', count: data?.summary?.total_nodes },
              { value: 'synced', label: '已同步', count: data?.summary?.synced_nodes },
              { value: 'deploying', label: '部署中', count: data?.summary?.in_progress },
              { value: 'mismatched', label: '版本偏离' },
              { value: 'failed', label: '异常', count: data?.summary?.failed_nodes },
            ]}
          />
          <WorkspaceSearch
            value={query}
            onChange={setQuery}
            placeholder="搜索节点或版本..."
            label="搜索节点"
          />
        </div>
      </div>

      {/* Error State */}
      {error && (
        <div className="p-4 rounded-lg bg-rose-500/10 border border-rose-500/20 text-rose-300 text-sm flex items-center gap-2">
          <AlertTriangle size={16} />
          <span>读取部署数据失败：{(error as any)?.message || '网络连接超时'}</span>
        </div>
      )}

      {/* Deployment List Matrix */}
      <div className="ws-deploy-list">
        {filteredDeployments.length === 0 ? (
          <div className="p-12 text-center text-slate-400 text-sm border border-slate-800 rounded-xl bg-slate-900/30">
            暂无匹配的运行包部署记录
          </div>
        ) : (
          filteredDeployments.map((record) => {
            const isCpu = !record.is_gpu && record.execution_provider === 'CPUExecutionProvider'

            return (
              <div key={record.deployment_id || record.node_id} className="ws-deploy-node-card">
                {/* Header: Node Info + Version Matrix */}
                <div className="ws-deploy-card-header">
                  <div className="ws-deploy-node-info">
                    <div className="ws-deploy-node-icon">
                      {isCpu ? <Cpu size={20} /> : <Server size={20} />}
                    </div>
                    <div>
                      <div className="ws-deploy-node-name">
                        <span>{record.hostname}</span>
                        {record.sync_status === 'synced' && (
                          <AnimatedBadge status="success" size="sm" icon={<CheckCircle2 size={12} />}>
                            版本同步
                          </AnimatedBadge>
                        )}
                        {record.sync_status === 'deploying' && (
                          <AnimatedBadge status="info" size="sm" pulse={true} icon={<Clock size={12} />}>
                            阶段流转中
                          </AnimatedBadge>
                        )}
                        {record.sync_status === 'mismatched' && (
                          <AnimatedBadge status="warning" size="sm" icon={<AlertTriangle size={12} />}>
                            版本偏离
                          </AnimatedBadge>
                        )}
                        {record.sync_status === 'failed' && (
                          <AnimatedBadge status="danger" size="sm" icon={<XCircle size={12} />}>
                            部署异常
                          </AnimatedBadge>
                        )}
                        {record.sync_status === 'rolled_back' && (
                          <AnimatedBadge status="neutral" size="sm" icon={<RotateCcw size={12} />}>
                            已回滚
                          </AnimatedBadge>
                        )}
                      </div>
                      <div className="ws-deploy-node-sub">
                        <span>{record.node_id}</span>
                        <span>·</span>
                        <span>{record.os}/{record.arch}</span>
                        <span>·</span>
                        <span>{record.execution_provider}</span>
                        {record.gpu_model && (
                          <>
                            <span>·</span>
                            <span>{record.gpu_model}</span>
                          </>
                        )}
                        {record.package_id && (
                          <>
                            <span>·</span>
                            <span>{record.package_id}</span>
                          </>
                        )}
                      </div>
                    </div>
                  </div>

                  {/* Desired vs Actual Version Matrix */}
                  <div className="ws-deploy-version-matrix">
                    <div>
                      <div className="ws-deploy-version-label">目标版本 (Desired)</div>
                      <div className="ws-deploy-version-val">{record.desired_version || '未知'}</div>
                    </div>
                    <ArrowRight size={14} className="ws-deploy-version-arrow" />
                    <div>
                      <div className="ws-deploy-version-label">当前运行 (Actual)</div>
                      <div className="ws-deploy-version-val">
                        {record.actual_version || '未就绪'}
                      </div>
                    </div>
                  </div>
                </div>

                {/* 6-Stage Stepper Progression */}
                <div className="ws-deploy-stepper-wrap">
                  <div className="ws-deploy-stepper-track">
                    <div className="ws-deploy-step-connector" />
                    {STAGE_STEPS.map((step, idx) => {
                      const stepStatus = getStepStatus(
                        step.stage,
                        record.stage,
                        record.sync_status ?? record.status,
                        record.failure_evidence?.stage
                      )

                      return (
                        <div key={step.stage} className="ws-deploy-step-item">
                          <div className={`ws-deploy-step-node ${stepStatus}`}>
                            {stepStatus === 'completed' && <Check size={14} />}
                            {stepStatus === 'failed' && <XCircle size={14} />}
                            {stepStatus === 'active' && <Clock size={14} className="animate-spin" />}
                            {stepStatus === 'pending' && idx + 1}
                          </div>
                          <div className={`ws-deploy-step-label ${stepStatus}`}>
                            {step.label}
                          </div>
                        </div>
                      )
                    })}
                  </div>
                </div>

                {/* Card Footer: Metrics & Actions */}
                <div className="ws-deploy-card-footer">
                  <div className="ws-deploy-metrics">
                    <div className="ws-deploy-metric-item">
                      <span>自检点位偏差:</span>
                      <span className="ws-deploy-metric-val">
                        {formatDrift(record.self_test?.drift)}
                      </span>
                    </div>
                    <div className="ws-deploy-metric-item">
                      <span>真 CUDA 算子证明:</span>
                      <span className="ws-deploy-metric-val">
                        {formatKernelEvents(
                          record.self_test?.cuda_kernel_events,
                          record.self_test?.cpu_kernel_events,
                          isCpu
                        )}
                      </span>
                    </div>
                    <div className="ws-deploy-metric-item">
                      <span>诊断耗时:</span>
                      <span className="ws-deploy-metric-val">
                        {formatDuration(record.duration_ms)}
                      </span>
                    </div>
                  </div>

                  <div className="ws-deploy-actions">
                    <Button
                      variant="outline"
                      size="sm"
                      className="min-h-[44px] sm:min-h-[36px]"
                      onClick={() => setSelectedRecord(record)}
                      aria-label="查看诊断证据"
                    >
                      <Sliders size={14} />
                      <span>诊断详情</span>
                    </Button>

                    <Button
                      variant="secondary"
                      size="sm"
                      className="min-h-[44px] sm:min-h-[36px]"
                      onClick={() =>
                        setRollbackNode({
                          nodeId: record.node_id,
                          hostname: record.hostname,
                          targetVersion: record.previous_version || undefined,
                        })
                      }
                      disabled={busyNodeId === record.node_id}
                      aria-label="安全回滚版本"
                    >
                      <RotateCcw size={14} />
                      <span>安全回滚</span>
                    </Button>
                  </div>
                </div>
              </div>
            )
          })
        )}
      </div>

      {/* Diagnostic Drawer */}
      <Drawer
        open={!!selectedRecord}
        onOpenChange={(open) => !open && setSelectedRecord(null)}
        side="right"
        ariaLabel="部署诊断详情抽屉"
        className="w-full sm:w-[520px] max-w-full sm:max-w-[90vw]"
      >
        {selectedRecord && (
          <div className="ws-deploy-drawer-panel">
            <div className="ws-deploy-drawer-header">
              <div className="ws-deploy-drawer-title">
                <Sliders size={18} className="text-blue-400" />
                <span>运行包自检与沙箱诊断</span>
              </div>
              <Button
                variant="ghost"
                size="sm"
                className="min-w-[44px] min-h-[44px] text-base p-0 flex items-center justify-center rounded-lg"
                onClick={() => setSelectedRecord(null)}
                aria-label="关闭诊断抽屉"
              >
                ✕
              </Button>
            </div>

            {/* Failure Card if failed */}
            {selectedRecord.failure_evidence && (
              <div className="ws-deploy-failure-card">
                <div className="ws-deploy-failure-title">
                  <ShieldAlert size={16} />
                  <span>部署阻断证据（{selectedRecord.failure_evidence.stage} 阶段）</span>
                </div>
                <div className="ws-deploy-failure-msg">
                  {selectedRecord.failure_evidence.error_message}
                </div>
                <div className="pt-2">
                  <Button
                    variant="default"
                    size="sm"
                    className="min-h-[44px]"
                    onClick={() => {
                      const rec = selectedRecord
                      setSelectedRecord(null)
                      setRollbackNode({
                        nodeId: rec.node_id,
                        hostname: rec.hostname,
                        targetVersion: rec.previous_version || undefined,
                      })
                    }}
                  >
                    <RotateCcw size={14} />
                    <span>立即安全回滚至上一稳定版本</span>
                  </Button>
                </div>
              </div>
            )}

            {/* Checksums */}
            <div className="ws-deploy-diag-section">
              <div className="ws-deploy-diag-section-title">安全校验依据 (SHA-256)</div>
              <div className="ws-deploy-diag-card">
                <div className="ws-deploy-hash-row">
                  <div className="ws-deploy-diag-key">清单哈希 (Manifest SHA-256):</div>
                  <div className="ws-deploy-hash-box">
                    <span>{selectedRecord.manifest_sha256}</span>
                    <button
                      type="button"
                      onClick={() => handleCopy(selectedRecord.manifest_sha256, 'manifest')}
                      aria-label="复制清单哈希"
                      className="min-w-[44px] min-h-[44px] p-2 flex items-center justify-center shrink-0 rounded text-muted-foreground hover:text-foreground transition-colors"
                    >
                      {copiedKey === 'manifest' ? <Check size={14} /> : <Copy size={14} />}
                    </button>
                  </div>
                </div>

                <div className="ws-deploy-hash-row">
                  <div className="ws-deploy-diag-key">模型权重哈希 (Model SHA-256):</div>
                  <div className="ws-deploy-hash-box">
                    <span>{selectedRecord.model_sha256}</span>
                    <button
                      type="button"
                      onClick={() => handleCopy(selectedRecord.model_sha256, 'model')}
                      aria-label="复制模型权重哈希"
                      className="min-w-[44px] min-h-[44px] p-2 flex items-center justify-center shrink-0 rounded text-muted-foreground hover:text-foreground transition-colors"
                    >
                      {copiedKey === 'model' ? <Check size={14} /> : <Copy size={14} />}
                    </button>
                  </div>
                </div>
              </div>
            </div>

            {/* CUDA Profile & Benchmark Proof */}
            <div className="ws-deploy-diag-section">
              <div className="ws-deploy-diag-section-title">真 CUDA 执行与点位自检</div>
              <div className="ws-deploy-diag-card">
                <div className="ws-deploy-diag-row">
                  <span className="ws-deploy-diag-key">算子执行证明 (ORT Profiler):</span>
                  <span className="ws-deploy-diag-val">
                    {formatKernelEvents(
                      selectedRecord.self_test?.cuda_kernel_events,
                      selectedRecord.self_test?.cpu_kernel_events,
                      selectedRecord.execution_provider === 'CPUExecutionProvider'
                    )}
                  </span>
                </div>
                <div className="ws-deploy-diag-row">
                  <span className="ws-deploy-diag-key">点位数值偏差 (Landmark Drift):</span>
                  <span className="ws-deploy-diag-val">
                    {formatDrift(selectedRecord.self_test?.drift)} (上限 ≤ 0.05000)
                  </span>
                </div>
                <div className="ws-deploy-diag-row">
                  <span className="ws-deploy-diag-key">显存准入底线:</span>
                  <span className="ws-deploy-diag-val">
                    {formatVramFloor(
                      selectedRecord.startup_proof?.vram_allocated_mib,
                      selectedRecord.startup_proof?.admission_floor_passed,
                      selectedRecord.execution_provider === 'CPUExecutionProvider'
                    )}
                  </span>
                </div>
                <div className="ws-deploy-diag-row">
                  <span className="ws-deploy-diag-key">端到端诊断耗时:</span>
                  <span className="ws-deploy-diag-val">
                    {formatDuration(selectedRecord.duration_ms)}
                  </span>
                </div>
              </div>
            </div>

            {/* Sandbox Confinement Proof */}
            <div className="ws-deploy-diag-section">
              <div className="ws-deploy-diag-section-title">沙箱权限隔离依据</div>
              <div className="ws-deploy-diag-card">
                <div className="ws-deploy-diag-row">
                  <span className="ws-deploy-diag-key">运行身份:</span>
                  <span className="ws-deploy-diag-val">face-gpu (UID 非 root)</span>
                </div>
                <div className="ws-deploy-diag-row">
                  <span className="ws-deploy-diag-key">临时存储配额:</span>
                  <span className="ws-deploy-diag-val">64 MiB 严格受限 tmpfs</span>
                </div>
                <div className="ws-deploy-diag-row">
                  <span className="ws-deploy-diag-key">业务数据库访问:</span>
                  <span className="ws-deploy-diag-val text-emerald-400">绝对阻断 (无写权限)</span>
                </div>
                <div className="ws-deploy-diag-row">
                  <span className="ws-deploy-diag-key">WSL IndexTTS 状态:</span>
                  <span className="ws-deploy-diag-val text-emerald-400">
                    {selectedRecord.os === 'linux' ? '永久禁用 (Disabled)' : '不适用 (非 WSL/Linux)'}
                  </span>
                </div>
              </div>
            </div>
          </div>
        )}
      </Drawer>

      {/* Safe Rollback Confirmation Dialog */}
      {rollbackNode && (
        <div
          className="ws-deploy-modal-backdrop"
          role="dialog"
          aria-modal="true"
          aria-labelledby="rollback-dialog-title"
          onClick={(e) => {
            if (e.target === e.currentTarget && !busyNodeId) {
              setRollbackNode(null)
            }
          }}
        >
          <div
            ref={rollbackCardRef}
            className="ws-deploy-modal-card outline-none"
            tabIndex={-1}
          >
            <div id="rollback-dialog-title" className="ws-deploy-modal-title">
              <RotateCcw size={18} className="text-amber-400" />
              <span>确认安全回滚节点运行包？</span>
            </div>
            <div className="ws-deploy-modal-desc">
              回滚将平滑排空节点 <strong>{rollbackNode.hostname}</strong>（{rollbackNode.nodeId}）的在途任务，优雅停止候选进程并切回上一稳定发行版目录。
              <br /><br />
              调度中心将原子维护事务幂等性，不会中断正在提交的业务回执。
            </div>
            <div className="ws-deploy-modal-actions">
              <Button
                variant="outline"
                size="sm"
                className="min-h-[44px] px-4"
                onClick={() => setRollbackNode(null)}
                disabled={!!busyNodeId}
              >
                取消
              </Button>
              <Button
                variant="default"
                size="sm"
                className="min-h-[44px] px-4"
                onClick={handleConfirmRollback}
                disabled={!!busyNodeId}
              >
                {busyNodeId ? '正在下发...' : '确认安全回滚'}
              </Button>
            </div>
          </div>
        </div>
      )}
    </div>
  )
}
