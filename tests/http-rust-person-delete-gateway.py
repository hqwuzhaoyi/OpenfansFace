"""Public Next #11 soft-delete/catalog/history/staging/Range contract. Dedicated fixture only."""
import argparse,json,sqlite3,urllib.request,urllib.parse,urllib.error
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--root',type=Path,required=True);args=p.parse_args()
root=args.root.resolve();assert root.parent==Path('/tmp').resolve() and root.name.startswith('face-admin-person-fileops-')
assert (root/'.fileops-fixture').read_text()=='person file operations'
url='http://127.0.0.1:18992'
def call(route,body=None,status=200):
    req=urllib.request.Request(url+route,data=None if body is None else json.dumps(body).encode(),headers={'Content-Type':'application/json'})
    try:
        with urllib.request.urlopen(req,timeout=120) as r:code,result=r.status,json.load(r)
    except urllib.error.HTTPError as e:code,result=e.code,json.load(e)
    assert code==status,(route,code,result)
    return result
catalog=call('/api/persons/library')['persons'];assert next(p for p in catalog if p['name']=='甲')['account_count']==1
before=call('/api/persons/library?name='+urllib.parse.quote('甲'))
source=Path(before['root']);content={str(p.relative_to(source)):p.read_bytes() for p in source.rglob('*') if p.is_file()}
plan=call('/api/persons/manage',{'mode':'preview','request':{'action':'delete-person','person':'更旧甲'}})
assert plan['backend']=='rust' and len(plan['files'])==len(content)
result=call('/api/persons/manage',{'mode':'apply','request':plan['request'],'version':plan['version']});assert result['ok'] and result['staged'] and result['changed']==len(content)
assert call('/api/persons/manage?id='+plan['id'])['status']=='complete'
assert call('/api/persons/manage',{'mode':'apply','request':plan['request'],'version':plan['version']})==result
assert not source.exists()
people=call('/api/persons/library');people=people.get('persons',people) if isinstance(people,dict) else people
assert all(p['name'] not in ('甲','旧甲','更旧甲') for p in people)
for name in ('甲','旧甲','更旧甲'):call('/api/persons/library?name='+urllib.parse.quote(name),status=404)
rows=call('/api/staging')['rows'];assert len(rows)==len(content)
for row in rows:
    original=Path(row['origin']['original_path']);assert row['origin']['action']=='delete-person' and row['origin']['operation']==plan['id']
    expected=content[str(original.relative_to(source))];assert Path(row['path']).read_bytes()==expected
    if original.suffix.lower()=='.mp4':
        req=urllib.request.Request(url+'/api/video?'+urllib.parse.urlencode({'p':row['vid']}),headers={'Range':'bytes=0-63'})
        with urllib.request.urlopen(req,timeout=30) as r:assert r.status==206 and r.read()==expected[:64]
history=json.loads((root/'base/archive-history.json').read_text());assert history[0]['creator']=='甲' and history[0]['file_state']=='staged'
assert call('/api/plan/preview')['history'][0]==history[0]
# Immutable old receipt remains queryable, while history cannot resurrect the catalog.
old='44444444-4444-4444-8444-444444444444';receipt=root/'base/person-operations'/old/'operation.json';receipt.parent.mkdir(parents=True)
receipt.write_text(json.dumps({'status':'complete','plan':{'action':'delete-person'},'result':{'ok':True,'id':old,'person':'historical'}}));raw=receipt.read_bytes()
assert call('/api/persons/manage?id='+old)['backend']=='legacy' and receipt.read_bytes()==raw
with sqlite3.connect(root/'ledger.db') as db:
    assert db.execute('SELECT canonical_name FROM entity WHERE id=1').fetchone()[0]=='甲'
    assert db.execute('SELECT count(*) FROM asset_entity').fetchone()[0]==2
    assert not db.execute('PRAGMA foreign_key_check').fetchall()
for page in ('/persons','/staging','/persons/'+urllib.parse.quote('甲')):
    with urllib.request.urlopen(url+page) as r:assert r.status==200
(root/'delete-gateway-evidence.json').write_text(json.dumps({'plan':plan,'result':result,'staged':rows},ensure_ascii=False))
print('PASS public native whole-person deletion/final receipt/replay/deleted catalog+alias/history/old receipt/staging/stable synthetic Range')
