import{test}from'node:test'
import assert from'node:assert/strict'
import fs from'node:fs'
import os from'node:os'
import path from'node:path'
import{fileStream}from'../lib/file-stream.ts'
const dir=fs.mkdtempSync(path.join(os.tmpdir(),'face-file-stream-')),file=path.join(dir,'video.bin'),content=Buffer.alloc(1024*1024,73)
fs.writeFileSync(file,content)
test('serves an exact byte range',async()=>{const r=new Response(fileStream(file,{start:10,end:109}));assert.deepEqual(Buffer.from(await r.arrayBuffer()),content.subarray(10,110))})
test('repeated cancellation stops paused and active file streams without late controller writes',async()=>{
 for(let i=0;i<40;i++){const reader=fileStream(file,{start:0,end:content.length-1}).getReader();await reader.read();await reader.cancel();assert.equal((await reader.read()).done,true)}
 await new Promise(r=>setTimeout(r,30))
})
test('request abort finishes the stream; abort after EOF is harmless',async()=>{
 const c=new AbortController(),reader=fileStream(file,{start:0,end:content.length-1,signal:c.signal}).getReader();await reader.read();c.abort();let n=0;while(!(await reader.read()).done)assert.ok(++n<4)
 const d=new AbortController();await new Response(fileStream(file,{start:0,end:1,signal:d.signal})).arrayBuffer();d.abort()
})
process.on('exit',()=>fs.rmSync(dir,{recursive:true,force:true}))
