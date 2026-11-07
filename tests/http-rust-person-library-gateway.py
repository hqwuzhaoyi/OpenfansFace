"""Public library adapter contract; only the marked native-ledger gateway fixture.
Run after seeding with http-file-operation-backend.py --seed --native-ledger.
"""
import argparse,json,urllib.request,urllib.parse,urllib.error
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--root',type=Path,required=True);p.add_argument('--url',default='http://127.0.0.1:18984');args=p.parse_args()
root=args.root.resolve();url=urllib.parse.urlparse(args.url)
assert root.parent==Path('/tmp').resolve() and root.name.startswith('face-admin-person-fileops-')
assert (root/'.fileops-fixture').read_text()=='person file operations'
assert url.hostname=='127.0.0.1' and url.port not in (None,80,443,8919)
base=root/'base';creators=root/'media/pikpak/creators'
def call(route,status=200):
    try:
        with urllib.request.urlopen(args.url+route,timeout=60) as r:code=r.status;body=json.load(r)
    except urllib.error.HTTPError as e:code=e.code;body=json.load(e)
    assert code==status,(code,body)
    return body
route='/api/persons/library?'+urllib.parse.urlencode({'name':'甲'})
added=[];queue=base/'integration-queue.json';before=queue.read_bytes();rejections=base/'person-rejections.json';rejections_before=rejections.read_bytes()
try:
    for i in range(6):
        file=creators/'甲'/f'gateway-read-{i}.mp4';assert not file.exists();file.write_bytes(b'read fixture');added.append(file)
    names=call('/api/persons/library')['persons'];assert any(p['name']=='甲' for p in names)
    listing=call(route);assert listing['root']==str(creators/'甲')
    stat=call('/api/persons/library?stats=1')['stats']['甲']
    assert stat['video_count']==len(listing['files']) and stat['total_bytes']==sum(f['size'] for f in listing['files'])
    preview=call(route+'&preview=1');assert len(preview['files'])==4 and preview['next_offset']==4
    second=call(route+'&preview=1&offset=4');assert not {f['path'] for f in preview['files']} & {f['path'] for f in second['files']}
    for f in preview['files']:
        assert f['path'].startswith(str(creators)) and f['modified_at'].endswith('Z') and len(f['version'])==64 and 'modified_ms' not in f
    call(route+'&preview=1&offset=-1',400)
    missing=call('/api/persons/library?name='+urllib.parse.quote('不存在'));assert not missing['directory_exists'] and missing['files']==[]
    candidate=json.loads((base/'claims-detail.json').read_text())[0]['vid']
    incoming=root/'media/pikpak/My Telegram/incoming.mp4'
    queue.write_text('[]')
    recommended=next(r for r in call(route)['recommendations'] if r['vid']==candidate)
    assert recommended['path']==str(incoming) and recommended['size']==incoming.stat().st_size
    assert recommended['evidence'][0]['kind']=='face'
    saved=json.loads(rejections_before);saved[json.dumps(['甲',candidate],ensure_ascii=False,separators=(',',':'))]={'name':'甲','vid':candidate,'path':str(incoming),'size':incoming.stat().st_size,'mtime_ns':str(incoming.stat().st_mtime_ns),'id':'fixture-rejection','at':'fixture'}
    rejections.write_text(json.dumps(saved,ensure_ascii=False))
    assert all(r['vid']!=candidate for r in call(route)['recommendations'])
    print('PASS public names/stats/preview pagination/full paths/date/version, missing directory and native-observation recommendation/rejection joins')
finally:
    queue.write_bytes(before);rejections.write_bytes(rejections_before)
    for file in added:file.unlink()
