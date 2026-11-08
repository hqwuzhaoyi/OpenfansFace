"""Batch selection regression. Requires a fresh http-person-merge.py fixture on port 8923."""
import argparse,json,sqlite3,urllib.request,urllib.error,urllib.parse,hashlib,time,concurrent.futures
from pathlib import Path
parser=argparse.ArgumentParser();parser.add_argument('root',type=Path);parser.add_argument('--port',type=int,default=8923);args=parser.parse_args()
root=args.root.resolve();assert root.parent==Path('/tmp').resolve() and root.name.startswith('face-admin-person-fileops-merge-batch-') and (root/'.fileops-fixture').exists()
base=root/'base';creators=root/'media/pikpak/creators';url=f'http://127.0.0.1:{args.port}'
def call(endpoint,body=None,status=200):
 try:
  with urllib.request.urlopen(urllib.request.Request(url+endpoint,data=json.dumps(body).encode() if body is not None else None,headers={'Content-Type':'application/json'}),timeout=90) as r:
   assert r.status==status,(r.status,status);return json.load(r)
 except urllib.error.HTTPError as e:
  result=json.load(e);assert e.code==status,(e.code,result);return result
def preview(action,people,target=None,status=200):return call('/api/persons/batch',{'mode':'preview','request':dict(action=action,people=people,**({'target':target} if target else {}))},status)
def apply(plan,status=200):return call('/api/persons/batch',dict(mode='apply',id=plan['id'],version=plan['version']),status)
def names():return {p['name'] for p in call('/api/persons/library')['persons']}
def sql(command):
 with sqlite3.connect(root/'ledger.db') as con:return con.execute(command).fetchall()
def read(name):return json.loads((base/name).read_text())
def digest_tree():return {str(p.relative_to(creators)):hashlib.sha256(p.read_bytes()).hexdigest() for p in creators.rglob('*') if p.is_file()}
# Include source-to-source same-name collisions, empty folder, and metadata alias.
(creators/'丙').mkdir();(creators/'丙/已归档.mp4').write_bytes(b'third copy');(creators/'丙/空目录').mkdir()
preview('merge',['甲'], '甲',400);preview('merge',['甲','乙'],'空人物',400);preview('merge',['甲','A曾用名'],'甲',400)
plan=preview('merge',['甲','丙','乙'],'乙')
(creators/'甲/晚到.mp4').write_bytes(b'new');apply(plan,409);assert '甲' in names();(creators/'甲/晚到.mp4').unlink()
print('PASS selection validation and stale whole-batch rejection')
# Failure on the second source must keep the first result and stop exactly there.
plan=preview('merge',['丙','甲','乙'],'乙')
sql("CREATE TRIGGER fail_batch_merge BEFORE DELETE ON entity WHEN old.id=1 BEGIN SELECT RAISE(ABORT,'batch fixture failure'); END")
status=call('/api/persons/batch?id='+plan['id']);assert status['status']=='preview' and not status['completed']
observed=[]
with concurrent.futures.ThreadPoolExecutor() as pool:
 future=pool.submit(apply,plan)
 while not future.done():
  observed.append(call('/api/persons/batch?id='+plan['id']));time.sleep(.03)
 result=future.result()
final_status=call('/api/persons/batch?id='+plan['id'])
assert final_status['status']=='complete' and final_status['completed']==['丙'] and final_status['remaining']==['甲']
assert any(row['status']=='running' and row.get('active_person') in ['甲','丙'] for row in observed),observed
assert [len(row['completed']) for row in observed]==sorted(len(row['completed']) for row in observed)
assert not result['ok'] and [p['person'] for p in result['completed']]==['丙'] and result['remaining']==['甲'],result
assert not (creators/'丙').exists() and (creators/'甲').exists();assert apply(plan)==result
sql('DROP TRIGGER fail_batch_merge')
result=apply(preview('merge',['甲','乙'],'乙'));assert result['ok'];assert '甲' not in names() and '丙' not in names()
assert call('/api/persons/library?name='+urllib.parse.quote('乙'))['files'].__len__()==8
# Integer PK plus a separate role uniqueness constraint: retained target wins, other roles survive.
assert sql("SELECT source,confidence FROM asset_entity WHERE asset_id=1 AND entity_id=2 AND role='creator'")==[('target-existing',.75)]
assert sql("SELECT source FROM asset_entity WHERE asset_id=1 AND entity_id=2 AND role='appearance'")==[('source-extra',)]
assert 'A曾用名' in next(p for p in call('/api/persons/library')['persons'] if p['name']=='乙')['aliases']
first=call('/api/persons/library?preview=1&name='+urllib.parse.quote('乙'));second=call('/api/persons/library?preview=1&name='+urllib.parse.quote('乙')+'&offset=4')
assert first['total_videos']==8 and len(first['files'])==4 and first['next_offset']==4 and second['next_offset'] is None
assert len({file['path'] for file in first['files']+second['files']})==8
call('/api/persons/library?preview=1&name='+urllib.parse.quote('乙')+'&offset=-1',status=400)
print('PASS real video preview paging and receipt-backed progress')
print('PASS multi-source collisions, partial completion, idempotence and retry remaining selection')
# A late ledger failure must roll back the entire current person's deletion.
before=digest_tree();metadata={p:p.read_bytes() for p in base.rglob('*.json') if 'person-operations' not in p.parts}
sql("CREATE TRIGGER fail_batch_delete BEFORE UPDATE OF path ON asset BEGIN SELECT RAISE(ABORT,'batch delete failure'); END")
result=apply(preview('delete',['乙','空人物']));assert not result['ok'] and not result['completed'] and result['remaining']==['乙','空人物'],result
assert digest_tree()==before
assert all(p.read_bytes()==value for p,value in metadata.items());assert not (base/'.review-write.lock').exists()
sql('DROP TRIGGER fail_batch_delete')
print('PASS delete rollback preserves files and metadata')
plan=preview('delete',['乙','空人物']);staged={Path(f['dst']):Path(f['src']).read_bytes() for p in plan['plans'] for f in p['files']}
result=apply(plan);assert result['ok'] and len(result['completed'])==2,result
assert apply(plan)==result and not names(),names()
assert all(p.read_bytes()==data for p,data in staged.items())
assert all(not (creators/n).exists() for n in ['乙','空人物'])
assert not read('integration-queue.json') and not read('external-profiles/manual-links.json')['links'] and not read('watermarks/mappings.json')
assert read('archive-history.json'), 'historical audit must remain'
call('/api/persons/library?name='+urllib.parse.quote('乙'),status=404)
assert sql('SELECT count(*) FROM asset WHERE path LIKE \'%person-deletions%\'')[0][0]>=4
assert not (base/'.review-write.lock').exists()
print('PASS full-person and empty-person deletion, staged bytes, no ghost entries, ledger paths, and audit preservation')
