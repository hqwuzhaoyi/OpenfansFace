import {test} from 'node:test'
import assert from 'node:assert/strict'
import {groupBatches,selectedProblems} from '../lib/review-batches.ts'
import {enqueueItems,origin} from '../lib/review-logic.ts'

test('one batch confirmation shares an identifier; subsequent confirmation is a separate wave',()=>{
 const first=enqueueItems([],'娜娜',['a','b'],origin('cluster','p1')).added
 assert.ok(first[0].batch_id)
 assert.equal(first[0].batch_id,first[1].batch_id)
 const next=enqueueItems(first,'娜娜',['c'],origin('cluster','p1')).added
 assert.notEqual(first[0].batch_id,next[0].batch_id)
})
test('fresh valid video is not mixed with historical conflicting and missing files',()=>{
 const rows=[{vid:'old',creator:'旧人物',at:'2026-11-01 14:54:27',status:'conflict'},
 {vid:'missing',creator:'旧人物',at:'2026-11-01 14:54:27',status:'missing'},
 {vid:'new',creator:'新人物',at:'2026-11-02T15:31:25Z',status:'ok',batch_id:'new-wave'}]
 const waves=groupBatches(rows)
 assert.equal(waves.length,2)
 assert.deepEqual(waves[0].rows.map(r=>r.vid),['new'])
 assert.equal(waves[1].inferred,true)
 assert.equal(waves[1].rows.length,2)
})
test('mixed selection explains exactly why archive is blocked',()=>{
 assert.deepEqual(selectedProblems([{name:'new.mp4',status:'ok'},{name:'V (7).mp4',status:'conflict'}]),[{name:'V (7).mp4',reason:'目标同名冲突'}])
})

test('finishing a wave advances to the next actionable wave and skips error-only waves',async()=>{
 const {nextReviewWave}=await import('../lib/review-batches.ts')
 const rows=[{vid:'a',creator:'A',at:'2026-11-04T12:00:00Z',batch_id:'a',status:'ok'},{vid:'e',creator:'E',at:'2026-11-04T11:00:00Z',batch_id:'e',status:'missing'},{vid:'b',creator:'B',at:'2026-11-04T10:00:00Z',batch_id:'b',status:'ok'}]
 const before=groupBatches(rows),after=groupBatches(rows.slice(1))
 assert.equal(nextReviewWave(before,'a',after).key,'b')
 assert.equal(nextReviewWave(before,'b',[]),null)
 assert.equal(nextReviewWave(before,'a',groupBatches(rows.slice(1,2))).key,'e')
})
