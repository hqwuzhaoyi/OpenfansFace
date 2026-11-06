import {test} from 'node:test'
import assert from 'node:assert/strict'
import {groupBatches} from '../lib/review-batches.ts'
import {filterArchiveBatches,retainArchiveSelection,summarizeArchiveSelection} from '../lib/archive-batch-selection.ts'
import {toggleArchiveSelection} from '../lib/integration-view.ts'

const rows=[
 {vid:'a1',name:'a1.mp4',creator:'A',batch_id:'first',at:'2026-11-06T10:00:00Z',status:'ok',src:'/media/a/a1.mp4'},
 {vid:'a2',name:'a2.mp4',creator:'A',batch_id:'first',at:'2026-11-06T10:00:00Z',status:'conflict',src:'/media/a/a2.mp4'},
 {vid:'b1',name:'b1.mp4',creator:'B',batch_id:'second',at:'2026-11-06T09:00:00Z',status:'ok',src:'/media/b/b1.mp4'},
 {vid:'a3',name:'a3.mp4',creator:'A',batch_id:'third',at:'2026-11-05T10:00:00Z',status:'ok',src:'/media/a/a3.mp4'},
]
const waves=groupBatches(rows)

test('batch search reveals whole matching waves without merging the same creator across times',()=>{
 const result=filterArchiveBatches(waves,'a1.mp4','all',new Set())
 assert.equal(result.length,1)
 assert.deepEqual(result[0].rows.map(r=>r.vid),['a1','a2'])
 assert.equal(waves.filter(w=>w.creator==='A').length,2)
})
test('cross-wave mixed selection submits ready IDs even when search hides a selected wave',()=>{
 let selection=toggleArchiveSelection(new Set(),['a1','a2'])
 selection=toggleArchiveSelection(selection,['b1'])
 const visible=filterArchiveBatches(waves,'b1.mp4','all',selection)
 const summary=summarizeArchiveSelection(rows,selection,new Set(visible.flatMap(w=>w.rows.map(r=>r.vid))))
 assert.deepEqual(summary.ready.map(r=>r.vid),['a1','b1'])
 assert.deepEqual(summary.blocked.map(r=>r.vid),['a2'])
 assert.equal(summary.hidden,2)
 assert.equal(summary.chosen.length,3)
 assert.equal(selection.size,3)
})
test('status filters retain complete mixed waves; selected view finds every chosen wave',()=>{
 const selection=new Set(['a2','b1'])
 assert.deepEqual(filterArchiveBatches(waves,'','issues',selection).map(w=>w.key),['first'])
 assert.deepEqual(filterArchiveBatches(waves,'','ok',selection)[0].rows.map(r=>r.vid),['a1','a2'])
 assert.deepEqual(filterArchiveBatches(waves,'','selected',selection).map(w=>w.key),['first','second'])
})
test('single-video exclusion preserves other wave selections and partial failure keeps remaining IDs',()=>{
 const selection=toggleArchiveSelection(new Set(['a1','a2','b1','a3']),['a3'])
 assert.deepEqual([...selection],['a1','a2','b1'])
 const remaining=retainArchiveSelection(selection,rows.filter(r=>r.vid!=='a1'))
 assert.deepEqual([...remaining],['a2','b1'])
 assert.deepEqual(summarizeArchiveSelection(rows,remaining,new Set()).ready.map(r=>r.vid),['b1'])
})
test('refresh never selects newly queued rows; only-invalid selections have no executable IDs',()=>{
 const selected=new Set(['a2','gone'])
 const remaining=retainArchiveSelection(selected,[...rows,{vid:'new'}])
 assert.deepEqual([...remaining],['a2'])
 assert.equal(summarizeArchiveSelection(rows,remaining,new Set()).ready.length,0)
})
