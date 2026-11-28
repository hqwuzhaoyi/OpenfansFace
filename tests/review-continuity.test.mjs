import {test} from 'node:test'
import assert from 'node:assert/strict'
import {
  REVIEW_IDLE_MS,
  applyPendingReviewMutations,
  canAutoRefreshReview,
  hasProtectedReviewSelections,
  isCachedReviewPageReusable,
  isReviewAvailable,
  pruneSettledReviewMutations,
  resolveEmptyReviewPageTarget,
  reviewTaskBucket,
  shouldAutoRecoverExpiredReviewSession,
  shouldDiscardReviewRefreshResponse,
  shouldRefreshReviewOnServerEvent,
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
 for(const stage of ['queued','text','retry'])assert.equal(isReviewAvailable({stage,retry_stage:'text',manual:0,data:{ready:true}}),true)
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

test('expired review session only auto-recovers when idle without manual selections or drafts',()=>{
 assert.equal(shouldAutoRecoverExpiredReviewSession({hasManualSelections:false,hasDrafts:false,inFlightCount:0}),true)
 assert.equal(shouldAutoRecoverExpiredReviewSession({hasManualSelections:true,hasDrafts:false,inFlightCount:0}),false)
 assert.equal(shouldAutoRecoverExpiredReviewSession({hasManualSelections:false,hasDrafts:true,inFlightCount:0}),false)
 assert.equal(shouldAutoRecoverExpiredReviewSession({hasManualSelections:false,hasDrafts:false,inFlightCount:1}),false)
 assert.equal(shouldAutoRecoverExpiredReviewSession({hasManualSelections:false,hasDrafts:false,inFlightCount:0,alreadyRecovered:true}),false)
})

test('only automatic background refreshes discard on activity; manual refresh and recovery always apply',()=>{
 assert.equal(shouldDiscardReviewRefreshResponse({automatic:true,refresh:true,canAutoApply:true,activityChanged:false}),false)
 assert.equal(shouldDiscardReviewRefreshResponse({automatic:true,refresh:true,canAutoApply:false,activityChanged:false}),true)
 assert.equal(shouldDiscardReviewRefreshResponse({automatic:true,refresh:true,canAutoApply:true,activityChanged:true}),true)
 assert.equal(shouldDiscardReviewRefreshResponse({automatic:false,refresh:true,canAutoApply:false,activityChanged:true}),false)
 assert.equal(shouldDiscardReviewRefreshResponse({refresh:true,recovered:true,canAutoApply:false,activityChanged:true}),false)
 assert.equal(shouldDiscardReviewRefreshResponse({canAutoApply:false,activityChanged:true}),false)
})

test('server-op settlement refreshes only when visible, idle, and without in-flight mutations',()=>{
 assert.equal(shouldRefreshReviewOnServerEvent({protectedWork:false,inFlightCount:0,visible:true}),true)
 assert.equal(shouldRefreshReviewOnServerEvent({protectedWork:false,inFlightCount:0,visible:false}),false)
 assert.equal(shouldRefreshReviewOnServerEvent({protectedWork:true,inFlightCount:0,visible:true}),false)
 assert.equal(shouldRefreshReviewOnServerEvent({protectedWork:false,inFlightCount:1,visible:true}),false)
})

test('pending mutation projection keeps left group counts and right queue aligned across in-flight responses',()=>{
 const base={
  counts:{all:30,suggested:28,conflict:2,none:0,manual:0,deferred:1},
  groups:[{key:'person:kittyxkum',label:'kittyxkum',kind:'suggested',count:13},{key:'person:xxapple',label:'xxapple',kind:'suggested',count:15}],
  group:{key:'person:kittyxkum',label:'kittyxkum',kind:'suggested',count:13},
  total:13,
  remaining:13,
  rows:Array.from({length:13},(_,i)=>({vid:`k-${i}`,manual:0,data:{kind:'suggested'}})),
 }
 const op={
  id:'op-1',
  action:'confirm',
  groupKey:'person:kittyxkum',
  isDeferred:false,
  vids:base.rows.map(r=>r.vid),
  buckets:{suggested:13},
  settledAtSeq:null,
 }
 assert.equal(reviewTaskBucket({manual:1,data:{kind:'suggested'}}),'manual')
 assert.equal(reviewTaskBucket({manual:0,data:{kind:'conflict'}}),'conflict')
 assert.equal(reviewTaskBucket({manual:0,data:{}}),'none')

 const inFlight=applyPendingReviewMutations(base,0,[op],'all')
 assert.equal(inFlight.groups[0].count,0)
 assert.equal(inFlight.groups[1].count,15)
 assert.equal(inFlight.group.count,0)
 assert.equal(inFlight.remaining,0)
 assert.equal(inFlight.rows.length,0)
 assert.equal(inFlight.counts.all,17)
 assert.equal(inFlight.counts.suggested,15)

 // Settled at seq=1 still projects against a stale fetch started at seq=0, preventing bounce-back.
 const settled=[{...op,settledAtSeq:1}]
 const staleAfterSettle=applyPendingReviewMutations(base,0,settled,'all')
 assert.equal(staleAfterSettle.groups[0].count,0)
 assert.equal(staleAfterSettle.remaining,0)
 assert.equal(staleAfterSettle.rows.length,0)

 // Once a post-settlement fetch (fetchedAtSeq=1) lands, the settled op is pruned and not double-subtracted.
 const freshServer={...base,groups:[{key:'person:kittyxkum',label:'kittyxkum',kind:'suggested',count:0},base.groups[1]],group:{...base.group,count:0},total:0,remaining:0,rows:[],counts:{all:17,suggested:15,conflict:2,none:0,manual:0,deferred:1}}
 assert.deepEqual(pruneSettledReviewMutations(settled,1),[])
 const afterFresh=applyPendingReviewMutations(freshServer,1,settled,'all')
 assert.equal(afterFresh.groups[0].count,0)
 assert.equal(afterFresh.groups[1].count,15)
})

test('pending defer and restore mutations adjust deferred and view counts cleanly',()=>{
 const base={
  counts:{all:10,suggested:8,conflict:2,none:0,manual:0,deferred:4},
  groups:[{key:'person:a',label:'a',kind:'suggested',count:5}],
  group:{key:'person:a',label:'a',kind:'suggested',count:5},
  total:5,
  remaining:5,
  rows:[{vid:'a-1'},{vid:'a-2'}],
 }
 const deferred=applyPendingReviewMutations(base,0,[{
  id:'d-1',action:'defer',groupKey:'person:a',isDeferred:false,vids:['a-1','a-2'],buckets:{suggested:1,conflict:1},settledAtSeq:null,
 }],'suggested')
 assert.equal(deferred.counts.all,8)
 assert.equal(deferred.counts.suggested,7)
 assert.equal(deferred.counts.conflict,1)
 assert.equal(deferred.counts.deferred,6)
 assert.equal(deferred.groups[0].count,3)
 assert.equal(deferred.remaining,3)

 const restored=applyPendingReviewMutations({...base,group:{key:'person:a',label:'a',kind:'suggested',count:2},remaining:2,groups:[{key:'person:a',label:'a',kind:'suggested',count:2}]},0,[{
  id:'r-1',action:'restore',groupKey:'person:a',isDeferred:true,vids:['a-1'],buckets:{suggested:1},settledAtSeq:null,
 }],'deferred')
 assert.equal(restored.counts.deferred,3)
 assert.equal(restored.counts.all,11)
 assert.equal(restored.counts.suggested,9)
 assert.equal(restored.groups[0].count,1)
 assert.equal(restored.remaining,1)
})

test('pending delete mutations project across normal and deferred lanes and prune cleanly',()=>{
 const base={
  counts:{all:12,suggested:9,conflict:2,none:1,manual:0,deferred:3},
  groups:[{key:'person:菠萝啤beer',label:'菠萝啤beer',kind:'suggested',count:5}],
  group:{key:'person:菠萝啤beer',label:'菠萝啤beer',kind:'suggested',count:5},
  total:5,
  remaining:5,
  rows:[{vid:'b-1'},{vid:'b-2'},{vid:'b-3'}],
 }
 const op={
  id:'del-1',
  action:'delete',
  groupKey:'person:菠萝啤beer',
  isDeferred:false,
  vids:['b-1','b-2'],
  buckets:{suggested:2},
  settledAtSeq:null,
 }
 const inFlight=applyPendingReviewMutations(base,0,[op],'all')
 assert.equal(inFlight.counts.all,10)
 assert.equal(inFlight.counts.suggested,7)
 assert.equal(inFlight.counts.deferred,3)
 assert.equal(inFlight.groups[0].count,3)
 assert.equal(inFlight.group.count,3)
 assert.equal(inFlight.remaining,3)
 assert.deepEqual(inFlight.rows.map(r=>r.vid),['b-3'])

 const settled=[{...op,settledAtSeq:2}]
 const stillProjected=applyPendingReviewMutations(base,1,settled,'all')
 assert.equal(stillProjected.rows.length,1)
 assert.deepEqual(pruneSettledReviewMutations(settled,2),[])

 const deferredDelete=applyPendingReviewMutations(
  {...base,counts:{...base.counts},rows:[{vid:'d-1'},{vid:'d-2'}]},
  0,
  [{id:'del-2',action:'delete',groupKey:'person:菠萝啤beer',isDeferred:true,vids:['d-1'],buckets:{conflict:1},settledAtSeq:null}],
  'deferred',
 )
 assert.equal(deferredDelete.counts.deferred,2)
 assert.equal(deferredDelete.counts.all,12)
 assert.equal(deferredDelete.counts.conflict,2)
 assert.equal(deferredDelete.groups[0].count,4)
 assert.deepEqual(deferredDelete.rows.map(r=>r.vid),['d-2'])
})
