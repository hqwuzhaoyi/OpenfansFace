export type DeploymentStage =
  | 'download'
  | 'verify'
  | 'start'
  | 'warmup'
  | 'self_test'
  | 'ready'
  | 'failed'

export type DeploymentSyncStatus =
  | 'synced'
  | 'deploying'
  | 'mismatched'
  | 'failed'
  | 'rolled_back'

export interface DeploymentSelfTest {
  passed: boolean
  drift: number | null
  tolerance: number | null
  synthetic_fixture_hash?: string | null
  cuda_kernel_events?: number | null
  cpu_kernel_events?: number | null
  tf32_enabled?: boolean | null
  benchmark_duration_ms?: number | null
}

export interface DeploymentStartupProof {
  provider_proof?: string | null
  vram_allocated_mib?: number | null
  peak_memory_mib?: number | null
  admission_floor_passed?: boolean | null
  cuda_initialized?: boolean | null
  kernel_events?: number | null
  cpu_kernel_events?: number | null
  arena_allocated_bytes?: number | null
  provider?: string | null
  [key: string]: any
}

export interface StageStepInfo {
  stage: DeploymentStage
  status: 'pending' | 'in_progress' | 'completed' | 'failed'
  duration_ms?: number | null
  started_at?: number | null
  finished_at?: number | null
  error?: string | null
}

export interface DeploymentRecord {
  deployment_id: string
  node_id: string
  hostname: string
  os: string
  arch: string
  is_gpu: boolean
  gpu_model: string | null
  execution_provider: 'CUDAExecutionProvider' | 'CPUExecutionProvider' | string
  desired_version: string
  actual_version: string | null
  previous_version?: string | null
  package_id: string
  release_id?: string
  revision?: string
  manifest_sha256: string
  model_sha256: string
  code_sha256?: string | null
  stage: DeploymentStage
  status: string
  sync_status: DeploymentSyncStatus
  started_at?: number
  updated_at: number
  completed_at?: number | null
  duration_ms: number | null
  self_test_passed?: number | boolean | null
  self_test_drift?: number | null
  self_test?: DeploymentSelfTest | null
  startup_proof?: DeploymentStartupProof | null
  error?: string | null
  node_status?: string | null
  failure_evidence?: {
    stage: DeploymentStage
    error_message: string
    code?: string
    timestamp: number
  } | null
  stages_history?: StageStepInfo[]
}

export interface DeploymentListApiResponse {
  schema_version: 1
  deployments: DeploymentRecord[]
  summary?: {
    total_nodes: number
    synced_nodes: number
    in_progress: number
    failed_nodes: number
    evaluated_at: number
  }
}

export interface DeploymentControlRequest {
  action: 'deploy' | 'rollback'
  node_id: string
  package_id?: string
  target_version?: string
}

export interface DeploymentControlResponse {
  ok: boolean
  deployment_id?: string
  node_id?: string
  status?: 'accepted' | 'busy' | 'rejected' | string
  target_version?: string
  message?: string
  manifest_sha256?: string
  model_sha256?: string
  execution_provider?: string
}

export const STAGE_STEPS: Array<{ stage: DeploymentStage; label: string; desc: string }> = [
  { stage: 'download', label: '下载', desc: '分发受限运行包' },
  { stage: 'verify', label: '校验', desc: 'SHA-256 清单核对' },
  { stage: 'start', label: '启动', desc: '沙箱服务与权限初始化' },
  { stage: 'warmup', label: '预热', desc: '显存底线探测与 Arena 预分配' },
  { stage: 'self_test', label: '自检', desc: '基准人脸点位与真 CUDA 验证' },
  { stage: 'ready', label: '就绪', desc: '模型就绪并进入可接单状态' },
]

export function computeSyncStatus(
  desiredVersion: string,
  actualVersion: string | null | undefined,
  stage: DeploymentStage,
  status: string
): DeploymentSyncStatus {
  if (status === 'rolled_back') return 'rolled_back'
  if (stage === 'failed' || status === 'failed') return 'failed'
  if (['download', 'verify', 'start', 'warmup', 'self_test'].includes(stage)) return 'deploying'
  if (actualVersion && desiredVersion && actualVersion === desiredVersion && stage === 'ready') {
    return 'synced'
  }
  return 'mismatched'
}

export function getStepStatus(
  stepStage: DeploymentStage,
  currentStage: DeploymentStage,
  statusOrFailedStage?: string | DeploymentStage | null,
  maybeFailedStage?: DeploymentStage | null
): 'completed' | 'active' | 'failed' | 'pending' {
  const STAGE_ORDER: DeploymentStage[] = ['download', 'verify', 'start', 'warmup', 'self_test', 'ready']
  const stepIdx = STAGE_ORDER.indexOf(stepStage)
  if (stepIdx === -1) return 'pending'

  let status: string | null = null
  let failedStage: DeploymentStage | null = null

  const KNOWN_STATUSES = new Set([
    'ready',
    'downloading',
    'synced',
    'deploying',
    'mismatched',
    'rolled_back',
    'failed',
    'in_progress',
    'active',
    'completed',
    'accepted',
  ])

  if (arguments.length >= 4 || maybeFailedStage !== undefined) {
    status = statusOrFailedStage ?? null
    failedStage = maybeFailedStage ?? null
  } else if (statusOrFailedStage && KNOWN_STATUSES.has(statusOrFailedStage)) {
    status = statusOrFailedStage
    failedStage = null
  } else if (statusOrFailedStage && STAGE_ORDER.includes(statusOrFailedStage as DeploymentStage) && statusOrFailedStage !== 'ready') {
    status = 'failed'
    failedStage = statusOrFailedStage as DeploymentStage
  } else {
    status = statusOrFailedStage ?? null
    failedStage = null
  }

  const isFailed =
    status === 'failed' ||
    currentStage === 'failed' ||
    Boolean(failedStage && STAGE_ORDER.includes(failedStage))

  if (isFailed) {
    const effectiveFailedStage =
      (failedStage && STAGE_ORDER.includes(failedStage) ? failedStage : null) ||
      (STAGE_ORDER.includes(currentStage) ? currentStage : null) ||
      'download'
    const failedIdx = STAGE_ORDER.indexOf(effectiveFailedStage)
    const effectiveFailedIdx = failedIdx === -1 ? 0 : failedIdx
    if (stepIdx < effectiveFailedIdx) return 'completed'
    if (stepIdx === effectiveFailedIdx) return 'failed'
    return 'pending'
  }

  if (currentStage === 'ready' || status === 'ready' || status === 'synced') return 'completed'

  const currentIdx = STAGE_ORDER.indexOf(currentStage)
  if (currentIdx === -1) return 'pending'
  if (stepIdx < currentIdx) return 'completed'
  if (stepIdx === currentIdx) return 'active'
  return 'pending'
}

export function formatDrift(drift: number | null | undefined): string {
  if (typeof drift !== 'number' || !Number.isFinite(drift) || drift < 0) return '未知'
  return drift.toFixed(5)
}

export function formatDuration(ms: number | null | undefined): string {
  if (typeof ms !== 'number' || !Number.isFinite(ms) || ms < 0) return '未知'
  if (ms < 1000) return `${Math.round(ms)} ms`
  return `${(ms / 1000).toFixed(2)} s`
}

export function formatKernelEvents(
  cuda: number | null | undefined,
  cpu: number | null | undefined,
  isCpuOnly = false
): string {
  if (isCpuOnly) return '不适用'
  if (typeof cuda !== 'number' || !Number.isFinite(cuda) || cuda < 0) return '未知'
  const cpuVal = typeof cpu === 'number' && Number.isFinite(cpu) && cpu >= 0 ? Math.round(cpu) : 0
  return `${Math.round(cuda)} CUDA / ${cpuVal} CPU`
}

export function formatVramFloor(
  vramMib: number | null | undefined,
  passed: boolean | null | undefined,
  isCpuOnly = false
): string {
  if (isCpuOnly) return '不适用'
  if (typeof vramMib !== 'number' || !Number.isFinite(vramMib) || vramMib < 0) return '未知'
  const isPassed = typeof passed === 'boolean' ? passed : vramMib >= 600
  return `${vramMib.toFixed(1)} MiB (${isPassed ? '符合 ≥600MiB 底线' : '未达标'})`
}
