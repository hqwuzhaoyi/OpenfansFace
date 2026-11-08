"""Actual native closed admission + actual worker boot; marked files, no inference."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time

ROOT=Path(__file__).resolve().parents[1]
spec=importlib.util.spec_from_file_location('native_fixture',Path(__file__).with_name('http-rust-file-operations.py'))
n=importlib.util.module_from_spec(spec);spec.loader.exec_module(n)
f=n.Fixture();worker=None
try:
    f.call({'accepting':False},route='/v1/admission')
    f.call({'command':'state','body':{'scope':{}}},route='/v1/metadata',status=503)
    f.call({'command':'analysis-state','body':{}},route='/v1/metadata',status=503)
    reference=f.base/'processing/references.npz';reference.write_bytes(b'marked prior reference sentinel')
    database=f.base/'processing/processing.db';before=database.read_bytes()
    env={**os.environ,'FACES_BASE':str(f.base),'MEDIA2_ROOT':str(f.media),
         'FILE_OPERATIONS_RUST_URL':f.url,'FILE_OPERATIONS_RUST_TOKEN':'fixture-token',
         'PROCESSING_FACE_BACKEND':'cpu','FACE_ADMIN_URL':'http://127.0.0.1:1'}
    with (f.root/'worker.log').open('wb') as log:
        worker=subprocess.Popen([sys.executable,str(ROOT/'maintenance/processing_worker.py')],env=env,stdout=log,stderr=log)
        heartbeat=f.base/'processing/heartbeat.json';deadline=time.monotonic()+10
        while time.monotonic()<deadline:
            assert worker.poll() is None,(f.root/'worker.log').read_text()
            if heartbeat.exists():
                state=json.loads(heartbeat.read_text())
                if state.get('waiting_native_admission') and state.get('at',0)>0:break
            time.sleep(.05)
        else:raise AssertionError('missing closed-admission heartbeat')
        first=state['at'];time.sleep(2.2);state=json.loads(heartbeat.read_text())
        assert state['at']>first and state['active']==0 and state['native_ready'] is False
        assert worker.poll() is None and database.read_bytes()==before
        assert reference.read_bytes()==b'marked prior reference sentinel'
        assert not f.call(route='/v1/readiness')['accepting']
        worker.send_signal(signal.SIGTERM);worker.wait(timeout=8);assert worker.returncode==0
        assert json.loads(heartbeat.read_text())['at']==0
    print(json.dumps({'passed':True,'fixture_root':str(f.root),'native_binary_sha256':hashlib.sha256(n.BINARY.read_bytes()).hexdigest(),
      'worker_alive_while_closed':True,'fresh_waiting_heartbeat':True,'database_and_reference_unchanged':True,
      'native_still_closed':True,'sigterm_exit':0,'no_models_or_vectors':True}))
finally:
    if worker and worker.poll() is None:worker.terminate();worker.wait(timeout=8)
    f.stop()
