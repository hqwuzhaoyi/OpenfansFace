"""Read-only Peach identity resolution against an explicitly marked /tmp fixture."""
import argparse,json,sqlite3,hashlib,urllib.request,urllib.error,urllib.parse
from pathlib import Path
parser=argparse.ArgumentParser();parser.add_argument('root',type=Path);parser.add_argument('--port',type=int,default=8946);args=parser.parse_args()
root=args.root.resolve();assert root.parent==Path('/tmp').resolve() and root.name.startswith('face-admin-person-fileops-peach-') and(root/'.fileops-fixture').exists()
base=root/'base';ledger=root/'ledger.db';url=f'http://127.0.0.1:{args.port}'
def get(name,status=200):
 try:
  with urllib.request.urlopen(url+'/api/persons/peach?name='+urllib.parse.quote(name),timeout=15)as response:
   assert response.status==status;return json.load(response)
 except urllib.error.HTTPError as error:
  assert error.code==status,(error.code,error.read());return json.load(error)
with sqlite3.connect(ledger)as con:
 con.execute("INSERT INTO entity_alias(entity_id,alias) VALUES(1,'唯一别名')")
 con.executemany('INSERT INTO entity_alias(entity_id,alias) VALUES(?,?)',[(1,'重复别名'),(2,'重复别名'),(2,'甲')])
 con.execute("INSERT INTO entity(kind,canonical_name,normalized_name,metadata_json,created_at,updated_at) VALUES('performer','同名表演者','同名表演者','{}','fixture','fixture')")
(base/'person-renames.json').write_text(json.dumps({'旧甲':'甲'}))
before=hashlib.sha256(ledger.read_bytes()).hexdigest()
assert get('甲')['person']=={'id':'1','name':'甲','kind':'creator'}
assert get('旧甲')['person']['id']=='1'
assert get('唯一别名')['person']['id']=='1'
assert get('重复别名')['status']=='ambiguous'
assert get('不存在的人物')['status']=='missing'
assert get('同名表演者')['status']=='missing'
get('../unsafe',400)
assert hashlib.sha256(ledger.read_bytes()).hexdigest()==before
parked=ledger.with_suffix('.offline');ledger.rename(parked)
try:assert 'error' in get('甲',503)
finally:parked.rename(ledger)
print('PASS canonical priority, unique alias, local rename, ambiguity, missing identity, kind boundary, unavailable ledger, and no ledger writes')
