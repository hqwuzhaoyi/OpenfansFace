import {test} from 'node:test'
import assert from 'node:assert/strict'
import {sortedLibraryPeople} from '../lib/person-list-model.ts'
const rows=[{name:'乙',aliases:['旧名字'],video_count:2,total_bytes:30},{name:'甲',video_count:5,total_bytes:10},{name:'不可读',video_count:null,total_bytes:null}]
test('person search includes aliases and sorting uses numeric counts and bytes',()=>{
  assert.deepEqual(sortedLibraryPeople(rows,'旧名字','name-asc').map(p=>p.name),['乙'])
  assert.deepEqual(sortedLibraryPeople(rows,'','videos-desc').map(p=>p.name),['甲','乙','不可读'])
  assert.deepEqual(sortedLibraryPeople(rows,'','bytes-desc').map(p=>p.name),['乙','甲','不可读'])
  assert.deepEqual(sortedLibraryPeople(rows,'','videos-asc').map(p=>p.name),['乙','甲','不可读'])
  assert.equal(rows[0].name,'乙')
})
test('unknown counts stay last in either direction and names break ties predictably',()=>{
  const values=[{name:'person10',video_count:1,total_bytes:0},{name:'person2',video_count:1,total_bytes:0},{name:'unknown'}]
  assert.deepEqual(sortedLibraryPeople(values,'','videos-desc').map(p=>p.name),['person2','person10','unknown'])
  assert.deepEqual(sortedLibraryPeople(values,'PERSON','name-desc').map(p=>p.name),['person10','person2'])
})
