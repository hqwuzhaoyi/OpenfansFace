import {test} from 'node:test'
import assert from 'node:assert/strict'
import {archiveShortcut,canResolveArchiveRow,canStageArchiveRow} from '../lib/archive-actions.ts'
const page={editing:false,modal:false,nativeAction:false}
test('same-size duplicate is eligible for staging, but creators records never get conflict actions',()=>{
 const duplicate={status:'duplicate',size:123,original_size:123,src:'/media/a.mp4'}
 assert.equal(canResolveArchiveRow(duplicate),true)
 assert.equal(canStageArchiveRow(duplicate),true)
 assert.equal(canStageArchiveRow({...duplicate,original_size:124}),false)
 assert.equal(canStageArchiveRow({...duplicate,size:0,original_size:0}),false)
 assert.equal(canStageArchiveRow({...duplicate,src:null}),false)
 assert.equal(canResolveArchiveRow({...duplicate,status:'organized',resolution:{kind:'rename',name:'old.mp4'}}),false)
 assert.equal(canStageArchiveRow({...duplicate,status:'organized',resolution:{kind:'stage-duplicate'}}),false)
})
test('A selects the visible list and Enter requests confirmation, including checkbox focus',()=>{
 assert.equal(archiveShortcut({key:'a'},page),'all')
 assert.equal(archiveShortcut({key:'A',shiftKey:true},page),'all')
 assert.equal(archiveShortcut({key:'Enter'},page),'confirm')
 assert.equal(archiveShortcut({key:'x'},page),'toggle')
})
test('typing, media controls, IME composition, dialogs, key repeats and system shortcuts do not act',()=>{
 for(const key of ['a','Enter','j','x']){
  for(const context of [{...page,editing:true},{...page,modal:true}]) assert.equal(archiveShortcut({key},context),null)
  for(const flag of ['repeat','isComposing','ctrlKey','metaKey','altKey']) assert.equal(archiveShortcut({key,[flag]:true},page),null)
 }
})
test('Enter respects native action buttons; pressing A there still selects the list',()=>{
 assert.equal(archiveShortcut({key:'Enter'},{...page,nativeAction:true}),null)
 assert.equal(archiveShortcut({key:'a'},{...page,nativeAction:true}),'all')
 assert.equal(archiveShortcut({key:'Enter',shiftKey:true},page),null)
})
