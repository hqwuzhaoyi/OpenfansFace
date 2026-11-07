import { test } from 'node:test'
import assert from 'node:assert/strict'
import { jobPercent,normalizeJob,reconcileProcessingJob,homeTaskStage,taskMatchesStage } from '../lib/home-model.ts'
test('unknown, zero and invalid totals never produce a fabricated percentage',()=>{
 for(const total of [null,0,-1,NaN,Infinity])assert.equal(jobPercent({done:4,total}),null)
 assert.equal(jobPercent({done:null,total:8}),null)
 assert.equal(jobPercent({done:3,total:8}),37)
 assert.equal(jobPercent({done:9,total:8}),100)
})
test('a restarted worker cannot make an abandoned frame counter look active',()=>{
 const job={id:'old',kind:'text',vid:'v1',status:'running',done:3,total:8,updated_at:900}
 assert.equal(reconcileProcessingJob(job,{vid:'v1',stage:'retry',updated_at:1000,error:'服务重启'},true).status,'error')
 assert.equal(reconcileProcessingJob(job,{vid:'v1',stage:'face_queued',updated_at:1000},true).status,'complete')
 assert.equal(reconcileProcessingJob(job,undefined,true).status,'waiting')
 assert.equal(reconcileProcessingJob(job,{vid:'v1',stage:'text',updated_at:1000},true).status,'running')
})
test('queue filters and counters place a manually taken over error in review only',()=>{
 const task={stage:'error',manual:1}
 assert.equal(homeTaskStage(task),'review')
 assert.equal(taskMatchesStage(task,'review'),true)
 assert.equal(taskMatchesStage(task,'error'),false)
 assert.equal(taskMatchesStage({stage:'face_queued',manual:0},'face'),true)
 assert.equal(taskMatchesStage({stage:'text',manual:0},'face'),false)
})
test('stale and future heartbeats freeze active jobs without rewriting completed results',()=>{
 const job={id:'run1',status:'running',done:3,total:8}
 assert.equal(normalizeJob(job,1000,990).status,'running')
 assert.equal(normalizeJob(job,1000,800).status,'offline')
 assert.equal(normalizeJob(job,1000,1040).status,'offline')
 assert.equal(normalizeJob({...job,status:'complete'},1000,800).status,'complete')
 assert.equal(normalizeJob({...job,status:'error'},1000,null).status,'error')
 assert.equal(normalizeJob({...job,done:-1,total:NaN},1000,990).done,null)
})
