import { test } from 'node:test'
import assert from 'node:assert/strict'
import { enqueueItems, undoItems, origin, validateCreator, decodedName } from '../lib/review-logic.ts'

test('approval queues with a return path, and undo restores it to pending', () => {
  const {queue, added} = enqueueItems([], '娜娜', ['one'], origin('approval', ''))
  assert.equal(added.length, 1)
  assert.equal(queue[0].return_href, '/approvals')
  const undone = undoItems(queue, {approved:['one','unrelated'],rejected:[]}, ['one'])
  assert.equal(undone.queue.length, 0)
  assert.deepEqual(undone.decisions.approved, ['unrelated'])
  assert.equal(undone.removed[0].return_href, '/approvals')
})
test('partial undo preserves unrelated queued videos and creator conflicts never overwrite', () => {
  const {queue} = enqueueItems([], '娜娜', ['a','b'], origin('cluster','p1'))
  assert.equal(enqueueItems(queue,'娜娜',['a'],origin('person','娜娜')).added.length,0)
  assert.throws(()=>enqueueItems(queue,'其他人',['a'],origin('person','其他人')), /其他人物/)
  const result = undoItems(queue,{approved:[],rejected:[]},['a'])
  assert.deepEqual(result.queue.map(v=>v.vid),['b'])
  assert.equal(result.removed[0].return_href,'/clusters?cid=p1')
})
test('Chinese names decode once; invalid directory names are rejected', () => {
  assert.equal(decodedName('%E5%A8%9C%E5%A8%9C'),'娜娜')
  assert.equal(decodedName('%bad'),'％bad'.replace('％','%'))
  for (const n of ['..','../x','a/b','a\\b','']) assert.throws(()=>validateCreator(n))
})
