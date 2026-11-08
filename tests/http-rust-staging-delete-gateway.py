"""Public Next #16 adapter and stable playback test; requires isolated seeded fixture + servers."""
import json,sqlite3,urllib.request,urllib.parse,urllib.error,uuid,os
from pathlib import Path
root=Path(os.environ.get('STAGING_GATEWAY_FIXTURE','/tmp/face-admin-person-fileops-staging-16')).resolve();assert root.parent==Path('/tmp').resolve() and root.name.startswith('face-admin-person-fileops-staging-16');assert (root/'.fileops-fixture').read_text()=='person file operations'
url='http://127.0.0.1:19002';base=root/'base';media=root/'media'
def call(route,body=None,status=200):
    req=urllib.request.Request(url+route,data=None if body is None else json.dumps(body).encode(),headers={'Content-Type':'application/json'})
    try:
        with urllib.request.urlopen(req,timeout=120) as r:code=r.status;result=json.load(r)
    except urllib.error.HTTPError as e:code=e.code;result=json.load(e)
    assert code==status,(route,code,result)
    return result
def picked(row):return {k:row[k] for k in ('vid','path','version')}
def play(vid,content):
    req=urllib.request.Request(url+'/api/video?'+urllib.parse.urlencode({'p':vid}),headers={'Range':'bytes=0-63'})
    with urllib.request.urlopen(req) as r:assert r.status==206 and r.read()==content[:64]
files=call('/api/persons/library?name='+urllib.parse.quote('甲'))['files'];selected=[f for f in files if f['name'] in ('已归档.mp4','未扫描.mp4')];assert len(selected)==2
content={f['name']:Path(f['path']).read_bytes() for f in selected}
plan=call('/api/persons/manage',{'mode':'preview','request':{'action':'delete-files','person':'甲','files':[picked(f) for f in selected]}})
call('/api/persons/manage',{'mode':'apply','request':plan['request'],'version':plan['version']})
rows=call('/api/staging')['rows'];assert len(rows)==2
for row in rows:play(row['vid'],content[row['name']])
restored=call('/api/staging',{'mode':'preview','request':{'action':'restore-files','person':'','files':[picked(rows[0])]}})
call('/api/staging',{'mode':'apply','request':restored['request'],'version':restored['version']});play(rows[0]['vid'],content[rows[0]['name']])
remaining=call('/api/staging')['rows'];deleted=call('/api/staging',{'mode':'preview','request':{'action':'delete-staged','person':'','files':[picked(remaining[0])]}})
body={'mode':'apply','request':deleted['request'],'version':deleted['version']};call('/api/staging',body,400)
result=call('/api/staging',{**body,'confirmation':'永久删除'});assert result['completed']==1
assert call('/api/staging',{**body,'confirmation':'永久删除'})==result
assert call('/api/staging?id='+deleted['id'])['result']==result
assert call('/api/staging?id='+plan['id'])['status']=='complete'
# Canonical duplicate aliases and Range survive permanent disposal of a same-inode extra link.
canonical=Path(restored['files'][0]['dst']);duplicate=media/'pikpak/.face-cleanup-staging/duplicate.mp4';os.link(canonical,duplicate)
history=json.loads((base/'archive-history.json').read_text());m=next(h for h in history if h['vid']==rows[0]['vid']);m['staged_path']=str(duplicate);(base/'archive-history.json').write_text(json.dumps(history))
origins=json.loads((base/'staging-origins.json').read_text());origins[str(duplicate)]={'source':'native-receipt','action':'stage-duplicate','original_path':str(canonical),'vids':[rows[0]['vid']]};(base/'staging-origins.json').write_text(json.dumps(origins))
clear=call('/api/staging',{'mode':'preview','request':{'action':'clear-staging','person':'','files':[]}})
assert len(clear['files'])==1 and not clear['files'][0]['vids']
call('/api/staging',{'mode':'apply','request':clear['request'],'version':clear['version'],'confirmation':'永久删除'},400)
call('/api/staging',{'mode':'apply','request':clear['request'],'version':clear['version'],'confirmation':'清空回收站'})
play(rows[0]['vid'],content[rows[0]['name']]);assert canonical.exists() and not duplicate.exists()
call('/api/delete',{'vids':[rows[0]['vid']]},410)
with sqlite3.connect(root/'ledger.db') as db:assert not db.execute('PRAGMA foreign_key_check').fetchall()
# Leave two genuine sample files for keyboard/search/multiselect/playback browser acceptance.
staging=media/'pikpak/.face-cleanup-staging';(staging/'浏览器样片.mp4').write_bytes(content[rows[0]['name']]);(staging/'保留样片.mp4').write_bytes(content[rows[0]['name']])
print('PASS Next delete/list/restore/explicit delete+clear/replay/receipts/stable canonical Range/obsolete bypass; real samples ready')
