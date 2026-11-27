import {test} from 'node:test'
import assert from 'node:assert/strict'
import {archiveShortcut,autoArchiveReasonLabel,autoMatchTierLabel,buildAutoMergeApplyItems,canResolveArchiveRow,canStageArchiveRow,canUnarchiveHistoryItem,formatAutoMergeSize,isArchiveResolutionApplied,isSameSizeConflict} from '../lib/archive-actions.ts'
const page={editing:false,modal:false,nativeAction:false}
test('same-size and different-size regular conflicts are eligible for staging, while missing/empty/creators records are not',()=>{
 const duplicate={status:'duplicate',size:123,original_size:123,src:'/media/a.mp4'}
 assert.equal(canResolveArchiveRow(duplicate),true)
 assert.equal(canStageArchiveRow(duplicate),true)
 assert.equal(canStageArchiveRow({...duplicate,status:'conflict',original_size:124}),true)
 assert.equal(isSameSizeConflict(duplicate),true)
 assert.equal(isSameSizeConflict({...duplicate,original_size:124}),false)
 assert.equal(isSameSizeConflict({...duplicate,original_size:undefined}),false)
 assert.equal(isSameSizeConflict({...duplicate,size:0,original_size:0}),false)
 assert.equal(canStageArchiveRow({...duplicate,original_size:0}),false)
 assert.equal(canStageArchiveRow({...duplicate,original_size:undefined}),false)
 assert.equal(canStageArchiveRow({...duplicate,status:'dupe-target',original_size:undefined}),false)
 assert.equal(canStageArchiveRow({...duplicate,size:0,original_size:0}),false)
 assert.equal(canStageArchiveRow({...duplicate,src:null}),false)
 assert.equal(canResolveArchiveRow({...duplicate,status:'organized',resolution:{kind:'rename',name:'old.mp4'}}),false)
 assert.equal(canStageArchiveRow({...duplicate,status:'organized',resolution:{kind:'stage-duplicate'}}),false)
 assert.equal(isArchiveResolutionApplied({...duplicate,status:'ok',resolution:{kind:'stage-duplicate'}},'stage-duplicate'),true)
 assert.equal(isArchiveResolutionApplied({...duplicate,status:'ok',resolution:{kind:'stage-duplicate'}},'rename'),false)
 assert.equal(isArchiveResolutionApplied({...duplicate,status:'conflict',resolution:{kind:'rename',name:'dup.mp4'}},'rename'),false)
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
test('auto-merge apply payload preserves exact preview items and unarchive requires active original_path',()=>{
 const preview={ok:true,version:'v1',eligible_count:2,group_count:1,groups:[{person:'dreamymoo',directory:'/creators/dreamymoo',existing_files:12,count:2,items:[
  {vid:'v1',name:'a.mp4',path:'/src/a.mp4',size:1048576,mtime_ns:'123',target_name:'a.mp4',renamed_keep_both:false,match_tier:'confirmed_rule',watermark_text:'@dreamymoo'},
  {vid:'v2',name:'b.mp4',path:'/src/b.mp4',size:2147483648,mtime_ns:'456',target_name:'b (保留两份-12345678).mp4',renamed_keep_both:true,match_tier:'direct_handle',watermark_text:'onlyfans.com/dreamymoo'},
 ]}]}
 assert.deepEqual(buildAutoMergeApplyItems(preview),[
  {vid:'v1',person:'dreamymoo',path:'/src/a.mp4',size:1048576,mtime_ns:'123'},
  {vid:'v2',person:'dreamymoo',path:'/src/b.mp4',size:2147483648,mtime_ns:'456'},
 ])
 assert.equal(autoMatchTierLabel('confirmed_rule'),'已确认规则')
 assert.equal(autoMatchTierLabel('direct_handle'),'直连账号名')
 assert.equal(autoArchiveReasonLabel('confirmed_rule'),'由已确认水印规则自动合入人物目录并同步 Peach 账本。')
 assert.equal(autoArchiveReasonLabel('direct_handle'),'由直连账号水印自动合入人物目录并同步 Peach 账本。')
 assert.equal(autoArchiveReasonLabel('fallback_queue'),'自动水印转待归档后已完成移动与 Peach 账本更新。')
 assert.equal(autoArchiveReasonLabel(null),null)
 assert.equal(formatAutoMergeSize(1048576),'1.0 MB')
 assert.equal(formatAutoMergeSize(2147483648),'2.00 GB')
 assert.equal(canUnarchiveHistoryItem({original_path:'/src/a.mp4',undone:false}),true)
 assert.equal(canUnarchiveHistoryItem({original_path:'/src/a.mp4',undone:true}),false)
 assert.equal(canUnarchiveHistoryItem({original_path:'/src/a.mp4',undone:false,operation:'stage-duplicate'}),false)
 assert.equal(canUnarchiveHistoryItem({original_path:'/src/a.mp4',undone:false,staged_path:'/stage/a.mp4'}),false)
 assert.equal(canUnarchiveHistoryItem({original_path:'',undone:false}),false)
 assert.equal(canUnarchiveHistoryItem(null),false)
})
