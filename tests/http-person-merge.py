"""Statistics and whole-person merge regressions on a marked fixture at 127.0.0.1:8922."""
import argparse,datetime,hashlib,json,os,sqlite3,subprocess,sys,urllib.request,urllib.error,urllib.parse
from pathlib import Path
parser=argparse.ArgumentParser();parser.add_argument('root',type=Path);parser.add_argument('--seed',action='store_true');args=parser.parse_args()
root=args.root.resolve();assert root.parent==Path('/tmp').resolve()and root.name.startswith('face-admin-person-fileops-merge-')
base=root/'base';media=root/'media';creators=media/'pikpak/creators';ledger=root/'ledger.db'
known='R:\\media\\pikpak\\My Telegram\\archived.mp4';pending='R:\\media\\pikpak\\My Telegram\\incoming.mp4'
nested='R:\\media\\pikpak\\creators\\甲\\相册\\片段.mp4'
def read(name):return json.loads((base/name).read_text())
def write(name,value):
 p=base/name;p.parent.mkdir(parents=True,exist_ok=True);p.write_text(json.dumps(value,ensure_ascii=False))
def win(file):return 'R:\\media\\pikpak\\'+str(file).split('/pikpak/',1)[1].replace('/','\\')
if args.seed:
 subprocess.run([sys.executable,str(Path(__file__).with_name('http-person-file-operations.py')),str(root),'--seed'],check=True)
 sample=(creators/'甲/已归档.mp4').read_bytes()
 for relative,data in [('乙/已归档.mp4',sample+b'target'),('甲/相册/片段.mp4',sample),('乙/相册/片段.mp4',sample+b'target-two'),('甲/.hidden.mp4',sample),('乙/说明.txt',b'target note'),('乙/空子目录',b'keep target file')]:
  file=creators/relative;file.parent.mkdir(parents=True,exist_ok=True);file.write_bytes(data)
 report=read('report.json');report['identities'][0]['aliases']=['A别名'];report['identities'][0]['note']='source report'
 report['identities'][1].update(aliases=['B别名'],vids=[{'vid':pending}],note='target report')
 report['identities'].append({'name':'A曾用名','vids':[{'vid':nested,'name':'片段.mp4'}]});write('report.json',report)
 queue=read('integration-queue.json');queue[0]['creator']='A曾用名';queue[0]['note']='source task';queue.append({**queue[0],'creator':'乙','source_id':'乙','return_href':'/persons/'+urllib.parse.quote('乙'),'note':'target task'});write('integration-queue.json',queue)
 write('person-renames.json',{'A曾用名':'甲','B曾用名':'乙'})
 people=read('people-manual.json');people.append({'name':'乙','created_at':'target-origin','last_used_at':'fixture'});write('people-manual.json',people)
 write('template-overrides.json',{'甲':{'groups':[0]},'乙':{'groups':[2]}})
 rejections=read('person-rejections.json');rejections[json.dumps(['乙','rejected'],ensure_ascii=False,separators=(',',':'))]={'id':'target-rejection','name':'乙','vid':'rejected','path':'unrelated','size':1,'mtime_ns':'0','at':'2026-11-07T00:00:00Z'};write('person-rejections.json',rejections)
 now=datetime.datetime.now(datetime.timezone.utc).isoformat()
 def link(id,creator,pid,display):return {'id':id,'creator':creator,'profile':{'id':pid,'handle':'fixture_'+pid,'name':display,'description':'fixture','followers':0,'synced_at':None},'linked_at':now,'source_fetched_at':now}
 write('external-profiles/manual-links.json',{'version':1,'source':'nv-pu-sa','links':[link('source-common','甲','100','来源快照'),link('source-extra','A曾用名','101','甲'),link('target-common','乙','100','目标快照'),link('target-extra','乙','102','乙')]})
 write('external-profiles/reviewed-links.json',{'version':1,'source':'nv-pu-sa','links':[{'person':'甲','profile_id':'100','handle':'fixture_100','confidence':'candidate','evidence':'来源仍待核对','reviewed_at':now},{'person':'乙','profile_id':'100','handle':'fixture_100','confidence':'reference','evidence':'目标文字资料','reviewed_at':now}]})
 con=sqlite3.connect(ledger);old_links=con.execute('SELECT * FROM asset_entity').fetchall();con.executescript('''DROP TABLE asset_entity; DROP TABLE entity_alias;
 CREATE TABLE asset_entity(id INTEGER PRIMARY KEY,asset_id INTEGER REFERENCES asset(id),entity_id INTEGER REFERENCES entity(id),role TEXT,source TEXT,confidence REAL,metadata_json TEXT,first_seen_at TEXT,last_seen_at TEXT,UNIQUE(asset_id,entity_id,role));
 CREATE TABLE entity_alias(id INTEGER PRIMARY KEY,entity_id INTEGER REFERENCES entity(id),alias TEXT UNIQUE);''')
 con.executemany('INSERT INTO asset_entity VALUES(?,?,?,?,?,?,?,?,?)',old_links)
 con.execute("INSERT INTO asset_entity(asset_id,entity_id,role,source,confidence,metadata_json,first_seen_at,last_seen_at) VALUES(1,2,'creator','target-existing',.75,'{}','fixture','fixture')")
 con.execute("INSERT INTO asset_entity(asset_id,entity_id,role,source,confidence,metadata_json,first_seen_at,last_seen_at) VALUES(1,1,'appearance','source-extra',.8,'{}','fixture','fixture')")
 con.execute("INSERT INTO asset_entity(asset_id,entity_id,role,source,confidence,metadata_json,first_seen_at,last_seen_at) VALUES(1,3,'creator','unrelated',.5,'{}','fixture','fixture')")
 con.executemany('INSERT INTO entity_alias(entity_id,alias) VALUES(?,?)',[(1,'source-alias'),(2,'target-alias')])
 con.execute('UPDATE entity SET metadata_json=? WHERE id=1',(json.dumps({'sourceOnly':'keep source metadata'}),));con.execute('UPDATE entity SET metadata_json=? WHERE id=2',(json.dumps({'targetOnly':'keep target metadata'}),))
 for file in [creators/'乙/已归档.mp4',creators/'甲/相册/片段.mp4',creators/'乙/相册/片段.mp4']:
  aid=con.execute('INSERT INTO asset(path)VALUES(?)',(win(file),)).lastrowid;eid=1 if '/甲/'in str(file)else 2
  con.execute("INSERT INTO asset_entity(asset_id,entity_id,role,source,confidence,metadata_json,first_seen_at,last_seen_at) VALUES(?,?,'creator','fixture',1,'{}','fixture','fixture')",(aid,eid))
 con.commit();con.close()
 con=sqlite3.connect(base/'processing/processing.db');data={'ready':True,'kind':'conflict','suggested':None,'candidates':[{'name':'甲','sources':['face'],'score':.6,'evidence':[{'name':'甲','source':'face','detail':'face hint'}]},{'name':'乙','sources':['watermark'],'score':None,'evidence':[{'name':'乙','source':'watermark','detail':'text hint'}]}]}
 con.execute("UPDATE tasks SET data=?,stage='review' WHERE vid=?",(json.dumps(data,ensure_ascii=False),pending));con.commit();con.close()
 print('merge fixture ready');raise SystemExit
assert(root/'.fileops-fixture').is_file();url='http://127.0.0.1:8922'
def call(endpoint,body=None,status=200):
 try:
  with urllib.request.urlopen(urllib.request.Request(url+endpoint,data=json.dumps(body).encode()if body is not None else None,headers={'Content-Type':'application/json'}),timeout=50)as r:assert r.status==status;return json.load(r)
 except urllib.error.HTTPError as e:
  result=json.load(e);assert e.code==status,(e.code,result);return result
assert call('/api/persons/library?name='+urllib.parse.quote('甲'))['root']==str(creators/'甲')
def stats(refresh=False):return call('/api/persons/library?stats=1'+('&refresh=1'if refresh else''))['stats']
def preview():return call('/api/persons/manage',{'mode':'preview','request':{'action':'merge-person','person':'甲','target_person':'乙'}})
def apply(plan,status=200):return call('/api/persons/manage',{'mode':'apply','request':plan['request'],'version':plan['version']},status)
def sql(command,args=()):
 with sqlite3.connect(ledger)as con:return con.execute(command,args).fetchall()
def tree():return {str(file.relative_to(creators)):hashlib.sha256(file.read_bytes()).hexdigest()for file in creators.rglob('*')if file.is_file()and not file.is_symlink()}
def metadata():return {str(file.relative_to(base)):file.read_bytes()for file in base.rglob('*.json')if 'person-operations'not in file.relative_to(base).parts}
counts=stats();assert counts['甲']['video_count']==4 and counts['乙']['video_count']==3
size_before=counts['甲']['total_bytes']+counts['乙']['total_bytes']
# Hidden videos, non-media attachments, and symlinks do not inflate the list statistics.
link=creators/'甲/外部链接';link.symlink_to(creators/'乙',target_is_directory=True)
assert stats(True)['甲']['video_count']==4
call('/api/persons/manage',{'mode':'preview','request':{'action':'merge-person','person':'甲','target_person':'乙'}},409);link.unlink()
print('PASS real nested-video counts and sizes, excluding hidden files, attachments and symlinks')
plan=preview();assert plan['merge_summary']['renamed_files']>=3 and plan['merge_summary']['keep_target_template']and plan['merge_summary']['manual_accounts']==2
race=next(Path(file['dst'])for file in plan['files']if file.get('renamed')and Path(file['dst']).parent==creators/'乙');race.write_bytes(b'race');apply(plan,409);assert race.read_bytes()==b'race';race.unlink()
original_tree,original_metadata=tree(),metadata()
# SQLite rejects this merge late in the ledger transaction, after links and aliases have been migrated.
sql("CREATE TRIGGER fail_merge BEFORE DELETE ON entity WHEN old.id=1 BEGIN SELECT RAISE(ABORT,'injected merge failure'); END")
apply(preview(),500);assert tree()==original_tree and metadata()==original_metadata and not(base/'.review-write.lock').exists();sql('DROP TRIGGER fail_merge')
# A deferred local FK failure forces compensation AFTER the remote merge has committed.
with sqlite3.connect(base/'faces.db')as con:
 con.executescript("CREATE TABLE merge_guard(id TEXT PRIMARY KEY);CREATE TABLE merge_deferred(value TEXT REFERENCES merge_guard(id) DEFERRABLE INITIALLY DEFERRED);CREATE TRIGGER fail_local_commit AFTER UPDATE OF path ON videos BEGIN INSERT INTO merge_deferred VALUES('missing'); END;")
apply(preview(),500)
assert tree()==original_tree and metadata()==original_metadata and sql('SELECT canonical_name FROM entity WHERE id=1')==[('甲',)]
assert sql('SELECT alias FROM entity_alias WHERE entity_id=1')==[('source-alias',)] and sql("SELECT source FROM asset_entity WHERE asset_id=1 AND entity_id=2 AND role='creator'")==[('target-existing',)]
assert not(base/'.review-write.lock').exists()
with sqlite3.connect(base/'faces.db')as con:con.executescript('DROP TRIGGER fail_local_commit;DROP TABLE merge_deferred;DROP TABLE merge_guard;')
print('PASS stale-target protection and rollback before AND after ledger commit, including aliases and source entity restoration')
plan=preview();result=apply(plan);assert apply(plan)['id']==result['id']
assert result['person']=='乙'and not(creators/'甲').exists()
for move in plan['files']:assert Path(move['dst']).is_file()and hashlib.sha256(Path(move['dst']).read_bytes()).hexdigest()==original_tree[str(Path(move['src']).relative_to(creators))]
for name,digest in original_tree.items():
 if name.startswith('乙/'):assert hashlib.sha256((creators/name).read_bytes()).hexdigest()==digest
empty=next(row for row in plan['directory_targets']if row['src'].endswith('/空子目录'));assert Path(empty['dst']).is_dir()
after=stats();assert '甲'not in after and after['乙']['video_count']==7 and after['乙']['total_bytes']==size_before
people=call('/api/persons/library')['persons'];assert '甲'not in[p['name']for p in people]and '甲'in next(p for p in people if p['name']=='乙')['aliases']
assert call('/api/persons/library?name='+urllib.parse.quote('A曾用名'))['name']=='乙'
report=read('report.json');identities=[row for row in report['identities']if row['name']=='乙'];assert len(identities)==1 and identities[0]['note']=='target report'
assert len({row['vid']for row in identities[0]['vids']})==len(identities[0]['vids'])
queue=read('integration-queue.json');assert len(queue)==1 and queue[0]['creator']=='乙'and queue[0]['note']=='target task'
assert read('watermarks/mappings.json')['account']['creator']=='乙'
assert read('template-overrides.json')=={'乙':{'groups':[2]}}
manual=read('external-profiles/manual-links.json')['links'];assert len(manual)==3 and all(row['creator']=='乙'for row in manual)
assert next(row for row in manual if row['profile']['id']=='100')['id']=='target-common'
assert next(row for row in manual if row['profile']['id']=='101')['profile']['name']=='甲'
reviewed=read('external-profiles/reviewed-links.json')['links'];assert len(reviewed)==1 and reviewed[0]['person']=='乙'and reviewed[0]['confidence']=='candidate'
assert len(read('person-rejections.json'))==1 and next(iter(read('person-rejections.json').values()))['id']=='target-rejection'
assert sql('SELECT id FROM entity WHERE id=1')==[] and sql('SELECT alias FROM entity_alias WHERE entity_id=2 ORDER BY alias')==[('source-alias',),('target-alias',)]
assert sql('SELECT entity_id FROM asset_entity WHERE asset_id=5')==[(2,)]
assert sql("SELECT source,confidence FROM asset_entity WHERE asset_id=1 AND entity_id=2 AND role='creator'")==[('target-existing',.75)]
assert sql("SELECT source FROM asset_entity WHERE asset_id=1 AND entity_id=3")==[('unrelated',)]
meta=json.loads(sql('SELECT metadata_json FROM entity WHERE id=2')[0][0]);assert meta['targetOnly']=='keep target metadata'and json.loads(meta['face_admin_merged_people'][0]['entity']['metadata_json'])['sourceOnly']=='keep source metadata'
with sqlite3.connect(base/'processing/processing.db')as con:
 data=json.loads(con.execute('SELECT data FROM tasks WHERE vid=?',(pending,)).fetchone()[0]);assert data['kind']=='suggested'and data['suggested']=='乙'and len(data['candidates'])==1 and len(data['candidates'][0]['evidence'])==2
 assert con.execute("SELECT stage,updated_at FROM tasks WHERE vid='unrelated'").fetchone()==('face',0)
with urllib.request.urlopen(url+'/api/video?p='+urllib.parse.quote(known))as response:assert hashlib.sha256(response.read()).hexdigest()==original_tree['甲/已归档.mp4']
print('PASS collision-preserving merge, fresh statistics, alias search, stable playback, queues/accounts/templates/rejections and complete Peach relationship migration')
