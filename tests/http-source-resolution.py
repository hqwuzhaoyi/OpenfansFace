"""Moved-file regression on isolated fixtures, including actual-path ledger remap."""
import sys,json,urllib.request,urllib.error,urllib.parse,sqlite3,shutil
from pathlib import Path
BASE=sys.argv[1]if len(sys.argv)>1 else'http://127.0.0.1:8922';assert BASE.endswith(':8922')
root=Path('/tmp/face-admin-v10-fixture');assert(root/'.fixture-marker').exists()
def req(p,body=None,method=None,expected=200):
 r=urllib.request.Request(BASE+p,data=json.dumps(body).encode()if body is not None else None,headers={'Content-Type':'application/json'},method=method)
 try:
  with urllib.request.urlopen(r,timeout=30)as response:code=response.status;data=json.load(response)
 except urllib.error.HTTPError as e:code=e.code;data=json.load(e)
 assert code==expected,(p,code,data)
 return data
legacy='R:\\media\\pikpak\\old-location\\original.mp4'
actual_win='R:\\media\\pikpak\\relocated\\renamed.mp4'
actual=root/'media/pikpak/relocated/renamed.mp4';actual.parent.mkdir(parents=True,exist_ok=True);shutil.copyfile('/tmp/face-admin-fixture.mp4',actual)
db=sqlite3.connect(root/'base/faces.db');db.execute('INSERT INTO videos VALUES (?,?)',(legacy,'/fixture-media/pikpak/relocated/renamed.mp4'));db.commit();db.close()
db=sqlite3.connect(root/'ledger/ledger.db');db.execute('INSERT INTO asset(path)VALUES(?)',(actual_win,));db.commit();db.close()
clusters=root/'base/clusters.json';rows=json.loads(clusters.read_text());rows.append(dict(cid='p3',n_videos=1,total_gb=0,rep_thumb=None,vids=[dict(vid=legacy,name='original.mp4',size=actual.stat().st_size,thumb=None)]));clusters.write_text(json.dumps(rows))
detail=req('/api/cluster/p3')['vids'][0];assert detail['path']=='/fixture-media/pikpak/relocated/renamed.mp4'and detail['name']=='renamed.mp4'
video=urllib.request.Request(BASE+'/api/video?p='+urllib.parse.quote(legacy),headers={'Range':'bytes=0-15'})
with urllib.request.urlopen(video,timeout=10)as r:assert r.status==206 and len(r.read())==16
req('/api/plan/queue',dict(creator='娜娜',vids=[legacy],source='cluster',source_id='p3'),'POST')
p=req('/api/plan/preview');row=next(r for r in p['rows']if r['vid']==legacy)
assert row['status']=='ok'and row['src']=='/fixture-media/pikpak/relocated/renamed.mp4',row
r=req('/api/plan/apply',dict(vids=[legacy],version=p['version']),'POST');assert r['moved']==1 and not r['errors'],r
assert not actual.exists()and(root/'media/pikpak/creators/娜娜/renamed.mp4').is_file()
db=sqlite3.connect(root/'ledger/ledger.db');assert db.execute('SELECT COUNT(*)FROM asset WHERE path=?',('R:\\media\\pikpak\\creators\\娜娜\\renamed.mp4',)).fetchone()[0]==1;db.close()
print('PASS moved video displays current path, streams, archives, and updates the actual Peach path')
folder=root/'media/pikpak/folder-not-video';folder.mkdir()
folder_vid='R:\\media\\pikpak\\does-not-exist\\folder.mp4'
db=sqlite3.connect(root/'base/faces.db');db.execute('INSERT INTO videos VALUES (?,?)',(folder_vid,'/fixture-media/pikpak/folder-not-video'));db.commit();db.close()
req('/api/plan/queue',dict(creator='娜娜',vids=[folder_vid],source='person',source_id='娜娜'),'POST',409)
assert folder.is_dir()
print('PASS directories cannot be queued or moved as videos')
