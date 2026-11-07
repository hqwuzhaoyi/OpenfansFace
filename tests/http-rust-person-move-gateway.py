"""Public Next -> Rust person move acceptance, including real video Range through stable IDs.
Seed using http-file-operation-backend.py --seed --native-ledger; serve only that marked fixture.
"""
import argparse,hashlib,json,sqlite3,urllib.request,urllib.parse,urllib.error
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--root',type=Path,required=True);p.add_argument('--url',default='http://127.0.0.1:18984');args=p.parse_args()
root=args.root.resolve();assert root.parent==Path('/tmp').resolve() and root.name.startswith('face-admin-person-fileops-')
assert (root/'.fileops-fixture').read_text()=='person file operations'
url=urllib.parse.urlparse(args.url);assert url.hostname=='127.0.0.1' and url.port==18984
base=root/'base';creators=root/'media/pikpak/creators'
def call(route,body=None,status=200):
    req=urllib.request.Request(args.url+route,data=None if body is None else json.dumps(body).encode(),headers={'Content-Type':'application/json'})
    try:
        with urllib.request.urlopen(req,timeout=90) as r:code=r.status;result=json.load(r)
    except urllib.error.HTTPError as e:code=e.code;result=json.load(e)
    assert code==status,(route,code,result)
    return result
def listing(person):return call('/api/persons/library?name='+urllib.parse.quote(person))
def preview(request):return call('/api/persons/manage',{'mode':'preview','request':request})
def apply(plan):return call('/api/persons/manage',{'mode':'apply','request':plan['request'],'version':plan['version']})
def chosen(row):return {k:row[k] for k in ('vid','path','version')}
def range_bytes(vid,expected):
    req=urllib.request.Request(args.url+'/api/video?'+urllib.parse.urlencode({'p':vid}),headers={'Range':'bytes=0-63'})
    with urllib.request.urlopen(req,timeout=30) as r:assert r.status==206 and r.read()==expected[:64]
source=listing('甲');assert source['root']==str(creators/'甲')
files=[next(f for f in source['files'] if f['name']==name) for name in ('已归档.mp4','未扫描.mp4')]
content={f['name']:Path(f['path']).read_bytes() for f in files};untouched=(creators/'甲/占用.mp4').read_bytes()
plan=preview({'action':'move-files','person':'甲','target_person':'乙','target_folder':'子目录/移动验收','files':[chosen(f) for f in files]})
assert plan['backend']=='rust' and len(plan['files'])==2
assert call('/api/persons/manage?id='+plan['id'])['backend']=='rust'
result=apply(plan);assert result['ok'] and result['changed']==2 and apply(plan)==result
for file in plan['files']:
    dst=Path(file['dst']);assert dst.read_bytes()==content[dst.name] and not Path(file['src']).exists()
    for vid in file['vids']:range_bytes(vid,dst.read_bytes())
assert (creators/'甲/占用.mp4').read_bytes()==untouched
new=listing('乙');rows={f['vid']:f for f in new['files']}
for file in plan['files']:
    for vid in file['vids']:assert rows[vid]['path']==file['dst']
synthetic=next(v for f in plan['files'] for v in f['vids'] if v.startswith('file:'))
renamed=preview({'action':'rename-file','person':'乙','filename':'保持播放编号.mp4','files':[chosen(rows[synthetic])]})
assert renamed['files'][0]['vids']==[synthetic];apply(renamed);range_bytes(synthetic,content['未扫描.mp4'])
with sqlite3.connect(root/'ledger.db') as db:
    assert db.execute("SELECT asset_id,entity_id FROM asset_entity WHERE asset_id IN (1,2) AND role='creator' ORDER BY asset_id").fetchall()==[(1,2),(2,2)]
    assert db.execute("SELECT entity_id FROM asset_entity WHERE asset_id=3 AND role='creator'").fetchall()==[(1,)]
    assert not db.execute('PRAGMA foreign_key_check').fetchall()
# The public stable link must also recover after an actual native ledger rollback.
current=next(f for f in listing('乙')['files'] if f['vid']==synthetic)
rollback=preview({'action':'move-files','person':'乙','target_person':'甲','target_folder':'回退验收','files':[chosen(current)]})
with sqlite3.connect(root/'ledger.db') as db:db.executescript("CREATE TRIGGER fixture_gateway_reject BEFORE UPDATE OF path ON asset WHEN old.id=2 BEGIN SELECT RAISE(ABORT,'gateway rollback fixture'); END;")
try:
    call('/api/persons/manage',{'mode':'apply','request':rollback['request'],'version':rollback['version']},503)
    assert call('/api/persons/manage?id='+rollback['id'])['status']=='rolled_back'
    assert Path(current['path']).read_bytes()==content['未扫描.mp4'] and not Path(rollback['files'][0]['dst']).exists()
    range_bytes(synthetic,content['未扫描.mp4'])
finally:
    with sqlite3.connect(root/'ledger.db') as db:db.execute('DROP TRIGGER fixture_gateway_reject')
assert call('/api/persons/manage?id='+plan['id'])['status']=='complete'
assert not (base/'.review-write.lock').exists()
print('PASS public Next person move/registration/repeated rename/real video Range/native schema/scope/result lookup/rollback playback')
