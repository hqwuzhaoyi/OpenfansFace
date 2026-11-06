import { test } from 'node:test'
import assert from 'node:assert/strict'
import { healthMatches } from '../lib/video-health-model.ts'

test('a quarantine applies only to the exact checked file, never a replacement or new location',()=>{
  const row={vid:'one',path:'/media/a.mp4',status:'corrupt',hidden:true,size:123,mtime_ns:'1000000001'}
  assert.equal(healthMatches(row,'/media/a.mp4',123,'1000000001'),true)
  assert.equal(healthMatches(row,'/media/a.mp4',123,'1000000002'),false)
  assert.equal(healthMatches(row,'/media/a.mp4',124,'1000000001'),false)
  assert.equal(healthMatches(row,'/media/new/a.mp4',123,'1000000001'),false)
  assert.equal(healthMatches({...row,hidden:false},'/media/a.mp4',123,'1000000001'),false)
})
