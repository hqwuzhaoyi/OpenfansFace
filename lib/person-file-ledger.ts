import { execFile } from 'node:child_process'
import { promisify } from 'node:util'
import { peachExec } from './peach'
import type { PersonFilePlan } from './person-file-model'
import { toWin } from './paths'

const exec=promisify(execFile)
// A ledger-side receipt makes a lost Docker response distinguishable from a failed transaction.
const program=String.raw`import base64,json,sqlite3,hashlib,datetime,urllib.parse,sys
p=json.loads(base64.b64decode(''.join(sys.argv[1:])))
con=sqlite3.connect('file:'+urllib.parse.quote(p['database'])+'?mode=rw',uri=True,timeout=30);con.row_factory=sqlite3.Row
con.execute('PRAGMA foreign_keys=ON')
merge=p['kind']=='merge-person'
def rows(sql,args=()):return [dict(r) for r in con.execute(sql,args)]
def binary(value):
 if isinstance(value,bytes):return {'__face_bytes__':base64.b64encode(value).decode()}
 raise TypeError(type(value).__name__)
def decode(value):return base64.b64decode(value['__face_bytes__']) if set(value)=={'__face_bytes__'} else value
def dumps(value):return json.dumps(value,sort_keys=True,ensure_ascii=False,separators=(',',':'),default=binary)
def digest(value):return hashlib.sha256(dumps(value).encode()).hexdigest()
def qi(value):return '"'+value.replace('"','""')+'"'
def insert(table,row,keep_id=False):
 data={k:v for k,v in row.items() if keep_id or k!='id'};keys=list(data)
 con.execute('INSERT INTO '+qi(table)+'('+','.join(qi(k) for k in keys)+') VALUES('+','.join('?' for _ in keys)+')',[data[k] for k in keys])
def update_entity(eid,fields):con.execute('UPDATE entity SET '+','.join(qi(k)+'=?' for k in fields)+' WHERE id=?',[*fields.values(),eid])
def links_for(ids,all_roles):
 result=[]
 for aid in sorted(set(ids)):result.extend(rows('SELECT * FROM asset_entity WHERE asset_id=?'+('' if all_roles else " AND role='creator'"),(aid,)))
 return sorted(result,key=dumps)
def aliases_for(ids):
 result=[]
 for eid in sorted(set(ids)):result.extend(rows('SELECT * FROM entity_alias WHERE entity_id=?',(eid,)))
 return sorted(result,key=dumps)
def mapped(value):
 for item in p['moves']:
  if value==item['old']:return item['new']
 for item in sorted(p.get('directory_rules',[]),key=lambda item:len(item['old']),reverse=True):
  if value.startswith(item['old']):return item['new']+value[len(item['old']):]
 return p['new_prefix']+value[len(p['old_prefix']):]
def snapshot():
 changes=[]
 if p['kind'] in ('rename-person','merge-person'):
  old=p['old_prefix']
  for r in rows('SELECT id,path FROM asset WHERE substr(path,1,?)=?',(len(old),old)):changes.append(dict(id=r['id'],old=r['path'],new=mapped(r['path'])))
 else:
  for item in p['moves']:
   found=rows('SELECT id,path FROM asset WHERE path=? AND disposal IS NULL',(item['old'],))
   if len(found)>1:raise ValueError('Peach 原路径有多个有效记录，请先核对')
   changes.extend(dict(id=r['id'],old=r['path'],new=item['new']) for r in found)
 targets=set()
 for r in changes:
  if r['new'] in targets or any(v['id']!=r['id'] for v in rows('SELECT id FROM asset WHERE path=?',(r['new'],))):raise ValueError('Peach 目标路径已存在记录：'+r['new'])
  targets.add(r['new'])
 select='*' if merge else 'id,canonical_name,normalized_name'
 old=rows("SELECT "+select+" FROM entity WHERE kind='creator' AND canonical_name=?",(p['person'],))
 target=rows("SELECT "+select+" FROM entity WHERE kind='creator' AND (canonical_name=? OR normalized_name=?)",(p['target'],p['target'].lower()))
 if len(old)>1 or len(target)>1:raise ValueError('Peach 人物记录不唯一')
 if p['kind']=='rename-person' and target:raise ValueError('Peach 已有同名人物，不能直接覆盖或合并')
 ids={r['id'] for r in changes};aliases=[]
 if merge:
  if old and target and old[0]['id']==target[0]['id']:raise ValueError('Peach 中这两个名字已指向同一人物，请先核对目录')
  if old:
   eid=old[0]['id'];ids.update(r['asset_id'] for r in rows('SELECT asset_id FROM asset_entity WHERE entity_id=?',(eid,)))
   for table in rows("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'"):
    if table['name'] in ('asset_entity','entity_alias'):continue
    for fk in rows('PRAGMA foreign_key_list('+qi(table['name'])+')'):
     if fk['table']=='entity' and fk['to'] in ('id',None) and con.execute('SELECT 1 FROM '+qi(table['name'])+' WHERE '+qi(fk['from'])+'=? LIMIT 1',(eid,)).fetchone():raise ValueError('Peach 还有需核对的人物关联：'+table['name'])
  aliases=aliases_for([r['id'] for r in old+target])
  for entity in old+target:
   metadata=json.loads(entity.get('metadata_json') or '{}')
   if not isinstance(metadata,dict) or not isinstance(metadata.get('face_admin_merged_people',[]),list):raise ValueError('Peach 人物附加资料格式异常')
 return dict(changes=sorted(changes,key=lambda r:r['id']),old_entity=old,target_entity=target,link_asset_ids=sorted(ids),links=links_for(ids,merge),aliases=aliases,all_roles=merge)
try:
 action=p['action']
 if action=='preview':
  s=snapshot();print(json.dumps(dict(version=digest(s),registered=len(s['changes']))))
 elif action=='status':
  con.execute('BEGIN IMMEDIATE')
  exists=con.execute("SELECT 1 FROM sqlite_master WHERE type='table' AND name='_face_admin_person_ops'").fetchone()
  r=con.execute('SELECT state,signature FROM _face_admin_person_ops WHERE id=?',(p['id'],)).fetchone() if exists else None
  if r and r['signature']!=p['signature']:raise ValueError('操作记录不匹配')
  state=r['state'] if r else 'absent'
  if state=='prepared':con.execute("UPDATE _face_admin_person_ops SET state='aborted' WHERE id=?",(p['id'],));state='aborted'
  con.commit();print(json.dumps(dict(state=state)))
 else:
  con.execute('BEGIN IMMEDIATE')
  con.execute('CREATE TABLE IF NOT EXISTS _face_admin_person_ops (id TEXT PRIMARY KEY, signature TEXT NOT NULL, state TEXT NOT NULL, data TEXT NOT NULL)')
  previous=con.execute('SELECT * FROM _face_admin_person_ops WHERE id=?',(p['id'],)).fetchone()
  if action=='reserve':
   if previous:raise ValueError('操作编号已被使用')
   if digest(snapshot())!=p['version']:raise ValueError('Peach 记录已变化，请刷新操作预览')
   con.execute('INSERT INTO _face_admin_person_ops VALUES(?,?,?,?)',(p['id'],p['signature'],'prepared','{}'))
   con.commit();print(json.dumps(dict(state='prepared')))
  elif action=='apply':
   if not previous or previous['signature']!=p['signature'] or previous['state'] not in ('prepared','committed'):raise ValueError('操作未准备或已经中止')
   if previous['state']=='committed':con.commit();print(json.dumps(dict(state='committed',id=p['id'])))
   else:
    s=snapshot()
    if digest(s)!=p['version']:raise ValueError('Peach 记录已变化，请刷新操作预览')
    receipt=dict(snapshot=s,created=[],entity_changes=[],deleted_entities=[])
    now=datetime.datetime.now(datetime.timezone.utc).isoformat()
    if p['kind']=='rename-person' and s['old_entity']:
     entity=s['old_entity'][0];fields=dict(canonical_name=p['target'],normalized_name=p['target'].lower())
     receipt['entity_changes'].append(dict(id=entity['id'],old={key:entity[key] for key in fields},new=fields));update_entity(entity['id'],fields)
    target=s['target_entity'][0]['id'] if s['target_entity'] else None
    if (p['kind']=='move-files' and s['changes'] or merge and (s['changes'] or s['old_entity'])) and target is None:
     target=con.execute("INSERT INTO entity(kind,canonical_name,normalized_name,metadata_json,created_at,updated_at) VALUES('creator',?,?,?,?,?)",(p['target'],p['target'].lower(),'{}',now,now)).lastrowid;receipt['created'].append(target)
    for r in s['changes']:con.execute('UPDATE asset SET path=? WHERE id=?',(r['new'],r['id']))
    if merge and s['old_entity']:
     old=s['old_entity'][0];eid=old['id']
     con.execute('DELETE FROM asset_entity WHERE entity_id=?',(eid,))
     for link in [row for row in s['links'] if row['entity_id']==eid]:
      if not con.execute('SELECT 1 FROM asset_entity WHERE asset_id=? AND entity_id=? AND role=?',(link['asset_id'],target,link['role'])).fetchone():insert('asset_entity',{**link,'entity_id':target})
     con.execute('DELETE FROM entity_alias WHERE entity_id=?',(eid,))
     for alias in [row for row in s['aliases'] if row['entity_id']==eid]:
      if not con.execute('SELECT 1 FROM entity_alias WHERE entity_id=? AND alias=?',(target,alias['alias'])).fetchone():insert('entity_alias',{**alias,'entity_id':target})
     target_row=rows('SELECT metadata_json,updated_at FROM entity WHERE id=?',(target,))[0];metadata=json.loads(target_row['metadata_json'] or '{}')
     metadata.setdefault('face_admin_merged_people',[]).append(dict(operation=p['id'],entity=old,aliases=[row for row in s['aliases'] if row['entity_id']==eid]))
     fields=dict(metadata_json=dumps(metadata),updated_at=now)
     receipt['entity_changes'].append(dict(id=target,old={key:target_row[key] for key in fields},new=fields));update_entity(target,fields)
     con.execute('DELETE FROM asset_entity WHERE entity_id=?',(eid,));con.execute('DELETE FROM entity_alias WHERE entity_id=?',(eid,));con.execute('DELETE FROM entity WHERE id=?',(eid,))
     receipt['deleted_entities'].append(old)
    if p['kind']=='move-files' and p['person']!=p['target'] or merge:
     for r in s['changes']:
      if not merge and s['old_entity']:con.execute("DELETE FROM asset_entity WHERE asset_id=? AND entity_id=? AND role='creator'",(r['id'],s['old_entity'][0]['id']))
      if not con.execute("SELECT 1 FROM asset_entity WHERE asset_id=? AND entity_id=? AND role='creator'",(r['id'],target)).fetchone():con.execute("INSERT INTO asset_entity(asset_id,entity_id,role,source,confidence,metadata_json,first_seen_at,last_seen_at) VALUES(?,?,'creator','face-persons',1,'{}',?,?)",(r['id'],target,now,now))
    receipt['after_links']=links_for(s['link_asset_ids'],merge)
    if merge:receipt['alias_entities']=sorted(set([r['id'] for r in s['old_entity']+s['target_entity']]+receipt['created']));receipt['after_aliases']=aliases_for(receipt['alias_entities'])
    con.execute("UPDATE _face_admin_person_ops SET state='committed',data=? WHERE id=?",(dumps(receipt),p['id']))
    con.commit();print(json.dumps(dict(state='committed',id=p['id'],registered=len(s['changes']))))
  elif action=='rollback':
   if not previous:con.commit();print(json.dumps(dict(state='absent')))
   elif previous['state']=='rolled_back':con.commit();print(json.dumps(dict(state='rolled_back')))
   else:
    if previous['signature']!=p['signature']:raise ValueError('操作记录不匹配')
    receipt=json.loads(previous['data'],object_hook=decode);s=receipt['snapshot']
    for r in s['changes']:
     actual=con.execute('SELECT path FROM asset WHERE id=?',(r['id'],)).fetchone()
     if not actual or actual['path']!=r['new']:raise ValueError('Peach 路径已被其他操作修改，停止自动回退')
    if links_for(s['link_asset_ids'],s['all_roles'])!=receipt['after_links']:raise ValueError('Peach 人物关联已变化，停止自动回退')
    if s['all_roles'] and aliases_for(receipt['alias_entities'])!=receipt['after_aliases']:raise ValueError('Peach 别名已变化，停止自动回退')
    for entity in receipt['deleted_entities']:
     if con.execute('SELECT 1 FROM entity WHERE id=?',(entity['id'],)).fetchone():raise ValueError('原人物编号已被重新使用')
     insert('entity',entity,True)
    for entity in receipt['entity_changes']:
     fields=entity['new'];current=con.execute('SELECT '+','.join(qi(key) for key in fields)+' FROM entity WHERE id=?',(entity['id'],)).fetchone()
     if not current or dict(current)!=fields:raise ValueError('Peach 人物资料已被其他操作修改')
     update_entity(entity['id'],entity['old'])
    for r in s['changes']:con.execute('UPDATE asset SET path=? WHERE id=?',(r['old'],r['id']))
    for aid in s['link_asset_ids']:con.execute('DELETE FROM asset_entity WHERE asset_id=?'+('' if s['all_roles'] else " AND role='creator'"),(aid,))
    for link in s['links']:insert('asset_entity',link,True)
    if s['all_roles']:
     for eid in receipt['alias_entities']:con.execute('DELETE FROM entity_alias WHERE entity_id=?',(eid,))
     for alias in s['aliases']:insert('entity_alias',alias,True)
    for eid in receipt['created']:
     if con.execute('SELECT 1 FROM asset_entity WHERE entity_id=?',(eid,)).fetchone():raise ValueError('新人物已被其他记录引用')
     con.execute('DELETE FROM entity WHERE id=?',(eid,))
    con.execute("UPDATE _face_admin_person_ops SET state='rolled_back' WHERE id=?",(p['id'],));con.commit();print(json.dumps(dict(state='rolled_back')))
except Exception:
 con.rollback();raise
finally:con.close()
`

export async function personLedger(action:'preview'|'reserve'|'apply'|'status'|'rollback',plan:Pick<PersonFilePlan,'id'|'action'|'person'|'target_person'|'files'|'source_directory'|'target_directory'>&Partial<PersonFilePlan>) {
  const local=process.env.PEACH_LEDGER_PATH
  const directoryRules=(plan.directory_targets||[]).map(directory=>({old:toWin(directory.src)+'\\',new:toWin(directory.dst)+'\\'}))
  const defaultPath=(file:string)=>{
    const rule=[...directoryRules].sort((a,b)=>b.old.length-a.old.length).find(rule=>file.startsWith(rule.old))
    return rule?rule.new+file.slice(rule.old.length):file
  }
  const payload={action,id:plan.id,kind:plan.action,person:plan.person,target:plan.target_person,signature:plan.version,version:plan.ledger_version,
    old_prefix:plan.source_directory?toWin(plan.source_directory)+'\\':undefined,new_prefix:plan.target_directory?toWin(plan.target_directory)+'\\':undefined,
    directory_rules:directoryRules,
    moves:plan.action==='rename-person'?[]:plan.files.map(file=>({old:toWin(file.src),new:toWin(file.dst)})).filter(file=>plan.action!=='merge-person'||defaultPath(file.old)!==file.new),database:local||'/data/database/ledger.db'}
  const encoded=Buffer.from(JSON.stringify(payload)).toString('base64')
  if(encoded.length>(local&&process.platform==='darwin'?180_000:1_400_000))throw new Error('本次路径信息超过系统传输限制，请减少所选视频数量')
  // Keep each argv entry below Linux's single-argument limit for large batches.
  const args=encoded.match(/.{1,24000}/g)||[]
  let output:string
  if(local){try{const result=await exec(process.env.PYTHON_BIN||'python3',['-c',program,...args],{timeout:45_000,maxBuffer:2**20});output=result.stdout}catch(error:any){throw new Error(error.stderr?.trim().split('\n').slice(-1)[0]||'Peach 账本连接或执行失败')}}
  else{
    let timer:ReturnType<typeof setTimeout>|undefined
    try{const result=await Promise.race([peachExec(program,45_000,args),new Promise<never>((_,reject)=>{timer=setTimeout(()=>reject(new Error('Peach 账本请求超时')),45_000)})]);if(result.code!==0)throw new Error(result.err||'Peach 账本操作失败');output=result.out}
    finally{if(timer)clearTimeout(timer)}
  }
  return JSON.parse(output) as {version?:string;state?:string;registered?:number}
}
