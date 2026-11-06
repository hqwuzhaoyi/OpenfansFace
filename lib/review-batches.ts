import type { QueueItem } from './review-logic'
export interface BatchRow extends QueueItem { status: string }
export function batchKey(row: QueueItem) {
  // Old rows have no batch identifier. Keep these explicit as inferred groups.
  return row.batch_id || `old:${JSON.stringify([row.creator, row.at.slice(0,19)])}`
}
export function groupBatches<T extends BatchRow>(rows: T[]) {
  const map = new Map<string, {key:string; rows:T[]; at:string; creator:string; inferred:boolean}>()
  for (const row of rows) {
    const key=batchKey(row), current=map.get(key)
    if(current) current.rows.push(row)
    else map.set(key,{key,rows:[row],at:row.at,creator:row.creator,inferred:!row.batch_id})
  }
  const time=(s:string)=>Date.parse(s.includes('T')?s:s.replace(' ','T')+'Z')||0
  return [...map.values()].sort((a,b)=>time(b.at)-time(a.at))
}
export const statusTitle:Record<string,string>={empty:'源文件为空',corrupt:'视频体检异常',organized:'已归类（creators）','not-file':'记录指向文件夹',ok:'可以归档',missing:'找不到原文件',conflict:'目标同名冲突',duplicate:'目标文件需核对',already:'已在目标目录',invalid:'人物名需更正','dupe-target':'本波目标文件名重复'}
export function selectedProblems(rows: {status:string;name:string}[]) {
  return rows.filter(r=>r.status!=='ok').map(r=>({name:r.name,reason:statusTitle[r.status]||'需要核对文件'}))
}

export function nextReviewWave<T extends BatchRow>(previous:ReturnType<typeof groupBatches<T>>,current:string|null,next:ReturnType<typeof groupBatches<T>>) {
  const index=previous.findIndex(w=>w.key===current)
  const order=[...previous.slice(index+1),...previous.slice(0,Math.max(0,index)),...next]
  const candidates=[...new Set(order.map(w=>w.key))].filter(k=>k!==current).map(k=>next.find(w=>w.key===k)).filter((w):w is (typeof next)[number]=>!!w)
  return candidates.find(w=>w.rows.some(r=>r.status==='ok'))||candidates[0]||next.find(w=>w.key===current)||null
}
