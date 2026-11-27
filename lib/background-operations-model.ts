import type { EntityLockStatus, ServerActiveOp } from "./locks-model";

export const LEGACY_OPERATION_STORAGE_KEYS = [
  "person-file-operation-v1",
  "person-batch-operation-v1",
  "face-archive-operation",
] as const;

/**
 * Removes legacy operation-persistence keys from browser storage so stale
 * completed/failed operations never resurrect or replay on page load.
 */
export function clearLegacyOperationStorage(
  storage?: Pick<Storage, "removeItem"> | null,
): void {
  const target =
    storage !== undefined
      ? storage
      : typeof window !== "undefined"
        ? window.localStorage
        : null;
  if (!target) return;
  for (const key of LEGACY_OPERATION_STORAGE_KEYS) {
    try {
      target.removeItem(key);
    } catch {
      // Ignore storage access errors (e.g. private mode or quota restrictions).
    }
  }
}

/**
 * Returns server-active operations that are not already tracked in local
 * in-memory tasks, preserving server ordering.
 */
export function filterExternalServerActiveOps(
  serverActiveOps: readonly ServerActiveOp[],
  localTasks: readonly {
    id: string;
    kind: string;
    operationId?: string;
    plan?: { id?: string };
  }[],
): ServerActiveOp[] {
  const localIds = new Set<string>();
  for (const task of localTasks) {
    if (task.id) localIds.add(task.id);
    if (task.operationId) localIds.add(task.operationId);
    if (task.plan?.id) localIds.add(task.plan.id);
  }
  return serverActiveOps.filter(
    (op) => Boolean(op.op_id) && !localIds.has(op.op_id),
  );
}

const SERVER_OP_ACTION_TITLES: Record<string, string> = {
  "batch-delete": "删除人物",
  "delete-person": "删除人物",
  "batch-merge": "合并人物",
  "merge-person": "合并人物",
  "rename-person": "重命名人物",
  "move-files": "移动视频",
  "delete-files": "删除视频",
  "review-delete": "移入回收站",
  "rename-file": "重命名视频",
  "restore-files": "恢复文件",
  "staging-delete": "永久删除",
  archive: "归档",
  "auto-archive": "自动合入",
  "auto-merge": "自动合入",
  "auto-watermark-apply": "自动合入",
  "archive-unarchive": "撤回归档",
};

export interface ServerActiveOpCardModel {
  id: string;
  title: string;
  subtitle: string;
  stageLabel: string;
  status: EntityLockStatus;
  badgeText: string;
  badgeStatus: "loading" | "warning" | "neutral";
  startedAt: number;
}

/**
 * Formats a server-authoritative active operation into a read-only dock card model.
 */
export function formatServerActiveOpCard(
  op: ServerActiveOp,
): ServerActiveOpCardModel {
  const label = op.label?.trim() || "";
  const actionTitle = SERVER_OP_ACTION_TITLES[op.action] || label || "后台操作";
  const people = op.people.filter(Boolean);
  const title =
    people.length > 0
      ? `${actionTitle} · ${people.slice(0, 2).join("、")}${people.length > 2 ? ` 等 ${people.length} 人` : ""}`
      : actionTitle;
  const subtitle =
    op.video_count > 0
      ? `${op.video_count} 个视频`
      : people.length > 0
        ? `${people.length} 位人物`
        : label || "服务端同步中";
  const status: EntityLockStatus =
    op.status === "recovery_needed"
      ? "recovery_needed"
      : op.status === "running"
        ? "running"
        : "queued";
  const badgeStatus: "loading" | "warning" | "neutral" =
    status === "recovery_needed"
      ? "warning"
      : status === "running"
        ? "loading"
        : "neutral";
  const badgeText =
    status === "recovery_needed"
      ? "待核对"
      : status === "running"
        ? "执行中"
        : "排队中";
  const stageLabel =
    label ||
    (status === "recovery_needed"
      ? "写入已暂停，待核对恢复"
      : status === "running"
        ? "服务端执行中"
        : "等待服务端执行");
  const startedAt =
    typeof op.started_at === "number" && Number.isFinite(op.started_at)
      ? op.started_at
      : 0;

  return {
    id: op.op_id,
    title,
    subtitle,
    stageLabel,
    status,
    badgeText,
    badgeStatus,
    startedAt,
  };
}

/**
 * Detects an operation that was rejected prior to admission (`status === 'not_started'`)
 * and is no longer present in the server's active operations list.
 */
export function isUnadmittedNotStarted(
  receiptStatus: string,
  opId: string,
  activeOps: readonly ServerActiveOp[],
): boolean {
  return (
    receiptStatus === "not_started" &&
    !activeOps.some((op) => op.op_id === opId)
  );
}

/**
 * Synchronously settles a background task using the latest snapshot before
 * scheduling React state updates and notifying subscribers. Under React 19
 * automatic batching, updater callbacks passed to setState run asynchronously,
 * so side-effect emitters must never rely on values assigned inside updater callbacks.
 */
export function settleBackgroundTask<
  TaskUnion extends { id: string; kind: string },
  T extends TaskUnion,
>(params: {
  tasks: readonly TaskUnion[];
  fallback: T;
  apply: (task: T) => T;
  updateTask: (id: string, updater: (task: TaskUnion) => TaskUnion) => void;
  emit: (settled: T) => void;
}): T {
  const latest =
    params.tasks.find(
      (task): task is T =>
        task.id === params.fallback.id && task.kind === params.fallback.kind,
    ) ?? params.fallback;
  const settled = params.apply(latest);
  params.updateTask(params.fallback.id, (task) =>
    task.kind === params.fallback.kind ? params.apply(task as T) : task,
  );
  params.emit(settled);
  return settled;
}
