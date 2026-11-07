import {test} from 'node:test'
import assert from 'node:assert/strict'
import {parsePersonAccounts,manualAccountMatches} from '../lib/person-account-model.ts'

const profile={id:'100',handle:'sample_01',name:'完全不同的昵称',description:'账号简介',followers:12,synced_at:null}
const row={id:'link-one',creator:'本地人物',profile,linked_at:'2026-11-07T00:00:00Z',source_fetched_at:'2026-11-06T00:00:00Z'}
const store=links=>({version:1,source:'nv-pu-sa',links})
test('manual association works without a name match and survives an unavailable directory',()=>{
  const links=parsePersonAccounts(store([row])),match=manualAccountMatches(links,'本地人物')[0]
  assert.equal(match.profile.name,'完全不同的昵称');assert.equal(match.basis,'manual-link');assert.equal(match.manual_link_id,'link-one')
  assert.deepEqual(manualAccountMatches(links,'其他人'),[])
})
test('stable profile ID follows current account details, not a recycled handle',()=>{
  const current={...profile,handle:'renamed_01'},recycled={...profile,id:'200',name:'另一个账号'}
  assert.equal(manualAccountMatches([row],'本地人物',[recycled,current])[0].profile.handle,'renamed_01')
  assert.equal(row.profile.handle,'sample_01')
})
test('corrupt or ambiguous persisted links fail instead of silently replacing data',()=>{
  for(const value of [null,[],{version:2,links:[]},store([{...row,profile:{...profile,id:'not-an-id'}}]),store([{...row,linked_at:'invalid'}]),store([row,row]),store([row,{...row,id:'second'}])])assert.throws(()=>parsePersonAccounts(value))
  assert.equal(parsePersonAccounts(store([row,{...row,id:'second',creator:'另一个人物'}])).length,2)
})
