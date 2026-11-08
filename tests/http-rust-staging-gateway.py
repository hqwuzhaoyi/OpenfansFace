"""Public Next staging acceptance. Run only on the explicitly marked dedicated #8 fixture."""
import json,sqlite3,urllib.request,urllib.parse,urllib.error,uuid
from pathlib import Path
root=Path('/tmp/face-admin-person-fileops-staging-08').resolve();assert (root/'.fileops-fixture').read_text()=='person file operations'
url='http://127.0.0.1:18986';base=root/'base'
def call(route,body=None,status=200):
    req=urllib.request.Request(url+route,data=None if body is None else json.dumps(body).encode(),headers={'Content-Type':'application/json'})
    try:
        with urllib.request.urlopen(req,timeout=120) as r:code=r.status;result=json.load(r)
    except urllib.error.HTTPError as e:code=e.code;result=json.load(e)
    assert code==status,(route,code,result)
    return result
def picked(row):return {k:row[k] for k in ('vid','path','version')}
def range_bytes(vid,expected):
    req=urllib.request.Request(url+'/api/video?'+urllib.parse.urlencode({'p':vid}),headers={'Range':'bytes=0-63'})
    with urllib.request.urlopen(req) as r:assert r.status==206 and r.read()==expected[:64]
files=call('/api/persons/library?name='+urllib.parse.quote('甲'))['files'];selected=[f for f in files if f['name'] in ('已归档.mp4','未扫描.mp4')]
assert len(selected)==2
content={f['name']:Path(f['path']).read_bytes() for f in selected}
plan=call('/api/persons/manage',{'mode':'preview','request':{'action':'delete-files','person':'甲','files':[picked(f) for f in selected]}})
assert plan['backend']=='rust';result=call('/api/persons/manage',{'mode':'apply','request':plan['request'],'version':plan['version']});assert result['staged'] and result['changed']==2
rows=call('/api/staging')['rows'];assert len(rows)==2
for row in rows:
    assert row['origin']['operation']==plan['id'];range_bytes(row['vid'],content[row['name']])
    restored=call('/api/staging',{'mode':'preview','request':{'action':'restore-files','person':'','files':[picked(row)]}})
    assert restored['files'][0]['dst']==row['origin']['original_path']
    value=call('/api/staging',{'mode':'apply','request':restored['request'],'version':restored['version']});assert value['changed']==1
    assert call('/api/staging',{'mode':'apply','request':restored['request'],'version':restored['version']})==value
    assert call('/api/staging?id='+restored['id'])['status']=='complete';range_bytes(row['vid'],content[row['name']])
assert call('/api/staging')['files']==0
assert call('/api/persons/manage?id='+plan['id'])['result']==result
call('/api/delete',{'vids':[selected[0]['vid']]},410)
call('/api/staging',{'mode':'apply','request':{'action':'clear'}},400)
with sqlite3.connect(root/'ledger.db') as db:assert not db.execute('PRAGMA foreign_key_check').fetchall()
# Keep one staged sample for browser playback/restore verification.
file=next(f for f in call('/api/persons/library?name='+urllib.parse.quote('甲'))['files'] if f['name']=='未扫描.mp4')
ui=call('/api/persons/manage',{'mode':'preview','request':{'action':'delete-files','person':'甲','files':[picked(file)]}});call('/api/persons/manage',{'mode':'apply','request':ui['request'],'version':ui['version']})
print('PASS public Next delete/list/restore/replay/old receipts/stable-ID Range/obsolete delete 410/permanent actions rejected')
