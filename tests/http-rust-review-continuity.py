"""#18 Native immutable review sessions; marked local media only."""
import importlib.util, json, sqlite3, uuid
from pathlib import Path
spec=importlib.util.spec_from_file_location('archive_fixture',Path(__file__).with_name('http-rust-archive.py'))
n=importlib.util.module_from_spec(spec);spec.loader.exec_module(n)
def edit(f,ids,**values):
 with sqlite3.connect(f.base/'processing/processing.db') as c:
  for vid in ids:c.execute('UPDATE tasks SET '+','.join(k+'=?' for k in values)+' WHERE vid=?',(*values.values(),vid))
def confirm(f,page,rows=None,**extra):
 rows=page['rows'] if rows is None else rows
 return dict(vids=[t['vid'] for t in rows],creator='Momoyih',source='review',review_session=page['session'],review_group=dict(key=page['group']['key'],deferred=bool(rows[0]['deferred'])),review_tokens={t['vid']:t['review_token'] for t in rows},fingerprints={t['vid']:{k:t[k] for k in ['path','size','mtime_ns']} for t in rows},**extra)
def health_record(f,t):
 # #17 supplies the exact fingerprint + health material guard once integrated.
 import urllib.parse,urllib.request,urllib.error
 req=urllib.request.Request(f.url+'/v1/video-health?vid='+urllib.parse.quote(t['vid']),headers={'Authorization':'Bearer archive-fixture'})
 try:
  with urllib.request.urlopen(req) as response:guard=json.load(response)
 except urllib.error.HTTPError as error:
  if error.code!=404:raise
  guard={}
 return {**guard,'path':t['path'],'size':t['size'],'mtime_ns':t['mtime_ns'],'status':'corrupt','hidden':True,'restorations':[]}
def main():
 f=n.Fixture(50)
 try:
  a=f.call('review-page');b=f.call('review-page');assert a['session']!=b['session'];assert len(a['rows'])==24
  first=a['rows'][0];second=a['rows'][1];original_second=f.call('review-page',{'session':a['session'],'page':'1'})['rows'][0]['vid']
  invalid=confirm(f,a,[first]);invalid['review_tokens'][first['vid']]=str(uuid.uuid4());assert f.call('queue-confirm',invalid,status=409)['code']=='review_not_viewed'
  invalid=confirm(f,a,[first]);invalid['review_session']=b['session'];f.call('queue-confirm',invalid,status=409)
  invalid=confirm(f,a,[first]);invalid['vids']=[f.ids[-1]];f.call('queue-confirm',invalid,status=409)
  invalid=confirm(f,a,[first]);invalid.pop('review_session');f.call('queue-confirm',invalid,status=409)
  # Candidate replacement + stage/priority changes preserve delivered evidence and group.
  original=first['data'];changed={**original,'suggested':'Other','face':{'unknown_group':'p999'},'kind':'none'}
  edit(f,f.ids,data=json.dumps(changed),stage='face_queued',priority=100)
  f.add(50)
  assert f.call('review-status',{'session':a['session']})['has_updates']
  same=f.call('review-page',{'session':a['session']});assert [t['vid'] for t in same['rows']]==[t['vid'] for t in a['rows']];assert same['rows'][0]['data']==original
  # First completed scope only; next page starts at original slot 24, never shifts.
  body=confirm(f,a,[first],operation_id=str(uuid.uuid4()));result=f.call('queue-confirm',body);assert result['added']==1;assert f.call('queue-confirm',body)==result
  f.call('queue-confirm',confirm(f,b,[b['rows'][0]]),status=409)
  same=f.call('review-page',{'session':a['session']});assert len(same['rows'])==23 and same['total']==50
  page2=f.call('review-page',{'session':a['session'],'page':'1'});assert len(page2['rows'])==24 and page2['rows'][0]['vid']==original_second
  f.stop();f.start();assert f.call('review-page',{'session':a['session'],'page':'1'})['rows'][0]['vid']==original_second
  # Undo belongs to this session's original receipt; older delivered token is rejected (ABA).
  f.call('queue-undo',{'vids':[first['vid']],'review_session':b['session']},status=409)
  f.call('queue-undo',{'vids':[first['vid']],'review_session':a['session']})
  f.call('queue-confirm',confirm(f,a,[first]),status=409)
  renewed=f.call('review-page',{'session':a['session']});assert len(renewed['rows'])==24
  # Note/defer/restore update only explicit manual fields, retain model result and page identity.
  t=renewed['rows'][0];editbody=confirm(f,renewed,[t]);editbody['note']='saved note'
  f.call('review-note',editbody);f.call('review-note',editbody,status=409)
  noted=f.call('review-page',{'session':a['session']});assert noted['rows'][0]['note']=='saved note'
  with sqlite3.connect(f.base/'processing/processing.db') as c:assert json.loads(c.execute('SELECT data FROM tasks WHERE vid=?',[t['vid']]).fetchone()[0])==changed
  f.call('review-defer',confirm(f,noted,[noted['rows'][0]]));assert len(f.call('review-page',{'session':a['session']})['rows'])==23
  deferred=f.call('review-page',{'session':a['session'],'view':'deferred'});assert deferred['rows'][0]['vid']==t['vid']
  f.call('review-restore',confirm(f,deferred,[deferred['rows'][0]]));assert len(f.call('review-page',{'session':a['session']})['rows'])==24
  # Rule and file changes reject stale evidence, never silently look up latest candidates.
  n.write(f.base/'watermarks/mappings.json',{'X:fixture':{'kind':'source'}})
  f.call('queue-confirm',confirm(f,a,[second]),status=409)
  new=f.call('review-page');assert new['session']!=a['session'];t=new['rows'][0]
  target=Path(t['path']);target.write_bytes(target.read_bytes()+b'changed')
  f.call('queue-confirm',confirm(f,new,[t]),status=409)
  with sqlite3.connect(f.base/'file-operations-rust/operations.sqlite3') as c:c.execute('UPDATE review_sessions SET expires=0 WHERE id=?',[a['session']])
  assert f.call('review-page',{'session':a['session']},status=409)['code']=='review_session_expired'
  f.call('review-status',{'session':str(uuid.uuid4())},status=409)
  print('PASS #18 native sessions: delivered/scoped evidence, facial recompute, immutable paging, exact receipts, restart, multi-tab, ABA, note/defer/restore, file/rule guards and expiry',f.root)
 finally:f.stop()
def lane_slots_and_relevant_guards():
 f=n.Fixture(51)
 try:
  edit(f,[f.ids[0]],deferred=1)
  pending=f.call('review-page');p2=f.call('review-page',{'session':pending['session'],'page':'1'})
  deferred=f.call('review-page',{'session':pending['session'],'view':'deferred'})
  f.call('review-restore',confirm(f,deferred))
  after=f.call('review-page',{'session':pending['session']});after2=f.call('review-page',{'session':pending['session'],'page':'1'})
  assert [r['vid'] for r in after['rows']]==[r['vid'] for r in pending['rows']]
  assert [r['vid'] for r in after2['rows']]==[r['vid'] for r in p2['rows']]
  last=f.call('review-page',{'session':pending['session'],'page':'2'});assert last['rows'][-1]['vid']==f.ids[0]
  # A failed batch keeps every uncommitted item and original slots.
  one,two=after['rows'][:2];oldpath=two['path'];edit(f,[two['vid']],path=oldpath+'.changed')
  f.call('queue-confirm',confirm(f,after,[one,two]),status=409);assert not f.queue()
  assert len(f.call('review-page',{'session':pending['session']})['rows'])==24
  edit(f,[two['vid']],path=oldpath)
  # Health scope follows the effective file record, not unrelated report progress.
  import datetime
  n.write(f.base/'video-health.json',{'last_success_at':datetime.datetime.now(datetime.timezone.utc).isoformat(),'records':{'unrelated':{'hidden':True}}})
  f.call('review-note',{**confirm(f,after,[one]),'note':'unrelated health is harmless'})
  fresh=f.call('review-page');t=fresh['rows'][0]
  n.write(f.base/'video-health.json',{'last_success_at':datetime.datetime.now(datetime.timezone.utc).isoformat(),'schema':2,'records':{t['vid']:health_record(f,t)}})
  f.call('queue-confirm',confirm(f,fresh,[t]),status=409)
  assert t['vid'] not in [r['vid'] for r in f.call('review-page')['rows']]
  print('PASS stable lane append, atomic failed review batch, task-path guard, relevant health only',f.root)
 finally:f.stop()
if __name__=='__main__':main();lane_slots_and_relevant_guards()
