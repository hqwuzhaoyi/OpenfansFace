import {test} from 'node:test'
import assert from 'node:assert/strict'
import {peachPublicBase,peachPersonHref} from '../lib/person-peach-model.ts'

test('Peach links keep the current LAN/Tailnet host and use a stable creator ID',()=>{
 assert.equal(peachPersonHref('1095',null,'http://100.103.138.79:8919'),'http://100.103.138.79:8900/creators/1095')
 assert.equal(peachPersonHref('1095',null,'http://[::1]:8922'),'http://[::1]:8900/creators/1095')
 assert.equal(peachPersonHref('1095','https://peach.example/library','http://localhost:3000'),'https://peach.example/library/creators/1095')
})
test('Peach URLs reject credentials, non-web protocols and invalid identifiers',()=>{
 for(const base of ['javascript:alert(1)','file:///tmp/peach','https://name:secret@peach.example','https://peach.example?token=secret','https://peach.example/#other'])assert.throws(()=>peachPublicBase(base))
 for(const id of ['0','-1','001','../performers/1','1?next=evil','9223372036854775808'])assert.throws(()=>peachPersonHref(id,null,'http://localhost:8922'))
})
