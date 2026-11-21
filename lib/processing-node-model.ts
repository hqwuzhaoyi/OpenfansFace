export type NodeLifecycleStatus =
  | 'registered'
  | 'online'
  | 'model_ready'
  | 'accepting'
  | 'offline'

export interface NodeGpuTelemetry {
  model: string
  device_uuid: string
  vram_total_bytes: number
  free_vram_bytes: number | null
  gpu_util_percent: number | null
}

export interface NodeTelemetry {
  last_heartbeat_at: number | null
  instance_id: string | null
  cpu_cores: number
  ram_total_bytes: number
  free_ram_bytes: number | null
  gpu: NodeGpuTelemetry | null
  running_tasks: number
}

export interface AgentNode {
  node_id: string
  hostname: string
  os: string
  arch: string
  agent_version: string
  device_group_id: string
  status: NodeLifecycleStatus
  accepting_tasks: boolean
  model_ready: boolean
  registered_at: number
  updated_at: number
  desired_version: string
  current_version: string
  telemetry: NodeTelemetry
}

export interface DeviceGroupPool {
  device_group_id: string
  device_group_name: string
  hostname: string
  os: string
  arch: string
  is_gpu: boolean
  gpu_model: string | null
  gpu_device_uuid: string | null
  cpu_cores: number
  ram_total_bytes: number
  vram_total_bytes: number | null
  free_ram_bytes: number | null
  free_vram_bytes: number | null
  gpu_util_percent: number | null
  agents: AgentNode[]
  has_online_agent: boolean
  accepting_agents_count: number
  total_running_tasks: number
}

export interface NodeListApiResponse {
  schema_version: 1
  cluster: {
    total_nodes: number
    online_nodes: number
    accepting_nodes: number
    device_groups_count: number
    evaluated_at: number
  }
  nodes: AgentNode[]
}

export interface NodeControlRequest {
  node_id: string
  action: 'revoke_acceptance' | 'restore_acceptance' | 'drain'
}

export interface NodeControlResponse {
  ok: boolean
  node_id: string
  accepting: boolean
  status: string
}
