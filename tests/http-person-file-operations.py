"""Destructive regression cases run only against a new marked /tmp fixture on port 8922."""
import argparse,base64,hashlib,json,os,sqlite3,sys,time,urllib.request,urllib.error,urllib.parse
from pathlib import Path

parser=argparse.ArgumentParser();parser.add_argument('root',type=Path);parser.add_argument('--seed',action='store_true');parser.add_argument('--sample',type=Path,default=Path('/tmp/persons-flower.mp4'));args=parser.parse_args()
root=args.root.resolve();assert root.parent==Path('/tmp').resolve()and root.name.startswith('face-admin-person-fileops-')
base=root/'base';media=root/'media';creators=media/'pikpak/creators';ledger=root/'ledger.db'
def win(file):return 'R:\\media\\pikpak\\'+str(file).split('/pikpak/',1)[1].replace('/','\\')
def write(name,value):
 p=base/name;p.parent.mkdir(parents=True,exist_ok=True);p.write_text(json.dumps(value,ensure_ascii=False))
def read(name):return json.loads((base/name).read_text())
def file_hash(file):return hashlib.sha256(file.read_bytes()).hexdigest()
known='R:\\media\\pikpak\\My Telegram\\archived.mp4';pending='R:\\media\\pikpak\\My Telegram\\incoming.mp4'
if args.seed:
 assert not root.exists(),'seed requires a new directory'
 base.mkdir(parents=True);(root/'.fileops-fixture').write_text('person file operations')
 sample=args.sample.read_bytes()
 for name in ['甲/已归档.mp4','甲/未扫描.mp4','甲/占用.mp4','乙/保留.mp4']:
  p=creators/name;p.parent.mkdir(parents=True,exist_ok=True);p.write_bytes(sample)
 (creators/'甲/空子目录').mkdir();(creators/'空人物').mkdir()
 (creators/'甲/说明.txt').write_text('伴随人物目录一起保留')
 incoming=media/'pikpak/My Telegram/incoming.mp4';incoming.parent.mkdir(parents=True);incoming.write_bytes(sample)
 con=sqlite3.connect(base/'faces.db');con.executescript('CREATE TABLE videos(vid TEXT PRIMARY KEY,path TEXT,name TEXT,collection TEXT);CREATE TABLE faces(vid TEXT,score REAL,thumb TEXT);')
 con.executemany('INSERT INTO videos VALUES(?,?,?,?)',[(known,str(media/'pikpak/My Telegram/archived.mp4'),'archived.mp4','old'),(pending,str(incoming),'incoming.mp4',str(incoming.parent))]);con.commit();con.close()
 write('report.json',dict(stats={},identities=[dict(name='甲',tpl_faces=2,vids=[dict(vid=known,name='已归档.mp4',size=len(sample)),dict(vid=pending,size=len(sample))]),dict(name='乙',vids=[]),dict(name='空人物',vids=[])]))
 write('claims-detail.json',[dict(vid=pending,name='甲',s=.56,n=3)])
 write('clusters.json',[dict(cid='p1',vids=[dict(vid=known,name='已归档.mp4',path=str(creators/'甲/已归档.mp4'))])])
 write('archive-history.json',[dict(vid=known,creator='甲',dst=str(creators/'甲/已归档.mp4'),win_dst=win(creators/'甲/已归档.mp4'),at='fixture')])
 write('integration-queue.json',[dict(vid=pending,creator='甲',at='fixture',source='person',source_id='甲',return_href='/persons/'+urllib.parse.quote('甲'))])
 write('people-manual.json',[dict(name='甲',created_at='fixture',last_used_at='fixture')])
 write('template-overrides.json',{'甲':dict(groups=[0])})
 write('watermarks/mappings.json',{'account':dict(key='account',kind='person',creator='甲',token='artist',text='@artist',platform='X')})
 write('person-rejections.json',{json.dumps(['甲','rejected'],ensure_ascii=False,separators=(',',':')):dict(name='甲',vid='rejected',path='unrelated',size=1,mtime_ns='0',id='past',at='fixture')})
 con=sqlite3.connect(ledger);con.executescript('''
 CREATE TABLE asset(id INTEGER PRIMARY KEY,path TEXT UNIQUE,disposal TEXT);
 CREATE TABLE entity(id INTEGER PRIMARY KEY,kind TEXT,canonical_name TEXT,normalized_name TEXT,metadata_json TEXT,created_at TEXT,updated_at TEXT);
 CREATE TABLE asset_entity(id INTEGER PRIMARY KEY,asset_id INTEGER,entity_id INTEGER,role TEXT,source TEXT,confidence REAL,metadata_json TEXT,first_seen_at TEXT,last_seen_at TEXT);
 CREATE TABLE entity_alias(entity_id INTEGER,alias TEXT);''')
 con.executemany("INSERT INTO entity VALUES(?,'creator',?,?, '{}','fixture','fixture')",[(1,'甲','甲'),(2,'乙','乙'),(3,'空人物','空人物')])
 for i,file in enumerate([creators/'甲/已归档.mp4',creators/'甲/未扫描.mp4',creators/'甲/占用.mp4',creators/'乙/保留.mp4',incoming],1):
  con.execute('INSERT INTO asset VALUES(?,?,NULL)',(i,win(file)));con.execute("INSERT INTO asset_entity(asset_id,entity_id,role,source,confidence,metadata_json,first_seen_at,last_seen_at) VALUES(?,?,'creator','fixture',1,'{}','fixture','fixture')",(i,2 if i==4 else 1))
 con.commit();con.close()
 sys.path.insert(0,str(Path(__file__).resolve().parents[1]/'maintenance'))
 from processing_store import connect
 con=connect(base)
 con.execute("INSERT INTO tasks(vid,path,name,size,mtime_ns,stage,batch_id,created_at,updated_at,data) VALUES(?,?,?,?,?,'face','fixture',0,0,?)",(pending,str(incoming),incoming.name,len(sample),str(incoming.stat().st_mtime_ns),json.dumps({'suggested':'甲','candidates':[{'name':'甲'}]})))
 con.execute("INSERT INTO tasks(vid,path,name,size,mtime_ns,stage,batch_id,created_at,updated_at,data) VALUES('unrelated','unrelated.mp4','unrelated.mp4',1,'0','face','fixture',0,0,?)",(json.dumps({'suggested':'乙'}),));con.commit();con.close()
 wrapper=root/'python-wrapper'
 wrapper.write_text('#!'+sys.executable+'\n'+r"""import sys,subprocess,re,base64,json,time
from pathlib import Path
p=json.loads(base64.b64decode(''.join(sys.argv[3:])))
root=Path(__file__).parent
if p.get('action')=='apply' and (root/'delay-apply-start').exists():
 (root/'delay-apply-start').unlink()
 log=(root/'delayed-apply.log').open('wb')
 child="import sys,time,subprocess;time.sleep(1);sys.exit(subprocess.call(sys.argv[1:]))"
 subprocess.Popen([sys.executable,'-c',child,sys.executable,*sys.argv[1:]],stdout=log,stderr=log,start_new_session=True)
 sys.exit(1)
r=subprocess.run([sys.executable,*sys.argv[1:]],capture_output=True)
flag=root/'lose-apply-response'
if r.returncode==0 and p.get('action')=='apply' and flag.exists():
 flag.unlink();sys.exit(1)
sys.stdout.buffer.write(r.stdout);sys.stderr.buffer.write(r.stderr);sys.exit(r.returncode)
""")
 wrapper.chmod(0o755);print(root);raise SystemExit

assert (root/'.fileops-fixture').is_file()
url='http://127.0.0.1:8922'
def call(endpoint,body=None,status=200):
 req=urllib.request.Request(url+endpoint,data=json.dumps(body).encode()if body is not None else None,headers={'Content-Type':'application/json'})
 try:
  with urllib.request.urlopen(req,timeout=60)as response:
   assert response.status==status;return json.load(response)
 except urllib.error.HTTPError as e:
  data=json.load(e);assert e.code==status,(e.code,data);return data
def library(name='甲'):return call('/api/persons/library?name='+urllib.parse.quote(name))
def item(name,person='甲'):
 f=next(f for f in library(person)['files']if f['name']==name);return {k:f[k]for k in ['vid','path','version']}
def preview(action,files=None,person='甲',status=200,**kwargs):return call('/api/persons/manage',dict(mode='preview',request=dict(action=action,person=person,**({'files':files}if files is not None else{}),**kwargs)),status)
def apply(plan,status=200):return call('/api/persons/manage',dict(mode='apply',request=plan['request'],version=plan['version']),status)
def sql(statement,args=()):
 with sqlite3.connect(ledger)as con:return con.execute(statement,args).fetchall()
assert library()['root']==str(creators/'甲'),'refuse mutations against non-fixture server'
original=file_hash(creators/'甲/未扫描.mp4')
preview('rename-file',[item('保留.mp4','乙')],filename='outside.mp4',status=400)
link=creators/'甲/链接';link.symlink_to(creators/'乙',target_is_directory=True)
preview('rename-file',[dict(vid='irrelevant',path=str(link/'保留.mp4'),version='irrelevant')],filename='link.mp4',status=409)
preview('rename-person',new_name='符号链接不应改名',status=409)
link.unlink()
preview('rename-file',[item('未扫描.mp4')],filename='../escape.mp4',status=400)
preview('rename-file',[item('未扫描.mp4')],filename='改后.txt',status=400)
preview('rename-file',[item('未扫描.mp4')],filename='占用.mp4',status=409)
preview('move-files',[item('未扫描.mp4')],target_person='乙',target_folder='../escape',status=400)
stale=preview('rename-file',[item('未扫描.mp4')],filename='stale.mp4')
file=creators/'甲/未扫描.mp4';data=file.read_bytes();file.write_bytes(data+b'changed');apply(stale,409);file.write_bytes(data)
late=preview('rename-file',[item('未扫描.mp4')],filename='late.mp4');(creators/'甲/late.mp4').write_bytes(b'other');apply(late,409);assert (creators/'甲/late.mp4').read_bytes()==b'other';(creators/'甲/late.mp4').unlink()
print('PASS path and extension validation, collisions, stale source, no overwritten destination')
plan=preview('rename-file',[item('未扫描.mp4')],filename='新文件.mp4');result=apply(plan);assert apply(plan)['id']==result['id']
assert not file.exists()and file_hash(creators/'甲/新文件.mp4')==original
assert sql('SELECT path FROM asset WHERE id=2')[0][0]==win(creators/'甲/新文件.mp4')
stable=item('新文件.mp4')['vid'];assert stable.startswith('file:')
req=urllib.request.Request(url+'/api/video?p='+urllib.parse.quote(stable),headers={'Range':'bytes=0-15'})
with urllib.request.urlopen(req)as response:assert response.status==206 and len(response.read())==16
plan=preview('move-files',[item('已归档.mp4')],target_person='乙');apply(plan)
assert not(creators/'甲/已归档.mp4').exists()and(creators/'乙/已归档.mp4').is_file()
assert sql("SELECT entity_id FROM asset_entity WHERE asset_id=1 AND role='creator'")==[(2,)]
with sqlite3.connect(base/'faces.db')as con:assert con.execute('SELECT path FROM videos WHERE vid=?',(known,)).fetchone()[0]==str(creators/'乙/已归档.mp4')
assert read('archive-history.json')[0]['creator']=='乙'
assert read('archive-history.json')[0]['win_dst']==win(creators/'乙/已归档.mp4')
assert sql('SELECT entity_id FROM asset_entity WHERE asset_id=5')==[(1,)]
print('PASS unindexed rename, idempotent retry, stable playback, move updates faces/history/Peach while unrelated links remain')
plan=preview('delete-files',[item('新文件.mp4')]);apply(plan);staged=Path(plan['files'][0]['dst'])
assert staged.is_file()and file_hash(staged)==original and not(creators/'甲/新文件.mp4').exists()
assert sql('SELECT path FROM asset WHERE id=2')[0][0]==win(staged)
assert stable not in[v['vid']for v in library()['recommendations']]
call('/api/plan/queue',dict(creator='甲',vids=[stable],source='person',source_id='甲'),409)
preview('rename-person',new_name='乙',status=409)
plan=preview('rename-person',new_name='新甲');apply(plan)
assert not(creators/'甲').exists()and(creators/'新甲/空子目录').is_dir()and(creators/'新甲/说明.txt').read_text()=='伴随人物目录一起保留'
assert library('甲')['name']=='新甲'
assert '甲'not in[p['name']for p in call('/api/persons/library')['persons']]
assert read('integration-queue.json')[0]['creator']=='新甲'
assert read('integration-queue.json')[0]['return_href']=='/persons/'+urllib.parse.quote('新甲')
assert read('watermarks/mappings.json')['account']['creator']=='新甲'
assert '新甲'in read('template-overrides.json')and'甲'not in read('template-overrides.json')
assert read('people-manual.json')[0]['name']=='新甲'
assert all(r['name']=='新甲'for r in read('person-rejections.json').values())
assert sql('SELECT canonical_name FROM entity WHERE id=1')==[('新甲',)]
with sqlite3.connect(base/'processing/processing.db')as con:
 stage,raw=con.execute('SELECT stage,data FROM tasks WHERE vid=?',(pending,)).fetchone();assert stage=='queued'and json.loads(raw)['suggested']=='新甲'
 assert con.execute("SELECT stage,updated_at,data FROM tasks WHERE vid='unrelated'").fetchone()==('face',0,json.dumps({'suggested':'乙'}))
plan=preview('rename-person',person='空人物',new_name='空人物新名');apply(plan);assert(creators/'空人物新名').is_dir()
plan=preview('rename-person',person='空人物新名',new_name='空人物');apply(plan);assert(creators/'空人物').is_dir()and library('空人物新名')['name']=='空人物'
print('PASS deletion preserves bytes in staging and cannot requeue; person rename updates directory, references, pending work and old-link resolution')
# A real ledger trigger fails after local file and metadata writes, exercising compensation.
sql("CREATE TRIGGER injected_failure BEFORE UPDATE OF path ON asset WHEN old.id=3 BEGIN SELECT RAISE(ABORT,'injected ledger failure'); END")
before={f.name:f.read_bytes()for f in base.glob('*.json')}
plan=preview('rename-file',[item('占用.mp4','新甲')],person='新甲',filename='应回退.mp4');apply(plan,500)
assert(creators/'新甲/占用.mp4').exists()and not(creators/'新甲/应回退.mp4').exists()
assert all((base/name).read_bytes()==raw for name,raw in before.items())
assert not(base/'.review-write.lock').exists()
sql('DROP TRIGGER injected_failure')
# Interpreter wrapper discards one successful apply response. Ledger receipt must resolve ambiguity.
(root/'lose-apply-response').touch()
plan=preview('rename-file',[item('占用.mp4','新甲')],person='新甲',filename='已确认提交.mp4');apply(plan)
assert(creators/'新甲/已确认提交.mp4').exists()and not(root/'lose-apply-response').exists()
assert not(base/'.review-write.lock').exists()
print('PASS ledger failure restores local files/JSON/DB; lost commit response is reconciled through the ledger receipt')

# A request that starts late must be fenced off before local files are rolled back.
(root/'delay-apply-start').touch()
plan=preview('rename-file',[item('已确认提交.mp4','新甲')],person='新甲',filename='迟到请求.mp4');apply(plan,500)
time.sleep(1.5)
assert(creators/'新甲/已确认提交.mp4').exists()and not(creators/'新甲/迟到请求.mp4').exists()
assert sql('SELECT path FROM asset WHERE id=3')==[(win(creators/'新甲/已确认提交.mp4'),)]
assert sql('SELECT state FROM _face_admin_person_ops WHERE id=?',(plan['id'],))==[('aborted',)]
assert not(base/'.review-write.lock').exists()
print('PASS delayed remote execution is fenced before local rollback; it cannot commit after cancellation')

# A failed person rename must restore every mutable person reference, not just filenames.
sql("CREATE TRIGGER injected_person_failure BEFORE UPDATE OF canonical_name ON entity WHEN old.id=1 BEGIN SELECT RAISE(ABORT,'injected person failure'); END")
refs={str(f.relative_to(base)):f.read_bytes()for f in base.rglob('*.json') if 'person-operations'not in f.relative_to(base).parts}
plan=preview('rename-person',person='新甲',new_name='应回退人物');apply(plan,500)
assert(creators/'新甲').is_dir()and not(creators/'应回退人物').exists()
assert all((base/name).read_bytes()==raw for name,raw in refs.items())
assert sql('SELECT canonical_name FROM entity WHERE id=1')==[('新甲',)]
assert not(base/'.review-write.lock').exists()
sql('DROP TRIGGER injected_person_failure')
print('PASS a failed whole-person rename restores the directory, mappings, queue, overrides and rejection keys')

# Hard-linked entries remain distinct files in a bulk operation; link-count changes are not content changes.
os.link(creators/'新甲/已确认提交.mp4',creators/'新甲/硬链接副本.mp4')
plan=preview('delete-files',[item('已确认提交.mp4','新甲'),item('硬链接副本.mp4','新甲')],person='新甲');apply(plan)
assert all(Path(file['dst']).is_file()and not Path(file['src']).exists()for file in plan['files'])
assert not(base/'.review-write.lock').exists()
print('PASS bulk deletion handles hard-linked files without false fingerprint conflicts')
