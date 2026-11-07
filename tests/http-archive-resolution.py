"""Destructive tests ONLY against disposable /tmp fixtures and port 8922."""
import json,sys,urllib.request,urllib.error,sqlite3,os
from pathlib import Path
URL=sys.argv[1] if len(sys.argv)>1 else 'http://127.0.0.1:8922'
assert URL.endswith(':8922')
root=Path('/tmp/face-admin-v10-fixture');assert (root/'.fixture-marker').read_text()=='face-admin test data'
media=root/'media';base=root/'base';ledger=root/'ledger/ledger.db'
def req(path,body=None,expected=200):
 r=urllib.request.Request(URL+path,data=json.dumps(body).encode() if body is not None else None,headers={'Content-Type':'application/json'})
 try:
  with urllib.request.urlopen(r,timeout=60) as response:code=response.status;data=json.load(response)
 except urllib.error.HTTPError as e:code=e.code;data=json.load(e)
 assert code==expected,(path,code,data)
 return data
def vid(n):return 'R:\\media\\pikpak\\My Telegram\\test\\'+n+'.mp4'
def dest(n):return 'R:\\media\\pikpak\\creators\\娜娜\\'+n+'.mp4'
def srcfile(n):return media/'pikpak/My Telegram/test'/(n+'.mp4')
def dstfile(n):return media/'pikpak/creators/娜娜'/(n+'.mp4')
def create(n,a=b'source-video',b=None,canonical=True,source_ledger=True):
 srcfile(n).write_bytes(a);dstfile(n).write_bytes(a if b is None else b)
 con=sqlite3.connect(ledger)
 if source_ledger:con.execute('INSERT INTO asset(path) VALUES (?)',(vid(n),))
 if canonical:con.execute('INSERT INTO asset(path) VALUES (?)',(dest(n),))
 con.commit();con.close()
 req('/api/plan/queue',dict(creator='娜娜',vids=[vid(n)]))
def plan():return req('/api/plan/preview')
def row(n):return next(r for r in plan()['rows'] if r['vid']==vid(n))
def resolve(names,kind,**kw):return req('/api/plan/resolve',dict(vids=[vid(n) for n in names],version=plan()['version'],kind=kind,**kw))
def apply(names):return req('/api/plan/apply',dict(vids=[vid(n) for n in names],version=plan()['version']))

create('rename',b'long-source-content',b'short-target')
req('/api/plan/resolve',dict(vids=[vid('rename')],version=plan()['version'],kind='stage-duplicate'),409)
req('/api/plan/resolve',dict(vids=[vid('rename')],version=plan()['version'],kind='rename',name='../outside.mp4'),400)
resolve(['rename'],'rename');r=row('rename');assert r['status']=='ok' and r['dst'].endswith('.mp4')
assert srcfile('rename').read_bytes()==b'long-source-content' and dstfile('rename').read_bytes()==b'short-target'
resolve(['rename'],'clear');assert row('rename')['status']=='conflict';assert srcfile('rename').exists()
resolve(['rename'],'rename');r=row('rename');result=apply(['rename']);assert result['moved']==1 and not result['errors'],result
renamed=Path(str(r['dst']).replace('/fixture-media',str(media),1))
assert renamed.read_bytes()==b'long-source-content' and dstfile('rename').read_bytes()==b'short-target'
assert not srcfile('rename').exists()
print('PASS keep both, safe names, defer, explicit physical apply; canonical never overwritten')

for n in ['dup-a','dup-b']:create(n)
old=plan()['version'];resolve(['dup-a','dup-b'],'stage-duplicate')
assert all(srcfile(n).exists() for n in ['dup-a','dup-b'])
req('/api/plan/apply',dict(vids=[vid('dup-a')],version=old),409)
r=apply(['dup-a','dup-b']);assert r['moved']==r['staged']==2 and not r['errors'],r
hist={r['vid']:r for r in plan()['history']}
con=sqlite3.connect(ledger)
for n in ['dup-a','dup-b']:
 assert not srcfile(n).exists() and dstfile(n).read_bytes()==b'source-video'
 archived=hist[vid(n)];staged=Path(archived['staged_path'].replace('/fixture-media',str(media),1))
 assert staged.read_bytes()==b'source-video' and archived['win_dst']==dest(n)
 source=con.execute('SELECT path FROM asset WHERE path LIKE ?',('%archive-duplicates%'+n+'.mp4',)).fetchall();assert len(source)==1
 assert con.execute('SELECT COUNT(*) FROM asset WHERE path=?',(dest(n),)).fetchone()[0]==1
con.close()
print('PASS bulk exact duplicates staged reversibly, canonical preserved, ledger and history match actual paths')

create('false-duplicate',b'video-one',b'video-two');resolve(['false-duplicate'],'stage-duplicate')
r=apply(['false-duplicate']);assert r['moved']==0 and any('校验不一致' in e for e in r['errors']),r
assert srcfile('false-duplicate').read_bytes()==b'video-one' and dstfile('false-duplicate').read_bytes()==b'video-two'
assert row('false-duplicate')['status']=='ok'
print('PASS equal byte size with different content is never removed or moved')

create('rollback-duplicate',canonical=False);resolve(['rollback-duplicate'],'stage-duplicate')
r=apply(['rollback-duplicate']);assert r['moved']==0 and r['errors'],r
assert srcfile('rollback-duplicate').read_bytes()==dstfile('rollback-duplicate').read_bytes()==b'source-video'
manifest=json.loads(Path(r['manifest'].replace('/fixture-base',str(base),1)).read_text());assert manifest['status']=='failed'
assert row('rollback-duplicate')['status']=='ok'
print('PASS failed canonical ledger preflight leaves source untouched and task pending')

create('changed-target');resolve(['changed-target'],'stage-duplicate');p=plan()
dstfile('changed-target').write_bytes(b'source-video');s=dstfile('changed-target').stat();os.utime(dstfile('changed-target'),(s.st_atime,s.st_mtime+10))
req('/api/plan/apply',dict(vids=[vid('changed-target')],version=p['version']),409)
assert srcfile('changed-target').exists()
print('PASS destination changes invalidate the confirmation; all unrelated tasks retained')
print('ALL ARCHIVE RESOLUTION REGRESSIONS PASSED')
