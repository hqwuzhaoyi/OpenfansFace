export type EntityLockStatus = 'queued' | 'running' | 'recovery_needed'

export interface ServerEntityLock {
  op_id: string
  action: string
  status: EntityLockStatus | string
  label: string
  started_at: number
}

export interface ServerActiveOp {
  op_id: string
  action: string
  status: EntityLockStatus | string
  label: string
  video_count: number
  people: string[]
  started_at: number
}

export type ServerSettledOutcome =
  | 'complete'
  | 'failed'
  | 'rolled_back'
  | 'recovery_needed'
  | 'cancelled'

export interface ServerSettledOp {
  op_id: string
  action: string
  outcome: ServerSettledOutcome | string
  people: string[]
  vids: string[]
  settled_at: number
}

export interface ServerLockSnapshot {
  revision: number
  at: number
  videos: Record<string, ServerEntityLock>
  people: Record<string, ServerEntityLock>
  active_ops: ServerActiveOp[]
  last_settled?: ServerSettledOp | null
}

export interface ScopeLockInfo {
  label: string
  status: EntityLockStatus
  taskId: string
  action?: string
  source?: 'local' | 'server'
}

const DESTRUCTIVE_PERSON_ACTIONS = new Set([
  'delete-person',
  'batch-delete',
  'merge-person',
  'batch-merge',
  'rename-person',
])

const AUTO_MERGE_ACTIONS = new Set([
  'auto-archive',
  'auto-merge',
  'auto-watermark-apply',
])

export function isDestructivePersonAction(action?: string): boolean {
  return typeof action === 'string' && DESTRUCTIVE_PERSON_ACTIONS.has(action)
}

export function isAutoMergeAction(action?: string): boolean {
  return typeof action === 'string' && AUTO_MERGE_ACTIONS.has(action)
}

export function normalizeEntityLockStatus(status?: string): EntityLockStatus {
  if (status === 'recovery_needed') return 'recovery_needed'
  if (status === 'running') return 'running'
  return 'queued'
}

export function lockStatusRank(status?: string): number {
  if (status === 'recovery_needed') return 3
  if (status === 'running') return 2
  if (status === 'queued') return 1
  return 0
}

export function lockBadgeStatus(
  status?: EntityLockStatus | string,
): 'loading' | 'warning' | 'neutral' {
  if (status === 'recovery_needed') return 'warning'
  if (status === 'running') return 'loading'
  return 'neutral'
}

function fallbackLockLabel(status: EntityLockStatus): string {
  if (status === 'recovery_needed') return '待恢复'
  if (status === 'running') return '处理中'
  return '排队中'
}

function fromServerEntityLock(lock: ServerEntityLock): ScopeLockInfo {
  const status = normalizeEntityLockStatus(lock.status)
  return {
    label: lock.label?.trim() || fallbackLockLabel(status),
    status,
    taskId: lock.op_id,
    action: lock.action,
    source: 'server',
  }
}

/**
 * Merges local optimistic locks with server-authoritative locks.
 * Precedence: `recovery_needed` (3) > `running` (2) > `queued` (1).
 * On equal status precedence, the server lock wins as authoritative.
 */
export function mergeScopeLocks(
  localMap: ReadonlyMap<string, ScopeLockInfo>,
  serverRecord?: Record<string, ServerEntityLock> | null,
): Map<string, ScopeLockInfo> {
  const merged = new Map<string, ScopeLockInfo>()

  for (const [key, local] of localMap) {
    if (!key) continue
    const status = normalizeEntityLockStatus(local.status)
    merged.set(key, {
      ...local,
      status,
      label: local.label?.trim() || fallbackLockLabel(status),
      source: local.source ?? 'local',
    })
  }

  if (!serverRecord || typeof serverRecord !== 'object') {
    return merged
  }

  for (const [key, serverLock] of Object.entries(serverRecord)) {
    if (!key || !serverLock || typeof serverLock !== 'object') continue
    const incoming = fromServerEntityLock(serverLock)
    const existing = merged.get(key)
    if (!existing) {
      merged.set(key, incoming)
      continue
    }
    const incomingRank = lockStatusRank(incoming.status)
    const existingRank = lockStatusRank(existing.status)
    if (incomingRank >= existingRank) {
      merged.set(key, incoming)
    }
  }

  return merged
}

/**
 * Picks the highest-priority lock among multiple candidate locks (e.g. video lock + person lock).
 * `recovery_needed` > `running` > `queued`; on equal rank, destructive person actions take precedence.
 */
export function resolveEntityLock(
  ...locks: Array<ScopeLockInfo | undefined | null>
): ScopeLockInfo | undefined {
  let best: ScopeLockInfo | undefined
  for (const candidate of locks) {
    if (!candidate) continue
    if (!best) {
      best = candidate
      continue
    }
    const candidateRank = lockStatusRank(candidate.status)
    const bestRank = lockStatusRank(best.status)
    if (candidateRank > bestRank) {
      best = candidate
    } else if (
      candidateRank === bestRank &&
      isDestructivePersonAction(candidate.action) &&
      !isDestructivePersonAction(best.action)
    ) {
      best = candidate
    }
  }
  return best
}

export function parseLockSnapshotJson(raw: string): ServerLockSnapshot | null {
  try {
    const parsed = JSON.parse(raw) as Partial<ServerLockSnapshot> | null
    if (
      !parsed ||
      typeof parsed !== 'object' ||
      typeof parsed.revision !== 'number' ||
      !Number.isFinite(parsed.revision)
    ) {
      return null
    }
    return {
      revision: parsed.revision,
      at: typeof parsed.at === 'number' ? parsed.at : Date.now(),
      videos:
        parsed.videos && typeof parsed.videos === 'object' && !Array.isArray(parsed.videos)
          ? parsed.videos
          : {},
      people:
        parsed.people && typeof parsed.people === 'object' && !Array.isArray(parsed.people)
          ? parsed.people
          : {},
      active_ops: Array.isArray(parsed.active_ops) ? parsed.active_ops : [],
      last_settled:
        parsed.last_settled && typeof parsed.last_settled === 'object'
          ? {
              op_id: String(parsed.last_settled.op_id ?? ''),
              action: String(parsed.last_settled.action ?? ''),
              outcome: String(parsed.last_settled.outcome ?? ''),
              people: Array.isArray(parsed.last_settled.people)
                ? parsed.last_settled.people.filter((p): p is string => typeof p === 'string')
                : [],
              vids: Array.isArray(parsed.last_settled.vids)
                ? parsed.last_settled.vids.filter((v): v is string => typeof v === 'string')
                : [],
              settled_at:
                typeof parsed.last_settled.settled_at === 'number'
                  ? parsed.last_settled.settled_at
                  : Date.now(),
            }
          : null,
    }
  } catch {
    return null
  }
}
