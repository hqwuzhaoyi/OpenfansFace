"""Only the marked, isolated archive fixture on port 18952; never production data."""
import json,sqlite3,urllib.request,urllib.error,uuid,threading,time,os
from pathlib import Path
root=Path('/tmp/face-archive-reliability-fixture')
assert (root/'.fixture-marker').read_text()=='face-admin test data'
url='http://127.0.0.1:18952';base=root/'base';media=root/'media';db=root/'ledger/ledger.db'
def call(route,body=None,status=200):
 request=urllib.request.Request(url+route,data=json.dumps(body).encode() if body is not None else None,headers={'Content-Type':'application/json'})
 try:
  with urllib.request.urlopen(request,timeout=60) as r:code=r.status;data=json.load(r)
 except urllib.error.HTTPError as e:code=e.code;data=json.load(e)
 assert code==status,(route,code,data)
 return data
def plan():return call('/api/plan/preview')
assert all(not r.get('src') or r['src'].startswith(str(media)) for r in plan()['rows'])
def seed(name,creator='Momoyih'):
 p=media/'pikpak/My Telegram/reliability'/f'{name}.mp4';p.parent.mkdir(parents=True,exist_ok=True);p.write_bytes(Path('/tmp/face-admin-fixture.mp4').read_bytes())
 vid='R:\\media\\pikpak\\My Telegram\\reliability\\'+p.name
 with sqlite3.connect(db) as con:con.execute('INSERT INTO asset(path) VALUES(?)',(vid,))
 call('/api/plan/queue',{'creator':creator,'vids':[vid]})
 return p,vid
def payload(vid):return {'operation_id':str(uuid.uuid4()),'vids':[vid],'version':plan()['version']}
with sqlite3.connect(db) as con:
 con.execute('CREATE UNIQUE INDEX IF NOT EXISTS fixture_entity_unique ON entity(kind,normalized_name)')
 con.execute("INSERT INTO entity(kind,canonical_name,normalized_name,metadata_json,created_at,updated_at) VALUES('creator','momoyih','momoyih','{}','','')")
p,vid=seed('normalized');body=payload(vid)
result=call('/api/plan/apply',body);assert result['moved']==1 and not result['errors'],result
assert call('/api/plan/apply',body)==result
assert call('/api/plan/progress?id='+body['operation_id'])['result']==result
call('/api/plan/apply',{**body,'version':'different'},409)
with sqlite3.connect(db) as con:assert con.execute("SELECT canonical_name FROM entity WHERE normalized_name='momoyih'").fetchall()==[('momoyih',)]
assert not p.exists();print('PASS normalized identity reuse, idempotent receipt and request binding')

p,vid=seed('permission');os.chmod(p.parent,0o555)
try:
 row=next(r for r in plan()['rows'] if r['vid']==vid);assert row['status']=='permission' and '源目录' in row['reason'],row
finally:os.chmod(p.parent,0o755)
print('PASS permission failure blocks the preview before moving files')

p,vid=seed('rollback');body=payload(vid)
with sqlite3.connect(db) as con:con.execute("CREATE TRIGGER fixture_archive_failure BEFORE UPDATE OF path ON asset BEGIN SELECT RAISE(FAIL,'fixture ledger failure');END")
try:
 result=call('/api/plan/apply',body);assert result['moved']==0 and result['rolled_back'] and not result['recovery_needed'],result
 assert p.exists() and not (media/'pikpak/creators/Momoyih/rollback.mp4').exists()
 assert call('/api/plan/progress?id='+body['operation_id'])['result']==result
 assert not (base/'.review-write.lock').exists()
finally:
 with sqlite3.connect(db) as con:con.execute('DROP TRIGGER fixture_archive_failure')
print('PASS transaction failure restores file and preserves pending task')

p,vid=seed('progress');body=payload(vid);con=sqlite3.connect(db);con.execute('BEGIN IMMEDIATE')
results=[]
thread=threading.Thread(target=lambda:results.append(call('/api/plan/apply',body)));thread.start()
try:
 for _ in range(50):
  progress=call('/api/plan/progress?id='+body['operation_id'])
  if progress['job'] and progress['job']['stage']=='检查 Peach 账本':break
  time.sleep(.1)
 else:raise AssertionError('no live progress')
 assert progress['job']['status']=='running' and p.exists(),progress
 call('/api/plan/apply',body,409)
finally:con.rollback();con.close();thread.join(60)
assert results and results[0]['moved']==1,results
print('PASS live progress while ledger waits; overlapping submission blocked')

p,vid=seed('missing-ledger')
with sqlite3.connect(db) as con:con.execute('DELETE FROM asset WHERE path=?',(vid,))
result=call('/api/plan/apply',payload(vid));assert not result['moved'] and p.exists() and '源视频账本' in ''.join(result['errors']),result
print('PASS missing ledger rejected before moving any file')
print('ALL ARCHIVE RELIABILITY HTTP TESTS PASSED')
# A request that never reached the server can be fenced before a user retries.
late_id=str(uuid.uuid4());result=call('/api/plan/progress',{'id':late_id})
assert result['result']['moved']==0 and result['job']['stage']=='未执行'
call('/api/plan/apply',{'operation_id':late_id,'version':plan()['version'],'vids':[vid]},409)
assert p.exists()
print('PASS missing request fenced against a delayed POST')
