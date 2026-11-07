import {test} from 'node:test'
import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import os from 'node:os'
import {requireMovePermissions,requireWritableDirectory} from '../lib/file-permissions.ts'

test('move preflight checks mapped directory access even when a destination does not exist',t=>{
 const dir=fs.mkdtempSync(path.join(os.tmpdir(),'face-permissions-fixture-'))
 t.after(()=>fs.rmSync(dir,{recursive:true,force:true}))
 const src=path.join(dir,'source','video.mp4'),target=path.join(dir,'target')
 fs.mkdirSync(path.dirname(src));fs.mkdirSync(target);fs.writeFileSync(src,'fixture')
 const access=fs.accessSync.bind(fs)
 t.mock.method(fs,'accessSync',(p,mode)=>{if(p===target&&(mode&fs.constants.W_OK))throw Object.assign(new Error('NFS mapped user denied'),{code:'EACCES'});return access(p,mode)})
 assert.throws(()=>requireMovePermissions(src,path.join(target,'new','video.mp4')),e=>e.status===403&&e.message.includes('目标目录')&&e.message.includes(target))
 assert.throws(()=>requireWritableDirectory(path.join(target,'empty-subfolder'),'目标目录'),/不可写/)
 assert.equal(fs.readFileSync(src,'utf8'),'fixture')
 assert.deepEqual(fs.readdirSync(target),[])
})
