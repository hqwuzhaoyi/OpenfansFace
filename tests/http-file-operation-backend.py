"""Backend-selectable HTTP rename acceptance. Only explicitly marked /tmp fixtures."""
import argparse,hashlib,json,sqlite3,subprocess,sys,urllib.error,urllib.parse,urllib.request
from pathlib import Path

parser=argparse.ArgumentParser()
parser.add_argument('--url',default='http://127.0.0.1:18961')
parser.add_argument('--root',type=Path,required=True)
parser.add_argument('--backend',choices=['legacy','rust'],default='legacy')
parser.add_argument('--seed',action='store_true')
parser.add_argument('--sample',type=Path,default=Path('/tmp/face-admin-fixture.mp4'))
args=parser.parse_args();root=args.root.resolve()
assert root.parent==Path('/tmp').resolve() and root.name.startswith('face-admin-person-fileops-'), 'fixture root required'
parsed=urllib.parse.urlparse(args.url)
assert parsed.hostname in ('127.0.0.1','localhost') and parsed.port not in (None,80,443,8919), 'isolated loopback port required'
if args.seed:
 subprocess.run([sys.executable,str(Path(__file__).with_name('http-person-file-operations.py')),str(root),'--seed','--sample',str(args.sample)],check=True)
 raise SystemExit
assert (root/'.fileops-fixture').read_text()=='person file operations'
base=root/'base';creators=root/'media/pikpak/creators';ledger=root/'ledger.db'
def call(route,body=None,status=200):
 request=urllib.request.Request(args.url+route,data=None if body is None else json.dumps(body).encode(),headers={'Content-Type':'application/json'})
 try:
  with urllib.request.urlopen(request,timeout=60) as r:code=r.status;result=json.load(r)
 except urllib.error.HTTPError as e:code=e.code;result=json.load(e)
 assert code==status,(route,code,result)
 return result
def library():return call('/api/persons/library?name='+urllib.parse.quote('甲'))
def request(file,name):return {'action':'rename-file','person':'甲','files':[{k:file[k] for k in ('vid','path','version')}],'filename':name}
def preview(body):return call('/api/persons/manage',{'mode':'preview','request':body})
def apply(plan,status=200):return call('/api/persons/manage',{'mode':'apply','request':plan['request'],'version':plan['version']},status)
def digest(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def win(p):return 'R:\\media\\pikpak\\'+str(p).split('/pikpak/',1)[1].replace('/','\\')
listing=library();assert listing['root']==str(creators/'甲'), 'API is not serving this fixture'
file=next(f for f in listing['files'] if f['name']=='已归档.mp4');source=Path(file['path']);target=source.with_name('后端验收改名.mp4');before=digest(source);untouched=digest(creators/'乙/保留.mp4')
plan=preview(request(file,target.name));assert plan['files'][0]['src']==str(source) and plan['files'][0]['dst']==str(target)
status=call('/api/persons/manage?id='+plan['id']);assert status['backend']==args.backend and status['status']=='not_started',status
lock=base/'.review-write.lock';assert not lock.exists();lock.write_text(json.dumps({'reason':'isolated competing writer'}))
try:
 apply(plan,409)
 call('/api/plan/rollback',{'manifest':str(base/'not-used.json')},409)
 assert source.exists() and not target.exists()
finally:lock.unlink()
result=apply(plan);assert result['ok'] and result['changed']==1 and result['id']==plan['id'],result
assert not source.exists() and digest(target)==before and digest(creators/'乙/保留.mp4')==untouched
status=call('/api/persons/manage?id='+plan['id']);assert status['backend']==args.backend and status['status']=='complete' and status['result']==result,status
assert apply(plan)==result
call('/api/persons/manage',{'mode':'apply','request':{**plan['request'],'filename':'different.mp4'},'version':plan['version']},409)
with sqlite3.connect(base/'faces.db') as c:assert c.execute('SELECT path FROM videos WHERE vid=?',(file['vid'],)).fetchone()[0]==str(target)
history=json.loads((base/'archive-history.json').read_text());assert next(x for x in history if x['vid']==file['vid'])['dst']==str(target)
with sqlite3.connect(ledger) as c:
 assert c.execute('SELECT count(*) FROM asset WHERE path=?',(win(source),)).fetchone()[0]==0
 aid=c.execute('SELECT id FROM asset WHERE path=?',(win(target),)).fetchone()[0]
 assert c.execute("SELECT count(*) FROM asset_entity WHERE asset_id=? AND entity_id=1 AND role='creator'",(aid,)).fetchone()[0]==1
with urllib.request.urlopen(urllib.request.Request(args.url+'/api/video?'+urllib.parse.urlencode({'p':file['vid']}),headers={'Range':'bytes=0-31'}),timeout=30) as r:
 assert r.status==206 and r.read()==target.read_bytes()[:32],r.status
assert next(f for f in library()['files'] if f['vid']==file['vid'])['path']==str(target)
assert not lock.exists()
print('PASS',args.backend,'HTTP preview, lock exclusion, rename, metadata, native ledger bytes, idempotent receipt and stable Range playback')
