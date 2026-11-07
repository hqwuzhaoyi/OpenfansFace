export type CohortBucket='completed'|'running'|'waiting'|'error'|'excluded'
export interface CohortItem {
  batch_id:string;vid:string;added_at:number;completed:number;text_done:number;face_required:number|null;face_done:number;face_skipped:number;excluded:number;legacy:number
  current:number;stage:string|null;manual:number|null;handled?:boolean
}
export interface ProcessingBatch {
  id:string;started_at:number;total:number;completed:number;running:number;waiting:number;error:number;excluded:number
  text_done:number;face_required:number;face_done:number;face_skipped:number;legacy:boolean
  refreshing:number;refresh_waiting:number;refresh_error:number
}
export function cohortBucket(item:CohortItem):CohortBucket {
  if(item.completed)return 'completed'
  if(item.excluded||!item.current||item.handled||item.manual||item.stage==='excluded')return 'excluded'
  if(item.stage==='error')return 'error'
  if(item.stage==='text'||item.stage==='face')return 'running'
  return 'waiting'
}
export function buildProcessingBatches(items:CohortItem[]):ProcessingBatch[]{
  const groups=new Map<string,ProcessingBatch>()
  for(const item of items){
    const b=groups.get(item.batch_id)||{id:item.batch_id,started_at:item.added_at,total:0,completed:0,running:0,waiting:0,error:0,excluded:0,text_done:0,face_required:0,face_done:0,face_skipped:0,legacy:false,refreshing:0,refresh_waiting:0,refresh_error:0}
    b.started_at=Math.min(b.started_at,item.added_at);b.total++;b[cohortBucket(item)]++
    b.text_done+=Number(!!item.text_done);b.face_required+=Number(item.face_required===1);b.face_done+=Number(!!item.face_done);b.face_skipped+=Number(!!item.face_skipped);b.legacy||=!!item.legacy
    if(item.completed&&item.current&&!item.handled&&!item.manual){
      if(['text','face'].includes(item.stage||''))b.refreshing++
      if(['queued','face_queued','retry'].includes(item.stage||''))b.refresh_waiting++
      if(item.stage==='error')b.refresh_error++
    }
    groups.set(item.batch_id,b)
  }
  return [...groups.values()].sort((a,b)=>b.started_at-a.started_at||b.id.localeCompare(a.id))
}
export function batchTotals(batches:ProcessingBatch[]){
  const keys=['total','completed','running','waiting','error','excluded','text_done','face_required','face_done','face_skipped','refreshing','refresh_waiting','refresh_error'] as const
  const totals=Object.fromEntries(keys.map(k=>[k,0])) as Record<typeof keys[number],number>
  for(const b of batches)for(const key of keys)totals[key]+=b[key]
  return totals
}
export function validateConcurrency(value:unknown,maximum:number){
  const cap=Number.isFinite(maximum)?Math.max(1,Math.min(4,Math.floor(maximum))):1
  if(typeof value!=='number'||!Number.isInteger(value)||value<1||value>cap)throw new RangeError(`并发数必须为 1 至 ${cap} 的整数`)
  return value
}
