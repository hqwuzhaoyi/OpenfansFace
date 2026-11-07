import {test} from 'node:test'
import assert from 'node:assert/strict'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import {randomUUID} from 'node:crypto'
import {claimFileOperation,fileOperationOwner,selectedPersonFileBackend} from '../lib/file-operation-routing.ts'

function fixture(t){const base=fs.mkdtempSync(path.join(os.tmpdir(),'face-routing-test-'));t.after(()=>fs.rmSync(base,{recursive:true,force:true}));return base}
test('provider selection defaults to legacy and Rust opt-in selects migrated rename and move capabilities',()=>{
 assert.equal(selectedPersonFileBackend('rename-file',{}),'legacy')
 assert.throws(()=>selectedPersonFileBackend('rename-file',{FILE_OPERATIONS_BACKEND:'rust'}),{status:503})
 const enabled={FILE_OPERATIONS_BACKEND:'rust',FILE_OPERATIONS_ENABLE_RUST:'1'}
 assert.equal(selectedPersonFileBackend('rename-file',enabled),'rust')
 assert.equal(selectedPersonFileBackend('move-files',enabled),'rust')
 assert.equal(selectedPersonFileBackend('rename-person',enabled),'rust')
 assert.equal(selectedPersonFileBackend('merge-person',enabled),'legacy')
 assert.throws(()=>selectedPersonFileBackend('rename-file',{FILE_OPERATIONS_BACKEND:'typo'}),{status:503})
})
test('one operation ID has one immutable backend and action',t=>{
 const base=fixture(t),id=randomUUID()
 const claimed=claimFileOperation(base,id,'rename-file','rust')
 assert.deepEqual(fileOperationOwner(base,id),claimed)
 assert.deepEqual(claimFileOperation(base,id,'rename-file','rust'),claimed)
 assert.throws(()=>claimFileOperation(base,id,'rename-file','legacy'),{status:409})
 assert.throws(()=>claimFileOperation(base,id,'delete-files','rust'),{status:409})
 assert.equal(fileOperationOwner(base,id).backend,'rust')
 assert.throws(()=>claimFileOperation(base,'../../elsewhere','rename-file','legacy'),{status:400})
})
test('historical legacy operations cannot be adopted by Rust and malformed records fail closed',t=>{
 const base=fixture(t),id=randomUUID(),dir=path.join(base,'person-operations',id)
 fs.mkdirSync(dir,{recursive:true});fs.writeFileSync(path.join(dir,'operation.json'),JSON.stringify({plan:{action:'rename-file'},status:'complete'}))
 assert.equal(fileOperationOwner(base,id).backend,'legacy')
 assert.throws(()=>claimFileOperation(base,id,'rename-file','rust'),{status:409})
 fs.mkdirSync(path.join(base,'file-operation-routing'));fs.writeFileSync(path.join(base,'file-operation-routing',id+'.json'),'{}')
 assert.throws(()=>fileOperationOwner(base,id),{status:503})
})
