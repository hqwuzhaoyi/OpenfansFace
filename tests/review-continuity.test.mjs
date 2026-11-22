import {test} from 'node:test'
import assert from 'node:assert/strict'
import {
  REVIEW_IDLE_MS,
  canAutoRefreshReview,
  hasProtectedReviewSelections,
  isCachedReviewPageReusable,
  isReviewAvailable,
  resolveEmptyReviewPageTarget,
} from '../lib/review-continuity-model.ts'
test('idle boundary and responses started before new activity never replace a review snapshot',()=>{
 const state={now:REVIEW_IDLE_MS,lastActivity:0,protectedWork:false,requestActivity:7,currentActivity:7}
 assert.equal(canAutoRefreshReview(state),true)
 assert.equal(canAutoRefreshReview({...state,now:REVIEW_IDLE_MS-1}),false)
 assert.equal(canAutoRefreshReview({...state,protectedWork:true}),false)
 assert.equal(canAutoRefreshReview({...state,currentActivity:8}),false)
 assert.equal(canAutoRefreshReview({...state,lastActivity:1}),false)
 assert.equal(canAutoRefreshReview({...state,now:REVIEW_IDLE_MS*10,lastAutoRefresh:REVIEW_IDLE_MS,lastAutoRefreshActivity:7}),false)
 assert.equal(canAutoRefreshReview({...state,now:REVIEW_IDLE_MS+5000,lastActivity:1,requestActivity:8,currentActivity:8,lastAutoRefresh:REVIEW_IDLE_MS,lastAutoRefreshActivity:7}),false)
 assert.equal(canAutoRefreshReview({...state,now:REVIEW_IDLE_MS*2+1,lastActivity:1,requestActivity:8,currentActivity:8,lastAutoRefresh:REVIEW_IDLE_MS,lastAutoRefreshActivity:7}),true)
})

test('ready face evidence remains reviewable after terminal failure, without reviving text failures or missing evidence',()=>{
 for(const stage of ['face_queued','face','retry','error'])assert.equal(isReviewAvailable({stage,retry_stage:'face',manual:0,data:{ready:true}}),true)
 assert.equal(isReviewAvailable({stage:'error',retry_stage:'text',manual:0,data:{ready:true}}),false)
 assert.equal(isReviewAvailable({stage:'error',retry_stage:'face',manual:0,data:{ready:false}}),false)
 assert.equal(isReviewAvailable({stage:'excluded',retry_stage:'face',manual:0,data:{ready:true}}),false)
})

test('cached review pages are only reused when session, group, page, and in-flight state all match',()=>{
 const cached={session:'sess-1',group:{key:'person:miao'},page:2}
 assert.equal(isCachedReviewPageReusable(cached,{currentSession:'sess-1',group:'person:miao',page:2,inFlightCount:0}),true)
 assert.equal(isCachedReviewPageReusable(cached,{currentSession:'sess-1',group:'',page:2,inFlightCount:0}),true)
 assert.equal(isCachedReviewPageReusable(cached,{currentSession:undefined,group:'person:miao',page:2,inFlightCount:0}),false)
 assert.equal(isCachedReviewPageReusable(cached,{currentSession:'sess-2',group:'person:miao',page:2,inFlightCount:0}),false)
 assert.equal(isCachedReviewPageReusable(cached,{currentSession:'sess-1',group:'person:other',page:2,inFlightCount:0}),false)
 assert.equal(isCachedReviewPageReusable(cached,{currentSession:'sess-1',group:'person:miao',page:3,inFlightCount:0}),false)
 assert.equal(isCachedReviewPageReusable(cached,{currentSession:'sess-1',group:'person:miao',page:2,inFlightCount:1}),false)
 assert.equal(isCachedReviewPageReusable(undefined,{currentSession:'sess-1',group:'person:miao',page:2,inFlightCount:0}),false)
})

test('empty review pages redirect to the next or previous remaining batch when unfiltered',()=>{
 const group={key:'person:miao'}
 assert.equal(resolveEmptyReviewPageTarget({group,page:1,remaining:19,next_page:3,previous_page:0},{visibleRowCount:0,hasFilter:false}),3)
 assert.equal(resolveEmptyReviewPageTarget({group,page:2,remaining:19,next_page:null,previous_page:0},{visibleRowCount:0,hasFilter:false}),0)
 assert.equal(resolveEmptyReviewPageTarget({group,page:1,remaining:19,next_page:3,previous_page:0},{visibleRowCount:0,hasFilter:true}),null)
 assert.equal(resolveEmptyReviewPageTarget({group,page:1,remaining:19,next_page:3,previous_page:0},{visibleRowCount:4,hasFilter:false}),null)
 assert.equal(resolveEmptyReviewPageTarget({group,page:1,remaining:0,next_page:3,previous_page:0},{visibleRowCount:0,hasFilter:false}),null)
 assert.equal(resolveEmptyReviewPageTarget({group,page:1,remaining:19,next_page:1,previous_page:null},{visibleRowCount:0,hasFilter:false}),null)
})

test('protected review selections ignore default auto-selected scopes until manually changed',()=>{
 const auto=new Set(['suggested:person:miao'])
 assert.equal(hasProtectedReviewSelections({'suggested:person:miao':[{vid:'v1'}]},auto),false)
 assert.equal(hasProtectedReviewSelections({'suggested:person:miao':[{vid:'v1'}],'suggested:person:other':[{vid:'v2'}]},auto),true)
 assert.equal(hasProtectedReviewSelections({'suggested:person:miao':[],'suggested:person:other':[]},auto),false)
})
