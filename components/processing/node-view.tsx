'use client'

import { useState, useMemo, useCallback } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { RefreshCw, Server, Cpu, CirclePause, CirclePlay } from 'lucide-react'
import { workspaceToast as toast } from '@/components/workbench/feedback'
import { WorkspaceCard, WorkspaceButton as Button } from '@/components/workbench/inner'
import { WorkspaceTabs, WorkspaceSearch } from '@/components/workbench/controls'
import { api } from '@/lib/face-api'
import type { AgentNode, DeviceGroupPool, NodeListApiResponse } from '@/lib/processing-node-model'
import { HardwareDial } from './hardware-dial'
import { NodeStatusBadge } from './node-status-badge'
import './node-view.css'

export default function NodeView() {
  const queryClient = useQueryClient()
  const [filter, setFilter] = useState<string>('all')
  const [query, setQuery] = useState<string>('')
  const [busyNodeId, setBusyNodeId] = useState<string | null>(null)

  // Polling nodes every 5 seconds
  const { data, isFetching, error, refetch } = useQuery<NodeListApiResponse>({
    queryKey: ['processing', 'nodes'],
    queryFn: async ({ signal }) => {
      const res = await api<NodeListApiResponse>('/api/processing/nodes', {
        signal: AbortSignal.any([signal, AbortSignal.timeout(10000)]),
        cache: 'no-store',
      })
      if (res?.schema_version !== 1) {
        throw new Error('节点接口协议版本不兼容，已暂停操作')
      }
      return res
    },
    refetchInterval: 5000,
    refetchIntervalInBackground: false,
    staleTime: 3000,
    retry: false,
  })

  // Action toggle: Revoke or Restore Acceptance
  const handleToggleAcceptance = useCallback(async (nodeId: string, currentAccepting: boolean) => {
    const action = currentAccepting ? 'revoke_acceptance' : 'restore_acceptance'
    setBusyNodeId(nodeId)
    try {
      await api('/api/processing/nodes/control', {
        method: 'POST',
        body: JSON.stringify({ node_id: nodeId, action }),
      })
      toast.success(
        action === 'revoke_acceptance'
          ? `已撤销节点 ${nodeId} 的接单权，在途任务完成后不再派发新任务`
          : `已恢复节点 ${nodeId} 的接单权`
      )
      await queryClient.invalidateQueries({ queryKey: ['processing', 'nodes'] })
    } catch (e: any) {
      toast.error(e.message || '接单权操作失败')
    } finally {
      setBusyNodeId(null)
    }
  }, [queryClient])

  // Group nodes by device_group_id into physical device pools
  const devicePools = useMemo(() => {
    if (!data?.nodes) return []
    const map = new Map<string, DeviceGroupPool>()

    for (const node of data.nodes) {
      const groupId = node.device_group_id || node.hostname || node.node_id
      let pool = map.get(groupId)
      if (!pool) {
        pool = {
          device_group_id: groupId,
          device_group_name: node.hostname,
          hostname: node.hostname,
          os: node.os,
          arch: node.arch,
          is_gpu: !!node.telemetry.gpu,
          gpu_model: node.telemetry.gpu?.model ?? null,
          gpu_device_uuid: node.telemetry.gpu?.device_uuid ?? null,
          cpu_cores: node.telemetry.cpu_cores,
          ram_total_bytes: node.telemetry.ram_total_bytes,
          vram_total_bytes: node.telemetry.gpu?.vram_total_bytes ?? null,
          free_ram_bytes: node.telemetry.free_ram_bytes,
          free_vram_bytes: node.telemetry.gpu?.free_vram_bytes ?? null,
          gpu_util_percent: node.telemetry.gpu?.gpu_util_percent ?? null,
          agents: [],
          has_online_agent: false,
          accepting_agents_count: 0,
          total_running_tasks: 0,
        }
        map.set(groupId, pool)
      }
      pool.agents.push(node)
      if (node.status !== 'offline') pool.has_online_agent = true
      if (node.accepting_tasks && node.status === 'accepting') pool.accepting_agents_count++
      pool.total_running_tasks += node.telemetry.running_tasks || 0

      // If multiple agents share GPU, pick newest valid telemetry sample
      if (node.status !== 'offline' && node.telemetry.free_ram_bytes !== null) {
        pool.free_ram_bytes = node.telemetry.free_ram_bytes
        if (node.telemetry.gpu) {
          pool.free_vram_bytes = node.telemetry.gpu.free_vram_bytes
          pool.gpu_util_percent = node.telemetry.gpu.gpu_util_percent
        }
      }
    }
    return Array.from(map.values())
  }, [data?.nodes])

  // Filtering & search
  const filteredPools = useMemo(() => {
    return devicePools.filter((pool) => {
      const q = query.trim().toLowerCase()
      const matchesQuery =
        !q ||
        pool.hostname.toLowerCase().includes(q) ||
        (pool.gpu_model && pool.gpu_model.toLowerCase().includes(q)) ||
        pool.agents.some((a) => a.node_id.toLowerCase().includes(q))

      if (!matchesQuery) return false

      if (filter === 'all') return true
      if (filter === 'accepting') return pool.agents.some((a) => a.status === 'accepting')
      if (filter === 'model_ready') return pool.agents.some((a) => a.status === 'model_ready')
      if (filter === 'online') return pool.agents.some((a) => a.status === 'online')
      if (filter === 'registered') return pool.agents.some((a) => a.status === 'registered')
      if (filter === 'offline') return pool.agents.every((a) => a.status === 'offline')
      return true
    })
  }, [devicePools, query, filter])

  const cluster = data?.cluster

  return (
    <div className="ws-node-view">
      {/* Header bar */}
      <div className="ws-node-view-header">
        <div>
          <h2>节点资源与设备池</h2>
          <span>
            {cluster
              ? `${cluster.device_groups_count} 个物理设备 · ${cluster.total_nodes} 个 Agent (${cluster.accepting_nodes} 个可接单)`
              : '正在连接节点服务…'}
          </span>
        </div>
        <div className="ws-node-header-actions">
          <Button
            variant="ghost"
            size="sm"
            disabled={isFetching}
            onClick={() => void refetch()}
            aria-label="刷新节点列表"
          >
            <RefreshCw size={14} className={isFetching ? 'animate-spin' : ''} />
            刷新
          </Button>
        </div>
      </div>

      {error && (
        <p className="ws-error" role="alert">
          {(error as Error)?.message || '节点状态读取失败'}
          <Button size="sm" variant="ghost" onClick={() => void refetch()}>
            重试读取
          </Button>
        </p>
      )}

      {/* Filter and Search toolbar */}
      <div className="ws-node-toolbar">
        <WorkspaceTabs
          value={filter}
          onChange={setFilter}
          label="筛选节点状态"
          items={[
            { value: 'all', label: '全部设备', count: devicePools.length },
            {
              value: 'accepting',
              label: '可接单',
              count: devicePools.filter((p) => p.agents.some((a) => a.status === 'accepting')).length,
            },
            {
              value: 'model_ready',
              label: '模型就绪',
              count: devicePools.filter((p) => p.agents.some((a) => a.status === 'model_ready')).length,
            },
            {
              value: 'online',
              label: '在线',
              count: devicePools.filter((p) => p.agents.some((a) => a.status === 'online')).length,
            },
            {
              value: 'registered',
              label: '已登记',
              count: devicePools.filter((p) => p.agents.some((a) => a.status === 'registered')).length,
            },
            {
              value: 'offline',
              label: '已失联',
              count: devicePools.filter((p) => p.agents.every((a) => a.status === 'offline')).length,
            },
          ]}
        />
        <WorkspaceSearch
          value={query}
          onChange={setQuery}
          label="搜索节点或硬件"
          placeholder="搜索主机名、GPU 型号或节点 ID…"
        />
      </div>

      {/* Main Pools Grid */}
      <div className="ws-node-cards-list">
        {!data ? (
          <p className="bm-empty">正在加载集群节点状态…</p>
        ) : filteredPools.length === 0 ? (
          <p className="bm-empty">当前筛选无符合条件的计算设备</p>
        ) : (
          filteredPools.map((pool) => (
            <WorkspaceCard key={pool.device_group_id} className="ws-device-card">
              {/* Card Header: Physical Hardware Identity */}
              <div className="ws-device-card-header">
                <div className="ws-device-info">
                  <div className="ws-device-icon" data-gpu={pool.is_gpu}>
                    {pool.is_gpu ? <Cpu size={20} /> : <Server size={20} />}
                  </div>
                  <div>
                    <div className="ws-device-title-row">
                      <strong className="ws-device-name">
                        {pool.is_gpu ? pool.gpu_model : pool.hostname}
                      </strong>
                      {pool.agents.length > 1 && (
                        <span className="ws-device-pool-badge">
                          共享硬件 · {pool.agents.length} 个 Agent 实例
                        </span>
                      )}
                    </div>
                    <div className="ws-device-meta">
                      <span>{pool.hostname}</span>
                      <span>·</span>
                      <span>
                        {pool.os} ({pool.arch})
                      </span>
                      <span>·</span>
                      <span>{pool.cpu_cores} 核 CPU</span>
                      {pool.gpu_device_uuid && (
                        <>
                          <span>·</span>
                          <code className="ws-device-uuid" title={pool.gpu_device_uuid}>
                            {pool.gpu_device_uuid.slice(0, 16)}…
                          </code>
                        </>
                      )}
                    </div>
                  </div>
                </div>

                <div className="ws-device-header-status">
                  <span className="ws-device-task-count">
                    {pool.total_running_tasks > 0
                      ? `执行中 ${pool.total_running_tasks} 任务`
                      : '无在途任务'}
                  </span>
                </div>
              </div>

              {/* Shared Real Hardware Dials (Truth in UI: "未知", NEVER 0) */}
              <div className="ws-device-dials-row">
                <HardwareDial
                  label="内存"
                  type="ram"
                  usedBytes={
                    typeof pool.free_ram_bytes === 'number' &&
                    Number.isFinite(pool.free_ram_bytes) &&
                    typeof pool.ram_total_bytes === 'number' &&
                    Number.isFinite(pool.ram_total_bytes)
                      ? pool.ram_total_bytes - pool.free_ram_bytes
                      : null
                  }
                  totalBytes={pool.ram_total_bytes}
                  isOffline={!pool.has_online_agent}
                />
                <HardwareDial
                  label="显存"
                  type="vram"
                  usedBytes={
                    pool.is_gpu &&
                    typeof pool.free_vram_bytes === 'number' &&
                    Number.isFinite(pool.free_vram_bytes) &&
                    typeof pool.vram_total_bytes === 'number' &&
                    Number.isFinite(pool.vram_total_bytes)
                      ? pool.vram_total_bytes - pool.free_vram_bytes
                      : null
                  }
                  totalBytes={pool.vram_total_bytes}
                  isCpuOnly={!pool.is_gpu}
                  isOffline={!pool.has_online_agent}
                />
                <HardwareDial
                  label="GPU 负载"
                  type="gpu_util"
                  percent={pool.is_gpu ? pool.gpu_util_percent : null}
                  isCpuOnly={!pool.is_gpu}
                  isOffline={!pool.has_online_agent}
                />
              </div>

              {/* Associated Agent Instances List */}
              <div className="ws-agents-table">
                <div className="ws-agents-table-head">
                  <span>Agent 实例</span>
                  <span>状态</span>
                  <span>在途</span>
                  <span>心跳与实例</span>
                  <span>接单权操作</span>
                </div>

                {pool.agents.map((agent) => (
                  <div className="ws-agent-row" key={agent.node_id}>
                    <div className="ws-agent-name-cell">
                      <strong>{agent.node_id}</strong>
                      <small>{agent.agent_version}</small>
                    </div>

                    <div className="ws-agent-status-cell">
                      <NodeStatusBadge status={agent.status} />
                    </div>

                    <div className="ws-agent-task-cell">
                      {agent.telemetry.running_tasks > 0 ? (
                        <span className="ws-agent-busy-tag">
                          {agent.telemetry.running_tasks} 任务
                        </span>
                      ) : (
                        <span className="ws-agent-idle-tag">空闲</span>
                      )}
                    </div>

                    <div className="ws-agent-heartbeat-cell">
                      <span>
                        {typeof agent.telemetry.last_heartbeat_at === 'number' &&
                        Number.isFinite(agent.telemetry.last_heartbeat_at) &&
                        agent.telemetry.last_heartbeat_at > 0
                          ? `${Math.max(
                              0,
                              Math.round(Date.now() / 1000 - agent.telemetry.last_heartbeat_at)
                            )} 秒前`
                          : '尚未收到心跳'}
                      </span>
                      {agent.telemetry.instance_id && (
                        <small title={agent.telemetry.instance_id}>
                          {agent.telemetry.instance_id.slice(0, 8)}…
                        </small>
                      )}
                    </div>

                    <div className="ws-agent-action-cell">
                      <Button
                        variant="outline"
                        size="sm"
                        disabled={agent.status === 'offline' || busyNodeId === agent.node_id}
                        onClick={() =>
                          handleToggleAcceptance(agent.node_id, agent.accepting_tasks)
                        }
                        title={
                          agent.status === 'offline'
                            ? '节点已失联，恢复心跳后方可切换接单状态'
                            : agent.accepting_tasks
                            ? '暂停向此节点派发新任务，在途任务不受影响'
                            : '恢复向此节点派发任务'
                        }
                      >
                        {agent.accepting_tasks ? (
                          <>
                            <CirclePause size={13} className="text-amber-600 dark:text-amber-400" />
                            <span>撤销接单权</span>
                          </>
                        ) : (
                          <>
                            <CirclePlay size={13} className="text-emerald-600 dark:text-emerald-400" />
                            <span>恢复接单权</span>
                          </>
                        )}
                      </Button>
                    </div>
                  </div>
                ))}
              </div>
            </WorkspaceCard>
          ))
        )}
      </div>
    </div>
  )
}
