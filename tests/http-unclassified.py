import json,urllib.request,urllib.error,sqlite3
from pathlib import Path
base='http://127.0.0.1:8922';root=Path('/tmp/face-admin-v10-fixture');assert(root/'.fixture-marker').exists()
def req(p,body=None,method=None,expected=200):
 r=urllib.request.Request(base+p,data=json.dumps(body).encode()if body is not None else None,headers={'Content-Type':'application/json'},method=method)
 try:
  with urllib.request.urlopen(r,timeout=30)as response:code=response.status;data=json.load(response)
 except urllib.error.HTTPError as e:code=e.code;data=json.load(e)
 assert code==expected,(code,data)
 return data
vid=lambda n:'R:\\media\\pikpak\\My Telegram\\test\\'+n+'.mp4'
f=root/'media/pikpak/My Telegram/test/orphan.mp4';f.write_bytes((f.parent/'a.mp4').read_bytes())
c=sqlite3.connect(root/'base/faces.db');c.executemany('INSERT INTO videos VALUES (?,?)',[(vid('orphan'),'/fixture-media/pikpak/My Telegram/test/orphan.mp4'),(vid('alias'),'/fixture-media/pikpak/My Telegram/test/orphan.mp4')]);c.commit();c.close()
d=req('/api/unclassified');rows=[v for v in d['videos']if v['path'].endswith('/orphan.mp4')];assert len(rows)==1 and rows[0]['uncovered'];assert len({v['path']for v in d['videos']})==d['total'];v=rows[0]['vid']
req('/api/plan/queue',dict(vids=[v],creator='新人物',source='all',source_id=''))
assert not any(x['path'].endswith('/orphan.mp4')for x in req('/api/unclassified')['videos'])
other=vid('alias')if v!=vid('alias')else vid('orphan');req('/api/plan/queue',dict(vids=[other],creator='新人物',source='all'),expected=409)
r=req('/api/plan/queue',dict(vids=[v]),method='DELETE');assert r['removed'][0]['return_href']=='/unclassified'
assert any(x['path'].endswith('/orphan.mp4')for x in req('/api/unclassified')['videos'])
assert f.exists()
print('PASS no-face orphan visible, physical aliases deduplicated, batch queue and undo, stale alias protection, file preserved')
