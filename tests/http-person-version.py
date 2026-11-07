"""Preview version regression against a dedicated, marked merge fixture on 18955."""
import json,sqlite3,urllib.request,urllib.error,threading,time,os
from pathlib import Path
root=Path('/tmp/face-admin-person-fileops-merge-version-20261107').resolve()
assert (root/'.fileops-fixture').exists()
base=root/'base';url='http://127.0.0.1:18955';creators=root/'media/pikpak/creators'
def call(endpoint,body,status=200):
 try:
  with urllib.request.urlopen(urllib.request.Request(url+endpoint,data=json.dumps(body).encode(),headers={'Content-Type':'application/json'}),timeout=60) as r:code=r.status;value=json.load(r)
 except urllib.error.HTTPError as e:code=e.code;value=json.load(e)
 assert code==status,(code,value)
 return value
def preview():return call('/api/persons/batch',{'mode':'preview','request':{'action':'merge','people':['甲','乙'],'target':'乙'}})
def repreview(p):return call('/api/persons/manage',{'mode':'preview','request':p['plans'][0]['request']})
def apply(p,status=200):return call('/api/persons/batch',{'mode':'apply','id':p['id'],'version':p['version']},status)
jobdb=base/'processing/processing.db'
with sqlite3.connect(jobdb) as c:
 c.execute("DELETE FROM tasks WHERE vid='fixture-unrelated-job'")
 c.row_factory=sqlite3.Row;row=dict(c.execute('SELECT * FROM tasks LIMIT 1').fetchone());row.update(vid='fixture-unrelated-job',path=str(root/'media/pikpak/My Telegram/unrelated.mp4'),name='unrelated.mp4',data=json.dumps({'candidates':[{'name':'无关人物'}]}),stage='face_queued')
 keys=list(row);c.execute('INSERT INTO tasks('+','.join(keys)+') VALUES('+','.join('?' for _ in keys)+')',list(row.values()))
with sqlite3.connect(base/'faces.db') as c:c.execute("DELETE FROM videos WHERE vid='fixture-unrelated-video'");c.execute('INSERT INTO videos(vid,path)VALUES(?,?)',('fixture-unrelated-video',str(root/'media/unrelated.mp4')))
p=preview()
with sqlite3.connect(jobdb) as c:c.execute("UPDATE tasks SET face_attempts=face_attempts+1,updated_at=?,data=?,stage='review' WHERE vid='fixture-unrelated-job'",(time.time(),json.dumps({'candidates':[{'name':'仍是无关人物'}]})))
with sqlite3.connect(base/'faces.db') as c:c.execute("UPDATE videos SET path=path||'.changed' WHERE vid='fixture-unrelated-video'")
assert repreview(p)['version']==p['plans'][0]['version'],'Unrelated background activity invalidated the preview'
print('PASS unrelated task progress and video indexing keep preview valid')
# Selected video indexing remains protected, even when archive history resolves the path.
ids=[vid for f in p['plans'][0]['files'] for vid in f['vids']]
with sqlite3.connect(base/'faces.db') as c:
 indexed=c.execute('SELECT vid,path FROM videos WHERE vid IN ('+','.join('?' for _ in ids)+') LIMIT 1',ids).fetchone();assert indexed
 c.execute('UPDATE videos SET path=? WHERE vid=?',(indexed[1]+'.changed',indexed[0]))
apply(p,409)
with sqlite3.connect(base/'faces.db') as c:c.execute('UPDATE videos SET path=? WHERE vid=?',(indexed[1],indexed[0]))
print('PASS affected video indexing still invalidates preview')
# Related candidate changes still invalidate the confirmation.
with sqlite3.connect(jobdb) as c:
 original=c.execute("SELECT vid,data FROM tasks WHERE vid!='fixture-unrelated-job' LIMIT 1").fetchone()
 c.execute('UPDATE tasks SET data=? WHERE vid=?',(json.dumps({'candidates':[{'name':'甲','score':.99}]}),original[0]))
apply(p,409)
with sqlite3.connect(jobdb) as c:c.execute('UPDATE tasks SET data=? WHERE vid=?',(original[1],original[0]))
print('PASS affected candidate metadata still requires a fresh preview')
p=preview()
with sqlite3.connect(root/'ledger.db') as c:c.execute("UPDATE entity SET metadata_json='{}' WHERE canonical_name='乙'")
apply(p,409);print('PASS related ledger edits invalidate preview')
p=preview();source=Path(p['plans'][0]['files'][0]['src']);old=source.read_bytes();source.write_bytes(old+b'changed')
apply(p,409);source.write_bytes(old);print('PASS actual media changes invalidate preview')
p=preview();stop=threading.Event();updates=[]
def background():
 with sqlite3.connect(jobdb,timeout=30) as c:
  while not stop.is_set():
   c.execute("UPDATE tasks SET updated_at=?,face_attempts=face_attempts+1 WHERE vid='fixture-unrelated-job'",(time.time(),));c.commit();updates.append(1);stop.wait(.03)
thread=threading.Thread(target=background);thread.start()
try:result=apply(p);assert result['ok'],result
finally:stop.set();thread.join(35)
assert not (creators/'甲').exists() and (creators/'乙').exists() and updates
with sqlite3.connect(jobdb) as c:assert c.execute("SELECT count(*) FROM tasks WHERE vid='fixture-unrelated-job'").fetchone()[0]==1
print('PASS merge succeeds during unrelated background updates; real protections preserved')
