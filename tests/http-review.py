"""Integration regression against the isolated fixture server, never production."""
import json,urllib.request,urllib.error,sys,sqlite3
from pathlib import Path
BASE=sys.argv[1] if len(sys.argv)>1 else 'http://127.0.0.1:8922'
assert BASE.endswith(':8922'), 'Fixture tests must not target production'
root=Path('/tmp/face-admin-v10-fixture');assert (root/'.fixture-marker').exists()
def request(path,body=None,method=None,expected=200):
 data=None if body is None else json.dumps(body).encode()
 req=urllib.request.Request(BASE+path,data=data,method=method,headers={'Content-Type':'application/json'})
 try:
  with urllib.request.urlopen(req,timeout=60)as r:status=r.status;d=json.load(r)
 except urllib.error.HTTPError as e:status=e.code;d=json.load(e)
 assert status==expected,(path,status,d)
 return d

def vid(n):return 'R:\\media\\pikpak\\My Telegram\\test\\'+n+'.mp4'
def queue(n,source='cluster',source_id='p1'):
 return request('/api/plan/queue',dict(creator='娜娜',vids=[vid(n)],source=source,source_id=source_id),'POST')
def undo(n):return request('/api/plan/queue',dict(vids=[vid(n)]),'DELETE')

assert len(request('/api/cluster/p1')['vids'])==2
queue('a');assert [v['vid']for v in request('/api/cluster/p1')['vids']]==[vid('b')]
assert request('/api/clusters')[0]['weak_count']==1
assert undo('a')['removed'][0]['return_href']=='/clusters?cid=p1'
assert len(request('/api/cluster/p1')['vids'])==2
print('PASS partial cluster queue, reload, and undo')

request('/api/claims/decide',dict(vid=vid('review'),approve=True),'POST')
assert request('/api/claims/pending')['total']==0
r=undo('review');assert r['removed'][0]['return_href'].startswith('/approvals?vid=')
assert request('/api/claims/pending')['total']==1
assert request('/api/claims/pending')['decided']['approved']==0
print('PASS approval really queues; undo restores pending')

queue('a','person','娜娜');d=request('/api/identities');assert d['identities'][0]['vids'][0]['state']=='queued'
assert undo('a')['removed'][0]['return_href']=='/persons/%E5%A8%9C%E5%A8%9C'
print('PASS person confirmation and return path')

plan=request('/api/plan/preview')
request('/api/plan/apply',{},'POST',400)
request('/api/plan/apply',dict(vids=[vid('conflict')],version=plan['version']),'POST',409)
queue('a');request('/api/plan/apply',dict(vids=[vid('keep')],version=plan['version']),'POST',409);undo('a')
assert request('/api/plan/preview')['n']==3
print('PASS explicit selection, conflict, stale preview, and no accidental writes')

request('/api/claims/decide',dict(vid=vid('review'),approve=True),'POST')
p=request('/api/plan/preview');r=request('/api/plan/apply',dict(vids=[vid('review')],version=p['version']),'POST')
assert r['moved']==1 and r['ledger']==1 and not r['errors'],r
assert request('/api/plan/preview')['n']==3
assert not (root/'media/pikpak/My Telegram/test/review.mp4').exists()
assert (root/'media/pikpak/creators/娜娜/review.mp4').exists()
con=sqlite3.connect(root/'ledger/ledger.db');assert con.execute('SELECT COUNT(*) FROM asset_entity').fetchone()[0]==1;con.close()
assert request('/api/claims/pending')['total']==0
print('PASS selected file move plus Peach remap; missing/conflicting/unselected tasks retained')

queue('b');p=request('/api/plan/preview');r=request('/api/plan/apply',dict(vids=[vid('b')],version=p['version']),'POST')
assert r['moved']==0 and r['errors'],r
assert (root/'media/pikpak/My Telegram/test/b.mp4').exists()
assert not (root/'media/pikpak/creators/娜娜/b.mp4').exists()
assert request('/api/plan/preview')['n']==4
undo('b');assert request('/api/plan/preview')['n']==3
print('PASS failed ledger update restores files and preserves queue')

p=request('/api/plan/preview')
qfile=root/'base/integration-queue.json';q=json.loads(qfile.read_text())
lock=root/'base/.review-write.lock';lock.write_text('fixture lock')
try:request('/api/plan/queue',dict(creator='娜娜',vids=[vid('a')],source='cluster',source_id='p1'),'POST',409)
finally:lock.unlink()
assert json.loads(qfile.read_text())==q
print('PASS concurrent write guard')
print('ALL HTTP REGRESSIONS PASSED')
