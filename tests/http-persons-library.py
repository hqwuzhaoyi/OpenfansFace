"""Seed/check an isolated persons workbench at port 8922. Never contacts production."""
import argparse, hashlib, json, shutil, sqlite3, time, urllib.request, urllib.error
from pathlib import Path

parser=argparse.ArgumentParser()
parser.add_argument('root',type=Path)
parser.add_argument('--seed',action='store_true')
parser.add_argument('--sample',type=Path,default=Path('/tmp/persons-flower.mp4'))
args=parser.parse_args()
root=args.root.resolve()
assert root.resolve().parent==Path('/tmp').resolve() and root.name.startswith('face-admin-persons-fixture-'), 'isolated fixture prefix required'
base=root/'base';media=root/'media'
def save(name,value):
 p=base/name;p.parent.mkdir(parents=True,exist_ok=True);p.write_text(json.dumps(value,ensure_ascii=False))
def source(name):return media/'pikpak'/'My Telegram'/(name+'.mp4')
def vid(name):return 'R:\\media\\pikpak\\My Telegram\\'+name+'.mp4'

if args.seed:
 assert not root.exists(), 'seed only a new directory'
 base.mkdir(parents=True);media.mkdir();(root/'.persons-fixture').write_text('isolated persons tests')
 sample=args.sample.read_bytes()
 for name in ['face','report','watermark','stale','source-mark','assigned','ready','other']:
  file=source(name);file.parent.mkdir(parents=True,exist_ok=True);file.write_bytes(sample)
 for relative in ['娜娜/owned.mp4','娜娜/unscanned.mp4','娜娜/旅行/海边.mp4','只有目录的人/独立文件.mp4']:
  file=media/'pikpak'/'creators'/relative;file.parent.mkdir(parents=True,exist_ok=True);file.write_bytes(sample)
 (media/'pikpak'/'creators'/'娜娜'/'outside').symlink_to(source('face').parent,target_is_directory=True)
 con=sqlite3.connect(base/'faces.db');con.executescript('CREATE TABLE videos(vid TEXT PRIMARY KEY,path TEXT);CREATE TABLE faces(vid TEXT,thumb TEXT,score REAL);')
 for name in ['face','report','watermark','stale','source-mark','assigned','ready','other']:
  con.execute('INSERT INTO videos VALUES(?,?)',(vid(name),str(source(name))))
 con.execute('INSERT INTO videos VALUES(?,?)',('duplicate-assigned',str(source('assigned'))));con.commit();con.close()
 def item(name):return dict(vid=vid(name),size=len(sample),duration=5.055,thumb=None)
 save('report.json',dict(stats={},identities=[dict(name='娜娜',tpl_faces=3,vids=[item(n)for n in ['face','report','assigned','missing']]+[dict(vid='duplicate-assigned',size=len(sample))]),dict(name='其他人',vids=[item('other')])]))
 save('clusters.json',[dict(cid='p1',vids=[item(n)for n in ['face','report','watermark']])])
 save('claims-detail.json',[dict(vid=vid('face'),name='娜娜',s=.572,n=3)])
 save('integration-queue.json',[dict(vid=vid('ready'),creator='娜娜',at='2026-11-06T00:00:00Z'),dict(vid=vid('missing'),creator='娜娜',at='2026-11-06T00:00:00Z'),dict(vid=vid('assigned'),creator='其他人',at='2026-11-06T00:00:00Z')])
 save('review-decisions.json',dict(approved=[],rejected=[]))
 records={}
 for name in ['watermark','stale','source-mark']:
  file=source(name);stat=file.stat();records[vid(name)]=dict(vid=vid(name),path=str(file),size=stat.st_size,mtime_ns=str(stat.st_mtime_ns if name!='stale' else 0),pipeline='fixture',job_id='fixture',status='done',checked_at='2026-11-06',sampled_frames=4,candidates=[dict(key=name,text='@'+name,token=name,platform='test',kind='account',frames=3,score=.96,timestamp=1.5,evidence='11111111.jpg')])
 save('watermarks/results.json',records);save('watermarks/worker.json',dict(pipeline='fixture'))
 save('watermarks/mappings.json',{name:dict(key=name,text=name,token=name,platform='test',kind='source' if name=='source-mark' else 'person',creator='娜娜',confirmed_at='2026-11-06')for name in ['watermark','stale','source-mark']})
 print(root);raise SystemExit

assert (root/'.persons-fixture').is_file()
url='http://127.0.0.1:8922'
def request(endpoint,body=None,method='GET',status=200):
 data=json.dumps(body).encode()if body is not None else None
 req=urllib.request.Request(url+endpoint,data=data,method=method,headers={'Content-Type':'application/json'})
 try:
  with urllib.request.urlopen(req,timeout=30)as r:assert r.status==status;return json.load(r)
 except urllib.error.HTTPError as e:
  assert e.code==status,(endpoint,e.code,e.read().decode());return json.load(e)
q=urllib.parse.quote('娜娜')
def library():return request('/api/persons/library?name='+q)
initial=library();assert initial['root']==str(media/'pikpak'/'creators'/'娜娜'),'refuse writes against a non-fixture server'
assert {v['name']for v in initial['files']}=={'owned.mp4','unscanned.mp4','海边.mp4'}
assert {f['name']for f in initial['folders']}=={'.','旅行'}
assert initial['queued']==2
assert '只有目录的人'in[p['name']for p in request('/api/persons/library')['persons']]
rows={v['vid']:v for v in initial['recommendations']}
assert set(rows)=={vid('face'),vid('report'),vid('watermark')},set(rows)
assert rows[vid('face')]['evidence'][0]['score']==.572
assert 'score'not in rows[vid('report')]['evidence'][0]
assert rows[vid('watermark')]['evidence'][0]['image'].startswith('/api/watermarks/evidence?')
request('/api/persons/library?name=..',status=400)
print('PASS actual files, unscanned folders, symlink exclusion, queue scope, real evidence and stale watermark filtering')
before=hashlib.sha256(source('face').read_bytes()).hexdigest()
rejected=request('/api/persons/recommendations',dict(name='娜娜',vid=vid('face')),'POST')
assert vid('face')not in[v['vid']for v in library()['recommendations']]
request('/api/persons/recommendations',dict(name='娜娜',vid=vid('face'),id='stale-token'),'DELETE',409)
request('/api/persons/recommendations',dict(name='娜娜',vid=vid('face'),id=rejected['id']),'DELETE')
assert vid('face')in[v['vid']for v in library()['recommendations']]
request('/api/persons/recommendations',dict(name='其他人',vid=vid('face')),'POST',409)
request('/api/plan/queue',dict(creator='娜娜',vids=[vid('face')],source='person',source_id='娜娜'),'POST')
assert library()['queued']==3
assert vid('face')not in[v['vid']for v in library()['recommendations']]
request('/api/plan/queue',dict(vids=[vid('face')]),'DELETE')
assert library()['queued']==2
assert hashlib.sha256(source('face').read_bytes()).hexdigest()==before
request('/api/plan/apply',dict(vids=[vid('ready')],version='stale'),'POST',409)
assert source('ready').is_file()
print('PASS per-person reject/undo, stale undo protection, enqueue/return, and no unintended file move')
