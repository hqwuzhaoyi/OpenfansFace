"""Release safety on marked synthetic files. Does not access production paths."""
import importlib.util, pathlib, json, tempfile, sqlite3, hashlib, subprocess, time, threading
spec=importlib.util.spec_from_file_location('fixture', pathlib.Path(__file__).with_name('http-rust-file-operations.py'))
f=importlib.util.module_from_spec(spec);spec.loader.exec_module(f)

def hashes(root):
    return {str(p.relative_to(root)):hashlib.sha256(p.read_bytes()).hexdigest() for p in root.rglob('*') if p.is_file() and not p.name.endswith(('-wal','-shm'))}

def drain():
    x=f.Fixture()
    try:
        p=x.preview();x.fault('admitted','pause')
        result=[]
        t=threading.Thread(target=lambda:result.append(x.apply(p)));t.start()
        for _ in range(100):
            if (x.root/'rust-fault-reached.json').exists():break
            time.sleep(.02)
        else:raise AssertionError('checkpoint missing')
        state=x.call({'accepting':False},'/v1/admission')
        assert not state['drained'] and state['pending']==1,state
        assert x.call({'mode':'apply','request':p['request'],'version':p['version']},status=503)['code']=='admission_closed'
        assert x.call(route='/v1/readiness',token=False,status=401)['code']=='unauthorized'
        assert x.status(p)['status'] in ('accepted','running')
        (x.root/'rust-fault.json').unlink();t.join(timeout=10);assert result
        state=x.call(route='/v1/readiness');assert state['drained'] and not state['accepting'],state
        x.stop();x.start();state=x.call(route='/v1/readiness');assert state['drained'] and not state['accepting'],state
        assert x.call({'accepting':True},'/v1/admission')['ready']
        x.assert_complete(p)
        print('PASS persisted close, admitted work drains, receipts remain readable, reopen')
    finally:x.stop()

def parity():
    x=f.Fixture();x.stop()
    root=pathlib.Path(tempfile.mkdtemp(prefix='face-admin-parity-',dir='/tmp')).resolve()
    try:
        # Prepare a consistent copy, excluding native authority and sqlite journals.
        import shutil
        shutil.copytree(x.base,root/'base',ignore=shutil.ignore_patterns('file-operations-rust','*-wal','*-shm'))
        for source,target in [(x.ledger,root/'ledger.db'),(x.base/'faces.db',root/'base/faces.db'),(x.base/'processing/processing.db',root/'base/processing/processing.db')]:
            with sqlite3.connect(source.as_uri()+'?mode=ro',uri=True) as src,sqlite3.connect(target) as dst:src.backup(dst);dst.execute("PRAGMA wal_checkpoint(TRUNCATE)");dst.execute("PRAGMA journal_mode=DELETE")
            dst.close();src.close()
        (root/'.fileops-parity').write_text('isolated sqlite backups')
        original=hashes(x.base);media=hashes(x.media);ledger=x.ledger.read_bytes();backup=hashes(root/'base')
        auxiliary={str(p):p.read_bytes() for p in (root/'base').rglob('*-shm')}
        x.env.pop('FILE_OPERATIONS_FIXTURE_ROOT');x.env.update(FACES_BASE=str(root/'base'),PEACH_LEDGER_PATH=str(root/'ledger.db'),FILE_OPERATIONS_PARITY_ROOT=str(root),VIDEO_HEALTH_ENABLED='1')
        x.start();p=x.preview();assert p['files'][0]['src']==str(x.source)
        for route,body in [('/v1/person-files',{'mode':'apply','request':p['request'],'version':p['version']}),('/v1/person-files',{'mode':'reconcile','request':p['request']}),('/v1/person-batch',{'mode':'apply'}),('/v1/archive',{'command':'queue-confirm','body':{}}),('/v1/metadata',{'command':'processing-result','body':{}}),('/v1/video-health',{})]:
            assert x.call(body,route,status=403)['code']=='read_only_parity'
        assert x.call({'accepting':True},'/v1/admission',status=403)['code']=='read_only_parity'
        assert x.call(route='/v1/readiness')['mode']=='parity'
        x.stop()
        assert hashes(x.base)==original and hashes(x.media)==media and x.ledger.read_bytes()==ledger
        assert hashes(root/'base')==backup and not (root/'base/file-operation-routing').exists()
        assert {str(p):p.read_bytes() for p in (root/'base').rglob('*-shm')}==auxiliary
        proc=subprocess.run([str(f.BINARY)],env=x.env,capture_output=True,timeout=10)
        assert proc.returncode!=0 and b'parity_configuration' in proc.stderr,proc.stderr
        print('PASS parity previews, no source/snapshot business writes, every mutation denied, no scheduler/recovery, reused authority rejected')
    finally:
        x.stop()

drain();parity()
