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
