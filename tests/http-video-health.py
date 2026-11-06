"""Visibility and reporting tests on disposable data; never target production."""
import json,sys,urllib.request,urllib.error,datetime,os
from pathlib import Path
URL=sys.argv[1]if len(sys.argv)>1 else'http://127.0.0.1:8922';assert URL.endswith(':8922')
root=Path('/tmp/face-admin-v10-fixture');assert(root/'.fixture-marker').exists()
base=root/'base';media=root/'media'
def req(p,body=None,expected=200):
 r=urllib.request.Request(URL+p,data=json.dumps(body).encode()if body is not None else None,headers={'Content-Type':'application/json'})
 try:
  with urllib.request.urlopen(r,timeout=30)as response:code=response.status;d=json.load(response)
 except urllib.error.HTTPError as e:code=e.code;d=json.load(e)
 assert code==expected,(p,code,d)
 return d
def vid(n):return 'R:\\media\\pikpak\\My Telegram\\test\\'+n+'.mp4'
def atomic(p,d):
 tmp=p.with_suffix('.test-tmp');tmp.write_text(json.dumps(d));tmp.replace(p)
def record(n):
 p=media/'pikpak/My Telegram/test'/(n+'.mp4');s=p.stat()
 return dict(vid=vid(n),path='/fixture-media/pikpak/My Telegram/test/'+n+'.mp4',size=s.st_size,mtime_ns=str(s.st_mtime_ns),status='corrupt',hidden=True,consecutive=3)
when=datetime.datetime.now(datetime.timezone.utc).isoformat()
original=(media/'pikpak/My Telegram/test/a.mp4').read_bytes()
atomic(base/'video-health.json',dict(schema=1,status='complete',last_success_at=when,records={vid(n):record(n)for n in ['a','review','keep']}))
assert [v['vid']for v in req('/api/cluster/p1')['vids']]==[vid('b')]
assert req('/api/claims/pending')['total']==0
req('/api/plan/queue',dict(creator='娜娜',vids=[vid('a')]),409)
req('/api/claims/decide',dict(vid=vid('review'),approve=True),409)
assert next(r for r in req('/api/plan/preview')['rows']if r['vid']==vid('keep'))['status']=='corrupt'
assert len(req('/api/video-health')['rows'])==3
print('PASS confirmed invalid videos leave pending lists and cannot be queued by stale clients; pending tasks remain visible with a reason')

p=media/'pikpak/My Telegram/test/a.mp4';s=p.stat();os.utime(p,ns=(s.st_atime_ns,s.st_mtime_ns+1000000000))
assert len(req('/api/cluster/p1')['vids'])==2
assert p.read_bytes()==original
print('PASS a replaced or repaired file immediately reappears without deleting media or face data')

body=dict(vid=vid('a'),path='/fixture-media/pikpak/My Telegram/test/a.mp4',code=4)
req('/api/video-health/report',body);req('/api/video-health/report',body)
stored=json.loads((base/'video-health-reports.json').read_text());assert stored[vid('a')]['count']==1
req('/api/video-health/report',dict(vid='unknown',code=4),404)
req('/api/video-health/report',{**body,'path':'/fixture-media/pikpak/My Telegram/test/b.mp4'},409)
print('PASS playback reports are deduplicated, stay local, and match only the current known video')

p=media/'pikpak/My Telegram/test/b.mp4';p.rename(p.with_suffix('.backup'));p.mkdir()
state=json.loads((base/'video-health.json').read_text())
state['records'][vid('b')]=dict(vid=vid('b'),path='/fixture-media/pikpak/My Telegram/test/b.mp4',size=None,mtime_ns=None,status='not_file',hidden=True,consecutive=3)
atomic(base/'video-health.json',state)
identity=next(i for i in req('/api/identities')['identities']if i['name']=='娜娜')
assert all(v['vid']!=vid('b')for v in identity['vids'])
print('PASS confirmed directory records are excluded from person review as well')
