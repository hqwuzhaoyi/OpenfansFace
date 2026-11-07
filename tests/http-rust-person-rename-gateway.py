"""Public Next -> native whole-person rename; marked fixture only, stable Range and alias reads."""
import argparse,json,urllib.request,urllib.parse,urllib.error
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--root',type=Path,required=True);p.add_argument('--url',default='http://127.0.0.1:18988');args=p.parse_args()
root=args.root.resolve();assert root.parent==Path('/tmp').resolve() and root.name.startswith('face-admin-person-fileops-')
assert (root/'.fileops-fixture').read_text()=='person file operations'
url=urllib.parse.urlparse(args.url);assert url.hostname=='127.0.0.1' and url.port==18988
creators=root/'media/pikpak/creators'
def call(route,body=None):
    req=urllib.request.Request(args.url+route,data=None if body is None else json.dumps(body).encode(),headers={'Content-Type':'application/json'})
    with urllib.request.urlopen(req,timeout=120) as r:return json.load(r)
def listing(name):return call('/api/persons/library?name='+urllib.parse.quote(name))
before=listing('甲');content={row['vid']:Path(row['path']).read_bytes() for row in before['files']}
plan=call('/api/persons/manage',{'mode':'preview','request':{'action':'rename-person','person':'甲','new_name':'改名验收'}})
assert plan['backend']=='rust' and len(plan['files'])>=len(content)
result=call('/api/persons/manage',{'mode':'apply','request':plan['request'],'version':plan['version']});assert result['ok'] and result['person']=='改名验收'
assert call('/api/persons/manage?id='+plan['id'])['status']=='complete'
after=listing('甲');assert Path(after['root']).resolve()==creators/'改名验收' and not (creators/'甲').exists()
for file in plan['files']:
    for vid in file['vids']:
        if not Path(file['dst']).suffix.lower() in ('.mp4','.mkv','.mov'):continue
        req=urllib.request.Request(args.url+'/api/video?'+urllib.parse.urlencode({'p':vid}),headers={'Range':'bytes=0-63'})
        with urllib.request.urlopen(req,timeout=30) as r:assert r.status==206 and r.read()==Path(file['dst']).read_bytes()[:64]
for name in ('甲','改名验收'):
    with urllib.request.urlopen(args.url+'/persons/'+urllib.parse.quote(name),timeout=30) as r:assert r.status==200
assert not (root/'base/external-profiles/manual-links.json').exists()
assert not (root/'base/external-profiles/reviewed-links.json').exists()
assert call('/api/persons/manage',{'mode':'apply','request':plan['request'],'version':plan['version']})==result
(root/'rename-gateway-evidence.json').write_text(json.dumps({'plan':plan,'result':result},ensure_ascii=False))
print('PASS public whole-person rename/old alias page+library/stable and synthetic ID Range/result/replay')
