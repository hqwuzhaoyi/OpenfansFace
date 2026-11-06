import {test} from 'node:test'
import assert from 'node:assert/strict'
import {chooseSource,sourceCandidates} from '../lib/source-resolution.ts'
const root='/mnt/media2',vid='R:\\media\\pikpak\\My Telegram\\old.mp4'
test('uses the recorded new path after files move, keeping the stable ID',()=>{
 const actual='/mnt/media2/pikpak/creators/person/new.mp4'
 assert.equal(chooseSource(vid,actual,root,p=>p===actual),actual)
})
test('an incorrect directory in the database does not hide a valid original file',()=>{
 const original='/mnt/media2/pikpak/My Telegram/old.mp4'
 assert.equal(chooseSource(vid,'/mnt/media2/pikpak/creators/person',root,p=>p===original),original)
})
test('unrelated paths and path traversal are not source candidates',()=>{
 assert.deepEqual(sourceCandidates('R:\\media\\pikpak\\..\\etc','/etc/passwd',root),[])
})

test('organized scope follows the resolved path rather than the historical ID',async()=>{
 const {creatorFolder}=await import('../lib/source-resolution.ts')
 const oldId='R:\\media\\pikpak\\My Telegram\\old.mp4'
 const moved='/mnt/media2/pikpak/creators/小敏儿/clip.mp4'
 assert.equal(creatorFolder(chooseSource(oldId,moved,root,p=>p===moved),root),'小敏儿')
 const unsorted='/mnt/media2/pikpak/My Telegram/clip.mp4'
 assert.equal(creatorFolder(chooseSource('R:\\media\\pikpak\\creators\\old\\clip.mp4',unsorted,root,p=>p===unsorted),root),null)
 assert.equal(creatorFolder('/mnt/media2/pikpak/creators-other/clip.mp4',root),null)
})
