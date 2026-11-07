import type {ProcessingTask} from './processing-model'
export const REVIEW_IDLE_MS = 120_000
export function canAutoRefreshReview({now,lastActivity,protectedWork,requestActivity,currentActivity}:{now:number;lastActivity:number;protectedWork:boolean;requestActivity:number;currentActivity:number}) {
  return !protectedWork && now-lastActivity>=REVIEW_IDLE_MS && requestActivity===currentActivity
}

export function isReviewAvailable(task:ProcessingTask){return task.stage==='review'||!!task.manual||task.data.ready===true&&(task.stage==='face'||task.stage==='face_queued'||(task.stage==='retry'||task.stage==='error')&&task.retry_stage==='face')}
