import {test} from 'node:test'
import assert from 'node:assert/strict'
import {savePerson,mergePeople} from '../lib/people-model.ts'
test('manual person survives an empty queue, archive list, and recognition report',()=>{
 const saved=savePerson([],'嗨酱','2026-11-04T07:00:30Z')
 assert.deepEqual(mergePeople([],saved,[]).map(p=>p.name),['嗨酱'])
})
test('existing user-created people can be recovered from pending and historical assignments',()=>{
 const people=mergePeople([{name:'模型人物'}],[],[{name:'嗨酱',recent_at:'2026-11-04T07:00:30Z'},{name:'嗨酱',recent_at:'2026-11-04T06:00:30Z',rep_thumb:'/thumb?p=face.jpg'}])
 assert.equal(people.filter(p=>p.name==='嗨酱').length,1)
 assert.equal(people[0].name,'嗨酱')
 assert.equal(people[0].recent_at,'2026-11-04T07:00:30Z')
})
test('reusing a person preserves creation time and refreshes recent usage',()=>{
 const a=savePerson([],'嗨酱','2026-11-04T06:00:00Z','face.jpg')
 const b=savePerson(a,'嗨酱','2026-11-04T07:00:00Z')
 assert.equal(b.length,1);assert.equal(b[0].created_at,a[0].created_at);assert.equal(b[0].rep_thumb,'face.jpg')
})
