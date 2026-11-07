import {test} from 'node:test'
import assert from 'node:assert/strict'
import {reviewListSummary,candidateReasons} from '../lib/review-list-summary.ts'
const candidate=(name,evidence)=>({name,sources:evidence.map(e=>e.source),score:null,evidence})
const task=(candidates,extra={})=>({manual:0,error:null,data:{kind:'suggested',candidates,...extra}})
const mark=(key,text,platform,time)=>({key,text,platform,timestamp:time,frames:4,score:.99,kind:'account'})
test('unconfirmed watermark text never becomes confirmed identity or a confidence percentage',()=>{
 const r=reviewListSummary(task([candidate('Alice',[{source:'watermark',key:'a',confirmed:false,detail:'账号尚未确认：@Alice'}])],{watermark:{status:'done',candidates:[mark('a','@Alice','X',12)]}}))
 assert.equal(r.status,'待核对');assert.equal(r.marks[0].label,'账号待核对');assert.equal(r.candidates[0].reasons[0].confirmed,false);assert.match(r.candidates[0].reasons[0].text,/尚未确认/);assert.doesNotMatch(JSON.stringify(r),/99%/)
})
test('confirmed mapping and channel text remain distinct; relevant author frame comes first',()=>{
 const r=reviewListSummary(task([candidate('Alice',[{source:'watermark',key:'a',confirmed:true,detail:'@author'}])],{watermark:{status:'done',candidates:[mark('channel','@mirror','Telegram',1),mark('a','@author','X',28)]}}))
 assert.equal(r.status,'待核对');assert.equal(r.sampleTime,28);assert.equal(r.marks[0].label,'已确认映射');assert.equal(r.marks[1].label,'频道线索');assert.match(r.candidates[0].reasons[0].text,/本轮命中已确认账号映射/)
})
test('conflicting candidates preserve their separate reasons and do not rank one as resolved',()=>{
 const r=reviewListSummary(task([candidate('Alice',[{source:'folder',detail:'文件名或文件夹名称命中'}]),candidate('Bob',[{source:'face',score:.71,support:2,detail:'需人工核对'}])]))
 assert.equal(r.conflict,true);assert.equal(r.status,'候选冲突');assert.deepEqual(r.candidates.map(c=>c.name),['Alice','Bob']);assert.match(r.candidates[1].reasons[0].text,/0.710.*2 张脸/);assert.doesNotMatch(r.candidates[1].reasons[0].text,/71%/)
})
test('missing measurements and reasons stay unknown instead of invented zeroes',()=>{
 assert.match(candidateReasons(candidate('A',[{source:'face'}]))[0].text,/未记录/)
 assert.doesNotMatch(candidateReasons(candidate('A',[{source:'face'}]))[0].text,/0 张脸/)
 assert.match(candidateReasons(candidate('A',[]))[0].text,/依据未记录/)
 const r=reviewListSummary(task([],{watermark:{candidates:[{key:'x',text:'raw',frames:NaN,timestamp:NaN}]}}));assert.equal(r.status,'需选择人物');assert.equal(r.marks[0].frames,null);assert.equal(r.sampleTime,.1)
})
test('manual takeover and conflicting evidence remain visible together',()=>{
 const r=reviewListSummary({...task([candidate('A',[]),candidate('B',[])]),manual:1,error:'原视频读取失败'})
 assert.equal(r.status,'候选冲突');assert.equal(r.warning,'原视频读取失败')
})
