import { test } from 'node:test'
import assert from 'node:assert/strict'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { alternateName, validateArchiveName, identicalFiles, moveExclusive } from '../lib/archive-files.ts'

test('same size is not enough to remove a duplicate; every byte must match', async () => {
  const dir=fs.mkdtempSync(path.join(os.tmpdir(),'face-content-test-'))
  try {
    const a=path.join(dir,'a'),b=path.join(dir,'b')
    fs.writeFileSync(a,'video-one'); fs.writeFileSync(b,'video-two')
    assert.equal(fs.statSync(a).size,fs.statSync(b).size)
    assert.equal(await identicalFiles(a,b),false)
    fs.copyFileSync(a,b); assert.equal(await identicalFiles(a,b),true)
  } finally { fs.rmSync(dir,{recursive:true,force:true}) }
})

test('exclusive moves preserve an occupied destination and are recoverable', () => {
  const dir=fs.mkdtempSync(path.join(os.tmpdir(),'face-move-test-'))
  try {
    const a=path.join(dir,'a'),b=path.join(dir,'b'),stage=path.join(dir,'stage','a')
    fs.writeFileSync(a,'source'); fs.writeFileSync(b,'organized')
    assert.throws(()=>moveExclusive(a,b),{code:'EEXIST'})
    assert.equal(fs.readFileSync(a,'utf8'),'source'); assert.equal(fs.readFileSync(b,'utf8'),'organized')
    moveExclusive(a,stage); assert.equal(fs.existsSync(a),false)
    moveExclusive(stage,a); assert.equal(fs.readFileSync(a,'utf8'),'source')
  } finally { fs.rmSync(dir,{recursive:true,force:true}) }
})

test('alternate filenames preserve extension, fit filesystem limits and forbid traversal', () => {
  const name=alternateName('很'.repeat(180)+'.mp4','one')
  assert.ok(Buffer.byteLength(name)<=240); assert.ok(name.endsWith('.mp4'))
  assert.notEqual(alternateName('v2.mp4','one'),alternateName('v2.mp4','two'))
  for(const bad of ['../v.mp4','x\\v.mp4','a/b.mp4','a:video.mp4','..','a.mp4 ']) assert.throws(()=>validateArchiveName(bad))
})
