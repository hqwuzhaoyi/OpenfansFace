"""Actual Docker release-controller rehearsal; model/web peers are explicit simulators.

Requires --execute-isolated, a schema-9 Linux native image and an image containing
Python 3 + Docker CLI. All containers, paths, networks and evidence are uniquely
marked. No SSH, systemd or production/GPU service is accessed. Simulated gate
receipts cannot pass the controller without its marked rehearsal guard.
"""
import argparse,copy,hashlib,importlib.util,json,os,shutil,sqlite3,subprocess,sys,tempfile,time,urllib.request,uuid
from pathlib import Path
REPO=Path(__file__).resolve().parents[1]

def docker(*args):
 result=subprocess.run(['docker',*args],text=True,capture_output=True)
 if result.returncode:raise RuntimeError(f'docker {args[0]} failed: {result.stderr[-1000:]}')
 return result.stdout.strip()

def inspect(name):return json.loads(docker('inspect',name))[0]
def wait(url,headers=None):
 for _ in range(150):
  try:
   with urllib.request.urlopen(urllib.request.Request(url,headers=headers or {}),timeout=1) as response:return json.load(response)
  except OSError:time.sleep(.1)
 raise AssertionError('fixture endpoint did not become available: '+url)

def inside(root):
 root=Path(root);assert (root/'.release-rehearsal').read_text()=='isolated controller rehearsal'
 cfg=json.loads((root/'fixture.json').read_text());rid=cfg['id'];prefix='face-release-rehearsal-'+rid+'-';network=cfg['network'];peerimage=cfg['controller_image'];nativeimage=cfg['native_image'];token='rehearsal-only-token'
 os.environ.update(FILE_OPERATIONS_RELEASE_REHEARSAL_ROOT=str(root),REHEARSAL_TOKEN=token)
 spec=importlib.util.spec_from_file_location('controller',root/'release_control.py');ctl=importlib.util.module_from_spec(spec);spec.loader.exec_module(ctl)
 base=root/'base';media=root/'media';base.mkdir();(base/'processing').mkdir();source=media/'pikpak/creators/甲/original.mp4';source.parent.mkdir(parents=True);source.write_bytes(b'marked isolated synthetic video bytes')
 vid=r'R:\media\pikpak\My Telegram\stable.mp4';win=lambda p:'R:\\media\\pikpak\\'+str(p.relative_to(media/'pikpak')).replace('/','\\')
 with sqlite3.connect(base/'faces.db') as db:
  db.executescript('CREATE TABLE videos(vid TEXT PRIMARY KEY,path TEXT,name TEXT,collection TEXT,size INTEGER);CREATE TABLE faces(vid TEXT,score REAL,thumb TEXT);')
  db.execute('INSERT INTO videos VALUES(?,?,?,?,?)',(vid,str(source),source.name,str(source.parent),source.stat().st_size))
 with sqlite3.connect(base/'processing/processing.db') as db:
  db.executescript("CREATE TABLE tasks(vid TEXT PRIMARY KEY,path TEXT,name TEXT,size INTEGER,mtime_ns TEXT,stage TEXT,updated_at REAL,data TEXT);CREATE TABLE settings(key TEXT PRIMARY KEY,value TEXT);INSERT INTO settings VALUES('paused','false');")
 with sqlite3.connect(root/'ledger.db') as db:
  db.executescript((root/'peach.sql').read_text());db.execute('INSERT INTO asset(id,location,path,name) VALUES(1,?,?,?)',('fixture',win(source),'fixture'))
  db.execute("INSERT INTO entity(id,kind,canonical_name,normalized_name,created_at,updated_at) VALUES(1,'creator','甲','甲','fixture','fixture')")
  db.execute("INSERT INTO asset_entity(asset_id,entity_id,role,source) VALUES(1,1,'creator','manual')")
 def write(path,obj):path.parent.mkdir(parents=True,exist_ok=True);path.write_text(json.dumps(obj,ensure_ascii=False))
 write(base/'archive-history.json',[{'vid':vid,'dst':str(source),'win_dst':win(source),'creator':'甲','original_vid':vid}]);write(base/'report.json',{'identities':[{'name':'甲','vids':[{'vid':vid,'path':str(source),'name':source.name}]}]})
 for path in [root,*root.rglob('*')]:os.chown(path,1000,1000)
 def peerargs(role):return ['--name',prefix+role,'--network',network,'--label','face-admin.release-rehearsal='+rid,'-v',f'{root}:{root}','-e','REHEARSAL_ROOT='+str(root),'-e','FILE_OPERATIONS_RUST_URL=http://'+prefix+'native:8930',peerimage,str(root/'peer.py')]
 for role in ('web','worker','calibrator','gpu','peach'):
  cid=docker('create',*peerargs(role));docker('start',cid);wait('http://'+prefix+role+':8080/health')
 nativeargs=['--name',prefix+'native','--network',network,'--label','face-admin.release-rehearsal='+rid,'-v',f'{root}:{root}','-e','FACES_BASE='+str(base),'-e','MEDIA2_ROOT='+str(media),'-e','PEACH_LEDGER_PATH='+str(root/'ledger.db'),'-e','FILE_OPERATIONS_BIND=0.0.0.0:8930','-e','FILE_OPERATIONS_RUST_TOKEN='+token,'-e','FILE_OPERATIONS_ADMISSION=closed',nativeimage]
 proof=root/'SIMULATED-GATES-NOT-PRODUCTION.json';write(proof,{'simulated':True,'scope':'actual local Docker controller only; no GPU or production acceptance','id':rid})
 legacy_proof=root/'SIMULATED-LEGACY-REFERENCE-GATE.json';write(legacy_proof,{'simulated':True,'gate':'legacy_reference_compatibility','audit_schema':1,'complete':True,'status':'passed','metadata_only':True})
 proofhash=hashlib.sha256(proof.read_bytes()).hexdigest();service=lambda role,args:{'name':prefix+role,'expected_id':inspect(prefix+role)['Id'],'expected_image':inspect(prefix+role)['Image'],'candidate_image':peerimage,'create_args':args}
 m={'version':1,'release_id':'bootstrap','purpose':'release','rehearsal':{'root':str(root),'id':rid},'native_schema':9,'native_max_schema':9,'single_native_writer':True,'workers_start_paused':True,'native_starts_closed':True,'deploy_lock':str(root/'deploy.lock'),'evidence_dir':str(root/'evidence'),'base':str(base),'media_root':str(media),'ledger':str(root/'ledger.db'),'authority_dir':str(base/'file-operations-rust'),'authority_identity':None,'native_uid':1000,'native_gid':1000,'native_url':'http://'+prefix+'native:8930','native_token_env':'REHEARSAL_TOKEN','web_url':'http://'+prefix+'web:8080','calibrator_url':'http://'+prefix+'calibrator:8080','minimum_free_after_backup_bytes':1024*1024,'drain_timeout':2,'services':{role:service(role,peerargs(role)) for role in ('worker','calibrator','web')},'evidence':{g:{'passed':True,'path':str(proof),'sha256':proofhash} for g in ctl.GATES}}
 m['services']['native']={'name':prefix+'native','initially_absent':True,'expected_id':None,'expected_image':None,'candidate_image':nativeimage,'create_args':nativeargs}
 # Exact retained Python namespace fixture. All bytes/links are synthetic and
 # remain in place through bootstrap, failure, repair and compatible rollback.
 runtime=base/'venv'
 for name in ('bin','lib/python3.11/site-packages','include'):(runtime/name).mkdir(parents=True,exist_ok=True)
 (runtime/'pyvenv.cfg').write_text('home = /usr/bin\ninclude-system-site-packages = false\nversion = 3.11.2\nexecutable = /usr/bin/python3.11\n')
 for name,target in {'bin/python':'python3','bin/python3.11':'python3','bin/python3':'/usr/bin/python3','lib64':'lib'}.items():(runtime/name).symlink_to(target)
 (runtime/'lib/python3.11/site-packages/fixture.py').write_text('# isolated retained runtime bytes\n')
 runtime_before=ctl.runtime_inventory(base)
 def ref(path):return {'path':str(path),'sha256':hashlib.sha256(path.read_bytes()).hexdigest()}
 runtime_inventory_path=root/'runtime-inventory.json';write(runtime_inventory_path,runtime_before)
 def freeze_runtime(manifest,suffix):
  review=root/('runtime-review-'+suffix+'.json')
  write(review,{'version':1,'kind':'preserved-runtime-review','base':str(base),'relative_path':'venv','inventory_sha256':ref(runtime_inventory_path)['sha256'],'retained_in_place':True,'candidate_uses_image_runtimes':True,'candidate_mutates_preserved_runtime':False,'contains_business_state':False,'source_files':[ref(root/'peer.py')],'candidate_configuration':ctl.candidate_runtime_configuration(manifest),'runtime_namespace_evidence':ref(proof),'runtime_source_review':ref(proof),'simulated':True})
  manifest['preserved_runtime']={'relative_path':'venv','inventory':ref(runtime_inventory_path),'review':ref(review)}
 freeze_runtime(m,'initial')

 peach=inspect(prefix+'peach');m['peach']={'name':prefix+'peach','expected_id':peach['Id'],'expected_image':peach['Image']}
 with sqlite3.connect(root/'ledger.db') as db:rows=db.execute("SELECT type,name,tbl_name,sql FROM sqlite_master WHERE sql IS NOT NULL ORDER BY type,name").fetchall()
 m['peach']['schema_sha256']=hashlib.sha256(json.dumps(rows,ensure_ascii=False,separators=(',',':')).encode()).hexdigest()
 m['legacy_health']={'adapter':'rehearsal','states':{'timer':{'LoadState':'loaded','ActiveState':'inactive','UnitFileState':'disabled'},'service':{'LoadState':'loaded','ActiveState':'inactive','UnitFileState':'static'}}}
 m['gpu_peer']={'adapter':'external-systemd','unit':'SIMULATED-NO-SYSTEMD','url':'http://'+prefix+'gpu:8080/health/ready','token_env':'REHEARSAL_TOKEN','expected_health':{'simulated':True,'live':True,'accepting':True,'service_instance':'simulated-local-peer'},'prepared_receipt':{'path':str(proof),'sha256':proofhash}}
 m['probes']=[{'kind':kind,'url':'http://'+prefix+('worker' if kind=='worker' else 'calibrator' if kind=='calibrator' else 'web')+':8080/'+('playback' if kind=='playback-range' else kind),'status':206 if kind=='playback-range' else 200} for kind in ('web','static','playback-range','legacy-receipt','native-receipt','gpu','worker','calibrator')]
 history=[]
 m['evidence']['legacy_reference_compatibility']={'passed':True,'path':str(legacy_proof),'sha256':hashlib.sha256(legacy_proof.read_bytes()).hexdigest()}
 def control(action,manifest,success=True):
  file=root/(manifest['release_id']+'-'+action+'.manifest.json');write(file,manifest)
  result=subprocess.run([sys.executable,str(root/'release_control.py'),action,'--manifest',str(file),'--execute'],text=True,capture_output=True)
  write(root/(manifest['release_id']+'-'+action+'.result.json'),{'simulated':True,'code':result.returncode,'stdout':result.stdout,'stderr':result.stderr})
  assert (result.returncode==0)==success,(action,result.returncode,result.stderr)
  history.append({'action':action,'release':manifest['release_id'],'success':success})
  return result
 def activate(manifest,action='replace'):
  control(action,manifest)
  return json.loads((root/'evidence'/(manifest['release_id']+'-'+action)/'activated-manifest.json').read_text())
 def native(path,body=None):return ctl.api(m,path,body)
 bad=copy.deepcopy(m);bad['release_id']='active-timer';bad['legacy_health']['states']['timer'].update(ActiveState='active',UnitFileState='enabled')
 control('fence-peers',bad,False);assert inspect(prefix+'web')['State']['Running'] and not (base/'.review-write.lock').exists()
 m['release_id']='held-legacy';foreign=base/'.review-write.lock';foreign.write_text('isolated foreign legacy lock')
 control('fence-peers',m,False);assert inspect(prefix+'web')['State']['Running'] and foreign.read_text()=='isolated foreign legacy lock'
 control('release-fence',m,False);assert foreign.read_text()=='isolated foreign legacy lock'
 foreign.unlink();m['release_id']='owned-fence-recovery'
 journal=base/'person-operations'/str(uuid.uuid4())/'operation.json';write(journal,{'status':'moving'})
 control('fence-peers',m,False);assert inspect(prefix+'web')['State']['Running']
 assert json.loads(foreign.read_text())['backend']=='deployment'
 # Repair only the marked historical-journal fault, then explicitly stop the
 # idle protocol peers under the still-held fence before exact-owner recovery.
 write(journal,{'status':'rolled_back'})
 for role in ('web','worker','calibrator'):docker('stop','--time','5',prefix+role)
 control('release-fence',m);assert not foreign.exists()
 for role in ('web','worker','calibrator'):docker('start',prefix+role);wait('http://'+prefix+role+':8080/health')
 m['release_id']='bootstrap'
 print('PASS unresolved legacy journal retains durable owned fence; explicit exact-owner recovery after stopped peers',flush=True)
 print('PASS active legacy timer blocks before fencing; held legacy lock timeout leaves web alive and foreign lock untouched',flush=True)
 # Exercise the real scalar gate with exactly ten marked unsupported inputs.
 # Only the operator authorization/peer services are simulated; audit and backup
 # operate on real isolated files. No model/vector payload exists in these caches.
 import zipfile
 import legacy_reference_audit as audit
 def scalar(value):
  value=str(value);header=repr({'descr':'<U'+str(len(value)),'fortran_order':False,'shape':()}).encode()+b'\n'
  return b'\x93NUMPY\x01\x00'+len(header).to_bytes(2,'little')+header+value.encode('utf-32-le')
 cache_dir=base/'processing/faces';cache_dir.mkdir(exist_ok=True)
 old_history=json.loads((base/'archive-history.json').read_text())
 for index in range(10):
  stable='marked-unsupported-'+str(index);missing=media/('missing-'+str(index)+'.mp4')
  old_history.append({'vid':stable,'creator':'marked unsupported person'})
  with sqlite3.connect(base/'faces.db') as db:db.execute('INSERT INTO videos(vid,path) VALUES(?,?)',(stable,str(missing)))
  with zipfile.ZipFile(cache_dir/(hashlib.sha256(stable.encode()).hexdigest()+'.npz'),'w') as archive:
   for key,value in {'version':audit.VERSION,'path':str(missing),'size':1,'mtime_ns':'1'}.items():archive.writestr(key+'.npy',scalar(value))
 write(base/'archive-history.json',old_history)
 references=base/'processing/references.npz';references.write_bytes(b'marked prior reference backup sentinel')
 os.chown(references,1000,1000)
 diagnostic=root/'actual-legacy-audit.json';write(diagnostic,audit.audit(base,media));assert json.loads(diagnostic.read_text())['counts']['unresolved']==10
 simulated_evidence=m['evidence']['legacy_reference_compatibility']
 m['evidence']['legacy_reference_compatibility']={**ref(diagnostic),'passed':True}
 control('fence-peers',m,False);assert inspect(prefix+'web')['State']['Running'] and not (base/'.review-write.lock').exists()
 decision=root/'marked-legacy-exclusion.json'
 write(decision,{'version':1,'simulated':True,'purpose':'exclude_from_reference_build','release_id':m['release_id'],
  'reason':'Marked rehearsal: exclude precisely these ten unsupported synthetic inputs', 'reviewed_by':'isolated rehearsal operator',
  'reviewed_at':'2026-01-01T00:00:00Z','authorization_reference':ref(proof),'preserve_original_bytes':True,
  'worker_policy':'source_unresolved_skipped_before_cache_read_v1','reviewed_cases':json.loads(diagnostic.read_text())['cases']})
 m['legacy_reference_exclusion']=ref(decision)
 control('fence-peers',m);m=activate(m);identity=m['authority_identity'];assert native('/v1/readiness')['schema']==9
 snapshot=root/'evidence/bootstrap-replace'
 policy=json.loads((snapshot/'backup-policy.json').read_text())
 assert policy['preserved_runtime']['disposition']=='retained-in-place-not-restored-from-backup'
 assert not (snapshot/'base/venv').exists() and (snapshot/'base/report.json').exists()
 assert json.loads((snapshot/'preserved-runtime-inventory.json').read_text())==runtime_before
 assert (snapshot/'backup-complete.json').is_file() and ctl.runtime_inventory(base)==runtime_before
 print('PASS frozen retained runtime inventory, shared capacity/snapshot policy and backup receipts; runtime bytes/links unchanged',flush=True)
 assert (snapshot/'base/processing/references.npz').read_bytes()==references.read_bytes()
 gate=json.loads((snapshot/'legacy-reference-gate.json').read_text())
 assert gate['status']=='passed_with_approved_exclusions' and gate['counts']['unresolved']==10 and gate['counts']['approved_excluded']==10 and gate['counts']['native_proven']==0
 assert (snapshot/'legacy-reference-backup.json').is_file() and (snapshot/'legacy-reference-decision.json').is_file()
 control('open',m)
 print('PASS exact ten-input decision: real scalar gate blocks before pause without approval; normal backup preserves reference before actual candidate startup',flush=True)
 # Remaining controller fault scenarios retain their explicitly simulated gate.
 del m['legacy_reference_exclusion'];m['evidence']['legacy_reference_compatibility']=simulated_evidence
 stat=source.stat();fingerprint=hashlib.sha256(json.dumps([str(source),str(stat.st_dev),str(stat.st_ino),str(stat.st_size),str(stat.st_mtime_ns)],separators=(',',':'),ensure_ascii=False).encode()).hexdigest()
 request={'id':str(uuid.uuid4()),'action':'rename-file','person':'甲','files':[{'vid':vid,'path':str(source),'version':fingerprint}],'filename':'renamed.mp4'}
 plan=native('/v1/person-files',{'mode':'preview','request':request});receipt=native('/v1/person-files',{'mode':'apply','request':request,'version':plan['version']});assert receipt['ok'];assert source.with_name('renamed.mp4').read_bytes()==b'marked isolated synthetic video bytes'
 # Freeze schema after legitimate first native receipt-table creation.
 with sqlite3.connect(root/'ledger.db') as db:rows=db.execute("SELECT type,name,tbl_name,sql FROM sqlite_master WHERE sql IS NOT NULL ORDER BY type,name").fetchall()
 m['peach']['schema_sha256']=hashlib.sha256(json.dumps(rows,ensure_ascii=False,separators=(',',':')).encode()).hexdigest()
 oldid=m['services']['native']['expected_id'];m['release_id']='compatible';control('fence-peers',m);m=activate(m);assert m['authority_identity']==identity and m['services']['native']['expected_id']!=oldid
 assert native('/v1/person-files?id='+request['id'])['result']==receipt
 print('PASS actual bootstrap/create/stop/rename/image+container identities and same-inode receipt across compatible replacement',flush=True)
 # Actual Docker create failure after native replacement: no auto rollback, no new admission.
 m['release_id']='partial-create';control('fence-peers',m);broken=copy.deepcopy(m);broken['services']['worker']['create_args'].insert(0,'--intentionally-invalid-rehearsal-option')
 freeze_runtime(broken,'intentional-create-failure')
 control('replace',broken,False);assert not native('/v1/readiness')['accepting'];directory=root/'evidence/partial-create-replace';assert (directory/'native-started.json').exists() and (directory/'worker-stopped.json').exists() and not (directory/'web-stopped.json').exists()
 # Explicitly reconstruct only the stopped fixture peer name from its recorded anchor.
 anchor=json.loads((directory/'worker-stopped.json').read_text())['anchor'];docker('rename',anchor,prefix+'worker')
 for role,s in m['services'].items():state=inspect(s['name']);s.update(expected_id=state['Id'],expected_image=state['Image'])
 m['release_id']='partial-repair';m=activate(m);assert native('/v1/person-files?id='+request['id'])['result']==receipt
 print('PASS actual mid-replacement create failure, durable partial receipts, closed authority and explicit compatible repair',flush=True)
 # Protected unresolved record blocks rollback before touching the native container.
 m['release_id']='blocked-unresolved';m['purpose']='compatible-native-rollback';control('fence-peers',m)
 state_db=base/'file-operations-rust/operations.sqlite3'
 with sqlite3.connect(state_db) as db:db.execute("UPDATE operations SET status='recovery_needed' WHERE id=?",(request['id'],))
 before=inspect(prefix+'native')['Id'];control('rollback',m,False);assert inspect(prefix+'native')['Id']==before and not native('/v1/readiness')['accepting']
 # Isolated fault fixture restoration, never a production recovery instruction.
 with sqlite3.connect(state_db) as db:db.execute("UPDATE operations SET status='complete' WHERE id=?",(request['id'],))
 m['release_id']='compatible-rollback';m=activate(m,'rollback');assert m['authority_identity']==identity and native('/v1/person-files?id='+request['id'])['result']==receipt
 print('PASS actual unresolved-operation refusal and compatible native rollback with historical query intact',flush=True)
 # Final fixture is deliberately left with a future schema; never downgrade it.
 m['release_id']='blocked-newer-schema';control('fence-peers',m)
 with sqlite3.connect(state_db) as db:db.execute('PRAGMA user_version=10')
 before=inspect(prefix+'native')['Id'];result=control('rollback',m,False);assert 'newer than candidate' in result.stderr and inspect(prefix+'native')['Id']==before and not native('/v1/readiness')['accepting']
 print('PASS actual schema10 state refuses schema9 rollback before container replacement; admission remains closed',flush=True)
 assert ctl.runtime_inventory(base)==runtime_before
 write(root/'summary.json',{'simulated':True,'scope':'actual local Docker controller; no production/GPU acceptance','native_image':nativeimage,'controller_image':peerimage,'controller_sha256':hashlib.sha256((root/'release_control.py').read_bytes()).hexdigest(),'native_source':'13fc3bfa5e5e0f67594393e7b95994a4ff0b1b33','authority':identity,'receipt_id':request['id'],'history':history,'passed':True})

if __name__=='__main__':
 parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--inside');parser.add_argument('--execute-isolated',action='store_true');parser.add_argument('--native-image');parser.add_argument('--controller-image');args=parser.parse_args()
 if args.inside:inside(args.inside);raise SystemExit()
 if not args.execute_isolated:parser.error('explicit --execute-isolated required')
 rid=uuid.uuid4().hex[:10];root=Path('/tmp')/('face-admin-release-rehearsal-'+rid);root.mkdir();(root/'.release-rehearsal').write_text('isolated controller rehearsal');network='face-release-rehearsal-'+rid
 nativeimage=json.loads(docker('image','inspect',args.native_image))[0]['Id'];controllerimage=json.loads(docker('image','inspect',args.controller_image))[0]['Id']
 for source,name in [(Path(__file__),'rehearse.py'),(REPO/'maintenance/release_control.py','release_control.py'),(REPO/'maintenance/legacy_reference_audit.py','legacy_reference_audit.py'),(REPO/'tests/fixtures/release-controller/peer.py','peer.py'),(REPO/'tests/fixtures/peach-contract-schema.sql','peach.sql')]:shutil.copyfile(source,root/name)
 (root/'fixture.json').write_text(json.dumps({'id':rid,'network':network,'native_image':nativeimage,'controller_image':controllerimage}))
 docker('network','create','--label','face-admin.release-rehearsal='+rid,network)
 socket=os.environ.get('DOCKER_HOST') or json.loads(docker('context','inspect'))[0]['Endpoints']['docker']['Host'];assert socket.startswith('unix://');socket=socket[7:]
 try:
  result=subprocess.run(['docker','run','--rm','--platform','linux/amd64','--name',network+'-controller','--network',network,'--label','face-admin.release-rehearsal='+rid,'-v',f'{root}:{root}','-v',socket+':/var/run/docker.sock',controllerimage,str(root/'rehearse.py'),'--inside',str(root)],text=True,capture_output=True)
  (root/'controller-run.log').write_text(result.stdout+result.stderr);print(result.stdout,end='');print('Isolated rehearsal evidence:',root)
  if result.returncode:print(result.stderr,file=sys.stderr);raise SystemExit(result.returncode)
 finally:
  owned=docker('ps','-aq','--filter','label=face-admin.release-rehearsal='+rid).split()
  if owned:docker('rm','-f',*owned)
  docker('network','rm',network)
