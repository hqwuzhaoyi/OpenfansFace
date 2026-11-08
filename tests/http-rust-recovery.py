"""Rust #3 crash/reconciliation acceptance. Only explicitly marked, disposable fixtures.

python3 tests/http-rust-recovery.py --port 18977
"""
import importlib.util
import http.client
import json
from pathlib import Path
import signal
import sqlite3
import subprocess
import time

spec = importlib.util.spec_from_file_location('native_fixture', Path(__file__).with_name('http-rust-file-operations.py'))
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)
Fixture, PAYLOAD, VID = fixture.Fixture, fixture.PAYLOAD, fixture.VID


def post_async(f, plan):
    client = http.client.HTTPConnection('127.0.0.1', f.port, timeout=15)
    client.request('POST', '/v1/person-files', json.dumps({'mode':'apply','request':plan['request'],'version':plan['version']}), {'Content-Type':'application/json','Authorization':'Bearer fixture-token'})
    return client


def killed(f, plan, phase):
    f.fault(phase,'pause',milliseconds=10000)
    client = post_async(f,plan)
    reached = f.root / 'rust-fault-reached.json'
    for _ in range(500):
        if reached.exists() and json.loads(reached.read_text())['phase'] == phase:
            break
        time.sleep(.01)
    else:
        raise AssertionError(('checkpoint not reached',phase,f.root))
    f.process.send_signal(signal.SIGKILL)
    assert f.process.wait(timeout=5) == -signal.SIGKILL
    client.close()
    (f.root/'rust-fault.json').unlink()
    f.stop()


def counters(f):
    for path, table in [(f.base/'faces.db','videos'),(f.base/'processing/processing.db','tasks'),(f.ledger,'asset')]:
        with sqlite3.connect(path) as db:
            db.executescript(f'CREATE TABLE fixture_writes(n INTEGER); INSERT INTO fixture_writes VALUES(0); CREATE TRIGGER fixture_count AFTER UPDATE OF path ON {table} BEGIN UPDATE fixture_writes SET n=n+1; END;')


def counts(f):
    out=[]
    for path in [f.base/'faces.db',f.base/'processing/processing.db',f.ledger]:
        with sqlite3.connect(path) as db:
            out.append(db.execute('SELECT n FROM fixture_writes').fetchone()[0])
    return out


def reconcile(f,plan,status=200):
    return f.call({'mode':'reconcile','request':plan['request'],'version':plan['version']},status=status)


def kill_window(phase):
    f=Fixture()
    try:
        counters(f)
        plan=f.preview()
        inode=f.source.stat().st_ino
        killed(f,plan,phase)
        f.start()
        if phase=='gate_acquired':
            assert f.status(plan)['status']=='cancelled'
            f.apply(plan,409)
            assert f.source.read_bytes()==PAYLOAD and not f.target.exists()
            assert counts(f)==[0,0,0]
        else:
            result=f.assert_complete(plan)
            assert f.target.stat().st_ino==inode
            assert counts(f)==[1,1,1], (phase,counts(f))
            assert f.apply(plan)==result
            assert reconcile(f,plan)['result']==result
            f.stop();f.start()
            assert f.apply(plan)==result and counts(f)==[1,1,1]
        assert not (f.base/'.review-write.lock').exists()
        print('PASS SIGKILL',phase,flush=True)
    finally:f.stop()


def compensation_window(phase):
    f=Fixture()
    try:
        counters(f)
        plan=f.preview()
        before={p:p.read_bytes() for p in f.base.rglob('*.json')}
        with sqlite3.connect(f.ledger) as db:
            db.executescript("CREATE TRIGGER fixture_reject BEFORE UPDATE OF path ON asset BEGIN SELECT RAISE(ABORT,'fixture rollback'); END;")
        killed(f,plan,phase)
        f.start()
        status=f.status(plan)
        assert status['status']=='rolled_back',status
        assert 'fixture rollback' in status['error'],status
        assert f.source.read_bytes()==PAYLOAD and not f.target.exists()
        assert not (f.base/'.review-write.lock').exists()
        for path,raw in before.items():assert path.read_bytes()==raw,path
        assert not (f.base/'video-path-overrides.json').exists()
        assert counts(f)==[2,2,0],counts(f)
        with sqlite3.connect(f.ledger) as db:
            assert db.execute('SELECT state FROM _face_admin_person_ops WHERE id=?',(plan['id'],)).fetchone()==('cancelled',)
        for path in [f.base/'faces.db',f.base/'processing/processing.db']:
            with sqlite3.connect(path) as db:
                receipt=json.loads(db.execute('SELECT data FROM _face_admin_rust_commits WHERE id=?',(plan['id'],)).fetchone()[0])
                assert receipt['state']=='rolled_back'
        previous=counts(f)
        f.apply(plan,503)
        f.stop();f.start()
        assert f.status(plan)['status']=='rolled_back' and counts(f)==previous
        print('PASS compensation SIGKILL',phase,flush=True)
    finally:f.stop()


def fence_requests():
    f=Fixture()
    try:
        request=f.request()
        # Passive lookup has no side effects; explicit reconcile durably fences an absent POST.
        f.call(route='/v1/person-files?id='+request['id'],status=404)
        value=f.call({'mode':'reconcile','request':request})
        assert value['status']=='cancelled'
        f.preview(request,409)
        f.call({'mode':'apply','request':request,'version':'never-arrived'},status=409)
        f.call({'mode':'reconcile','request':{**request,'filename':'other.mp4'}},status=409)
        f.stop();f.start()
        f.preview(request,409)
        plan=f.preview()
        assert reconcile(f,plan)['status']=='cancelled'
        f.apply(plan,409)
        assert reconcile(f,plan)['status']=='cancelled'
        reconcile(f,{**plan,'request':{**plan['request'],'filename':'different.mp4'}},409)
        assert f.source.read_bytes()==PAYLOAD and not f.target.exists()
        # A delayed submit queued while another executor call is waiting remains the same ID.
        other=f.preview()
        f.fault('destination_publish_intent','pause',milliseconds=600)
        client=post_async(f,other)
        for _ in range(100):
            if f.status(other)['phase']=='destination_publish_intent':break
            time.sleep(.01)
        status=f.status(other)
        assert status['io_state']=='running'
        duplicate=subprocess.Popen([str(fixture.BINARY)],env=f.env,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
        assert duplicate.wait(timeout=5)!=0  # cannot fence a live old executor by stealing its gate
        result=f.apply(other)
        assert result==f.assert_complete(other)
        assert json.load(client.getresponse())==result
        client.close()
        assert reconcile(f,other)['status']=='complete'
        print('PASS absent POST, delayed POST, changed scope, restart fence, live executor exclusion',flush=True)
    finally:f.stop()


def foreign_state(kind):
    f=Fixture();external=None
    try:
        counters(f)
        plan=f.preview()
        phase='ledger_committed_unrecorded' if kind=='committed-ledger' else 'faces_committed_unrecorded'
        killed(f,plan,phase)
        if kind=='target':
            f.target.rename(f.target.with_suffix('.owned'))
            f.target.write_bytes(b'foreign target')
        elif kind=='source':f.source.write_bytes(b'foreign source')
        elif kind=='both-missing':f.target.rename(f.target.with_suffix('.owned'))
        elif kind=='json':(f.base/'report.json').write_text('{"external":"retained"}')
        elif kind=='faces':
            with sqlite3.connect(f.base/'faces.db') as db:db.execute("UPDATE videos SET name='foreign' WHERE vid=?",(VID,))
        elif kind=='committed-ledger':
            with sqlite3.connect(f.ledger) as db:db.execute("UPDATE asset SET name='foreign' WHERE id=1")
        elif kind=='gate':
            lock=f.base/'.review-write.lock';original=lock.read_bytes();lock.rename(f.base/'owned-gate');lock.write_bytes(original)
        elif kind=='ledger-unavailable':
            external=sqlite3.connect(f.ledger);external.execute('BEGIN EXCLUSIVE')
        count_before=counts(f) if external is None else None
        f.start()
        status=f.status(plan)
        assert status['status']=='recovery_needed',status
        assert status['io_state']=='unknown'
        assert (f.base/'.review-write.lock').exists()
        if external:
            external.rollback();external.close();external=None
            assert reconcile(f,plan)['status']=='complete'
            f.assert_complete(plan)
        else:
            f.apply(plan,503)
            assert reconcile(f,plan)['status']=='recovery_needed'
            assert counts(f)==count_before
            if kind=='target':assert f.target.read_bytes()==b'foreign target'
            if kind=='source':assert f.source.read_bytes()==b'foreign source'
        print('PASS unknown/foreign outcome',kind,flush=True)
    finally:
        if external:external.close()
        f.stop()


def replaced_authority():
    f=Fixture()
    try:
        plan=f.preview();killed(f,plan,'destination_published_unrecorded')
        lock=f.base/'file-operations-rust/executor.lock'
        lock.rename(lock.with_suffix('.original'));lock.touch()
        child=subprocess.run([str(fixture.BINARY)],env=f.env,capture_output=True,timeout=5)
        assert child.returncode!=0 and b'writer_authority_changed' in child.stderr
        assert f.source.exists() and f.target.exists()
        print('PASS replaced executor lock rejects authority takeover',flush=True)
    finally:f.stop()


def unadmitted_gate_proof(kind):
    f=Fixture()
    try:
        plan=f.preview();killed(f,plan,'gate_acquired')
        lock=f.base/'.review-write.lock'
        if kind=='replaced':
            raw=lock.read_bytes();lock.rename(f.base/'original-gate');lock.write_bytes(raw)
        else:
            # Simulate death between marker fsync and durable gate proof.
            with sqlite3.connect(f.base/'file-operations-rust/operations.sqlite3') as db:
                db.execute('DELETE FROM gates WHERE id=?',(plan['id'],))
        f.start()
        assert lock.exists() and f.source.read_bytes()==PAYLOAD and not f.target.exists()
        f.apply(plan,409)
        assert reconcile(f,plan)['status']=='cancelled'
        assert lock.exists()  # cancellation alone does not establish marker ownership
        print('PASS unadmitted gate proof',kind,flush=True)
    finally:f.stop()


fence_requests()
for phase in ['gate_acquired','admitted','ledger_reserve_intent','ledger_reserved','destination_publish_intent','destination_published_unrecorded','destination_published','source_remove_intent','source_removed_unrecorded','source_removed','faces_commit_intent','faces_committed_unrecorded','faces_committed','jobs_commit_intent','jobs_committed_unrecorded','jobs_committed','json_replace_intent','json_replaced_unrecorded','json_replaced','ledger_commit_intent','ledger_committed_unrecorded','ledger_committed','final_receipt_intent','final_receipt_unreleased','complete'] + [phase+':'+name for name in ['report.json','archive-history.json','video-path-overrides.json'] for phase in ['json_replace_intent','json_replaced_unrecorded']]:
    kill_window(phase)
for phase in ['compensation_intent','compensation_ledger_fenced','compensation_documents_restored','compensation_jobs_restored','compensation_faces_restored','compensation_source_published','compensation_destination_remove_intent','compensation_destination_removed','compensation_file_restored'] + [phase+':'+name for name in ['report.json','archive-history.json','video-path-overrides.json'] for phase in ['compensation_json_restore_intent','compensation_json_restored']]:
    compensation_window(phase)
for kind in ['target','source','both-missing','json','faces','committed-ledger','gate','ledger-unavailable']:
    foreign_state(kind)
replaced_authority()

for kind in ['replaced','missing-proof']:
    unadmitted_gate_proof(kind)
