"""Real API regression on the isolated fixture, including durable state and naming guards."""
import json
import os
from pathlib import Path
import sqlite3
import sys
import time
import urllib.request
import urllib.error
import urllib.parse

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'maintenance'))
from processing_store import connect, reconcile

ROOT=Path(os.environ.get('PROCESSING_FIXTURE_ROOT','/tmp/face-admin-v10-fixture'))
PORT=int(os.environ.get('PROCESSING_TEST_PORT','8922'))
assert PORT in (8922,8932) and str(ROOT).startswith('/tmp/face-') and (ROOT/'.fixture-marker').exists()
BASE=f'http://127.0.0.1:{PORT}'
def request(route, body=None, method=None, expected=200):
    req=urllib.request.Request(BASE+route,data=json.dumps(body).encode()if body is not None else None,headers={'Content-Type':'application/json'},method=method)
    try:
        with urllib.request.urlopen(req,timeout=40)as response:status,data=response.status,json.load(response)
    except urllib.error.HTTPError as error:status,data=error.code,json.load(error)
    assert status==expected,(route,status,data)
    return data
def vid(stem):return 'R:\\media\\pikpak\\My Telegram\\test\\'+stem+'.mp4'

media=ROOT/'media/pikpak/My Telegram/test';error_file=media/'pipeline-error.mp4';error_file.write_bytes((media/'a.mp4').read_bytes())
faces=sqlite3.connect(ROOT/'base/faces.db');faces.execute('INSERT OR REPLACE INTO videos VALUES (?,?)',(vid('pipeline-error'),str(error_file)));faces.commit();faces.close()
db=connect(ROOT/'base');items=[]
for name in ['a','b','review','pipeline-error']:
    file=media/(name+'.mp4');st=file.stat();items.append(dict(vid=vid(name),path=str(file),name=file.name,size=st.st_size,mtime_ns=str(st.st_mtime_ns)))
with db:
    reconcile(db,items,'fixture',False)
    for item in items:
        name=item['name'];kind='suggested'if name=='a.mp4'else'conflict'if name=='b.mp4'else'none'
        candidates=[]if kind=='none'else[dict(name='Alice',sources=['face'],score=.65,evidence=[])]
        if kind=='conflict':candidates.append(dict(name='Bob',sources=['folder'],score=None,evidence=[]))
        data=dict(kind=kind,suggested='Alice'if kind=='suggested'else None,candidates=candidates,ready=True)
        db.execute('UPDATE tasks SET stage=?,data=?,text_attempts=3 WHERE vid=?',('error'if name=='pipeline-error.mp4'else'review',json.dumps(data),item['vid']))
db.close()
before=json.loads((ROOT/'base/integration-queue.json').read_text())
video_url=BASE+'/api/video?'+urllib.parse.urlencode({'p':str(media/'a.mp4')})
with urllib.request.urlopen(urllib.request.Request(video_url,headers={'Range':'bytes=0-0'}))as response:
    assert response.status==206 and len(response.read())==1
outside=ROOT/'outside.bin';outside.write_bytes(b'not media');link=media/'escape.mp4';link.symlink_to(outside)
try:
    urllib.request.urlopen(BASE+'/api/video?'+urllib.parse.urlencode({'p':str(link)}))
    raise AssertionError('Media-root symlink escape must be forbidden')
except urllib.error.HTTPError as error:assert error.code==403
link.unlink()
ops=request('/api/processing');assert ops['counts']['review']==3 and ops['counts']['error']==1
request('/api/processing',dict(action='pause'));assert request('/api/processing')['paused'] is True
request('/api/processing',dict(action='resume'));assert request('/api/processing')['paused'] is False
request('/api/processing',dict(action='priority',vids=[vid('review')]))
request('/api/processing',dict(action='retry',vids=[vid('a')]),expected=409)
review=request('/api/review');assert review['counts']['all']==3 and sum(g['count']for g in review['groups'])==3
assert request('/api/review?person=NotInLibrary')['total']==0
request('/api/processing',dict(action='takeover',vids=[vid('pipeline-error')]))
assert request('/api/review')['counts']['manual']==1
request('/api/review',dict(action='defer',vids=[vid('a')]))
assert request('/api/review')['counts']['deferred']==1
request('/api/review',dict(action='restore',vids=[vid('a')]))
request('/api/review',dict(action='note',vids=[vid('a')],note='另有其他出镜人物'))
fingerprint={k:items[0][k]for k in ['path','size','mtime_ns']}
body=dict(vids=[vid('a')],creator='Alice',source='review',fingerprints={vid('a'):fingerprint})
request('/api/plan/queue',{**body,'fingerprints':{}},expected=409)
request('/api/plan/queue',body)
queue=json.loads((ROOT/'base/integration-queue.json').read_text());added=next(v for v in queue if v['vid']==vid('a'))
assert added['note']=='另有其他出镜人物'and added['return_href']=='/review'
assert all(v in queue for v in before)
assert request('/api/review')['counts']['all']==3
# Simulate the discovery pass excluding a now-queued task, then test undo restoration.
db=connect(ROOT/'base')
with db:db.execute("UPDATE tasks SET stage='excluded' WHERE vid=?",(vid('a'),))
db.close()
request('/api/plan/queue',dict(vids=[vid('a')]),method='DELETE')
assert request('/api/review')['counts']['all']==4
(media/'a.mp4').write_bytes((media/'a.mp4').read_bytes()+b'replaced')
request('/api/plan/queue',body,expected=409)
print('PASS pipeline controls, candidate grouping, explicit takeover, persistent defer/note, fingerprint guard, queue preservation and undo restoration')
