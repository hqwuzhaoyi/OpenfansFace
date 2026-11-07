import {test} from 'node:test'
import assert from 'node:assert/strict'
import http from 'node:http'
import {nativeJsonRequest} from '../lib/native-json-transport.ts'

test('native HTTP transport preserves auth, GET/POST and structured failures without treating acceptance as completion',async t=>{
 const previous={url:process.env.FILE_OPERATIONS_RUST_URL,token:process.env.FILE_OPERATIONS_RUST_TOKEN}
 const requests=[]
 let status=200,value={ok:true}
 const server=http.createServer(async(req,res)=>{
  let body='';for await(const chunk of req)body+=chunk
  requests.push({url:req.url,method:req.method,auth:req.headers.authorization,body})
  res.writeHead(status,{'Content-Type':'application/json'});res.end(typeof value==='string'?value:JSON.stringify(value))
 })
 await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve))
 t.after(async()=>{await new Promise(resolve=>server.close(resolve));for(const [key,value]of Object.entries({FILE_OPERATIONS_RUST_URL:previous.url,FILE_OPERATIONS_RUST_TOKEN:previous.token}))if(value===undefined)delete process.env[key];else process.env[key]=value})
 process.env.FILE_OPERATIONS_RUST_URL=`http://127.0.0.1:${server.address().port}/`
 process.env.FILE_OPERATIONS_RUST_TOKEN='isolated-transport-test'
 for(const label of ['归档','文件操作']){
  const policy={label,completion:'required',error:(message,status)=>Object.assign(new Error(message),{status})}
  status=200;value={ok:true}
  assert.deepEqual(await nativeJsonRequest('v1/fixture',{command:'confirm'},policy),value)
  assert.deepEqual(requests.at(-1),{url:'/v1/fixture',method:'POST',auth:'Bearer isolated-transport-test',body:'{"command":"confirm"}'})
  await nativeJsonRequest('v1/fixture',null,policy,'origin-id')
  assert.equal(requests.at(-1).url,'/v1/fixture?id=origin-id');assert.equal(requests.at(-1).method,'GET');assert.equal(requests.at(-1).body,'')
  status=409;value={error:'changed',code:'stale',step:'fingerprint',path:'/fixture/video.mp4'}
  await assert.rejects(nativeJsonRequest('v1/fixture',{},policy),{status:409,code:'stale',step:'fingerprint',path:'/fixture/video.mp4',message:'changed · fingerprint · /fixture/video.mp4'})
  status=202;value={accepted:true}
  await assert.rejects(nativeJsonRequest('v1/fixture',{},policy),{status:503,code:'operation_pending'})
  status=200
  for(value of ['invalid JSON',null,[],false])await assert.rejects(nativeJsonRequest('v1/fixture',{},policy),{status:502})
 }
})
