import{test}from'node:test'
import assert from'node:assert/strict'
import{api,NetworkRequestError}from'../lib/face-api.ts'
test('retries a transient failed GET once',async t=>{let n=0;t.mock.method(globalThis,'fetch',async()=>{if(++n===1)throw new TypeError('Failed to fetch');return Response.json({ok:true})});assert.deepEqual(await api('/read'),{ok:true});assert.equal(n,2)})
test('never retries writes and reports an unknown outcome instead of definite failure',async t=>{let n=0;t.mock.method(globalThis,'fetch',async()=>{n++;throw new TypeError('Failed to fetch')});await assert.rejects(api('/write',{method:'POST'}),e=>e instanceof NetworkRequestError&&e.resultUnknown&&e.message.includes('勿重复提交'));assert.equal(n,1)})
test('HTTP errors do not trigger network retries',async t=>{let n=0;t.mock.method(globalThis,'fetch',async()=>{n++;return Response.json({error:'人物名无效'},{status:400})});await assert.rejects(api('/read'),/人物名无效/);assert.equal(n,1)})
