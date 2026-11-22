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

export function shouldAutoRecoverExpiredReviewSession(state:{
  hasManualSelections:boolean
  hasDrafts:boolean
  inFlightCount:number
  alreadyRecovered?:boolean
}):boolean{
  return !state.alreadyRecovered&&!state.hasManualSelections&&!state.hasDrafts&&state.inFlightCount===0
}

export function shouldDiscardReviewRefreshResponse(options:{
  automatic?:boolean
  refresh?:boolean
  recovered?:boolean
  canAutoApply:boolean
  activityChanged?:boolean
}):boolean{
  if(!options.automatic)return false
  return !options.canAutoApply||Boolean(options.activityChanged)
}

export interface ReviewProjectionGroup{
  key:string
  label:string
  kind:string
  count:number
}

export interface ReviewProjectionSnapshot<
  TTask extends {vid:string}=ProcessingTask,
  TGroup extends ReviewProjectionGroup=ReviewProjectionGroup,
>{
  counts:Record<string,number>
  groups:TGroup[]
  group:TGroup|null
  total:number
  remaining?:number
  rows:TTask[]
}

export interface PendingReviewMutation{
  id:string
  action:'confirm'|'defer'|'restore'|'delete'
  groupKey:string
  isDeferred:boolean
  vids:string[]
  buckets:Record<string,number>
  settledAtSeq:number|null
}

export function reviewTaskBucket(task:{manual?:number;data?:{kind?:string}}):'suggested'|'conflict'|'none'|'manual'{
  if(task.manual)return 'manual'
  const kind=task.data?.kind
  if(kind==='suggested'||kind==='conflict'||kind==='manual')return kind
  return 'none'
}

export function pruneSettledReviewMutations(
  pending:readonly PendingReviewMutation[],
  fetchedAtSeq:number,
):PendingReviewMutation[]{
  return pending.filter(op=>op.settledAtSeq===null||op.settledAtSeq>fetchedAtSeq)
}

export function applyPendingReviewMutations<TSnapshot extends ReviewProjectionSnapshot<any,any>>(
  data:TSnapshot,
  fetchedAtSeq:number,
  pending:readonly PendingReviewMutation[],
  view:string,
):TSnapshot{
  const active=pruneSettledReviewMutations(pending,fetchedAtSeq)
  if(active.length===0)return data
  const pendingVidSet=new Set<string>()
  const groupDelta=new Map<string,number>()
  const counts={...data.counts}
  const viewDeferred=view==='deferred'
  for(const op of active){
    for(const vid of op.vids)pendingVidSet.add(vid)
    if(op.isDeferred===viewDeferred){
      groupDelta.set(op.groupKey,(groupDelta.get(op.groupKey)??0)+op.vids.length)
    }
    if(op.action==='confirm'||op.action==='delete'){
      if(op.isDeferred){
        counts.deferred=Math.max(0,(counts.deferred??0)-op.vids.length)
      }else{
        counts.all=Math.max(0,(counts.all??0)-op.vids.length)
        for(const [bucket,n] of Object.entries(op.buckets)){
          counts[bucket]=Math.max(0,(counts[bucket]??0)-n)
        }
      }
    }else if(op.action==='defer'){
      if(!op.isDeferred){
        counts.all=Math.max(0,(counts.all??0)-op.vids.length)
        for(const [bucket,n] of Object.entries(op.buckets)){
          counts[bucket]=Math.max(0,(counts[bucket]??0)-n)
        }
        counts.deferred=(counts.deferred??0)+op.vids.length
      }
    }else if(op.action==='restore'){
      if(op.isDeferred){
        counts.deferred=Math.max(0,(counts.deferred??0)-op.vids.length)
        counts.all=(counts.all??0)+op.vids.length
        for(const [bucket,n] of Object.entries(op.buckets)){
          counts[bucket]=(counts[bucket]??0)+n
        }
      }
    }
  }
  const rows=data.rows.filter(r=>!pendingVidSet.has(r.vid))
  const groups=data.groups.map(g=>{
    const delta=groupDelta.get(g.key)??0
    return delta>0?{...g,count:Math.max(0,g.count-delta)}:g
  })
  const selectedDelta=data.group?(groupDelta.get(data.group.key)??0):0
  const group=data.group&&selectedDelta>0?{...data.group,count:Math.max(0,data.group.count-selectedDelta)}:data.group
  const baseRemaining=data.remaining??data.total
  const remaining=selectedDelta>0?Math.max(0,baseRemaining-selectedDelta):data.remaining
  return {...data,counts,groups,group,remaining,rows}
}
