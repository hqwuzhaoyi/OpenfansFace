"""Public Next -> native merge, retained viewer and stable/source/canonical/staged Range."""
import argparse,json,urllib.request,urllib.parse
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--root',type=Path,required=True);p.add_argument('--url',default='http://127.0.0.1:18990');args=p.parse_args()
root=args.root.resolve();assert root.parent==Path('/tmp').resolve() and root.name.startswith('face-admin-person-fileops-')
assert (root/'.fileops-fixture').read_text()=='person file operations'
url=urllib.parse.urlparse(args.url);assert url.hostname=='127.0.0.1' and url.port==18990
creators=root/'media/pikpak/creators'
def call(route,body=None):
    req=urllib.request.Request(args.url+route,data=None if body is None else json.dumps(body).encode(),headers={'Content-Type':'application/json'})
    with urllib.request.urlopen(req,timeout=120) as r:return json.load(r)
def listing(name):return call('/api/persons/library?name='+urllib.parse.quote(name))
source=listing('甲');target=listing('乙');assert source['files'] and target['files']
content={str(p):p.read_bytes() for p in (creators/'甲').rglob('*') if p.is_file()}
kept={str(p):p.read_bytes() for p in (creators/'乙').rglob('*') if p.is_file()}
plan=call('/api/persons/manage',{'mode':'preview','request':{'action':'merge-person','person':'甲','target_person':'乙'}})
assert plan['backend']=='rust' and plan['person']=='甲' and plan['target_person']=='乙'
assert plan['execution']['data']['target_preview']['files']
result=call('/api/persons/manage',{'mode':'apply','request':plan['request'],'version':plan['version']});assert result['ok'] and result['person']=='乙'
assert call('/api/persons/manage?id='+plan['id'])['status']=='complete'
after=listing('甲');assert Path(after['root']).resolve()==creators/'乙' and not (creators/'甲').exists()
for path,data in kept.items():assert Path(path).read_bytes()==data
checks={}
for file in plan['files']:
    assert Path(file['dst']).read_bytes()==content[file['src']]
    for vid in file['vids']:
        if Path(file['dst']).suffix.lower() in ('.mp4','.mkv','.mov'):checks[vid]=Path(file['dst'])
for name in ('held-source','held-canonical','held-staged'):
    overrides=json.loads((root/'base/video-path-overrides.json').read_text());checks[name]=Path(overrides[name])
for vid,path in checks.items():
    req=urllib.request.Request(args.url+'/api/video?'+urllib.parse.urlencode({'p':vid}),headers={'Range':'bytes=0-63'})
    with urllib.request.urlopen(req,timeout=30) as r:assert r.status==206 and r.read()==path.read_bytes()[:64]
for name in ('甲','乙','旧甲','旧乙'):
    with urllib.request.urlopen(args.url+'/persons/'+urllib.parse.quote(name),timeout=30) as r:assert r.status==200
assert call('/api/persons/manage',{'mode':'apply','request':plan['request'],'version':plan['version']})==result
(root/'merge-gateway-evidence.json').write_text(json.dumps({'plan':plan,'result':result},ensure_ascii=False))
print('PASS public native merge/actual two-person previews/old alias viewer/result replay/stable+source+canonical+staged Range')
