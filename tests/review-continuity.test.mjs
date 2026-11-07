import {test} from 'node:test'
import assert from 'node:assert/strict'
import {REVIEW_IDLE_MS,canAutoRefreshReview,isReviewAvailable} from '../lib/review-continuity-model.ts'
test('idle boundary and responses started before new activity never replace a review snapshot',()=>{
 const state={now:REVIEW_IDLE_MS,lastActivity:0,protectedWork:false,requestActivity:7,currentActivity:7}
 assert.equal(canAutoRefreshReview(state),true)
 assert.equal(canAutoRefreshReview({...state,now:REVIEW_IDLE_MS-1}),false)
 assert.equal(canAutoRefreshReview({...state,protectedWork:true}),false)
 assert.equal(canAutoRefreshReview({...state,currentActivity:8}),false)
 assert.equal(canAutoRefreshReview({...state,lastActivity:1}),false)
})

test('ready face evidence remains reviewable after terminal failure, without reviving text failures or missing evidence',()=>{
 for(const stage of ['face_queued','face','retry','error'])assert.equal(isReviewAvailable({stage,retry_stage:'face',manual:0,data:{ready:true}}),true)
 assert.equal(isReviewAvailable({stage:'error',retry_stage:'text',manual:0,data:{ready:true}}),false)
 assert.equal(isReviewAvailable({stage:'error',retry_stage:'face',manual:0,data:{ready:false}}),false)
 assert.equal(isReviewAvailable({stage:'excluded',retry_stage:'face',manual:0,data:{ready:true}}),false)
})
