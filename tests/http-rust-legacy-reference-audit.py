"""Real native movement + closed-admission scalar preflight; no vectors/models loaded."""
import importlib.util,json,os,sqlite3,sys,zipfile
from pathlib import Path
from unittest.mock import patch
REPO=Path(__file__).resolve().parents[1]
def module(name,path):
 spec=importlib.util.spec_from_file_location(name,path);value=importlib.util.module_from_spec(spec);spec.loader.exec_module(value);return value
n=module('native_fixture',Path(__file__).with_name('http-rust-file-operations.py'))
fns=module('scalar_fixtures',Path(__file__).with_name('test_legacy_reference_audit.py'))
a=fns.a;r=fns.r
for copied in (False,True):
 f=n.Fixture()
 live_queue=sqlite3.connect(f.base/'processing/processing.db');live_queue.execute('SELECT COUNT(*) FROM tasks').fetchone()
 try:
  (f.base/'processing/faces').mkdir()
  cache=f.base/'processing/faces'/(fns.hashlib.sha256(n.VID.encode()).hexdigest()+'.npz')
  fns.cache(cache,f.source)
  reference=f.base/'processing/references.npz';reference.write_bytes(b'unchanged accepted reference snapshot')
  before=reference.read_bytes();direct=a.audit(f.base,f.media);assert direct['status']=='passed' and direct['counts']['direct']==1,direct
  plan=f.preview()
  if copied:n.write(f.root/'rust-fault.json',{'force_copy':True})
  f.apply(plan)
  f.call({'accepting':False},route='/v1/admission')
  f.call({'command':'analysis-state','body':{}},route='/v1/metadata',status=503)
  opened=[];original=zipfile.ZipFile.open
  def guard(archive,key,*args,**kwargs):
   name=key.filename if isinstance(key,zipfile.ZipInfo) else key
   assert name in a.SCALARS,name
   opened.append(name);return original(archive,key,*args,**kwargs)
  with patch.object(zipfile.ZipFile,'open',guard):
   proof=a.audit(f.base,f.media,a.origin_client(f.url,'fixture-token'))
  assert proof['status']=='passed' and proof['counts']['native_proven']==1,proof
  assert set(opened)=={'path.npy','size.npy','mtime_ns.npy','version.npy'}
  m={'base':str(f.base),'media_root':str(f.media),'native_url':f.url,'native_token_env':'LEGACY_FIXTURE_TOKEN'}
  with patch.dict(os.environ,{'LEGACY_FIXTURE_TOKEN':'fixture-token'}):assert r.legacy_reference_gate(m,proof)['input_version']==proof['input_version']
  with sqlite3.connect(f.base/'file-operations-rust/operations.sqlite3') as db:db.execute("UPDATE operations SET status='recovery_needed' WHERE id=?",[plan['id']])
  assert a.audit(f.base,f.media,a.origin_client(f.url,'fixture-token'))['status']=='failed'
  assert reference.read_bytes()==before
  print('PASS scalar-only closed-admission native origin diagnostic',{'copy':copied,'members':opened,'references_preserved':True})
 finally:live_queue.close();f.stop()
