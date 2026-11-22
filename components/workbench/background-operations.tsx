"use client";

import {
  AlertTriangle,
  Archive,
  ArrowUpRight,
  Check,
  Clock3,
  FolderInput,
  LoaderCircle,
  Merge,
  Pencil,
  RefreshCw,
  Sparkles,
  Trash2,
  X,
} from "lucide-react";
import { useRouter } from "next/navigation";
import {
  createContext,
  type ReactNode,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import {
  AgentActivity,
  type AgentActivityItem,
} from "@/components/beui/agents/agent-activity";
import { AgentProgress } from "@/components/beui/agents/loading-states/agent-progress";
import {
  Alert,
  AlertContent,
  AlertDescription,
  AlertIcon,
} from "@/components/beui/motion/alert";
import { AnimatedBadge } from "@/components/beui/motion/animated-badge";
import { DigitSwap } from "@/components/beui/motion/digit-swap";
import {
  MorphPopover,
  MorphPopoverContent,
  MorphPopoverTrigger,
} from "@/components/beui/motion/popover-morph";
import { ButtonGroup } from "@/components/ui/button-group";
import { FILE_OPERATION_STORAGE_KEY } from "@/components/persons/file-actions";
import { deleteWorkbenchCache } from "@/components/workbench/state";
import { settleBackgroundTask } from "@/lib/background-operations-model";

function invalidatePersonLibraryCaches(
  people: Iterable<string | undefined | null>,
) {
  for (const person of people) {
    if (person) {
      deleteWorkbenchCache("persons:library:" + person);
    }
  }
  deleteWorkbenchCache("persons:list");
  deleteWorkbenchCache("persons:stats");
}

function invalidateReviewCaches({
  resetSession = false,
  includePeople = false,
}: {
  resetSession?: boolean;
  includePeople?: boolean;
} = {}) {
  if (resetSession) {
    deleteWorkbenchCache("review:session");
  }
  deleteWorkbenchCache("review:page:");
  deleteWorkbenchCache("review:lastData");
  if (includePeople) {
    deleteWorkbenchCache("review:people");
  }
}
import { workspaceToast as toast } from "@/components/workbench/feedback";
import type {
  AutoWatermarkApplyItemInput,
  AutoWatermarkApplyResult,
} from "@/lib/archive-actions";
import type { ArchiveResult } from "@/lib/archive-run-model";
import { api } from "@/lib/face-api";
import type {
  PersonBatchAction,
  PersonBatchPlan,
  PersonBatchProgress,
  PersonBatchResult,
} from "@/lib/person-batch-model";
import type {
  PersonFileAction,
  PersonFilePlan,
  PersonFileResult,
} from "@/lib/person-file-model";
import {
  isDestructivePersonAction,
  lockStatusRank,
  mergeScopeLocks,
  parseLockSnapshotJson,
  type ScopeLockInfo,
  type ServerActiveOp,
  type ServerLockSnapshot,
  type ServerSettledOp,
} from "@/lib/locks-model";
import "./background-operations.css";

export type {
  ScopeLockInfo,
  ServerActiveOp,
  ServerLockSnapshot,
  ServerSettledOp,
};

const BATCH_STORAGE_KEY = "person-batch-operation-v1";
const ARCHIVE_STORAGE_KEY = "face-archive-operation";

function makeUuid(): string {
  if (
    typeof crypto !== "undefined" &&
    typeof crypto.randomUUID === "function"
  ) {
    return crypto.randomUUID();
  }
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  bytes[6] = (bytes[6] & 15) | 64;
  bytes[8] = (bytes[8] & 63) | 128;
  const hex = [...bytes].map((b) => b.toString(16).padStart(2, "0")).join("");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

export type BackgroundTaskStatus =
  | "queued"
  | "running"
  | "complete"
  | "failed"
  | "rolled_back"
  | "recovery_needed"
  | "cancelled";

export type PersonBatchTask = {
  id: string;
  kind: "person-batch";
  action: PersonBatchAction;
  title: string;
  subtitle: string;
  people: string[];
  target?: string;
  thumbs: Record<string, string | null>;
  plan: PersonBatchPlan;
  status: BackgroundTaskStatus;
  createdAt: number;
  startedAt?: number;
  finishedAt?: number;
  needsFreshPreview?: boolean;
  progress?: PersonBatchProgress | null;
  result?: PersonBatchResult | null;
  error?: string;
};

export type PersonFileTask = {
  id: string;
  kind: "person-file";
  action: PersonFileAction;
  title: string;
  subtitle: string;
  person: string;
  targetPerson?: string;
  vids: string[];
  plan: PersonFilePlan;
  status: BackgroundTaskStatus;
  phase?: string;
  createdAt: number;
  startedAt?: number;
  finishedAt?: number;
  needsFreshPreview?: boolean;
  result?: PersonFileResult | null;
  error?: string;
};

export type ArchiveJobProgress = {
  id?: string;
  status?: string;
  done?: number;
  total?: number;
  stage?: string;
  vid?: string;
  error?: string;
};

export type ArchiveTaskResult = ArchiveResult;

export type ArchiveTask = {
  id: string;
  kind: "archive";
  title: string;
  subtitle: string;
  vids: string[];
  creators: string[];
  personName?: string;
  version: string;
  operationId: string;
  status: BackgroundTaskStatus;
  createdAt: number;
  startedAt?: number;
  finishedAt?: number;
  needsFreshPreview?: boolean;
  progress?: ArchiveJobProgress | null;
  result?: ArchiveTaskResult | null;
  error?: string;
};

export type AutoMergeTask = {
  id: string;
  kind: "auto-merge";
  title: string;
  subtitle: string;
  vids: string[];
  creators: string[];
  version: string;
  items: AutoWatermarkApplyItemInput[];
  operationId: string;
  status: BackgroundTaskStatus;
  createdAt: number;
  startedAt?: number;
  finishedAt?: number;
  progress?: ArchiveJobProgress | null;
  result?: AutoWatermarkApplyResult | null;
  error?: string;
};

export type ReviewMutationTask = {
  id: string;
  kind: "review-mutation";
  action: "confirm" | "defer" | "restore" | "undo";
  title: string;
  subtitle: string;
  vids: string[];
  creator?: string;
  status: BackgroundTaskStatus;
  createdAt: number;
  startedAt?: number;
  finishedAt?: number;
  error?: string;
};

export type BackgroundTask =
  | PersonBatchTask
  | PersonFileTask
  | ArchiveTask
  | AutoMergeTask
  | ReviewMutationTask;

export type BackgroundTaskEvent =
  | {
      type: "person-batch-settled";
      task: PersonBatchTask;
    }
  | {
      type: "person-file-settled";
      task: PersonFileTask;
    }
  | {
      type: "archive-settled";
      task: ArchiveTask;
    }
  | {
      type: "auto-merge-settled";
      task: AutoMergeTask;
    }
  | {
      type: "server-op-settled";
      settled: ServerSettledOp;
    };

interface BackgroundOperationsContextValue {
  tasks: BackgroundTask[];
  open: boolean;
  setOpen: (open: boolean) => void;
  lockedPeople: Map<string, ScopeLockInfo>;
  lockedVids: Map<string, ScopeLockInfo>;
  serverActiveOps: ServerActiveOp[];
  hasActiveWrite: boolean;
  enqueuePersonBatch: (input: {
    plan: PersonBatchPlan;
    people: string[];
    thumbs?: Record<string, string | null>;
  }) => string;
  enqueuePersonFile: (input: {
    plan: PersonFilePlan;
    person: string;
    label?: string;
  }) => string;
  enqueueArchive: (input: {
    vids: string[];
    version: string;
    creators: string[];
    sizeGb?: string;
    personName?: string;
  }) => string;
  enqueueAutoMerge: (input: {
    version: string;
    items: AutoWatermarkApplyItemInput[];
    creators: string[];
  }) => string;
  trackReviewMutation: <T>(input: {
    action: "confirm" | "defer" | "restore" | "undo";
    title: string;
    subtitle: string;
    vids: string[];
    creator?: string;
    operationId?: string;
    promise: Promise<T>;
  }) => Promise<T>;
  cancelQueuedTask: (taskId: string) => void;
  dismissTask: (taskId: string) => void;
  clearCompleted: () => void;
  reconcileTask: (taskId: string) => Promise<void>;
  subscribe: (listener: (event: BackgroundTaskEvent) => void) => () => void;
}

const BackgroundOperationsContext =
  createContext<BackgroundOperationsContextValue | null>(null);

export function useBackgroundOperations() {
  const ctx = useContext(BackgroundOperationsContext);
  if (!ctx) {
    throw new Error(
      "useBackgroundOperations must be used within <BackgroundOperationsProvider>",
    );
  }
  return ctx;
}

const fileActionLabels: Record<PersonFileAction, string> = {
  "rename-file": "重命名视频",
  "move-files": "移动视频",
  "delete-files": "删除视频",
  "review-delete": "移入回收站",
  "rename-person": "重命名人物",
  "merge-person": "合并人物",
  "delete-person": "删除人物",
};

function localPersonFileLockLabel(
  action: PersonFileAction,
  status: "running" | "queued" | "recovery_needed",
): string {
  if (status === "recovery_needed") return "待恢复";
  const running = status === "running";
  if (
    action === "delete-files" ||
    action === "review-delete" ||
    action === "delete-person"
  ) {
    return running ? "删除中" : "排队删除";
  }
  if (action === "merge-person") return running ? "合并中" : "排队合并";
  if (action === "rename-person" || action === "rename-file") {
    return running ? "改名中" : "排队改名";
  }
  if (action === "move-files") return running ? "移动中" : "排队移动";
  return running ? "处理中" : "排队中";
}

function setLocalLock(
  map: Map<string, ScopeLockInfo>,
  key: string,
  lock: ScopeLockInfo,
) {
  if (!key) return;
  const existing = map.get(key);
  if (!existing || lockStatusRank(lock.status) > lockStatusRank(existing.status)) {
    map.set(key, lock);
  }
}

function isActiveStatus(status: BackgroundTaskStatus) {
  return status === "queued" || status === "running";
}

function isTerminalStatus(status: BackgroundTaskStatus) {
  return (
    status === "complete" ||
    status === "failed" ||
    status === "rolled_back" ||
    status === "recovery_needed" ||
    status === "cancelled"
  );
}

function sameFileMoves(
  a: { src: string; dst: string; version: string; renamed?: boolean }[],
  b: { src: string; dst: string; version: string; renamed?: boolean }[],
) {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) {
    if (
      a[i].src !== b[i].src ||
      a[i].version !== b[i].version ||
      Boolean(a[i].renamed) !== Boolean(b[i].renamed)
    ) {
      return false;
    }
  }
  return true;
}

export function BackgroundOperationsProvider({
  children,
}: {
  children: ReactNode;
}) {
  const [tasks, setTasks] = useState<BackgroundTask[]>([]);
  const [open, setOpen] = useState(false);
  const tasksRef = useRef<BackgroundTask[]>(tasks);
  tasksRef.current = tasks;

  const listenersRef = useRef(new Set<(event: BackgroundTaskEvent) => void>());
  const runningIdRef = useRef<string | null>(null);
  const reconcilingRef = useRef(new Set<string>());
  const localTaskIdsRef = useRef(new Set<string>());
  const [serverSnapshot, setServerSnapshot] = useState<ServerLockSnapshot | null>(
    null,
  );
  const hasInitialSnapshotRef = useRef(false);
  const handledSettledOpIdsRef = useRef(new Set<string>());
  const sseFramesRef = useRef(0);

  const rememberLocalOpId = useCallback((id?: string) => {
    if (!id) return;
    const set = localTaskIdsRef.current;
    set.add(id);
    if (set.size > 256) {
      const oldest = set.values().next().value;
      if (oldest !== undefined) set.delete(oldest);
    }
  }, []);

  const emit = useCallback((event: BackgroundTaskEvent) => {
    for (const listener of listenersRef.current) {
      try {
        listener(event);
      } catch {
        // Ignore subscriber errors so queue progression never stalls.
      }
    }
  }, []);

  const applyServerSnapshot = useCallback(
    (snapshot: ServerLockSnapshot) => {
      setServerSnapshot(snapshot);
      const settled = snapshot.last_settled;
      if (!hasInitialSnapshotRef.current) {
        hasInitialSnapshotRef.current = true;
        if (settled?.op_id) {
          handledSettledOpIdsRef.current.add(settled.op_id);
        }
        return;
      }
      if (!settled?.op_id) return;
      const handled = handledSettledOpIdsRef.current;
      if (handled.has(settled.op_id)) return;
      handled.add(settled.op_id);
      if (handled.size > 256) {
        const oldest = handled.values().next().value;
        if (oldest !== undefined) handled.delete(oldest);
      }
      const isLocal =
        localTaskIdsRef.current.has(settled.op_id) ||
        tasksRef.current.some(
          (t) =>
            t.id === settled.op_id ||
            (t.kind === "archive" && t.operationId === settled.op_id) ||
            (t.kind === "auto-merge" && t.operationId === settled.op_id) ||
            (t.kind === "person-batch" && t.plan.id === settled.op_id) ||
            (t.kind === "person-file" && t.plan.id === settled.op_id),
        );
      if (isLocal || settled.outcome === "cancelled") return;

      invalidateReviewCaches({
        includePeople:
          isDestructivePersonAction(settled.action) ||
          settled.people.length > 0,
      });
      deleteWorkbenchCache("persons:list");
      deleteWorkbenchCache("persons:stats");
      for (const p of settled.people) {
        if (p) deleteWorkbenchCache("persons:library:" + p);
      }
      deleteWorkbenchCache("integration:plan");
      deleteWorkbenchCache("staging:list:");
      emit({ type: "server-op-settled", settled });
    },
    [emit],
  );

  // Subscribe to server-authoritative entity lock SSE stream with initial GET fallback & backoff reconnect.
  useEffect(() => {
    if (typeof window === "undefined") return;
    let disposed = false;
    let es: EventSource | null = null;
    let reconnectTimer: ReturnType<typeof setTimeout> | undefined;
    let fallbackController: AbortController | null = null;
    let retryAttempt = 0;

    const fetchFallbackSnapshot = async () => {
      fallbackController?.abort();
      const controller = new AbortController();
      fallbackController = controller;
      const framesAtStart = sseFramesRef.current;
      try {
        const res = await fetch("/api/locks", {
          cache: "no-store",
          signal: controller.signal,
        });
        if (!res.ok || disposed || controller.signal.aborted) return;
        const text = await res.text();
        if (
          disposed ||
          controller.signal.aborted ||
          sseFramesRef.current !== framesAtStart
        ) {
          return;
        }
        const snap = parseLockSnapshotJson(text);
        if (snap) applyServerSnapshot(snap);
      } catch {
        // Ignore transient fallback failures; SSE reconnect will retry.
      }
    };

    const scheduleReconnect = () => {
      if (disposed || reconnectTimer !== undefined) return;
      const base = Math.min(15_000, 1000 * 2 ** Math.min(retryAttempt, 4));
      const jitter = Math.floor(Math.random() * 400);
      retryAttempt++;
      reconnectTimer = setTimeout(() => {
        reconnectTimer = undefined;
        if (disposed) return;
        void fetchFallbackSnapshot();
        connectStream();
      }, base + jitter);
    };

    const connectStream = () => {
      if (disposed || typeof EventSource === "undefined") return;
      if (es) {
        es.close();
        es = null;
      }
      const source = new EventSource("/api/locks/stream");
      es = source;

      const handleEvent = (ev: MessageEvent) => {
        if (disposed) return;
        const snap = parseLockSnapshotJson(String(ev.data ?? ""));
        if (!snap) return;
        sseFramesRef.current++;
        applyServerSnapshot(snap);
      };

      source.addEventListener("locks", handleEvent as EventListener);
      source.onmessage = handleEvent;
      source.onopen = () => {
        retryAttempt = 0;
      };
      source.onerror = () => {
        if (disposed) return;
        if (es === source) {
          source.close();
          es = null;
        }
        scheduleReconnect();
      };
    };

    const onVisibilityChange = () => {
      if (
        disposed ||
        document.visibilityState !== "visible" ||
        typeof EventSource === "undefined"
      ) {
        return;
      }
      if (!es || es.readyState === EventSource.CLOSED) {
        if (reconnectTimer !== undefined) {
          clearTimeout(reconnectTimer);
          reconnectTimer = undefined;
        }
        retryAttempt = 0;
        void fetchFallbackSnapshot();
        connectStream();
      }
    };

    void fetchFallbackSnapshot();
    connectStream();
    document.addEventListener("visibilitychange", onVisibilityChange);

    return () => {
      disposed = true;
      if (reconnectTimer !== undefined) clearTimeout(reconnectTimer);
      fallbackController?.abort();
      document.removeEventListener("visibilitychange", onVisibilityChange);
      if (es) {
        es.close();
        es = null;
      }
    };
  }, [applyServerSnapshot]);

  const subscribe = useCallback(
    (listener: (event: BackgroundTaskEvent) => void) => {
      listenersRef.current.add(listener);
      return () => {
        listenersRef.current.delete(listener);
      };
    },
    [],
  );

  const updateTask = useCallback(
    (id: string, updater: (task: BackgroundTask) => BackgroundTask) => {
      setTasks((prev) => prev.map((t) => (t.id === id ? updater(t) : t)));
    },
    [],
  );

  // Restore any interrupted operation from localStorage on mount
  useEffect(() => {
    try {
      const rawFile = localStorage.getItem(FILE_OPERATION_STORAGE_KEY);
      if (rawFile) {
        const plan = JSON.parse(rawFile) as PersonFilePlan;
        if (plan?.backend === "rust" && plan.id && plan.request?.id === plan.id) {
          rememberLocalOpId(plan.id);
          const payloadVids = Array.isArray(plan.request.payload?.vids)
            ? plan.request.payload.vids.filter((v): v is string => typeof v === "string")
            : [];
          const restoredVids = [
            ...new Set([
              ...payloadVids,
              ...plan.files.flatMap((f) => f.vids || []),
            ]),
          ];
          const restoredTitle =
            plan.action === "review-delete"
              ? "移入回收站 · 人工确认"
              : `${fileActionLabels[plan.action] || "文件操作"} · ${plan.person}${plan.target_person ? ` → ${plan.target_person}` : ""}`;
          setTasks((prev) =>
            prev.some((t) => t.id === plan.id)
              ? prev
              : [
                  ...prev,
                  {
                    id: plan.id,
                    kind: "person-file",
                    action: plan.action,
                    title: restoredTitle,
                    subtitle: `${plan.files.length} 个文件 · ${(plan.total_bytes / 2 ** 20).toFixed(1)} MB`,
                    person: plan.person,
                    targetPerson: plan.target_person,
                    vids: restoredVids,
                    plan,
                    status: "running",
                    createdAt: Date.now(),
                    startedAt: Date.now(),
                  },
                ],
          );
        }
      }
    } catch {
      // Ignore malformed storage
    }
    try {
      const rawBatch = localStorage.getItem(BATCH_STORAGE_KEY);
      if (rawBatch) {
        const saved = JSON.parse(rawBatch) as {
          plan: PersonBatchPlan;
          people: string[];
          thumbs?: Record<string, string | null>;
        };
        if (saved?.plan?.id) {
          rememberLocalOpId(saved.plan.id);
          const merging = saved.plan.request.action === "merge";
          const totalFiles = saved.plan.plans.reduce(
            (s, p) => s + p.files.length,
            0,
          );
          const totalMb = (
            saved.plan.plans.reduce((s, p) => s + p.total_bytes, 0) /
            2 ** 20
          ).toFixed(1);
          setTasks((prev) =>
            prev.some((t) => t.id === saved.plan.id)
              ? prev
              : [
                  ...prev,
                  {
                    id: saved.plan.id,
                    kind: "person-batch",
                    action: saved.plan.request.action,
                    title: merging
                      ? `合并 ${saved.people.filter((p) => p !== saved.plan.request.target).join("、")} → ${saved.plan.request.target}`
                      : `删除人物 ${saved.people.join("、")}`,
                    subtitle: `${totalFiles} 个文件 · ${totalMb} MB`,
                    people: saved.people,
                    target: saved.plan.request.target,
                    thumbs: saved.thumbs || {},
                    plan: saved.plan,
                    status: "running",
                    createdAt: Date.now(),
                    startedAt: Date.now(),
                  },
                ],
          );
        }
      }
    } catch {
      // Ignore malformed storage
    }
  }, [rememberLocalOpId]);

  const enqueuePersonBatch = useCallback(
    ({
      plan,
      people,
      thumbs = {},
    }: {
      plan: PersonBatchPlan;
      people: string[];
      thumbs?: Record<string, string | null>;
    }) => {
      const merging = plan.request.action === "merge";
      const sources = people.filter((p) => p !== plan.request.target);
      const totalFiles = plan.plans.reduce((sum, p) => sum + p.files.length, 0);
      const totalMb = (
        plan.plans.reduce((sum, p) => sum + p.total_bytes, 0) /
        2 ** 20
      ).toFixed(1);
      const hasPriorActive = tasksRef.current.some((t) =>
        isActiveStatus(t.status),
      );
      const task: PersonBatchTask = {
        id: plan.id,
        kind: "person-batch",
        action: plan.request.action,
        title: merging
          ? `合并 ${sources.slice(0, 2).join("、")}${sources.length > 2 ? ` 等 ${sources.length} 人` : ""} → ${plan.request.target}`
          : `删除 ${people.slice(0, 2).join("、")}${people.length > 2 ? ` 等 ${people.length} 人` : ""}`,
        subtitle: `${totalFiles} 个文件 · ${totalMb} MB`,
        people,
        target: plan.request.target,
        thumbs,
        plan,
        status: "queued",
        createdAt: Date.now(),
        needsFreshPreview: hasPriorActive,
      };
      rememberLocalOpId(task.id);
      setTasks((prev) => [...prev.filter((t) => t.id !== task.id), task]);
      return task.id;
    },
    [rememberLocalOpId],
  );

  const enqueuePersonFile = useCallback(
    ({
      plan,
      person,
      label: scopeLabel,
    }: {
      plan: PersonFilePlan;
      person: string;
      label?: string;
    }) => {
      const payloadVids = Array.isArray(plan.request.payload?.vids)
        ? plan.request.payload.vids.filter((v): v is string => typeof v === "string")
        : [];
      const vids = [
        ...new Set([
          ...(plan.request.files?.map((f) => f.vid) || []),
          ...payloadVids,
          ...plan.files.flatMap((f) => f.vids || []),
        ]),
      ];
      const hasPriorActive = tasksRef.current.some((t) =>
        isActiveStatus(t.status),
      );
      const label = fileActionLabels[plan.action] || "文件操作";
      const title =
        plan.action === "review-delete"
          ? scopeLabel
            ? `${label} · ${scopeLabel}`
            : `${label} · 人工确认`
          : plan.action === "merge-person" || plan.action === "rename-person"
            ? `${label} · ${person} → ${plan.target_person}`
            : `${label} · ${person}`;
      const subtitle =
        plan.files.length > 0
          ? `${plan.files.length} 个文件 · ${(plan.total_bytes / 2 ** 20).toFixed(1)} MB`
          : "更新人物资料与归属记录";
      const task: PersonFileTask = {
        id: plan.id,
        kind: "person-file",
        action: plan.action,
        title,
        subtitle,
        person,
        targetPerson: plan.target_person,
        vids,
        plan,
        status: "queued",
        createdAt: Date.now(),
        needsFreshPreview: hasPriorActive,
      };
      rememberLocalOpId(task.id);
      setTasks((prev) => [...prev.filter((t) => t.id !== task.id), task]);
      return task.id;
    },
    [rememberLocalOpId],
  );

  const enqueueArchive = useCallback(
    ({
      vids,
      version,
      creators,
      sizeGb,
      personName,
    }: {
      vids: string[];
      version: string;
      creators: string[];
      sizeGb?: string;
      personName?: string;
    }) => {
      const operationId = makeUuid();
      const uniqueCreators = [...new Set(creators.filter(Boolean))];
      const hasPriorActive = tasksRef.current.some((t) =>
        isActiveStatus(t.status),
      );
      const creatorLabel =
        uniqueCreators.length === 1
          ? uniqueCreators[0]
          : uniqueCreators.length > 1
            ? `${uniqueCreators[0]} 等 ${uniqueCreators.length} 人`
            : personName || "所选视频";
      const task: ArchiveTask = {
        id: operationId,
        kind: "archive",
        title: `归档 · ${creatorLabel}`,
        subtitle: `${vids.length} 个视频${sizeGb ? ` · ${sizeGb} GB` : ""}`,
        vids,
        creators: uniqueCreators,
        personName,
        version,
        operationId,
        status: "queued",
        createdAt: Date.now(),
        needsFreshPreview: hasPriorActive,
      };
      rememberLocalOpId(operationId);
      setTasks((prev) => [...prev, task]);
      return task.id;
    },
    [rememberLocalOpId],
  );

  const enqueueAutoMerge = useCallback(
    ({
      version,
      items,
      creators,
    }: {
      version: string;
      items: AutoWatermarkApplyItemInput[];
      creators: string[];
    }) => {
      const operationId = makeUuid();
      const uniqueCreators = [...new Set(creators.filter(Boolean))];
      const creatorLabel =
        uniqueCreators.length === 1
          ? uniqueCreators[0]
          : uniqueCreators.length > 1
            ? `${uniqueCreators[0]} 等 ${uniqueCreators.length} 人`
            : `${items.length} 个视频`;
      const task: AutoMergeTask = {
        id: operationId,
        kind: "auto-merge",
        title: `自动合入 · ${creatorLabel}`,
        subtitle: `${items.length} 个视频 · ${uniqueCreators.length} 位人物`,
        vids: items.map((item) => item.vid),
        creators: uniqueCreators,
        version,
        items,
        operationId,
        status: "queued",
        createdAt: Date.now(),
      };
      rememberLocalOpId(operationId);
      setTasks((prev) => [...prev, task]);
      return task.id;
    },
    [rememberLocalOpId],
  );

  const trackReviewMutation = useCallback(
    async <T,>({
      action,
      title,
      subtitle,
      vids,
      creator,
      operationId,
      promise,
    }: {
      action: "confirm" | "defer" | "restore" | "undo";
      title: string;
      subtitle: string;
      vids: string[];
      creator?: string;
      operationId?: string;
      promise: Promise<T>;
    }): Promise<T> => {
      const id = operationId || makeUuid();
      rememberLocalOpId(id);
      const now = Date.now();
      const task: ReviewMutationTask = {
        id,
        kind: "review-mutation",
        action,
        title,
        subtitle,
        vids,
        creator,
        status: "running",
        createdAt: now,
        startedAt: now,
      };
      setTasks((prev) => [...prev, task]);
      try {
        const result = await promise;
        deleteWorkbenchCache("integration:plan");
        if (creator) {
          deleteWorkbenchCache("persons:library:" + creator);
        }
        invalidateReviewCaches();
        updateTask(id, (t) => ({
          ...t,
          status: "complete",
          finishedAt: Date.now(),
        }));
        return result;
      } catch (e) {
        const message = (e as Error).message || "人工确认操作失败";
        setOpen(true);
        updateTask(id, (t) => ({
          ...t,
          status: "failed",
          error: message,
          finishedAt: Date.now(),
        }));
        throw e;
      }
    },
    [rememberLocalOpId, updateTask],
  );

  const cancelQueuedTask = useCallback((taskId: string) => {
    setTasks((prev) =>
      prev.filter((t) => !(t.id === taskId && t.status === "queued")),
    );
  }, []);

  const dismissTask = useCallback((taskId: string) => {
    setTasks((prev) =>
      prev.filter((t) => !(t.id === taskId && isTerminalStatus(t.status))),
    );
  }, []);

  const clearCompleted = useCallback(() => {
    setTasks((prev) => prev.filter((t) => !isTerminalStatus(t.status)));
  }, []);

  // Auto-clear when all tasks have completed and the popover is closed
  useEffect(() => {
    if (open || tasks.length === 0) return;
    const allComplete = tasks.every((t) => t.status === "complete");
    if (!allComplete) return;
    const timer = setTimeout(() => {
      setTasks((prev) =>
        prev.every((t) => t.status === "complete") ? [] : prev,
      );
    }, 3000);
    return () => clearTimeout(timer);
  }, [tasks, open]);

  // Execute a PersonBatchTask
  const runPersonBatchTask = useCallback(
    async (initialTask: PersonBatchTask) => {
      let currentPlan = initialTask.plan;
      const pollController = new AbortController();
      let pollTimer: ReturnType<typeof setTimeout> | undefined;

      const startPolling = (batchId: string) => {
        const poll = async () => {
          try {
            const next = await api<PersonBatchProgress>(
              `/api/persons/batch?id=${encodeURIComponent(batchId)}`,
              { signal: pollController.signal },
            );
            if (!pollController.signal.aborted) {
              updateTask(initialTask.id, (t) =>
                t.kind === "person-batch" ? { ...t, progress: next } : t,
              );
            }
          } catch {
            // Keep polling while apply is in flight
          }
          if (!pollController.signal.aborted) {
            pollTimer = setTimeout(() => void poll(), 600);
          }
        };
        void poll();
      };

      const refreshPreviewIfUnchanged = async (): Promise<PersonBatchPlan> => {
        const fresh = await api<PersonBatchPlan>("/api/persons/batch", {
          method: "POST",
          body: JSON.stringify({
            mode: "preview",
            request: currentPlan.request,
          }),
        });
        if (fresh.plans.length !== currentPlan.plans.length) {
          throw new Error("排队期间所选人物列表已变化，请重新预览确认");
        }
        for (let i = 0; i < fresh.plans.length; i++) {
          const prevChild = currentPlan.plans[i];
          const nextChild = fresh.plans[i];
          if (
            prevChild.person !== nextChild.person ||
            !sameFileMoves(prevChild.files, nextChild.files)
          ) {
            throw new Error(
              `排队期间「${prevChild.person}」的文件或冲突范围已变化，请重新预览确认`,
            );
          }
        }
        return fresh;
      };

      try {
        if (initialTask.needsFreshPreview) {
          currentPlan = await refreshPreviewIfUnchanged();
          rememberLocalOpId(currentPlan.id);
          updateTask(initialTask.id, (t) =>
            t.kind === "person-batch" ? { ...t, plan: currentPlan } : t,
          );
        }
        try {
          localStorage.setItem(
            BATCH_STORAGE_KEY,
            JSON.stringify({
              plan: currentPlan,
              people: initialTask.people,
              thumbs: initialTask.thumbs,
            }),
          );
        } catch {}

        startPolling(currentPlan.id);

        let saved: PersonBatchResult;
        try {
          saved = await api<PersonBatchResult>("/api/persons/batch", {
            method: "POST",
            body: JSON.stringify({
              mode: "apply",
              id: currentPlan.id,
              version: currentPlan.version,
            }),
          });
        } catch (applyErr) {
          const msg = (applyErr as Error).message || "";
          if (
            !initialTask.needsFreshPreview &&
            msg.includes("人物、文件或账本已变化，请重新预览")
          ) {
            pollController.abort();
            clearTimeout(pollTimer);
            currentPlan = await refreshPreviewIfUnchanged();
            rememberLocalOpId(currentPlan.id);
            updateTask(initialTask.id, (t) =>
              t.kind === "person-batch" ? { ...t, plan: currentPlan } : t,
            );
            saved = await api<PersonBatchResult>("/api/persons/batch", {
              method: "POST",
              body: JSON.stringify({
                mode: "apply",
                id: currentPlan.id,
                version: currentPlan.version,
              }),
            });
          } else {
            throw applyErr;
          }
        }

        pollController.abort();
        clearTimeout(pollTimer);

        for (const item of saved.completed) {
          deleteWorkbenchCache("persons:library:" + item.person);
        }
        invalidatePersonLibraryCaches([
          ...initialTask.people,
          saved.target,
        ]);
        invalidateReviewCaches({ resetSession: true, includePeople: true });

        const nextStatus: BackgroundTaskStatus = saved.ok
          ? "complete"
          : saved.recovery_needed
            ? "recovery_needed"
            : "failed";
        if (saved.ok) {
          try {
            localStorage.removeItem(BATCH_STORAGE_KEY);
          } catch {}
        } else {
          setOpen(true);
        }

        const finishedAt = Date.now();
        settleBackgroundTask({
          tasks: tasksRef.current,
          fallback: initialTask,
          apply: (t) => ({
            ...t,
            plan: currentPlan,
            status: nextStatus,
            result: saved,
            error: saved.error,
            finishedAt,
          }),
          updateTask,
          emit: (settledTask) => {
            emit({ type: "person-batch-settled", task: settledTask });
          },
        });
      } catch (e) {
        pollController.abort();
        clearTimeout(pollTimer);
        const message = (e as Error).message || "批次执行异常";
        setOpen(true);
        const finishedAt = Date.now();
        settleBackgroundTask({
          tasks: tasksRef.current,
          fallback: initialTask,
          apply: (t) => ({
            ...t,
            plan: currentPlan,
            status: "failed" as const,
            error: `${message}（可点击「核对恢复」查询同一批次结果）`,
            finishedAt,
          }),
          updateTask,
          emit: (settledTask) => {
            emit({ type: "person-batch-settled", task: settledTask });
          },
        });
      }
    },
    [emit, updateTask],
  );

  // Execute a PersonFileTask
  const runPersonFileTask = useCallback(
    async (initialTask: PersonFileTask) => {
      let currentPlan = initialTask.plan;
      const pollController = new AbortController();
      let pollTimer: ReturnType<typeof setTimeout> | undefined;

      type FileReceipt = {
        id: string;
        status: string;
        revision?: number;
        phase?: string;
        result?: PersonFileResult;
        error?: string;
        failure?: { path?: string; step?: string };
      };

      const refreshPreviewIfUnchanged = async (): Promise<PersonFilePlan> => {
        const freshRequest = { ...currentPlan.request, id: undefined };
        const fresh = await api<PersonFilePlan>("/api/persons/manage", {
          method: "POST",
          body: JSON.stringify({ mode: "preview", request: freshRequest }),
        });
        if (!sameFileMoves(currentPlan.files, fresh.files)) {
          throw new Error("排队期间所选文件或目标冲突范围已变化，请重新预览确认");
        }
        return fresh;
      };

      try {
        if (initialTask.needsFreshPreview && currentPlan.backend === "rust") {
          currentPlan = await refreshPreviewIfUnchanged();
          rememberLocalOpId(currentPlan.id);
          updateTask(initialTask.id, (t) =>
            t.kind === "person-file" ? { ...t, plan: currentPlan } : t,
          );
        }
        if (currentPlan.backend === "rust") {
          try {
            localStorage.setItem(
              FILE_OPERATION_STORAGE_KEY,
              JSON.stringify(currentPlan),
            );
          } catch {}
          const poll = async () => {
            try {
              const receipt = await api<FileReceipt>(
                `/api/persons/manage?id=${encodeURIComponent(currentPlan.id)}`,
                { signal: pollController.signal },
              );
              if (!pollController.signal.aborted) {
                updateTask(initialTask.id, (t) =>
                  t.kind === "person-file"
                    ? { ...t, phase: receipt.phase || "" }
                    : t,
                );
              }
            } catch {}
            if (!pollController.signal.aborted) {
              pollTimer = setTimeout(() => void poll(), 800);
            }
          };
          void poll();
        }

        type ApplyOutcome =
          | PersonFileResult
          | { id: string; status: string; accepted: true };
        const submitApply = () =>
          api<ApplyOutcome>("/api/persons/manage", {
            method: "POST",
            body: JSON.stringify({
              mode: "apply",
              async: true,
              request: currentPlan.request,
              version: currentPlan.version,
            }),
          });

        let outcome: ApplyOutcome;
        try {
          outcome = await submitApply();
        } catch (applyErr) {
          const msg = (applyErr as Error).message || "";
          if (
            !initialTask.needsFreshPreview &&
            currentPlan.backend === "rust" &&
            msg.includes("变化")
          ) {
            pollController.abort();
            clearTimeout(pollTimer);
            currentPlan = await refreshPreviewIfUnchanged();
            rememberLocalOpId(currentPlan.id);
            updateTask(initialTask.id, (t) =>
              t.kind === "person-file" ? { ...t, plan: currentPlan } : t,
            );
            outcome = await submitApply();
          } else {
            throw applyErr;
          }
        }

        pollController.abort();
        clearTimeout(pollTimer);

        let result: PersonFileResult;
        if (
          "accepted" in outcome &&
          outcome.accepted === true &&
          !("ok" in outcome)
        ) {
          let transientErrors = 0;
          let terminalReceipt: FileReceipt | null = null;
          while (true) {
            await new Promise((resolve) => setTimeout(resolve, 1000));
            let receipt: FileReceipt;
            try {
              receipt = await api<FileReceipt>(
                `/api/persons/manage?id=${encodeURIComponent(currentPlan.id)}`,
              );
              transientErrors = 0;
            } catch (pollErr) {
              transientErrors++;
              if (transientErrors >= 5) {
                throw pollErr;
              }
              continue;
            }
            updateTask(initialTask.id, (t) =>
              t.kind === "person-file"
                ? { ...t, phase: receipt.phase || t.phase || "" }
                : t,
            );
            if (
              [
                "complete",
                "rolled_back",
                "failed",
                "recovery_needed",
                "cancelled",
              ].includes(receipt.status)
            ) {
              terminalReceipt = receipt;
              break;
            }
          }
          if (terminalReceipt.status === "complete" && terminalReceipt.result) {
            result = terminalReceipt.result;
          } else {
            const detail =
              [
                terminalReceipt.error,
                terminalReceipt.failure?.step,
                terminalReceipt.failure?.path,
              ]
                .filter(Boolean)
                .join(" · ") || "文件操作未完成";
            const nextStatus: BackgroundTaskStatus =
              terminalReceipt.status === "rolled_back"
                ? "rolled_back"
                : terminalReceipt.status === "recovery_needed"
                  ? "recovery_needed"
                  : "failed";
            if (nextStatus === "rolled_back") {
              try {
                localStorage.removeItem(FILE_OPERATION_STORAGE_KEY);
              } catch {}
            }
            invalidatePersonLibraryCaches([
              initialTask.person,
              initialTask.targetPerson,
              terminalReceipt.result?.person,
            ]);
            if (
              currentPlan.action === "review-delete" ||
              currentPlan.action === "delete-files" ||
              currentPlan.action === "delete-person"
            ) {
              deleteWorkbenchCache("staging:list:");
            }
            invalidateReviewCaches({
              resetSession: currentPlan.action !== "review-delete",
              includePeople: currentPlan.action !== "review-delete",
            });
            setOpen(true);
            const finishedAt = Date.now();
            settleBackgroundTask({
              tasks: tasksRef.current,
              fallback: initialTask,
              apply: (t) => ({
                ...t,
                plan: currentPlan,
                status: nextStatus,
                result: terminalReceipt.result || t.result,
                error: detail,
                finishedAt,
              }),
              updateTask,
              emit: (settledTask) => {
                emit({ type: "person-file-settled", task: settledTask });
              },
            });
            return;
          }
        } else {
          result = outcome as PersonFileResult;
        }

        try {
          localStorage.removeItem(FILE_OPERATION_STORAGE_KEY);
        } catch {}

        invalidatePersonLibraryCaches([
          initialTask.person,
          initialTask.targetPerson,
          result.person,
        ]);
        if (
          currentPlan.action === "review-delete" ||
          currentPlan.action === "delete-files" ||
          currentPlan.action === "delete-person"
        ) {
          deleteWorkbenchCache("staging:list:");
        }
        invalidateReviewCaches({
          resetSession: currentPlan.action !== "review-delete",
          includePeople: currentPlan.action !== "review-delete",
        });

        const finishedAt = Date.now();
        settleBackgroundTask({
          tasks: tasksRef.current,
          fallback: initialTask,
          apply: (t) => ({
            ...t,
            plan: currentPlan,
            status: "complete" as const,
            result,
            error: "",
            finishedAt,
          }),
          updateTask,
          emit: (settledTask) => {
            emit({ type: "person-file-settled", task: settledTask });
          },
        });
      } catch (e) {
        pollController.abort();
        clearTimeout(pollTimer);
        const message = (e as Error).message || "文件操作失败";
        setOpen(true);
        const finishedAt = Date.now();
        settleBackgroundTask({
          tasks: tasksRef.current,
          fallback: initialTask,
          apply: (t) => ({
            ...t,
            plan: currentPlan,
            status: "failed" as const,
            error: message,
            finishedAt,
          }),
          updateTask,
          emit: (settledTask) => {
            emit({ type: "person-file-settled", task: settledTask });
          },
        });
      }
    },
    [emit, updateTask],
  );

  // Execute an ArchiveTask
  const runArchiveTask = useCallback(
    async (initialTask: ArchiveTask) => {
      let currentVersion = initialTask.version;
      const pollController = new AbortController();
      let pollTimer: ReturnType<typeof setTimeout> | undefined;

      try {
        if (initialTask.needsFreshPreview) {
          const fresh = await api<{
            version: string;
            rows: { vid: string; status: string }[];
          }>("/api/plan/preview");
          const rowMap = new Map(fresh.rows.map((r) => [r.vid, r]));
          const invalid = initialTask.vids.filter(
            (v) => rowMap.get(v)?.status !== "ok",
          );
          if (invalid.length > 0) {
            throw new Error(
              `排队期间有 ${invalid.length} 个待归档视频状态发生变化，请刷新待归档列表后重新确认`,
            );
          }
          currentVersion = fresh.version;
        }
        try {
          localStorage.setItem(ARCHIVE_STORAGE_KEY, initialTask.operationId);
        } catch {}

        const poll = async () => {
          try {
            const prog = await api<{
              job: ArchiveJobProgress | null;
              result?: ArchiveTaskResult | null;
            }>(
              `/api/plan/progress?id=${encodeURIComponent(initialTask.operationId)}`,
              { signal: pollController.signal },
            );
            if (!pollController.signal.aborted && prog.job) {
              updateTask(initialTask.id, (t) =>
                t.kind === "archive" ? { ...t, progress: prog.job } : t,
              );
            }
          } catch {}
          if (!pollController.signal.aborted) {
            pollTimer = setTimeout(() => void poll(), 700);
          }
        };
        void poll();

        const response = await fetch("/api/plan/apply", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({
            vids: initialTask.vids,
            version: currentVersion,
            operation_id: initialTask.operationId,
            async: true,
          }),
        });
        const data = await response.json();
        pollController.abort();
        clearTimeout(pollTimer);

        let result: ArchiveTaskResult;
        if (response.status === 202 && data?.accepted === true) {
          let transientErrors = 0;
          let polledResult: ArchiveTaskResult | null = null;
          while (true) {
            await new Promise((resolve) => setTimeout(resolve, 1000));
            let prog: {
              job: ArchiveJobProgress | null;
              result?: ArchiveTaskResult | null;
            };
            try {
              prog = await api<{
                job: ArchiveJobProgress | null;
                result?: ArchiveTaskResult | null;
              }>(
                `/api/plan/progress?id=${encodeURIComponent(initialTask.operationId)}`,
              );
              transientErrors = 0;
            } catch (pollErr) {
              transientErrors++;
              if (transientErrors >= 5) {
                throw pollErr;
              }
              continue;
            }
            if (prog.job) {
              updateTask(initialTask.id, (t) =>
                t.kind === "archive" ? { ...t, progress: prog.job } : t,
              );
            }
            if (prog.result) {
              polledResult = prog.result;
              break;
            }
            if (
              prog.job &&
              (prog.job.status === "complete" || prog.job.status === "error")
            ) {
              throw new Error(prog.job.error || "归档结果尚未返回");
            }
          }
          result = polledResult;
        } else {
          result = response.ok
            ? data
            : {
                ok: false,
                moved: 0,
                remaining: initialTask.vids.length,
                errors: [data.error || "归档请求未被接受"],
              };
        }

        invalidatePersonLibraryCaches(initialTask.creators);
        deleteWorkbenchCache("integration:plan");
        invalidateReviewCaches({ resetSession: true });

        const nextStatus: BackgroundTaskStatus = result.recovery_needed
          ? "recovery_needed"
          : result.rolled_back
            ? "rolled_back"
            : result.ok && !result.errors?.length
              ? "complete"
              : result.moved > 0
                ? "complete"
                : "failed";

        if (nextStatus === "complete") {
          try {
            localStorage.removeItem(ARCHIVE_STORAGE_KEY);
          } catch {}
        } else {
          setOpen(true);
        }

        const finishedAt = Date.now();
        settleBackgroundTask({
          tasks: tasksRef.current,
          fallback: initialTask,
          apply: (t) => ({
            ...t,
            version: currentVersion,
            status: nextStatus,
            result,
            error: result.errors?.[0] || "",
            finishedAt,
          }),
          updateTask,
          emit: (settledTask) => {
            emit({ type: "archive-settled", task: settledTask });
          },
        });
      } catch (e) {
        pollController.abort();
        clearTimeout(pollTimer);
        const message = (e as Error).message || "归档连接中断";
        setOpen(true);
        const finishedAt = Date.now();
        settleBackgroundTask({
          tasks: tasksRef.current,
          fallback: initialTask,
          apply: (t) => ({
            ...t,
            status: "failed" as const,
            error: `${message}（可点击「核对恢复」查询本次归档结果）`,
            finishedAt,
          }),
          updateTask,
          emit: (settledTask) => {
            emit({ type: "archive-settled", task: settledTask });
          },
        });
      }
    },
    [emit, updateTask],
  );

  // Execute an AutoMergeTask
  const runAutoMergeTask = useCallback(
    async (initialTask: AutoMergeTask) => {
      const pollController = new AbortController();
      let pollTimer: ReturnType<typeof setTimeout> | undefined;

      try {
        const poll = async () => {
          try {
            const prog = await api<{
              job: ArchiveJobProgress | null;
              result?: AutoWatermarkApplyResult | null;
            }>(
              `/api/plan/progress?id=${encodeURIComponent(initialTask.operationId)}`,
              { signal: pollController.signal },
            );
            if (!pollController.signal.aborted && prog.job) {
              updateTask(initialTask.id, (t) =>
                t.kind === "auto-merge" ? { ...t, progress: prog.job } : t,
              );
            }
          } catch {}
          if (!pollController.signal.aborted) {
            pollTimer = setTimeout(() => void poll(), 700);
          }
        };
        void poll();

        const response = await fetch("/api/review/auto-merge/apply", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({
            operation_id: initialTask.operationId,
            version: initialTask.version,
            items: initialTask.items,
            async: true,
          }),
        });
        const data = await response.json().catch(() => ({}));
        pollController.abort();
        clearTimeout(pollTimer);

        let result: AutoWatermarkApplyResult;
        if (response.status === 202 && data?.accepted === true) {
          let transientErrors = 0;
          let polledResult: AutoWatermarkApplyResult | null = null;
          while (true) {
            await new Promise((resolve) => setTimeout(resolve, 1000));
            let prog: {
              job: ArchiveJobProgress | null;
              result?: AutoWatermarkApplyResult | null;
            };
            try {
              prog = await api<{
                job: ArchiveJobProgress | null;
                result?: AutoWatermarkApplyResult | null;
              }>(
                `/api/plan/progress?id=${encodeURIComponent(initialTask.operationId)}`,
              );
              transientErrors = 0;
            } catch (pollErr) {
              transientErrors++;
              if (transientErrors >= 5) {
                throw pollErr;
              }
              continue;
            }
            if (prog.job) {
              updateTask(initialTask.id, (t) =>
                t.kind === "auto-merge" ? { ...t, progress: prog.job } : t,
              );
            }
            if (prog.result) {
              polledResult = prog.result;
              break;
            }
            if (
              prog.job &&
              (prog.job.status === "complete" || prog.job.status === "error")
            ) {
              throw new Error(prog.job.error || "自动合入结果尚未返回");
            }
          }
          result = polledResult;
        } else {
          result = response.ok
            ? (data as AutoWatermarkApplyResult)
            : {
                ok: false,
                archived: 0,
                fallback_queued: 0,
                skipped: 0,
                results: [],
                errors: [data?.error || "自动合入请求未被接受"],
              };
        }

        invalidatePersonLibraryCaches(initialTask.creators);
        deleteWorkbenchCache("integration:plan");
        invalidateReviewCaches({ resetSession: true });

        const archivedCount = result.archived ?? result.moved ?? 0;
        const fallbackCount = result.fallback_queued ?? 0;
        const skippedCount = result.skipped ?? 0;
        const nextStatus: BackgroundTaskStatus = result.recovery_needed
          ? "recovery_needed"
          : result.rolled_back
            ? "rolled_back"
            : result.ok || archivedCount > 0 || fallbackCount > 0
              ? "complete"
              : "failed";

        if (nextStatus !== "complete") {
          setOpen(true);
          toast.error(result.errors?.[0] || "自动合入未完成，请核对后台任务");
        } else if (fallbackCount > 0) {
          toast.warning(
            `已合入 ${archivedCount} 个，${fallbackCount} 个转入待归档${skippedCount ? `，${skippedCount} 个已跳过` : ""}`,
          );
        } else if (archivedCount > 0) {
          toast.success(
            `已直接合入 ${archivedCount} 个视频${skippedCount ? `（${skippedCount} 个已跳过）` : ""}`,
          );
        } else {
          toast.warning(
            skippedCount > 0
              ? `本次未合入，${skippedCount} 个视频已变化并跳过`
              : "本次无可合入视频",
          );
        }

        const finishedAt = Date.now();
        settleBackgroundTask({
          tasks: tasksRef.current,
          fallback: initialTask,
          apply: (t) => ({
            ...t,
            status: nextStatus,
            result,
            error: result.errors?.[0] || "",
            finishedAt,
          }),
          updateTask,
          emit: (settledTask) => {
            emit({ type: "auto-merge-settled", task: settledTask });
          },
        });
      } catch (e) {
        pollController.abort();
        clearTimeout(pollTimer);
        const message = (e as Error).message || "自动合入连接中断";
        setOpen(true);
        toast.error(message);
        const finishedAt = Date.now();
        settleBackgroundTask({
          tasks: tasksRef.current,
          fallback: initialTask,
          apply: (t) => ({
            ...t,
            status: "failed" as const,
            error: `${message}（可点击「核对恢复」查询本次合入结果）`,
            finishedAt,
          }),
          updateTask,
          emit: (settledTask) => {
            emit({ type: "auto-merge-settled", task: settledTask });
          },
        });
      }
    },
    [emit, updateTask],
  );

  // Serial Queue Runner Effect (only for heavy file operations; review-mutation runs independently)
  useEffect(() => {
    if (runningIdRef.current) return;
    const fileTasks = tasks.filter((t) => t.kind !== "review-mutation");
    const runningTask = fileTasks.find((t) => t.status === "running");
    const nextTask =
      runningTask || fileTasks.find((t) => t.status === "queued");
    if (!nextTask) return;

    // Do not start a new queued task if an earlier task is stuck in recovery_needed
    if (
      nextTask.status === "queued" &&
      fileTasks.some((t) => t.status === "recovery_needed")
    ) {
      return;
    }

    runningIdRef.current = nextTask.id;
    if (nextTask.status === "queued") {
      updateTask(nextTask.id, (t) => ({
        ...t,
        status: "running",
        startedAt: Date.now(),
      }));
    }

    const execute = async () => {
      try {
        if (nextTask.kind === "person-batch") {
          await runPersonBatchTask(nextTask);
        } else if (nextTask.kind === "person-file") {
          await runPersonFileTask(nextTask);
        } else if (nextTask.kind === "archive") {
          await runArchiveTask(nextTask);
        } else if (nextTask.kind === "auto-merge") {
          await runAutoMergeTask(nextTask);
        }
      } finally {
        runningIdRef.current = null;
      }
    };
    void execute();
  }, [tasks, runPersonBatchTask, runPersonFileTask, runArchiveTask, runAutoMergeTask, updateTask]);

  const reconcileTask = useCallback(
    async (taskId: string) => {
      const target = tasksRef.current.find((t) => t.id === taskId);
      if (!target || reconcilingRef.current.has(taskId)) return;
      reconcilingRef.current.add(taskId);
      try {
        if (target.kind === "person-batch") {
          const saved = await api<PersonBatchResult>("/api/persons/batch", {
            method: "POST",
            body: JSON.stringify({
              mode: "apply",
              id: target.plan.id,
              version: target.plan.version,
            }),
          });
          const nextStatus: BackgroundTaskStatus = saved.ok
            ? "complete"
            : saved.recovery_needed
              ? "recovery_needed"
              : "failed";
          if (saved.ok) {
            try {
              localStorage.removeItem(BATCH_STORAGE_KEY);
            } catch {}
          }
          invalidatePersonLibraryCaches([
            ...target.people,
            target.target,
            ...(saved.completed || []).map((item) => item.person),
            saved.target,
          ]);
          invalidateReviewCaches({ resetSession: true, includePeople: true });
          const finishedAt = Date.now();
          settleBackgroundTask({
            tasks: tasksRef.current,
            fallback: target,
            apply: (t) => ({
              ...t,
              status: nextStatus,
              result: saved,
              error: saved.error || "",
              finishedAt,
            }),
            updateTask,
            emit: (settled) => emit({ type: "person-batch-settled", task: settled }),
          });
        } else if (target.kind === "person-file") {
          const receipt = await api<{
            status: string;
            result?: PersonFileResult;
            error?: string;
          }>("/api/persons/manage", {
            method: "POST",
            body: JSON.stringify({
              mode: "reconcile",
              request: target.plan.request,
              version: target.plan.version,
            }),
          });
          const nextStatus: BackgroundTaskStatus =
            receipt.status === "complete"
              ? "complete"
              : receipt.status === "rolled_back"
                ? "rolled_back"
                : receipt.status === "recovery_needed"
                  ? "recovery_needed"
                  : "failed";
          if (nextStatus === "complete" || nextStatus === "rolled_back") {
            try {
              localStorage.removeItem(FILE_OPERATION_STORAGE_KEY);
            } catch {}
          }
          invalidatePersonLibraryCaches([
            target.person,
            target.targetPerson,
            receipt.result?.person,
          ]);
          if (
            target.action === "review-delete" ||
            target.action === "delete-files" ||
            target.action === "delete-person"
          ) {
            deleteWorkbenchCache("staging:list:");
          }
          invalidateReviewCaches({
            resetSession: target.action !== "review-delete",
            includePeople: target.action !== "review-delete",
          });
          const finishedAt = Date.now();
          settleBackgroundTask({
            tasks: tasksRef.current,
            fallback: target,
            apply: (t) => ({
              ...t,
              status: nextStatus,
              result: receipt.result || t.result,
              error: receipt.error || "",
              finishedAt,
            }),
            updateTask,
            emit: (settled) => emit({ type: "person-file-settled", task: settled }),
          });
        } else if (target.kind === "archive") {
          const prog = await api<{
            job: ArchiveJobProgress | null;
            result?: ArchiveTaskResult | null;
          }>("/api/plan/progress", {
            method: "POST",
            body: JSON.stringify({ id: target.operationId }),
          });
          if (prog.result) {
            const res = prog.result;
            const nextStatus: BackgroundTaskStatus = res.recovery_needed
              ? "recovery_needed"
              : res.rolled_back
                ? "rolled_back"
                : res.ok
                  ? "complete"
                  : "failed";
            if (nextStatus === "complete") {
              try {
                localStorage.removeItem(ARCHIVE_STORAGE_KEY);
              } catch {}
            }
            invalidatePersonLibraryCaches(target.creators);
            deleteWorkbenchCache("integration:plan");
            invalidateReviewCaches({ resetSession: true });
            const finishedAt = Date.now();
            settleBackgroundTask({
              tasks: tasksRef.current,
              fallback: target,
              apply: (t) => ({
                ...t,
                status: nextStatus,
                result: res,
                error: res.errors?.[0] || "",
                finishedAt,
              }),
              updateTask,
              emit: (settled) => emit({ type: "archive-settled", task: settled }),
            });
          }
        } else if (target.kind === "auto-merge") {
          const prog = await api<{
            job: ArchiveJobProgress | null;
            result?: AutoWatermarkApplyResult | null;
          }>("/api/plan/progress", {
            method: "POST",
            body: JSON.stringify({ id: target.operationId }),
          });
          if (prog.result) {
            const res = prog.result;
            const archivedCount = res.archived ?? res.moved ?? 0;
            const nextStatus: BackgroundTaskStatus = res.recovery_needed
              ? "recovery_needed"
              : res.rolled_back
                ? "rolled_back"
                : res.ok || archivedCount > 0 || (res.fallback_queued ?? 0) > 0
                  ? "complete"
                  : "failed";
            invalidatePersonLibraryCaches(target.creators);
            deleteWorkbenchCache("integration:plan");
            invalidateReviewCaches({ resetSession: true });
            const finishedAt = Date.now();
            settleBackgroundTask({
              tasks: tasksRef.current,
              fallback: target,
              apply: (t) => ({
                ...t,
                status: nextStatus,
                result: res,
                error: res.errors?.[0] || "",
                finishedAt,
              }),
              updateTask,
              emit: (settled) => emit({ type: "auto-merge-settled", task: settled }),
            });
          }
        }
      } catch (e) {
        updateTask(taskId, (t) => ({
          ...t,
          error: (e as Error).message,
        }));
      } finally {
        reconcilingRef.current.delete(taskId);
      }
    },
    [emit, updateTask],
  );

  // Compute scope locks for people and videos (merging local optimistic locks with server-authoritative SSE locks)
  const { lockedPeople, lockedVids, serverActiveOps, hasActiveWrite } = useMemo(() => {
    const localPeopleMap = new Map<string, ScopeLockInfo>();
    const localVidsMap = new Map<string, ScopeLockInfo>();
    let localActiveWrite = false;

    for (const task of tasks) {
      const isRecovery = task.status === "recovery_needed";
      if (!isActiveStatus(task.status) && !isRecovery) continue;
      const lockStatus: ScopeLockInfo["status"] = isRecovery
        ? "recovery_needed"
        : task.status === "running"
          ? "running"
          : "queued";
      if (task.kind === "review-mutation") {
        const label =
          task.action === "confirm"
            ? "归名中"
            : task.action === "defer"
              ? "暂缓中"
              : task.action === "restore"
                ? "恢复中"
                : "撤销中";
        for (const vid of task.vids) {
          setLocalLock(localVidsMap, vid, {
            label,
            status: lockStatus,
            taskId: task.id,
            action: task.action,
            source: "local",
          });
        }
        continue;
      }
      if (isActiveStatus(task.status)) {
        localActiveWrite = true;
      }

      if (task.kind === "person-batch") {
        const action = task.action === "merge" ? "batch-merge" : "batch-delete";
        const label =
          lockStatus === "recovery_needed"
            ? "待恢复"
            : task.action === "merge"
              ? lockStatus === "running"
                ? "合并中"
                : "排队合并"
              : lockStatus === "running"
                ? "删除中"
                : "排队删除";
        for (const p of task.people) {
          setLocalLock(localPeopleMap, p, {
            label,
            status: lockStatus,
            taskId: task.id,
            action,
            source: "local",
          });
        }
        if (task.target) {
          setLocalLock(localPeopleMap, task.target, {
            label,
            status: lockStatus,
            taskId: task.id,
            action,
            source: "local",
          });
        }
      } else if (task.kind === "person-file") {
        const isPersonLevel =
          task.action === "merge-person" ||
          task.action === "rename-person" ||
          task.action === "delete-person";
        const label = localPersonFileLockLabel(task.action, lockStatus);
        if (isPersonLevel) {
          setLocalLock(localPeopleMap, task.person, {
            label,
            status: lockStatus,
            taskId: task.id,
            action: task.action,
            source: "local",
          });
          if (task.targetPerson) {
            setLocalLock(localPeopleMap, task.targetPerson, {
              label,
              status: lockStatus,
              taskId: task.id,
              action: task.action,
              source: "local",
            });
          }
        }
        for (const vid of task.vids) {
          setLocalLock(localVidsMap, vid, {
            label,
            status: lockStatus,
            taskId: task.id,
            action: task.action,
            source: "local",
          });
        }
      } else if (task.kind === "archive") {
        const label =
          lockStatus === "recovery_needed"
            ? "待恢复"
            : lockStatus === "running"
              ? "归档中"
              : "排队归档";
        for (const vid of task.vids) {
          setLocalLock(localVidsMap, vid, {
            label,
            status: lockStatus,
            taskId: task.id,
            action: "archive",
            source: "local",
          });
        }
        const creators = task.personName
          ? [...new Set([...task.creators, task.personName])]
          : task.creators;
        for (const creator of creators) {
          setLocalLock(localPeopleMap, creator, {
            label,
            status: lockStatus,
            taskId: task.id,
            action: "archive",
            source: "local",
          });
        }
      } else if (task.kind === "auto-merge") {
        const label =
          lockStatus === "recovery_needed"
            ? "待恢复"
            : lockStatus === "running"
              ? "合入中"
              : "排队合入";
        for (const vid of task.vids) {
          setLocalLock(localVidsMap, vid, {
            label,
            status: lockStatus,
            taskId: task.id,
            action: "auto-merge",
            source: "local",
          });
        }
        for (const creator of task.creators) {
          setLocalLock(localPeopleMap, creator, {
            label,
            status: lockStatus,
            taskId: task.id,
            action: "auto-merge",
            source: "local",
          });
        }
      }
    }

    const activeOps = serverSnapshot?.active_ops ?? [];
    const mergedPeople = mergeScopeLocks(localPeopleMap, serverSnapshot?.people);
    const mergedVids = mergeScopeLocks(localVidsMap, serverSnapshot?.videos);

    return {
      lockedPeople: mergedPeople,
      lockedVids: mergedVids,
      serverActiveOps: activeOps,
      hasActiveWrite:
        localActiveWrite ||
        activeOps.some(
          (op) => op.status === "running" || op.status === "queued",
        ),
    };
  }, [tasks, serverSnapshot]);

  const value = useMemo<BackgroundOperationsContextValue>(
    () => ({
      tasks,
      open,
      setOpen,
      lockedPeople,
      lockedVids,
      serverActiveOps,
      hasActiveWrite,
      enqueuePersonBatch,
      enqueuePersonFile,
      enqueueArchive,
      enqueueAutoMerge,
      trackReviewMutation,
      cancelQueuedTask,
      dismissTask,
      clearCompleted,
      reconcileTask,
      subscribe,
    }),
    [
      tasks,
      open,
      lockedPeople,
      lockedVids,
      serverActiveOps,
      hasActiveWrite,
      enqueuePersonBatch,
      enqueuePersonFile,
      enqueueArchive,
      enqueueAutoMerge,
      trackReviewMutation,
      cancelQueuedTask,
      dismissTask,
      clearCompleted,
      reconcileTask,
      subscribe,
    ],
  );

  return (
    <BackgroundOperationsContext.Provider value={value}>
      {children}
    </BackgroundOperationsContext.Provider>
  );
}

const REVIEW_MUTATION_STEP_LABEL: Record<
  ReviewMutationTask["action"],
  string
> = {
  confirm: "写入待归档队列",
  defer: "移入暂缓核对列表",
  restore: "恢复至待确认列表",
  undo: "从待归档队列撤回",
};

const REVIEW_MUTATION_DONE_SUMMARY: Record<
  ReviewMutationTask["action"],
  string
> = {
  confirm: "已写入待归档队列",
  defer: "已移入暂缓核对",
  restore: "已恢复至待确认",
  undo: "已从待归档队列撤回",
};

const PERSON_FILE_STEP_LABEL: Record<PersonFileAction, string> = {
  "rename-file": "校验并重命名文件",
  "move-files": "校验并移动媒体文件",
  "delete-files": "校验并移入回收站",
  "review-delete": "校验并移入回收站",
  "rename-person": "校验并重命名人物目录",
  "merge-person": "合并人物目录文件",
  "delete-person": "删除人物目录并移入回收站",
};

const PERSON_FILE_DONE_SUMMARY: Record<PersonFileAction, string> = {
  "rename-file": "已完成文件重命名与账本同步",
  "move-files": "已完成文件移动与账本同步",
  "delete-files": "已移入回收站并同步账本",
  "review-delete": "已移入回收站并同步账本",
  "rename-person": "已完成人物重命名与账本同步",
  "merge-person": "已完成人物合并与账本同步",
  "delete-person": "已移入回收站并清理记录",
};

function getPersonFilePhaseFlags(task: PersonFileTask) {
  const phase = task.phase || "";
  const isComplete = task.status === "complete";
  const filesDone =
    isComplete ||
    phase === "commit_scope" ||
    phase === "successful_file_subset" ||
    phase.startsWith("ledger_") ||
    phase.startsWith("faces_") ||
    phase.startsWith("jobs_") ||
    phase.startsWith("json_") ||
    phase.startsWith("source_directory_") ||
    phase === "archive_scanner_indexes_reconciled";
  const localIndexDone =
    isComplete ||
    phase === "ledger_commit_intent" ||
    phase === "ledger_committed" ||
    phase === "archive_scanner_indexes_reconciled" ||
    phase.startsWith("source_directory_");
  const peachDone =
    isComplete ||
    phase === "ledger_committed" ||
    phase.startsWith("source_directory_");
  return { filesDone, localIndexDone, peachDone };
}

function getActiveStageLabel(task: BackgroundTask): string {
  if (task.kind === "review-mutation") {
    return REVIEW_MUTATION_STEP_LABEL[task.action];
  }
  if (task.kind === "person-batch") {
    if (task.progress?.active_person) {
      return `正在处理 ${task.progress.active_person}`;
    }
    return task.action === "merge"
      ? "合并人物目录并同步账本"
      : "移入回收站并同步账本";
  }
  if (task.kind === "person-file") {
    const { filesDone, localIndexDone, peachDone } =
      getPersonFilePhaseFlags(task);
    if (localIndexDone && !peachDone) return "同步 Peach 账本";
    if (filesDone && !localIndexDone) return "更新本地记录与索引";
    return PERSON_FILE_STEP_LABEL[task.action];
  }
  if (task.kind === "auto-merge") {
    return task.progress?.stage || "自动合入至人物目录";
  }
  return task.progress?.stage || "核对归档并移动文件";
}

function getCompletedSummary(task: BackgroundTask): string {
  if (task.kind === "review-mutation") {
    return REVIEW_MUTATION_DONE_SUMMARY[task.action];
  }
  if (task.kind === "person-batch") {
    return `已完成 ${task.result?.completed.length ?? task.plan.plans.length} 人处理`;
  }
  if (task.kind === "archive") {
    return `已归档 ${task.result?.moved ?? task.vids.length} 个视频`;
  }
  if (task.kind === "auto-merge") {
    const archived = task.result?.archived ?? task.result?.moved ?? 0;
    const fallback = task.result?.fallback_queued ?? 0;
    const skipped = task.result?.skipped ?? 0;
    return [
      `已合入 ${archived} 个`,
      fallback > 0 ? `${fallback} 个转待归档` : "",
      skipped > 0 ? `${skipped} 个跳过` : "",
    ]
      .filter(Boolean)
      .join(" · ");
  }
  return PERSON_FILE_DONE_SUMMARY[task.action];
}

function buildActivityItems(task: BackgroundTask): AgentActivityItem[] {
  if (task.kind === "review-mutation") {
    const baseLabel = REVIEW_MUTATION_STEP_LABEL[task.action];
    return [
      {
        id: `${task.id}-review`,
        type: "step",
        status:
          task.status === "complete"
            ? "complete"
            : task.status === "running"
              ? "active"
              : "pending",
        label:
          task.action === "confirm" && task.creator
            ? `${baseLabel} · ${task.creator}`
            : baseLabel,
        meta: `${task.vids.length} 个视频`,
      },
    ];
  }
  if (task.kind === "person-batch") {
    const completedSet = new Set([
      ...(task.progress?.completed || []),
      ...(task.result?.completed.map((c) => c.person) || []),
    ]);
    const activePerson = task.progress?.active_person;
    return task.plan.plans.map((p) => {
      const done = completedSet.has(p.person);
      const active =
        task.status === "running" &&
        (activePerson === p.person || (!activePerson && !done));
      return {
        id: p.id || p.person,
        type: "step",
        status: done ? "complete" : active ? "active" : "pending",
        label:
          task.action === "merge"
            ? `${p.person} → ${task.target}`
            : `移入回收站 · ${p.person}`,
        meta: `${p.files.length} 个文件`,
      };
    });
  }
  if (task.kind === "person-file") {
    const { filesDone, localIndexDone, peachDone } =
      getPersonFilePhaseFlags(task);
    const isRunning = task.status === "running";
    return [
      {
        id: `${task.id}-files`,
        type: "step",
        status: filesDone ? "complete" : isRunning ? "active" : "pending",
        label: PERSON_FILE_STEP_LABEL[task.action],
        meta: `${task.plan.files.length} 个文件`,
      },
      {
        id: `${task.id}-local`,
        type: "step",
        status: localIndexDone
          ? "complete"
          : filesDone && isRunning
            ? "active"
            : "pending",
        label: "更新本地记录与索引",
      },
      {
        id: `${task.id}-ledger`,
        type: "step",
        status: peachDone
          ? "complete"
          : localIndexDone && isRunning
            ? "active"
            : "pending",
        label: "同步 Peach 账本",
      },
    ];
  }
  if (task.kind === "auto-merge") {
    const doneCount =
      task.status === "complete"
        ? (task.result?.archived ?? task.result?.moved ?? 0) +
          (task.result?.fallback_queued ?? 0) +
          (task.result?.skipped ?? 0)
        : (task.progress?.done ?? 0);
    const totalCount = task.progress?.total ?? task.vids.length;
    return [
      {
        id: `${task.id}-auto-merge`,
        type: "step",
        status:
          task.status === "complete"
            ? "complete"
            : task.status === "running"
              ? "active"
              : "pending",
        label: task.progress?.stage || "直接合入已有的人物目录并更新账本",
        meta: `${doneCount} / ${totalCount}`,
      },
    ];
  }
  const doneCount =
    task.status === "complete"
      ? task.result?.moved ?? task.vids.length
      : task.progress?.done ?? 0;
  const totalCount = task.progress?.total ?? task.vids.length;
  return [
    {
      id: `${task.id}-move`,
      type: "step",
      status:
        task.status === "complete"
          ? "complete"
          : task.status === "running"
            ? "active"
            : "pending",
      label: task.progress?.stage || "移动视频至人物目录并保存记录",
      meta: `${doneCount} / ${totalCount}`,
    },
  ];
}

function TaskIcon({ task }: { task: BackgroundTask }) {
  if (task.kind === "review-mutation") return <Check className="size-3.5" />;
  if (task.kind === "archive") return <Archive className="size-3.5" />;
  if (task.kind === "auto-merge") return <Sparkles className="size-3.5" />;
  if (task.kind === "person-batch") {
    return task.action === "merge" ? (
      <Merge className="size-3.5" />
    ) : (
      <Trash2 className="size-3.5" />
    );
  }
  if (
    task.action === "delete-files" ||
    task.action === "review-delete" ||
    task.action === "delete-person"
  ) {
    return <Trash2 className="size-3.5" />;
  }
  if (task.action === "move-files") return <FolderInput className="size-3.5" />;
  if (task.action === "merge-person") return <Merge className="size-3.5" />;
  return <Pencil className="size-3.5" />;
}

function TaskActivityFeedback({
  task,
  items,
  isRunning,
  isComplete,
  isError,
}: {
  task: BackgroundTask;
  items: AgentActivityItem[];
  isRunning: boolean;
  isComplete: boolean;
  isError: boolean;
}) {
  const [mountNow] = useState(() => Date.now());
  const activeStageLabel = getActiveStageLabel(task);
  const initialSeconds = task.startedAt
    ? Math.max(0, (mountNow - task.startedAt) / 1000)
    : 0;

  return (
    <AgentActivity
      items={items}
      status={isRunning ? "working" : "complete"}
      defaultOpen={isRunning || isError}
      collapseOnComplete={!isError}
      activeLabel={activeStageLabel}
      renderWorkingStatus={({ label }) => (
        <AgentProgress
          label={typeof label === "string" ? label : activeStageLabel}
          initialSeconds={initialSeconds}
          labels={{ inProgress: "{label}，进行中" }}
          className="w-full min-w-0 gap-2 text-[12px] [&>span:nth-child(2)]:min-w-0 [&>span:nth-child(2)]:flex-1 [&>span:nth-child(2)]:truncate"
        />
      )}
      summary={isComplete ? getCompletedSummary(task) : "查看执行步骤明细"}
      maxHeight={148}
    />
  );
}

export function BackgroundOperationsDock({
  mobile = false,
}: {
  mobile?: boolean;
}) {
  const {
    tasks,
    open,
    setOpen,
    cancelQueuedTask,
    dismissTask,
    clearCompleted,
    reconcileTask,
  } = useBackgroundOperations();
  const router = useRouter();
  const [isMobileViewport, setIsMobileViewport] = useState(false);

  useEffect(() => {
    const mql = window.matchMedia("(max-width: 767px)");
    const update = () => setIsMobileViewport(mql.matches);
    update();
    mql.addEventListener("change", update);
    return () => mql.removeEventListener("change", update);
  }, []);

  if (mobile !== isMobileViewport) return null;
  if (tasks.length === 0) return null;

  const runningTask = tasks.find((t) => t.status === "running");
  const queuedCount = tasks.filter((t) => t.status === "queued").length;
  const errorTasks = tasks.filter(
    (t) =>
      t.status === "failed" ||
      t.status === "recovery_needed" ||
      t.status === "rolled_back",
  );
  const completedCount = tasks.filter((t) => t.status === "complete").length;

  const stateTone: "running" | "error" | "complete" = runningTask || queuedCount
    ? "running"
    : errorTasks.length > 0
      ? "error"
      : "complete";

  const pillLabel = runningTask
    ? runningTask.title
    : queuedCount > 0
      ? `排队等待执行 · ${queuedCount} 项`
      : errorTasks.length > 0
        ? `${errorTasks.length} 项操作需核对`
        : `已完成 ${completedCount} 项操作`;

  const badgeCount = queuedCount > 0 ? queuedCount : tasks.length;

  return (
    <div
      className={mobile ? "bg-ops-dock bg-ops-dock-mobile" : "bg-ops-dock bg-ops-dock-rail"}
      role="region"
      aria-label="后台文件操作状态"
    >
      <MorphPopover open={open} onOpenChange={setOpen}>
        <MorphPopoverTrigger>
          <button
            type="button"
            className="bg-ops-trigger"
            data-state={stateTone}
            data-variant={mobile ? "mobile" : "rail"}
            data-mobile={mobile || undefined}
            title={`后台任务：${pillLabel}`}
            aria-label={`后台任务：${pillLabel}`}
          >
            <span className="bg-ops-trigger-icon" data-tone={stateTone}>
              {stateTone === "running" ? (
                <LoaderCircle className="size-4 animate-spin motion-reduce:animate-none" />
              ) : stateTone === "error" ? (
                <AlertTriangle className="size-4" />
              ) : (
                <Check className="size-4" />
              )}
            </span>

            {mobile ? (
              <span className="bg-ops-trigger-count">
                {queuedCount > 0 ? (
                  <DigitSwap value={queuedCount} />
                ) : (
                  <>
                    <DigitSwap value={completedCount} />
                    <span>/</span>
                    <DigitSwap value={tasks.length} />
                  </>
                )}
              </span>
            ) : (
              <span className="bg-ops-rail-badge" data-tone={stateTone}>
                <DigitSwap value={badgeCount} />
              </span>
            )}
          </button>
        </MorphPopoverTrigger>

        <MorphPopoverContent
          side={mobile ? "bottom" : "top"}
          align={mobile ? "end" : "start"}
          sideOffset={10}
          radius={14}
          className="bg-ops-panel"
        >
          <div className="bg-ops-panel-head">
            <div className="bg-ops-panel-title">
              <span>后台任务队列</span>
              <AnimatedBadge status="neutral" showIcon={false}>
                <DigitSwap value={tasks.length} />
              </AnimatedBadge>
            </div>
            <ButtonGroup className="bg-ops-action-group">
              {completedCount > 0 && (
                <button
                  type="button"
                  className="bg-ops-group-btn"
                  onClick={clearCompleted}
                >
                  清除已完成
                </button>
              )}
              <button
                type="button"
                className="bg-ops-group-btn"
                aria-label="收起后台任务面板"
                title="收起"
                onClick={() => setOpen(false)}
              >
                <X className="size-3.5" />
              </button>
            </ButtonGroup>
          </div>

          <div className="bg-ops-list">
            {[...tasks].reverse().map((task) => {
              const items = buildActivityItems(task);
              const isRunning = task.status === "running";
              const isQueued = task.status === "queued";
              const isComplete = task.status === "complete";
              const isError =
                task.status === "failed" ||
                task.status === "recovery_needed" ||
                task.status === "rolled_back";
              const hasPrimaryAction =
                isQueued ||
                isError ||
                (isComplete &&
                  task.kind === "person-batch" &&
                  task.action === "merge" &&
                  !!task.target) ||
                (isComplete &&
                  task.kind === "person-file" &&
                  (task.action === "review-delete" ||
                    !!(task.result?.person || task.person))) ||
                (isComplete &&
                  task.kind === "auto-merge" &&
                  ((task.result?.fallback_queued ?? 0) > 0 ||
                    ((task.result?.archived ?? task.result?.moved ?? 0) > 0 &&
                      task.creators.length === 1)));
              const hasDismissAction = isComplete || isError;

              return (
                <div
                  key={task.id}
                  className="bg-ops-card"
                  data-status={task.status}
                >
                  <div className="bg-ops-card-head">
                    <div className="bg-ops-card-main">
                      <span className="bg-ops-trigger-icon">
                        <TaskIcon task={task} />
                      </span>
                      <div className="bg-ops-card-title">
                        <strong title={task.title}>{task.title}</strong>
                        <span>{task.subtitle}</span>
                      </div>
                    </div>

                    <AnimatedBadge
                      status={
                        isComplete
                          ? "success"
                          : isError
                            ? "warning"
                            : isRunning
                              ? "loading"
                              : "neutral"
                      }
                    >
                      {isRunning
                        ? "执行中"
                        : isQueued
                          ? "排队中"
                          : isComplete
                            ? "已完成"
                            : task.status === "recovery_needed"
                              ? "待核对"
                              : task.status === "rolled_back"
                                ? "已回退"
                                : "未完成"}
                    </AnimatedBadge>
                  </div>

                  {(isRunning || isComplete || isError) && (
                    <TaskActivityFeedback
                      task={task}
                      items={items}
                      isRunning={isRunning}
                      isComplete={isComplete}
                      isError={isError}
                    />
                  )}

                  {isError && task.error && (
                    <Alert
                      variant="warning"
                      className="rounded-lg p-2.5 text-[11.5px]"
                      contentClassName="gap-2"
                    >
                      <AlertIcon className="mt-0.5 size-3.5 [&>svg]:size-3.5" />
                      <AlertContent className="space-y-0">
                        <AlertDescription className="text-current leading-[1.45]">
                          {task.error}
                        </AlertDescription>
                      </AlertContent>
                    </Alert>
                  )}

                  <div className="bg-ops-card-foot">
                    <span className="text-[11px] text-muted-foreground">
                      {isQueued ? (
                        <span className="inline-flex items-center gap-1">
                          <Clock3 className="size-3" />
                          等待前序任务完成
                        </span>
                      ) : (
                        `编号 ${task.id.slice(0, 8)}`
                      )}
                    </span>

                    {(hasPrimaryAction || hasDismissAction) && (
                      <ButtonGroup className="bg-ops-action-group">
                        {isQueued && (
                          <button
                            type="button"
                            className="bg-ops-group-btn"
                            onClick={() => cancelQueuedTask(task.id)}
                          >
                            取消排队
                          </button>
                        )}

                        {isError && (
                          <button
                            type="button"
                            className="bg-ops-group-btn"
                            onClick={() => void reconcileTask(task.id)}
                          >
                            <RefreshCw className="size-3" />
                            核对恢复
                          </button>
                        )}

                        {isComplete &&
                          task.kind === "person-batch" &&
                          task.action === "merge" &&
                          task.target && (
                            <button
                              type="button"
                              className="bg-ops-group-btn"
                              onClick={() => {
                                router.push(
                                  `/persons/${encodeURIComponent(task.target!)}`,
                                );
                                setOpen(false);
                              }}
                            >
                              查看 {task.target}
                              <ArrowUpRight className="size-3" />
                            </button>
                          )}

                        {isComplete &&
                          task.kind === "person-file" &&
                          task.action === "review-delete" && (
                            <button
                              type="button"
                              className="bg-ops-group-btn"
                              onClick={() => {
                                router.push("/staging");
                                setOpen(false);
                              }}
                            >
                              查看回收站
                              <ArrowUpRight className="size-3" />
                            </button>
                          )}

                        {isComplete &&
                          task.kind === "person-file" &&
                          task.action !== "review-delete" &&
                          (task.result?.person || task.person) && (
                            <button
                              type="button"
                              className="bg-ops-group-btn"
                              onClick={() => {
                                router.push(
                                  `/persons/${encodeURIComponent(task.result?.person || task.person)}`,
                                );
                                setOpen(false);
                              }}
                            >
                              查看目录
                              <ArrowUpRight className="size-3" />
                            </button>
                          )}

                        {isComplete &&
                          task.kind === "auto-merge" &&
                          (task.result?.fallback_queued ?? 0) > 0 && (
                            <button
                              type="button"
                              className="bg-ops-group-btn"
                              onClick={() => {
                                router.push("/integration");
                                setOpen(false);
                              }}
                            >
                              查看待归档
                              <ArrowUpRight className="size-3" />
                            </button>
                          )}

                        {isComplete &&
                          task.kind === "auto-merge" &&
                          !(task.result?.fallback_queued ?? 0) &&
                          (task.result?.archived ?? task.result?.moved ?? 0) > 0 &&
                          task.creators.length === 1 && (
                            <button
                              type="button"
                              className="bg-ops-group-btn"
                              onClick={() => {
                                router.push(
                                  `/persons/${encodeURIComponent(task.creators[0])}`,
                                );
                                setOpen(false);
                              }}
                            >
                              查看 {task.creators[0]}
                              <ArrowUpRight className="size-3" />
                            </button>
                          )}

                        {hasDismissAction && (
                          <button
                            type="button"
                            className="bg-ops-group-btn"
                            aria-label="移除此条记录"
                            onClick={() => dismissTask(task.id)}
                          >
                            清除
                          </button>
                        )}
                      </ButtonGroup>
                    )}
                  </div>
                </div>
              );
            })}
          </div>
        </MorphPopoverContent>
      </MorphPopover>
    </div>
  );
}
