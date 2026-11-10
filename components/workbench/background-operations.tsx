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
import { ActionSwapText } from "@/components/beui/motion/action-swap";
import { AnimatedBadge } from "@/components/beui/motion/animated-badge";
import { AvatarGroup } from "@/components/beui/motion/avatar-group";
import { DigitSwap } from "@/components/beui/motion/digit-swap";
import {
  MorphPopover,
  MorphPopoverContent,
  MorphPopoverTrigger,
} from "@/components/beui/motion/popover-morph";
import { FILE_OPERATION_STORAGE_KEY } from "@/components/persons/file-actions";
import { deleteWorkbenchCache } from "@/components/workbench/state";
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
import "./background-operations.css";

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

export type BackgroundTask = PersonBatchTask | PersonFileTask | ArchiveTask;

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
    };

export type ScopeLockInfo = {
  label: string;
  status: "running" | "queued";
  taskId: string;
};

interface BackgroundOperationsContextValue {
  tasks: BackgroundTask[];
  open: boolean;
  setOpen: (open: boolean) => void;
  lockedPeople: Map<string, ScopeLockInfo>;
  lockedVids: Map<string, ScopeLockInfo>;
  hasActiveWrite: boolean;
  enqueuePersonBatch: (input: {
    plan: PersonBatchPlan;
    people: string[];
    thumbs?: Record<string, string | null>;
  }) => string;
  enqueuePersonFile: (input: {
    plan: PersonFilePlan;
    person: string;
  }) => string;
  enqueueArchive: (input: {
    vids: string[];
    version: string;
    creators: string[];
    sizeGb?: string;
    personName?: string;
  }) => string;
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
  "rename-person": "重命名人物",
  "merge-person": "合并人物",
  "delete-person": "删除人物",
};

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

  const emit = useCallback((event: BackgroundTaskEvent) => {
    for (const listener of listenersRef.current) {
      try {
        listener(event);
      } catch {
        // Ignore subscriber errors so queue progression never stalls.
      }
    }
  }, []);

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
          setTasks((prev) =>
            prev.some((t) => t.id === plan.id)
              ? prev
              : [
                  ...prev,
                  {
                    id: plan.id,
                    kind: "person-file",
                    action: plan.action,
                    title: `${fileActionLabels[plan.action] || "文件操作"} · ${plan.person}${plan.target_person ? ` → ${plan.target_person}` : ""}`,
                    subtitle: `${plan.files.length} 个文件 · ${(plan.total_bytes / 2 ** 20).toFixed(1)} MB`,
                    person: plan.person,
                    targetPerson: plan.target_person,
                    vids: plan.files.flatMap((f) => f.vids || []),
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
  }, []);

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
      setTasks((prev) => [...prev.filter((t) => t.id !== task.id), task]);
      return task.id;
    },
    [],
  );

  const enqueuePersonFile = useCallback(
    ({ plan, person }: { plan: PersonFilePlan; person: string }) => {
      const vids = [
        ...new Set([
          ...(plan.request.files?.map((f) => f.vid) || []),
          ...plan.files.flatMap((f) => f.vids || []),
        ]),
      ];
      const hasPriorActive = tasksRef.current.some((t) =>
        isActiveStatus(t.status),
      );
      const label = fileActionLabels[plan.action] || "文件操作";
      const title =
        plan.action === "merge-person" || plan.action === "rename-person"
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
      setTasks((prev) => [...prev.filter((t) => t.id !== task.id), task]);
      return task.id;
    },
    [],
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
      setTasks((prev) => [...prev, task]);
      return task.id;
    },
    [],
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
        if (saved.target) {
          deleteWorkbenchCache("persons:library:" + saved.target);
        }

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

        let settledTask: PersonBatchTask | null = null;
        updateTask(initialTask.id, (t) => {
          if (t.kind !== "person-batch") return t;
          settledTask = {
            ...t,
            plan: currentPlan,
            status: nextStatus,
            result: saved,
            error: saved.error,
            finishedAt: Date.now(),
          };
          return settledTask;
        });
        if (settledTask) {
          emit({ type: "person-batch-settled", task: settledTask });
        }
      } catch (e) {
        pollController.abort();
        clearTimeout(pollTimer);
        const message = (e as Error).message || "批次执行异常";
        setOpen(true);
        let settledTask: PersonBatchTask | null = null;
        updateTask(initialTask.id, (t) => {
          if (t.kind !== "person-batch") return t;
          settledTask = {
            ...t,
            plan: currentPlan,
            status: "failed",
            error: `${message}（可点击「核对恢复」查询同一批次结果）`,
            finishedAt: Date.now(),
          };
          return settledTask;
        });
        if (settledTask) {
          emit({ type: "person-batch-settled", task: settledTask });
        }
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

        let result: PersonFileResult;
        try {
          result = await api<PersonFileResult>("/api/persons/manage", {
            method: "POST",
            body: JSON.stringify({
              mode: "apply",
              request: currentPlan.request,
              version: currentPlan.version,
            }),
          });
        } catch (applyErr) {
          const msg = (applyErr as Error).message || "";
          if (
            !initialTask.needsFreshPreview &&
            currentPlan.backend === "rust" &&
            msg.includes("已变化")
          ) {
            pollController.abort();
            clearTimeout(pollTimer);
            currentPlan = await refreshPreviewIfUnchanged();
            updateTask(initialTask.id, (t) =>
              t.kind === "person-file" ? { ...t, plan: currentPlan } : t,
            );
            result = await api<PersonFileResult>("/api/persons/manage", {
              method: "POST",
              body: JSON.stringify({
                mode: "apply",
                request: currentPlan.request,
                version: currentPlan.version,
              }),
            });
          } else {
            throw applyErr;
          }
        }

        pollController.abort();
        clearTimeout(pollTimer);
        try {
          localStorage.removeItem(FILE_OPERATION_STORAGE_KEY);
        } catch {}

        deleteWorkbenchCache("persons:library:" + initialTask.person);
        if (result.person) {
          deleteWorkbenchCache("persons:library:" + result.person);
        }

        let settledTask: PersonFileTask | null = null;
        updateTask(initialTask.id, (t) => {
          if (t.kind !== "person-file") return t;
          settledTask = {
            ...t,
            plan: currentPlan,
            status: "complete",
            result,
             error: "",
            finishedAt: Date.now(),
          };
          return settledTask;
        });
        if (settledTask) {
          emit({ type: "person-file-settled", task: settledTask });
        }
      } catch (e) {
        pollController.abort();
        clearTimeout(pollTimer);
        const message = (e as Error).message || "文件操作失败";
        setOpen(true);
        let settledTask: PersonFileTask | null = null;
        updateTask(initialTask.id, (t) => {
          if (t.kind !== "person-file") return t;
          settledTask = {
            ...t,
            plan: currentPlan,
            status: "failed",
            error: message,
            finishedAt: Date.now(),
          };
          return settledTask;
        });
        if (settledTask) {
          emit({ type: "person-file-settled", task: settledTask });
        }
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
          }),
        });
        const data = await response.json();
        pollController.abort();
        clearTimeout(pollTimer);

        const result: ArchiveTaskResult = response.ok
          ? data
          : {
              ok: false,
              moved: 0,
              remaining: initialTask.vids.length,
              errors: [data.error || "归档请求未被接受"],
            };

        for (const creator of initialTask.creators) {
          deleteWorkbenchCache("persons:library:" + creator);
        }
        deleteWorkbenchCache("integration:plan");

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

        let settledTask: ArchiveTask | null = null;
        updateTask(initialTask.id, (t) => {
          if (t.kind !== "archive") return t;
          settledTask = {
            ...t,
            version: currentVersion,
            status: nextStatus,
            result,
            error: result.errors?.[0] || "",
            finishedAt: Date.now(),
          };
          return settledTask;
        });
        if (settledTask) {
          emit({ type: "archive-settled", task: settledTask });
        }
      } catch (e) {
        pollController.abort();
        clearTimeout(pollTimer);
        const message = (e as Error).message || "归档连接中断";
        setOpen(true);
        let settledTask: ArchiveTask | null = null;
        updateTask(initialTask.id, (t) => {
          if (t.kind !== "archive") return t;
          settledTask = {
            ...t,
            status: "failed",
            error: `${message}（可点击「核对恢复」查询本次归档结果）`,
            finishedAt: Date.now(),
          };
          return settledTask;
        });
        if (settledTask) {
          emit({ type: "archive-settled", task: settledTask });
        }
      }
    },
    [emit, updateTask],
  );

  // Serial Queue Runner Effect
  useEffect(() => {
    if (runningIdRef.current) return;
    const runningTask = tasks.find((t) => t.status === "running");
    const nextTask = runningTask || tasks.find((t) => t.status === "queued");
    if (!nextTask) return;

    // Do not start a new queued task if an earlier task is stuck in recovery_needed
    if (
      nextTask.status === "queued" &&
      tasks.some((t) => t.status === "recovery_needed")
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
        }
      } finally {
        runningIdRef.current = null;
      }
    };
    void execute();
  }, [tasks, runPersonBatchTask, runPersonFileTask, runArchiveTask, updateTask]);

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
          let settled: PersonBatchTask | null = null;
          updateTask(taskId, (t) => {
            if (t.kind !== "person-batch") return t;
            settled = {
              ...t,
              status: nextStatus,
              result: saved,
              error: saved.error || "",
              finishedAt: Date.now(),
            };
            return settled;
          });
          if (settled) emit({ type: "person-batch-settled", task: settled });
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
          let settled: PersonFileTask | null = null;
          updateTask(taskId, (t) => {
            if (t.kind !== "person-file") return t;
            settled = {
              ...t,
              status: nextStatus,
              result: receipt.result || t.result,
              error: receipt.error || "",
              finishedAt: Date.now(),
            };
            return settled;
          });
          if (settled) emit({ type: "person-file-settled", task: settled });
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
            let settled: ArchiveTask | null = null;
            updateTask(taskId, (t) => {
              if (t.kind !== "archive") return t;
              settled = {
                ...t,
                status: nextStatus,
                result: res,
                error: res.errors?.[0] || "",
                finishedAt: Date.now(),
              };
              return settled;
            });
            if (settled) emit({ type: "archive-settled", task: settled });
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

  // Compute scope locks for people and videos
  const { lockedPeople, lockedVids, hasActiveWrite } = useMemo(() => {
    const peopleMap = new Map<string, ScopeLockInfo>();
    const vidsMap = new Map<string, ScopeLockInfo>();
    let activeWrite = false;

    for (const task of tasks) {
      if (!isActiveStatus(task.status)) continue;
      activeWrite = true;
      const lockStatus = task.status === "running" ? "running" : "queued";

      if (task.kind === "person-batch") {
        const label =
          task.action === "merge"
            ? lockStatus === "running"
              ? "合并中"
              : "排队合并"
            : lockStatus === "running"
              ? "删除中"
              : "排队删除";
        for (const p of task.people) {
          if (!peopleMap.has(p)) {
            peopleMap.set(p, { label, status: lockStatus, taskId: task.id });
          }
        }
      } else if (task.kind === "person-file") {
        const isPersonLevel =
          task.action === "merge-person" ||
          task.action === "rename-person" ||
          task.action === "delete-person";
        const label =
          lockStatus === "running"
            ? fileActionLabels[task.action]?.slice(0, 2) + "中"
            : "排队中";
        if (isPersonLevel) {
          peopleMap.set(task.person, {
            label,
            status: lockStatus,
            taskId: task.id,
          });
          if (task.targetPerson) {
            peopleMap.set(task.targetPerson, {
              label,
              status: lockStatus,
              taskId: task.id,
            });
          }
        }
        for (const vid of task.vids) {
          if (!vidsMap.has(vid)) {
            vidsMap.set(vid, { label, status: lockStatus, taskId: task.id });
          }
        }
      } else if (task.kind === "archive") {
        const label = lockStatus === "running" ? "归档中" : "排队归档";
        for (const vid of task.vids) {
          if (!vidsMap.has(vid)) {
            vidsMap.set(vid, { label, status: lockStatus, taskId: task.id });
          }
        }
      }
    }

    return {
      lockedPeople: peopleMap,
      lockedVids: vidsMap,
      hasActiveWrite: activeWrite,
    };
  }, [tasks]);

  const value = useMemo<BackgroundOperationsContextValue>(
    () => ({
      tasks,
      open,
      setOpen,
      lockedPeople,
      lockedVids,
      hasActiveWrite,
      enqueuePersonBatch,
      enqueuePersonFile,
      enqueueArchive,
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
      hasActiveWrite,
      enqueuePersonBatch,
      enqueuePersonFile,
      enqueueArchive,
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
      <BackgroundOperationsDock />
    </BackgroundOperationsContext.Provider>
  );
}

function buildActivityItems(task: BackgroundTask): AgentActivityItem[] {
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
            : `移入暂存 · ${p.person}`,
        meta: `${p.files.length} 个文件`,
      };
    });
  }
  if (task.kind === "person-file") {
    const phase = task.phase || "";
    const fileStepDone =
      task.status === "complete" ||
      phase.startsWith("ledger") ||
      phase.startsWith("faces") ||
      phase.startsWith("jobs") ||
      phase.startsWith("json");
    const ledgerDone =
      task.status === "complete" ||
      phase.startsWith("faces") ||
      phase.startsWith("jobs") ||
      phase.startsWith("json");
    return [
      {
        id: `${task.id}-files`,
        type: "step",
        status: fileStepDone
          ? "complete"
          : task.status === "running"
            ? "active"
            : "pending",
        label: "校验并搬移媒体文件",
        meta: `${task.plan.files.length} 个文件`,
      },
      {
        id: `${task.id}-ledger`,
        type: "step",
        status: ledgerDone
          ? "complete"
          : fileStepDone && task.status === "running"
            ? "active"
            : "pending",
        label: "同步 Peach 账本与本地索引",
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
      label: task.progress?.stage || "移动视频至人物目录并同步 Peach 账本",
      meta: `${doneCount} / ${totalCount}`,
    },
  ];
}

function TaskIcon({ task }: { task: BackgroundTask }) {
  if (task.kind === "archive") return <Archive className="size-3.5" />;
  if (task.kind === "person-batch") {
    return task.action === "merge" ? (
      <Merge className="size-3.5" />
    ) : (
      <Trash2 className="size-3.5" />
    );
  }
  if (task.action === "delete-files" || task.action === "delete-person") {
    return <Trash2 className="size-3.5" />;
  }
  if (task.action === "move-files") return <FolderInput className="size-3.5" />;
  if (task.action === "merge-person") return <Merge className="size-3.5" />;
  return <Pencil className="size-3.5" />;
}

function BackgroundOperationsDock() {
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

  const activeAvatars =
    runningTask?.kind === "person-batch"
      ? runningTask.people.slice(0, 3).map((name) => ({
          id: name,
          name,
          src: runningTask.thumbs[name] || undefined,
        }))
      : [];

  return (
    <div className="bg-ops-dock" role="region" aria-label="后台文件操作状态">
      <MorphPopover open={open} onOpenChange={setOpen}>
        <MorphPopoverTrigger>
          <button
            type="button"
            className="bg-ops-trigger"
            data-state={stateTone}
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

            {activeAvatars.length > 0 && (
              <AvatarGroup members={activeAvatars} size="sm" max={3} />
            )}

            <ActionSwapText
              value={pillLabel}
              animation="roll"
              className="bg-ops-trigger-label"
            >
              {pillLabel}
            </ActionSwapText>

            <span className="bg-ops-trigger-count">
              {queuedCount > 0 ? (
                <>
                  <span>排队</span>
                  <DigitSwap value={queuedCount} />
                </>
              ) : (
                <>
                  <DigitSwap value={completedCount} />
                  <span>/</span>
                  <DigitSwap value={tasks.length} />
                </>
              )}
            </span>
          </button>
        </MorphPopoverTrigger>

        <MorphPopoverContent
          side="top"
          align="end"
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
            <div className="bg-ops-panel-actions">
              {completedCount > 0 && (
                <button
                  type="button"
                  className="bg-ops-text-btn"
                  onClick={clearCompleted}
                >
                  清除已完成
                </button>
              )}
              <button
                type="button"
                className="bg-ops-text-btn"
                aria-label="收起后台任务面板"
                onClick={() => setOpen(false)}
              >
                <X className="size-3.5" />
              </button>
            </div>
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
                    <AgentActivity
                      items={items}
                      status={isRunning ? "working" : "complete"}
                      defaultOpen={isRunning || isError}
                      collapseOnComplete={!isError}
                      activeLabel={
                        task.kind === "person-batch" &&
                        task.progress?.active_person
                          ? `正在处理 ${task.progress.active_person}…`
                          : "正在同步文件与 Peach 账本…"
                      }
                      summary={
                        isComplete
                          ? task.kind === "person-batch"
                            ? `已完成 ${task.result?.completed.length ?? task.plan.plans.length} 人处理`
                            : task.kind === "archive"
                              ? `已归档 ${task.result?.moved ?? task.vids.length} 个视频`
                              : "文件与账本已同步"
                          : "查看执行步骤明细"
                      }
                      maxHeight={148}
                    />
                  )}

                  {isError && task.error && (
                    <p className="bg-ops-card-error" role="alert">
                      {task.error}
                    </p>
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

                    <div className="inline-flex items-center gap-1.5">
                      {isQueued && (
                        <button
                          type="button"
                          className="bg-ops-text-btn"
                          data-variant="outline"
                          onClick={() => cancelQueuedTask(task.id)}
                        >
                          取消排队
                        </button>
                      )}

                      {isError && (
                        <button
                          type="button"
                          className="bg-ops-text-btn"
                          data-variant="outline"
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
                            className="bg-ops-text-btn"
                            data-variant="outline"
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
                        (task.result?.person || task.person) && (
                          <button
                            type="button"
                            className="bg-ops-text-btn"
                            data-variant="outline"
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

                      {(isComplete || isError) && (
                        <button
                          type="button"
                          className="bg-ops-text-btn"
                          aria-label="移除此条记录"
                          onClick={() => dismissTask(task.id)}
                        >
                          清除
                        </button>
                      )}
                    </div>
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
