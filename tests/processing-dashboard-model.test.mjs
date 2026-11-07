import {test} from 'node:test'
import assert from 'node:assert/strict'
import {buildProcessingBatches,batchTotals,cohortBucket,validateConcurrency} from '../lib/processing-dashboard-model.ts'
const item=(overrides={})=>({batch_id:'b',vid:'v',added_at:100,completed:0,text_done:0,face_required:null,face_done:0,face_skipped:0,excluded:0,legacy:0,current:1,stage:'queued',manual:0,...overrides})
test('cohort denominator and successful completion survive confirmation and archival',()=>{
 const before=buildProcessingBatches([item({completed:1,text_done:1,face_required:1,face_done:1,stage:'review'})])
 const after=buildProcessingBatches([item({completed:1,text_done:1,face_required:1,face_done:1,handled:true,stage:'excluded',excluded:1})])
 assert.deepEqual(batchTotals(before),batchTotals(after))
 assert.equal(after[0].total,1);assert.equal(after[0].completed,1)
})
test('manual takeover and removed inputs are never invented machine successes',()=>{
 assert.equal(cohortBucket(item({manual:1,stage:'error'})),'excluded')
 assert.equal(cohortBucket(item({handled:true,stage:'error'})),'excluded')
 assert.equal(cohortBucket(item({current:0})),'excluded')
})
test('reference refresh is visible but does not count a cohort member twice',()=>{
 const batches=buildProcessingBatches([item({vid:'a',completed:1,stage:'face'}),item({vid:'b',stage:'text'}),item({vid:'c',stage:'error'}),item({vid:'d',stage:'queued'}),item({vid:'e',excluded:1})])
 const totals=batchTotals(batches)
 assert.equal(totals.refreshing,1)
 assert.equal(totals.completed+totals.running+totals.waiting+totals.error+totals.excluded,totals.total)
 assert.equal(totals.total,5);assert.equal(totals.running,1)
})
test('face denominator excludes unknown and skipped items',()=>{
 const b=buildProcessingBatches([item({vid:'unknown'}),item({vid:'skip',text_done:1,face_required:0,face_skipped:1,completed:1}),item({vid:'face',text_done:1,face_required:1,face_done:1,completed:1})])[0]
 assert.equal(b.text_done,2);assert.equal(b.face_required,1);assert.equal(b.face_done,1);assert.equal(b.face_skipped,1)
})
test('concurrency accepts integers only, within the actual worker capacity',()=>{
 assert.equal(validateConcurrency(2,2),2)
 for(const v of [0,-1,3,2.5,'2',true,null,NaN,Infinity])assert.throws(()=>validateConcurrency(v,2))
 assert.equal(validateConcurrency(4,4),4);assert.throws(()=>validateConcurrency(5,99))
})
