"""Manual account linking checks against an explicitly marked local fixture only."""
import argparse,datetime,json,urllib.request,urllib.error,urllib.parse
from pathlib import Path

parser=argparse.ArgumentParser();parser.add_argument('root',type=Path);parser.add_argument('--keep-cache',action='store_true');args=parser.parse_args()
root=args.root.resolve();assert root.parent==Path('/tmp').resolve()and root.name.startswith('face-admin-person-fileops-')and(root/'.fileops-fixture').is_file()
base=root/'base';url='http://127.0.0.1:8922'
def call(path,body=None,method=None,status=200):
 request=urllib.request.Request(url+path,data=json.dumps(body).encode()if body is not None else None,method=method or('POST'if body is not None else'GET'),headers={'Content-Type':'application/json'})
 try:
  with urllib.request.urlopen(request,timeout=40)as response:assert response.status==status;return json.load(response)
 except urllib.error.HTTPError as e:
  result=json.load(e);assert e.code==status,(e.code,result);return result
def links(name):return call('/api/persons/accounts?name='+urllib.parse.quote(name))['links']
assert call('/api/persons/library?name='+urllib.parse.quote('乙'))['root']==str(root/'media/pikpak/creators/乙')
assert not links('乙')and not links('新甲'),'keep existing manual associations intact'
cache=base/'external-profiles/nv-pu-sa.json';before=cache.read_bytes()if cache.exists()else None
reviewed=base/'external-profiles/reviewed-links.json';before_reviewed=reviewed.read_bytes()if reviewed.exists()else None
profiles=[dict(id='990001',screen_name='demo_link_01',name='乙',description='用于关联测试的公开资料快照',followers_count=12),dict(id='990002',screen_name='other_artist',name='小猫的新名字',description='与本地人物名不同的账号',followers_count=34)]
cache.parent.mkdir(parents=True,exist_ok=True);cache.write_text(json.dumps({'fetched_at':datetime.datetime.now(datetime.timezone.utc).isoformat(),'data':profiles},ensure_ascii=False))
reviewed.write_text(json.dumps({'version':1,'source':'nv-pu-sa','links':[{'person':'乙','profile_id':'990002','handle':'other_artist','confidence':'candidate','evidence':'历史文字线索','reviewed_at':'2026-11-07T00:00:00Z'}]},ensure_ascii=False))
created=[];success=False
try:
 before_queue=(base/'integration-queue.json').read_bytes();before_mappings=(base/'watermarks/mappings.json').read_bytes()
 result=call('/api/persons/external?q='+urllib.parse.quote('猫'));assert any(m['profile']['id']=='990002'for m in result['matches'])
 call('/api/persons/accounts',{'name':'乙','profile_id':'990001','handle':'wrong_handle'},status=409)
 call('/api/persons/accounts',{'name':'不存在的人物','profile_id':'990001','handle':'demo_link_01'},status=404)
 def add(name,id,handle):
  row=call('/api/persons/accounts',{'name':name,'profile_id':id,'handle':handle})['link'];created.append((name,row['id']));return row
 first=add('乙','990001','demo_link_01');second=add('乙','990002','other_artist');other=add('新甲','990002','other_artist')
 assert add('乙','990001','demo_link_01')['id']==first['id']and len(links('乙'))==2
 matches=call('/api/persons/external?name='+urllib.parse.quote('乙'))['matches']
 assert len([m for m in matches if m['basis']=='manual-link'])==2
 assert (base/'integration-queue.json').read_bytes()==before_queue and(base/'watermarks/mappings.json').read_bytes()==before_mappings
 call('/api/persons/accounts',{'name':'新甲','id':first['id']},method='DELETE',status=409)
 call('/api/persons/accounts',{'name':'乙','id':second['id']},method='DELETE')
 assert len(links('乙'))==1 and len(links('新甲'))==1
 print('PASS relaxed manual search, unrelated-name linking, idempotency, scoped unlink, and no video/rule changes')
 def rename(person,new):
  plan=call('/api/persons/manage',{'mode':'preview','request':{'action':'rename-person','person':person,'new_name':new}})
  call('/api/persons/manage',{'mode':'apply','request':plan['request'],'version':plan['version']})
 rename('乙','乙账号关联测试')
 renamed=links('乙账号关联测试');assert len(renamed)==1 and renamed[0]['creator']=='乙账号关联测试'and renamed[0]['profile']['name']=='乙'
 assert json.loads(reviewed.read_text())['links'][0]['person']=='乙账号关联测试'
 rename('乙账号关联测试','乙')
 assert links('乙')[0]['profile']['name']=='乙'
 print('PASS person rename carries manual/reviewed links without changing external nicknames')
 success=True
finally:
 for name,id in set(created):
  call('/api/persons/accounts',{'name':name,'id':id},method='DELETE',status=200 if any(row['id']==id for row in links(name))else 409)
 if before_reviewed is None:reviewed.unlink(missing_ok=True)
 else:reviewed.write_bytes(before_reviewed)
 if success and args.keep_cache:
  (root/'account-cache-backup.json').write_text(json.dumps({'exists':before is not None,'raw':before.decode()if before else None}))
 elif before is None:cache.unlink(missing_ok=True)
 else:cache.write_bytes(before)
assert not links('乙')and not links('新甲')
print('PASS manual associations removed cleanly; fixture ready for browser checks')
