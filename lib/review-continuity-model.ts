import type {ProcessingTask} from './processing-model'
export const REVIEW_IDLE_MS = 120_000
export function canAutoRefreshReview({now,lastActivity,protectedWork,requestActivity,currentActivity,lastAutoRefresh=0,lastAutoRefreshActivity}:{now:number;lastActivity:number;protectedWork:boolean;requestActivity:number;currentActivity:number;lastAutoRefresh?:number;lastAutoRefreshActivity?:number}) {
  if(lastAutoRefreshActivity!==undefined&&lastAutoRefreshActivity===currentActivity)return false
  const idleSince=Math.max(lastActivity,lastAutoRefresh)
  return !protectedWork && now-idleSince>=REVIEW_IDLE_MS && requestActivity===currentActivity
}

export function isReviewAvailable(task:ProcessingTask){return task.stage==='review'||!!task.manual||task.data.ready===true&&(task.stage==='face'||task.stage==='face_queued'||(task.stage==='retry'||task.stage==='error')&&task.retry_stage==='face')}

export function isCachedReviewPageReusable(
  cached:{session?:string;group?:{key:string}|null;page:number}|undefined|null,
  request:{currentSession?:string;group:string;page:number;inFlightCount:number},
):boolean{
  if(request.inFlightCount!==0||!cached||!request.currentSession)return false
  if(cached.session!==request.currentSession)return false
  if(request.group&&cached.group?.key!==request.group)return false
  return cached.page===request.page
}

export function resolveEmptyReviewPageTarget(
  data:{remaining?:number;page:number;next_page?:number|null;previous_page?:number|null;group?:{key:string}|null},
  options:{visibleRowCount:number;hasFilter:boolean},
):number|null{
  if(options.hasFilter||!data.group||options.visibleRowCount>0||(data.remaining??0)<=0)return null
  const target=data.next_page??data.previous_page??null
  return target!=null&&target!==data.page?target:null
}

export function hasProtectedReviewSelections(
  selections:Record<string,readonly unknown[]>,
  autoSelectedScopes:ReadonlySet<string>,
):boolean{
  return Object.entries(selections).some(([scope,rows])=>rows.length>0&&!autoSelectedScopes.has(scope))
}
