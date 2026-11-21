import { NextRequest, NextResponse } from 'next/server'
import { invalidateIntegrationCacheAsync } from '@/lib/integration'
import { ReviewError } from '@/lib/review-logic'
import { nativeJsonRequest } from '@/lib/native-json-transport'
import {
  computeSyncStatus,
  type DeploymentListApiResponse,
  type DeploymentRecord,
  type DeploymentControlRequest,
  type DeploymentControlResponse,
} from '@/lib/processing-deployment-model'

export const runtime = 'nodejs'

interface BackendDeploymentItem {
  deployment_id: string
  node_id: string
  package_id: string
  status: string
  desired_version: string
  actual_version: string | null
  stage: string
  startup_proof: any
  self_test_passed: number | null
  self_test_drift: number | null
  duration_ms: number | null
  error: string | null
  updated_at: number
  pkg_version: string
  execution_provider: string
  manifest_sha256: string
  model_sha256: string
  hostname?: string | null
  arch?: string | null
  os?: string | null
  node_status?: string | null
  gpu_model?: string | null
}

interface BackendDeploymentResponse {
  deployments: BackendDeploymentItem[]
  total?: number
}

export async function GET() {
  try {
    const raw = await nativeJsonRequest<BackendDeploymentResponse>(
      'v1/agent/deployments',
      null,
      {
        label: '运行包部署记录读取',
        completion: 'required',
        error: (message, status) => new ReviewError(message, status),
      }
    )

    const rawList = raw?.deployments || []
    let totalNodes = 0
    let syncedNodes = 0
    let inProgress = 0
    let failedNodes = 0

    const deployments: DeploymentRecord[] = rawList.map((d) => {
      totalNodes++
      const stage = (d.stage || 'download') as any
      const syncStatus = computeSyncStatus(d.desired_version, d.actual_version, stage, d.status)

      if (syncStatus === 'synced') syncedNodes++
      else if (syncStatus === 'deploying') inProgress++
      else if (syncStatus === 'failed') failedNodes++

      const proof = typeof d.startup_proof === 'object' && d.startup_proof !== null ? d.startup_proof : null
      const effectiveGpuModel = d.gpu_model || proof?.gpu_model || proof?.device_name || null
      const isGpu =
        Boolean(effectiveGpuModel) ||
        d.execution_provider === 'CUDAExecutionProvider' ||
        d.execution_provider === 'CoreMLExecutionProvider'
      const effectiveProvider =
        d.os === 'macos' && effectiveGpuModel?.includes('Apple') && d.execution_provider === 'CPUExecutionProvider'
          ? 'CoreMLExecutionProvider'
          : d.execution_provider
      const effectiveError =
        syncStatus === 'synced' && d.status === 'ready' && d.self_test_passed === 1
          ? null
          : d.error

      return {
        deployment_id: d.deployment_id,
        node_id: d.node_id,
        hostname: d.hostname || d.node_id,
        os: d.os || 'linux',
        arch: d.arch || 'x86_64',
        is_gpu: isGpu,
        gpu_model: effectiveGpuModel,
        execution_provider: effectiveProvider,
        desired_version: d.desired_version,
        actual_version: d.actual_version,
        package_id: d.package_id,
        manifest_sha256: d.manifest_sha256,
        model_sha256: d.model_sha256,
        stage,
        status: d.status,
        sync_status: syncStatus,
        updated_at: d.updated_at,
        duration_ms: d.duration_ms,
        self_test_passed: d.self_test_passed,
        self_test_drift: d.self_test_drift,
        self_test: {
          passed: d.self_test_passed === 1,
          drift: d.self_test_drift,
          tolerance: 0.05,
          cuda_kernel_events: proof?.kernel_events ?? proof?.cuda_kernel_events ?? null,
          cpu_kernel_events: proof?.cpu_kernel_events ?? null,
          benchmark_duration_ms: d.duration_ms,
        },
        startup_proof: proof,
        error: effectiveError,
        node_status: d.node_status,
        failure_evidence:
          syncStatus === 'failed' || stage === 'failed'
            ? {
                stage,
                error_message: effectiveError || '部署异常中断',
                timestamp: d.updated_at,
              }
            : null,
      }
    })

    const payload: DeploymentListApiResponse = {
      schema_version: 1,
      deployments,
      summary: {
        total_nodes: totalNodes,
        synced_nodes: syncedNodes,
        in_progress: inProgress,
        failed_nodes: failedNodes,
        evaluated_at: Date.now() / 1000,
      },
    }

    return NextResponse.json(payload, { headers: { 'Cache-Control': 'no-store' } })
  } catch (e: any) {
    return NextResponse.json(
      { schema_version: 1, error: e.message || '运行包部署记录读取失败' },
      { status: e.status || 503 }
    )
  }
}

export async function POST(req: NextRequest) {
  let body: DeploymentControlRequest
  try {
    body = await req.json()
  } catch {
    return NextResponse.json(
      { schema_version: 1, error: '请求体必须为 JSON' },
      { status: 400 }
    )
  }

  if (!body?.node_id || typeof body.node_id !== 'string') {
    return NextResponse.json(
      { schema_version: 1, error: '缺少有效的 node_id' },
      { status: 400 }
    )
  }

  if (!['deploy', 'rollback'].includes(body?.action)) {
    return NextResponse.json(
      { schema_version: 1, error: '不支持的操作动作，仅支持 deploy 或 rollback' },
      { status: 400 }
    )
  }

  try {
    const route = body.action === 'rollback' ? 'v1/agent/deployments/rollback' : 'v1/agent/deploy'
    const label = body.action === 'rollback' ? '运行包安全回滚' : '运行包下发部署'

    const data = await nativeJsonRequest<DeploymentControlResponse>(
      route,
      body,
      {
        label,
        completion: 'required',
        error: (message, status) => new ReviewError(message, status),
      }
    )
    await invalidateIntegrationCacheAsync()
    return NextResponse.json(data)
  } catch (e: any) {
    return NextResponse.json(
      { schema_version: 1, error: e.message || '部署控制操作失败' },
      { status: e.status || 500 }
    )
  }
}
