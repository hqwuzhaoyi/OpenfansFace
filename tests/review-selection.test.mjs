import {matchesReviewSelection} from '../lib/processing-model.ts'
import {test} from 'node:test'
import assert from 'node:assert/strict'
import {toggleReviewSelection,removeReviewSelection,reviewSelectionKey} from '../lib/review-selection.ts'
const task=(vid,name='Alice',extras={})=>({vid,path:`/test/${vid}.mp4`,size:5,mtime_ns:'123',stage:'review',deferred:0,manual:0,data:{kind:'suggested',suggested:name},...extras})
test('page selection keeps hidden selections and preserves their submission fingerprints',()=>{
 const first=task('page-1'),second=task('page-2')
 const selected=toggleReviewSelection(toggleReviewSelection([],[first]),[second])
 assert.deepEqual(selected.map(t=>t.vid),['page-1','page-2'])
 const afterPageUncheck=toggleReviewSelection(selected,[second])
 assert.deepEqual(afterPageUncheck,[first])
 assert.equal(afterPageUncheck[0].mtime_ns,'123')
})
test('different candidate groups and deferred views have separate selection scopes',()=>{
 const a=reviewSelectionKey('person:Alice',false),b=reviewSelectionKey('person:Bob',false),deferred=reviewSelectionKey('person:Alice',true)
 assert.equal(new Set([a,b,deferred]).size,3)
 const state={[a]:[task('a')],[b]:[task('b','Bob')],[deferred]:[task('a','Alice',{deferred:1})]}
 const next=removeReviewSelection(state,['a'])
 assert.deepEqual(next[b],state[b]);assert.deepEqual(next[a],[]);assert.deepEqual(next[deferred],[])
 assert.equal(state[a].length,1)
})
test('server group check rejects regrouped, deferred and no-longer-ready snapshots',()=>{
 const scope={key:'person:Alice',deferred:false}
 assert.equal(matchesReviewSelection(task('a'),scope),true)
 assert.equal(matchesReviewSelection(task('a','Bob'),scope),false)
 assert.equal(matchesReviewSelection(task('a','Alice',{deferred:1}),scope),false)
 assert.equal(matchesReviewSelection(task('a','Alice',{stage:'queued'}),scope),false)
 assert.equal(matchesReviewSelection(task('a','Alice',{stage:'error',manual:1}),scope),false)
})
