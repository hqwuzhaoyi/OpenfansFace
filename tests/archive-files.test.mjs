import { test } from 'node:test'
import assert from 'node:assert/strict'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { alternateName, validateArchiveName, identicalFiles, moveExclusive, moveExclusiveAsync } from '../lib/archive-files.ts'

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

test('async cross-device moves never replace a destination and preserve bytes', async (t) => {
  const dir=fs.mkdtempSync(path.join(os.tmpdir(),'face-async-move-'))
  t.mock.method(fs.promises,'link',async()=>{throw Object.assign(new Error('cross-device'),{code:'EXDEV'})})
  try{
    const src=path.join(dir,'src'),dst=path.join(dir,'dst'),fresh=path.join(dir,'fresh')
    fs.writeFileSync(src,'source bytes');fs.writeFileSync(dst,'existing bytes')
    await assert.rejects(moveExclusiveAsync(src,dst),{code:'EEXIST'})
    assert.equal(fs.readFileSync(dst,'utf8'),'existing bytes');assert.ok(fs.existsSync(src))
    await moveExclusiveAsync(src,fresh)
    assert.equal(fs.readFileSync(fresh,'utf8'),'source bytes');assert.equal(fs.existsSync(src),false)
  }finally{fs.rmSync(dir,{recursive:true,force:true})}
})

test('async copy detects a changed source and retains that source', async (t) => {
  const dir=fs.mkdtempSync(path.join(os.tmpdir(),'face-copy-change-')),copy=fs.promises.copyFile.bind(fs.promises)
  t.mock.method(fs.promises,'link',async()=>{throw Object.assign(new Error('cross-device'),{code:'EXDEV'})})
  t.mock.method(fs.promises,'copyFile',async(src,dst,flags)=>{await copy(src,dst,flags);fs.writeFileSync(src,'external replacement content')})
  try{
    const src=path.join(dir,'src'),dst=path.join(dir,'dst');fs.writeFileSync(src,'before')
    await assert.rejects(moveExclusiveAsync(src,dst),/文件发生变化/)
    assert.equal(fs.readFileSync(src,'utf8'),'external replacement content');assert.equal(fs.existsSync(dst),false)
  }finally{fs.rmSync(dir,{recursive:true,force:true})}
})
