import {test} from 'node:test'
import assert from 'node:assert/strict'
import {readWorkspaceQuery,writeWorkspaceQuery} from '../lib/workspace-query.ts'
import {matchesPersonQuery} from '../lib/person-list-model.ts'
const defaults={stage:'',batch:'',q:'',page:'0'}
test('shared filter state restores search, batch and page together',()=>{
 const state=readWorkspaceQuery(new URLSearchParams('stage=review&batch=2026-11&q=%E5%90%8D%E5%AD%97+part&page=2'),defaults,{stage:['','review']})
 assert.deepEqual(state,{stage:'review',batch:'2026-11',q:'名字 part',page:'2'})
 const back=writeWorkspaceQuery(new URLSearchParams('external=kept'),state,defaults)
 assert.equal(back.get('external'),'kept');assert.deepEqual(readWorkspaceQuery(back,defaults),state)
})
test('invalid enums and pages cannot create broken queue navigation',()=>{
 for(const page of ['-1','NaN','Infinity',''])assert.equal(readWorkspaceQuery(new URLSearchParams({page,stage:'invalid'}),defaults,{stage:['','review']}).page,'0')
 assert.equal(readWorkspaceQuery(new URLSearchParams('stage=invalid'),defaults,{stage:['','review']}).stage,'')
 assert.equal(writeWorkspaceQuery(new URLSearchParams('stage=review&page=2'),defaults,defaults).toString(),'')
})
test('person picker and person list share alias, Unicode and multiword matching',()=>{
 const person={name:'Ａlice Lee',aliases:['小鹿','AL']}
 assert.equal(matchesPersonQuery(person,'alice lee'),true)
 assert.equal(matchesPersonQuery(person,'小鹿'),true)
 assert.equal(matchesPersonQuery(person,'alice missing'),false)
})
