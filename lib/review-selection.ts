import type { ProcessingTask } from './processing-model'

export type ReviewSelections = Record<string, ProcessingTask[]>
export const reviewSelectionKey = (group:string,deferred:boolean) => JSON.stringify([group,deferred])

/** Snapshot selected files across pages; the server rechecks their fingerprints on submit. */
export function toggleReviewSelection(selected:ProcessingTask[],rows:ProcessingTask[]) {
  const next=new Map(selected.map(t=>[t.vid,t]))
  const remove=rows.length>0&&rows.every(t=>next.has(t.vid))
  for(const row of rows)if(remove)next.delete(row.vid);else next.set(row.vid,row)
  return [...next.values()]
}
export function removeReviewSelection(selected:ReviewSelections,vids:string[]) {
  const removed=new Set(vids)
  return Object.fromEntries(Object.entries(selected).map(([key,rows])=>[key,rows.filter(t=>!removed.has(t.vid))]))
}
