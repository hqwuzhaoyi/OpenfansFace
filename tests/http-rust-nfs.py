"""Run inside the acceptance image with LOCAL_ROOT/media bind-mounted to marked NFS.
Only seeds synthetic files/database and drives native Rust HTTP/probe operations.
LOCAL_ROOT must be a new /tmp/face-admin-person-fileops-* directory. Preserve evidence.
"""
import concurrent.futures
import hashlib
import json
import os
from pathlib import Path
import signal
import sqlite3
import subprocess
import sys
import time
import urllib.error
import urllib.request
import uuid

probe_root = Path(sys.argv[2]).resolve()
assert '.face-admin-rust-fixtures' in probe_root.parts
probe = subprocess.run(['/usr/local/bin/storage-probe', 'exercise', str(probe_root)], capture_output=True, text=True, check=True)
evidence = json.loads(probe.stdout)
probe_dir = Path(evidence['directory'])
assert evidence['server_uid'] == 1000 and evidence['server_gid'] == 1000, evidence
print(json.dumps(evidence), flush=True)
blocked = probe_dir / 'blocked'
blocked.mkdir()
blocked.chmod(0o555)
for destination in [blocked / 'target.bin', blocked / 'not-yet-created/deeper/target.bin']:
    result = subprocess.run(['/usr/local/bin/storage-probe', 'check', str(probe_root), str(probe_dir / 'source.bin'), str(destination)], capture_output=True, text=True)
    error = json.loads(result.stdout)
    assert result.returncode == 1 and error['status'] == 403 and error['step'] == 'destination_directory' and error['path'] == str(blocked), error
blocked.chmod(0o775)
print('PASS destination and nearest existing ancestor permission diagnostics', flush=True)
subprocess.run(['getfacl', '-pn', str(probe_dir)], check=True)
root = Path(sys.argv[1]).resolve()
assert root.parent == Path('/tmp') and root.name.startswith('face-admin-person-fileops-')
media = root / 'media'
assert (media / '.rust-nfs-fixture').read_text() == 'rust nfs acceptance\n'
assert not (root / 'base').exists(), 'Use a fresh fixture; never replace existing data'
(root / '.fileops-fixture').write_text('person file operations')
base = root / 'base'
base.mkdir()
source = media / 'pikpak/creators/fixture-person/original.mp4'
source.parent.mkdir(parents=True)
payload = b'isolated NFS native operation\x00\xff' * 1024
source.write_bytes(payload)
target = source.with_name('renamed.mp4')
vid = 'fixture-stable-id'
win = lambda p: 'R:\\media\\pikpak\\' + str(p.relative_to(media / 'pikpak')).replace('/', '\\')
with sqlite3.connect(base / 'faces.db') as db:
    db.executescript('CREATE TABLE videos(vid TEXT PRIMARY KEY,path TEXT,name TEXT);CREATE TABLE faces(vid TEXT,score REAL,thumb TEXT);')
    db.execute('INSERT INTO videos VALUES(?,?,?)', (vid, str(source), source.name))
with sqlite3.connect(root / 'ledger.db') as db:
    db.executescript(Path('/acceptance/peach-contract-schema.sql').read_text())
    db.execute('INSERT INTO asset(id,location,path,name) VALUES(1,?,?,?)', ('fixture', win(source), source.name))
env = {**os.environ, 'FACES_BASE': str(base), 'MEDIA2_ROOT': str(media), 'PEACH_LEDGER_PATH': str(root / 'ledger.db'), 'FILE_OPERATIONS_FIXTURE_ROOT': str(root), 'FILE_OPERATIONS_BIND': '127.0.0.1:18979', 'FILE_OPERATIONS_RUST_TOKEN': 'isolated-fixture-token'}
url = 'http://127.0.0.1:18979'
log = (root / 'service.log').open('wb')
process = subprocess.Popen(['/usr/local/bin/face-file-operations'], env=env, stdout=log, stderr=log)

def call(body=None, route='/v1/person-files'):
    req = urllib.request.Request(url + route, data=None if body is None else json.dumps(body).encode(), headers={'Content-Type': 'application/json', 'Authorization': 'Bearer isolated-fixture-token'})
    try:
        with urllib.request.urlopen(req, timeout=25) as r:
            return r.status, json.load(r)
    except urllib.error.HTTPError as e:
        return e.code, json.load(e)

def request():
    stat = source.stat()
    version = hashlib.sha256(json.dumps([str(source), str(stat.st_dev), str(stat.st_ino), str(stat.st_size), str(stat.st_mtime_ns)], separators=(',', ':')).encode()).hexdigest()
    return {'id': str(uuid.uuid4()), 'action': 'rename-file', 'person': 'fixture-person', 'files': [{'vid': vid, 'path': str(source), 'version': version}], 'filename': target.name}

def preview(req=None):
    return call({'mode': 'preview', 'request': req or request()})

def apply(plan):
    return call({'mode': 'apply', 'request': plan['request'], 'version': plan['version']})

def fault(phase, mode, **extra):
    (root / 'rust-fault.json').write_text(json.dumps({'phase': phase, 'mode': mode, **extra}))

try:
    for _ in range(100):
        if process.poll() is not None:
            raise AssertionError((root / 'service.log').read_text())
        try:
            call(route='/health'); break
        except OSError:
            time.sleep(.05)
    else:
        raise AssertionError('service startup timeout')
    # Both preview and the final pre-admission check consult the server's mapped identity.
    source.parent.chmod(0o555)
    code, error = preview()
    assert code == 403 and error['step'] == 'source_directory' and error['path'] == str(source.parent), (code, error)
    assert not target.exists()
    source.parent.chmod(0o775)
    code, plan = preview()
    assert code == 200, plan
    source.parent.chmod(0o555)
    code, error = apply(plan)
    assert code == 403 and error['step'] == 'source_directory' and error['path'] == str(source.parent), (code, error)
    assert source.read_bytes() == payload and not target.exists() and not (base / '.review-write.lock').exists()
    source.parent.chmod(0o775)
    print('PASS HTTP preview and execution permission denial, concrete source_directory/path', flush=True)
    # Hold one actual worker at its durable fixture checkpoint; overload its bounded queue.
    fault('destination_publish_intent', 'pause', milliseconds=4000)
    with concurrent.futures.ThreadPoolExecutor(max_workers=48) as pool:
        active = pool.submit(apply, plan)
        for _ in range(100):
            _, state = call(route='/v1/person-files?id=' + plan['id'])
            if state['phase'] == 'destination_publish_intent': break
            time.sleep(.02)
        else: raise AssertionError('worker did not reach checkpoint')
        futures = [pool.submit(apply, plan) for _ in range(46)]
        # Status and liveness remain responsive while media execution is held.
        start = time.monotonic()
        assert call(route='/health')[0] == 200
        assert call(route='/v1/person-files?id=' + plan['id'])[1]['status'] == 'running'
        assert time.monotonic() - start < 2
        results = [active.result()] + [f.result() for f in futures]
    busy = [v for c, v in results if c == 503 and v['code'] == 'writer_busy']
    assert busy and all(c == 200 or v['code'] == 'writer_busy' for c, v in results), results
    assert source.exists() is False and target.read_bytes() == payload
    assert call(route='/v1/person-files?id=' + plan['id'])[1]['status'] == 'complete'
    assert apply(plan)[0] == 200
    with sqlite3.connect(base / 'faces.db') as db:
        assert db.execute('SELECT path FROM videos WHERE vid=?', (vid,)).fetchone()[0] == str(target)
    with sqlite3.connect(root / 'ledger.db') as db:
        assert db.execute('SELECT path FROM asset WHERE id=1').fetchone()[0] == win(target)
        assert db.execute('SELECT state FROM _face_admin_person_ops WHERE id=?', (plan['id'],)).fetchone()[0] == 'committed'
    print(f'PASS native NFS rename + local metadata + Peach receipt; bounded queue rejected {len(busy)}; status responsive; idempotent retry', flush=True)
    (root / 'rust-fault.json').unlink()
    # Test deterministic kernel-style failures through the marked fixture-only checkpoint.
    # A real disconnect of a shared production mount is deliberately never attempted.
    source, target = target, source
    _, plan = preview()
    injected_errno = os.environ.get('NFS_FIXTURE_ERRNO', 'ESTALE')
    fault('destination_publish_intent', 'io_error', errno=injected_errno)
    code, error = apply(plan)
    assert code == 503 and error['code'] == 'recovery_required' and ('Stale' if injected_errno == 'ESTALE' else 'Input/output' if injected_errno == 'EIO' else 'timed out') in error['error'], error
    assert source.read_bytes() == payload and not target.exists()
    assert call(route='/v1/person-files?id=' + plan['id'])[1]['status'] == 'recovery_needed'
    assert (base / '.review-write.lock').exists()
    print(f'PASS injected {injected_errno} after admission -> recovery_needed with original bytes and retained gate', flush=True)
    print(json.dumps({'root': str(root), 'process_uid': os.geteuid(), 'process_gid': os.getegid(), 'server_uid': source.stat().st_uid, 'server_gid': source.stat().st_gid, 'checks': 'passed'}), flush=True)
finally:
    process.send_signal(signal.SIGINT)
    process.wait(timeout=10)
    log.close()
