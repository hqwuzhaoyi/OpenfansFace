import { test } from 'node:test'
import assert from 'node:assert/strict'
import fs from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import http from 'node:http'
import { execFile } from 'node:child_process'
import { promisify } from 'node:util'

test('overlapping ledger refreshes extract separately and publish only a complete copy', {timeout:10000}, async()=>{
  const root=await fs.mkdtemp(path.join(os.tmpdir(),'face-ledger-test-'))
  const socket=path.join(root,'docker.sock'),dest=path.join(root,'ledger-readonly.db')
  process.env.DOCKER_SOCK=socket;process.env.PEACH_CONTAINER='fixture'
  await fs.mkdir(path.join(root,'source'))
  await fs.writeFile(path.join(root,'source','ledger.db'),'complete-ledger-snapshot')
  await promisify(execFile)('tar',['-cf',path.join(root,'snapshot.tar'),'-C',path.join(root,'source'),'ledger.db'])
  const tar=await fs.readFile(path.join(root,'snapshot.tar'))
  let requests=0,corrupt=false
  const server=http.createServer(async(req,res)=>{
    if(!req.url.includes('/containers/fixture/archive')) {res.writeHead(404);res.end();return}
    requests++;res.writeHead(200,{'Content-Type':'application/x-tar'})
    if(corrupt){res.end('invalid archive');return}
    // Force all callers to overlap while their temporary files are still being written.
    res.write(tar.subarray(0,512));await new Promise(r=>setTimeout(r,20));res.end(tar.subarray(512))
  })
  await new Promise(r=>server.listen(socket,r))
  try {
    const {copyPeachLedger}=await import('../lib/peach.ts')
    await Promise.all(Array.from({length:5},()=>copyPeachLedger(dest)))
    assert.equal(requests,5)
    assert.equal(await fs.readFile(dest,'utf8'),'complete-ledger-snapshot')
    assert.deepEqual((await fs.readdir(root)).filter(n=>n.startsWith('.ledger-copy-')),[])
    corrupt=true
    await assert.rejects(()=>copyPeachLedger(dest))
    assert.equal(await fs.readFile(dest,'utf8'),'complete-ledger-snapshot')
    assert.deepEqual((await fs.readdir(root)).filter(n=>n.startsWith('.ledger-copy-')),[])
  } finally {
    await new Promise(r=>server.close(r));await fs.rm(root,{recursive:true,force:true})
  }
})
