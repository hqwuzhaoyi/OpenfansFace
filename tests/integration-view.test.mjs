import {test} from 'node:test'
import assert from 'node:assert/strict'
import {groupSourceFolders,toggleArchiveSelection} from '../lib/integration-view.ts'

test('relocated videos group by actual source, not old ID or target creator',()=>{
  const rows=[
    {vid:'R:\\media\\old\\a.mp4',src:'/media/travel/clip/a.mp4',dst:'/creators/A/a.mp4'},
    {vid:'R:\\media\\another\\b.mp4',src:'/media/travel/clip/b.mp4',dst:'/creators/B/b.mp4'},
    {vid:'c',src:'/media/daily/clip/c.mp4',dst:'/creators/A/c.mp4'},
  ]
  const folders=groupSourceFolders(rows)
  assert.equal(folders.length,2)
  assert.equal(folders[0].path,'/media/travel/clip')
  assert.deepEqual(folders[0].items,[rows[0],rows[1]])
  assert.equal(folders[1].path,'/media/daily/clip')
})
test('missing sources retain their original directory, with an explicit fallback for bare IDs',()=>{
  const folders=groupSourceFolders([{vid:'R:\\media\\old\\missing.mp4',src:null},{vid:'unknown',src:null},{vid:'/root.mp4'}])
  assert.deepEqual(folders.map(f=>f.path),['R:/media/old','未记录目录','/'])
})
test('partially selected folder becomes selected without changing other folders or hidden items',()=>{
  const original=new Set(['a','outside','hidden'])
  const selected=toggleArchiveSelection(original,['a','b'])
  assert.deepEqual([...selected],['a','outside','hidden','b'])
  assert.deepEqual([...original],['a','outside','hidden'])
  assert.deepEqual([...toggleArchiveSelection(selected,['a','b'])],['outside','hidden'])
  assert.deepEqual([...toggleArchiveSelection(selected,[])],[...selected])
})
