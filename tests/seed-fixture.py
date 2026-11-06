"""Create disposable data only under /tmp/face-admin-v10-fixture; never use live media."""
from pathlib import Path
import json,sqlite3,shutil
root=Path('/tmp/face-admin-v10-fixture')
root.mkdir(exist_ok=True)
(root/'.fixture-marker').write_text('face-admin test data')
base=root/'base';media=root/'media';ledger=root/'ledger'
for p in [base,media,ledger]:
 p.mkdir(exist_ok=True)
 for child in p.iterdir():
  if child.is_dir():shutil.rmtree(child)
  else:child.unlink()
video=Path('/tmp/face-admin-fixture.mp4').read_bytes()

def vid(stem):return 'R:\\media\\pikpak\\My Telegram\\test\\'+stem+'.mp4'
names=['a','b','review','keep','conflict']
for n in names:
 p=media/'pikpak'/'My Telegram'/'test'/(n+'.mp4');p.parent.mkdir(parents=True,exist_ok=True);p.write_bytes(video)
p=media/'pikpak'/'creators'/'娜娜'/'conflict.mp4';p.parent.mkdir(parents=True,exist_ok=True);p.write_bytes(b'not the same file')
def entry(n,conf='ok'):return dict(vid=vid(n),name=n+'.mp4',size=len(video),duration=1,thumb=None,conf=conf)
def write(n,d):(base/n).write_text(json.dumps(d,ensure_ascii=False))
write('clusters.json',[dict(cid='p1',n_videos=2,total_gb=0,n_faces=5,rep_thumb=None,collections=[dict(name='My Telegram/test',n=2)],vids=[entry('a'),entry('b','weak')]),dict(cid='p2',n_videos=1,total_gb=0,n_faces=3,rep_thumb=None,collections=[dict(name='My Telegram/test',n=1)],vids=[entry('review')])])
write('report.json',dict(stats=dict(scanned=5,claimed_n=2,claimed_gb=0),identities=[dict(name='娜娜',claimed_n=2,claimed_gb=0,tpl_faces=3,rep_thumb=None,vids=[entry('a'),entry('b')]),dict(name='其他人',claimed_n=0,claimed_gb=0,tpl_faces=3,rep_thumb=None,vids=[])]))
write('claims-detail.json',[dict(vid=vid('review'),name='娜娜',s=0.55,n=2,zone='MyTG')])
write('integration-queue.json',[dict(vid=vid(n),creator='娜娜',at='fixture')for n in ['keep','missing','conflict']])
write('review-decisions.json',dict(approved=[],rejected=[]))
con=sqlite3.connect(base/'faces.db');con.execute('CREATE TABLE videos (vid TEXT PRIMARY KEY,path TEXT)');con.executemany('INSERT INTO videos VALUES (?,?)',[(vid(n),'/fixture-media/pikpak/My Telegram/test/'+n+'.mp4')for n in names]);con.execute('CREATE TABLE faces (vid TEXT,thumb TEXT,score REAL)');con.execute('INSERT INTO faces VALUES (?,NULL,1)',(vid('review'),));con.commit();con.close()
con=sqlite3.connect(ledger/'ledger.db')
con.executescript('''CREATE TABLE asset(id INTEGER PRIMARY KEY,path TEXT,disposal TEXT);
CREATE TABLE entity(id INTEGER PRIMARY KEY,kind TEXT,canonical_name TEXT,normalized_name TEXT,metadata_json TEXT,created_at TEXT,updated_at TEXT);
CREATE TABLE entity_alias(entity_id INTEGER,alias TEXT);
CREATE TABLE asset_entity(asset_id INTEGER,entity_id INTEGER,role TEXT,source TEXT,confidence REAL,metadata_json TEXT,first_seen_at TEXT,last_seen_at TEXT);''')
# b deliberately has no ledger row, for the auto-restore regression.
con.executemany('INSERT INTO asset(path) VALUES (?)',[(vid(n),)for n in ['a','review','keep','conflict']]);con.commit();con.close()
print('seeded isolated fixture')
