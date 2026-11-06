"""A manually created person is immediately reusable and survives undo and reload."""
import sys,json,urllib.request,urllib.error,urllib.parse
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
vid='R:\\media\\pikpak\\My Telegram\\test\\a.mp4'
req('/api/plan/queue',dict(creator='嗨酱',vids=[vid],source='cluster',source_id='p1'),'POST')
people=req('/api/persons?picker=1')['persons'];hi=[p for p in people if p['name']=='嗨酱'];assert len(hi)==1 and hi[0]['recent_at'];assert people[0]['name']=='嗨酱'
identity=next(i for i in req('/api/identities')['identities']if i['name']=='嗨酱');assert any(v['vid']==vid and v['state']=='queued'for v in identity['vids'])
assert any(p['name']=='嗨酱'for p in req('/api/persons')['persons'])
print('PASS new person immediately appears in picker, recent people, person list, and own video page')
req('/api/plan/queue',dict(vids=[vid]),'DELETE')
assert any(p['name']=='嗨酱'for p in req('/api/persons?picker=1')['persons'])
assert any(p['name']=='嗨酱'for p in req('/api/identities')['identities'])
stored=json.loads((root/'base/people-manual.json').read_text());assert len([p for p in stored if p['name']=='嗨酱'])==1
print('PASS undoing a naming batch does not delete the manually created person')
req('/api/claims/decide',dict(vid='R:\\media\\pikpak\\My Telegram\\test\\review.mp4',approve=True,creator='审批新人物'),'POST')
assert any(p['name']=='审批新人物'for p in req('/api/persons?picker=1')['persons'])
req('/api/plan/queue',dict(vids=['R:\\media\\pikpak\\My Telegram\\test\\review.mp4']),'DELETE')
assert any(p['name']=='审批新人物'for p in req('/api/persons?picker=1')['persons'])
print('PASS approval entry also persists new people independently of its batch')
