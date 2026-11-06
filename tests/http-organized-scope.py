"""Already-organized files are references, never eligible for curation or archive."""
import os,sys,json,urllib.request,urllib.error,urllib.parse,sqlite3,shutil
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
organized='R:\\media\\pikpak\\My Telegram\\historical-known.mp4'
unknown='R:\\media\\pikpak\\creators\\娜娜\\historical-unclassified.mp4'
other='R:\\media\\pikpak\\My Telegram\\wrong-suggestion.mp4'
files={organized:('creators/娜娜/known.mp4','娜娜'),unknown:('My Telegram/new/unclassified.mp4','娜娜'),other:('creators/其他人/other.mp4','其他人')}
db=sqlite3.connect(root/'base/faces.db')
for vid,(rel,name)in files.items():
 p=root/'media/pikpak'/rel;p.parent.mkdir(parents=True,exist_ok=True);shutil.copyfile('/tmp/face-admin-fixture.mp4',p)
 db.execute('INSERT INTO videos VALUES (?,?)',(vid,'/fixture-media/pikpak/'+rel))
db.commit();db.close()
def item(vid):return dict(vid=vid,name='old-name.mp4',size=len(Path('/tmp/face-admin-fixture.mp4').read_bytes()),thumb=None)
cs=root/'base/clusters.json';data=json.loads(cs.read_text());data.append(dict(cid='p4',vids=[item(organized),item(unknown),item(other)],n_videos=3,total_gb=0,rep_thumb=None));cs.write_text(json.dumps(data))
claims=root/'base/claims-detail.json';data=json.loads(claims.read_text());data.extend([dict(vid=v,name='娜娜',s=0.55,n=2)for v in files]);claims.write_text(json.dumps(data))
report=root/'base/report.json';data=json.loads(report.read_text());data['identities'][0]['vids'].extend(item(v)for v in files);report.write_text(json.dumps(data))
actual=req('/api/cluster/p4')['vids'];assert[v['vid']for v in actual]==[unknown],actual
wall=next(c for c in req('/api/clusters')if c['cid']=='p4');assert wall['n_videos']==1
pending=req('/api/claims/pending?limit=60')['claims'];pending_ids={c['vid']for c in pending};assert organized not in pending_ids and other not in pending_ids and unknown in pending_ids
print('PASS current creators location is excluded from cluster wall, cluster detail, and approvals')
req('/api/plan/queue',dict(creator='娜娜',vids=[organized],source='cluster',source_id='p4'),'POST',409)
req('/api/claims/decide',dict(vid=organized,approve=True),'POST',409)
print('PASS stale clients cannot queue or approve already-organized videos')
people=req('/api/identities')['identities'];nana=next(i for i in people if i['name']=='娜娜');byvid={v['vid']:v for v in nana['vids']}
assert byvid[organized]['state']=='archived' and byvid[organized]['organized']
assert byvid[unknown]['state']=='pending';assert other not in byvid
ref=req('/api/persons/reference?name='+urllib.parse.quote('娜娜'))
assert any(v['vid']==organized for v in ref['vids']) and all('/creators/娜娜/'in v['path']for v in ref['vids'])
ref_other=req('/api/persons/reference?name='+urllib.parse.quote('其他人'));assert any(v['vid']==other for v in ref_other['vids'])
print('PASS creator-folder truth is preserved in references and classified videos are not selectable')
req('/api/plan/queue',dict(creator='娜娜',vids=[unknown],source='cluster',source_id='p4'),'POST')
req('/api/plan/queue',dict(vids=[unknown]),'DELETE')
print('PASS historical creators ID remains eligible when its current file is genuinely unclassified')
qfile=root/'base/integration-queue.json';before=json.loads(qfile.read_text())
def write_queue(rows):
 temp=qfile.with_suffix('.test-tmp');temp.write_text(json.dumps(rows));os.replace(temp,qfile)
write_queue(before+[dict(vid=organized,creator='其他人',at='fixture')])
try:
 p=req('/api/plan/preview');row=next(r for r in p['rows']if r['vid']==organized);assert row['status']=='organized',row
 req('/api/plan/apply',dict(vids=[organized],version=p['version']),'POST',409)
finally:write_queue(before)
assert(root/'media/pikpak/creators/娜娜/known.mp4').is_file()
print('PASS already-organized files cannot move even from an old pending queue')
