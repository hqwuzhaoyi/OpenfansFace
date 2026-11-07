"""Native service acceptance with real files/SQLite and crash checkpoints. Never production.

Usage: python3 tests/http-rust-file-operations.py [--binary services/file-operations/target/debug/face-file-operations]
The Python code only seeds/asserts fixtures. Every tested operation runs in the Rust binary.
"""
import argparse
import hashlib
import http.client
import json
import os
from pathlib import Path
import signal
import socket
import sqlite3
import subprocess
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid

REPO = Path(__file__).resolve().parents[1]
SCHEMA = (REPO / 'tests/fixtures/peach-contract-schema.sql').read_text()
parser = argparse.ArgumentParser()
parser.add_argument('--binary', type=Path, default=REPO / 'services/file-operations/target/debug/face-file-operations')
parser.add_argument('--port',type=int,default=0)
args = parser.parse_args()
BINARY = args.binary.resolve()
VID = r'R:\media\pikpak\My Telegram\stable.mp4'
PAYLOAD = b'isolated fixture video bytes\x00\xff' * 1024


def write(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, ensure_ascii=False))


class Fixture:
    def __init__(self):
        self.root = Path(tempfile.mkdtemp(prefix='face-admin-person-fileops-rust-', dir='/tmp')).resolve()
        (self.root / '.fileops-fixture').write_text('person file operations')
        self.base = self.root / 'base'
        self.media = self.root / 'media'
        self.base.mkdir()
        self.source = self.media / 'pikpak/creators/甲/original.mp4'
        self.source.parent.mkdir(parents=True)
        self.source.write_bytes(PAYLOAD)
        self.target = self.source.with_name('renamed.mp4')
        self.ledger = self.root / 'ledger.db'
        with sqlite3.connect(self.base / 'faces.db') as db:
            db.executescript('CREATE TABLE videos(vid TEXT PRIMARY KEY,path TEXT,name TEXT,collection TEXT,size INTEGER);CREATE TABLE faces(vid TEXT,score REAL,thumb TEXT);')
            db.execute('INSERT INTO videos VALUES(?,?,?,?,?)', (VID, str(self.source), self.source.name, str(self.source.parent), len(PAYLOAD)))
        write(self.base / 'archive-history.json', [{'vid': VID, 'dst': str(self.source), 'win_dst': self.win(self.source), 'creator': '甲', 'original_vid': VID}])
        write(self.base / 'report.json', {'identities': [{'name': '甲', 'vids': [{'vid': VID, 'name': self.source.name, 'path': str(self.source), 'text': str(self.source)}]}]})
        write(self.base / 'external-profiles/manual-links.json', {'links': [{'creator': '甲', 'profile': {'id': 'unchanged', 'name': self.source.name}}]})
        with sqlite3.connect(self.ledger) as db:
            db.executescript(SCHEMA)
            db.execute('INSERT INTO asset(id,location,path,name) VALUES(1,?,?,?)', ('fixture', self.win(self.source), 'preserved ledger title'))
            db.execute("INSERT INTO entity(id,kind,canonical_name,normalized_name,created_at,updated_at) VALUES(1,'creator','甲','甲','fixture','fixture')")
            for source in ('manual', 'import'):
                db.execute("INSERT INTO asset_entity(asset_id,entity_id,role,source) VALUES(1,1,'creator',?)", (source,))
            db.execute("INSERT INTO entity_alias(entity_id,alias,normalized_alias,source) VALUES(1,'alias','alias','fixture')")
        (self.base / 'processing').mkdir()
        with sqlite3.connect(self.base / 'processing/processing.db') as db:
            db.executescript("PRAGMA journal_mode=WAL;CREATE TABLE tasks(vid TEXT PRIMARY KEY,path TEXT NOT NULL UNIQUE,name TEXT NOT NULL,size INTEGER NOT NULL,mtime_ns TEXT NOT NULL,stage TEXT NOT NULL,updated_at REAL NOT NULL,data TEXT NOT NULL);")
            db.execute('INSERT INTO tasks VALUES(?,?,?,?,?,?,?,?)', (VID, str(self.source), self.source.name, len(PAYLOAD), str(self.source.stat().st_mtime_ns), 'face', 0, json.dumps({'vid': VID, 'path': str(self.source), 'name': self.source.name, 'text': str(self.source)})))
            db.execute("INSERT INTO tasks VALUES('unrelated','other.mp4','other.mp4',1,'0','face',0,'{}')")
        with socket.socket() as s:
            s.bind(('127.0.0.1', 0))
            self.port = args.port or s.getsockname()[1]
        self.url = f'http://127.0.0.1:{self.port}'
        self.env = {**os.environ, 'FACES_BASE': str(self.base), 'MEDIA2_ROOT': str(self.media), 'PEACH_LEDGER_PATH': str(self.ledger), 'FILE_OPERATIONS_BIND': f'127.0.0.1:{self.port}', 'FILE_OPERATIONS_FIXTURE_ROOT': str(self.root), 'FILE_OPERATIONS_RUST_TOKEN': 'fixture-token', 'PATH': '/no-executables'}
        self.process = None
        self.start()

    def win(self, path):
        return 'R:\\media\\pikpak\\' + str(path.relative_to(self.media / 'pikpak')).replace('/', '\\')

    def start(self):
        self.log = (self.root / 'service.log').open('ab')
        self.process = subprocess.Popen([str(BINARY)], env=self.env, stdout=self.log, stderr=self.log)
        for _ in range(600):
            if self.process.poll() is not None:
                raise AssertionError((self.process.returncode, (self.root / 'service.log').read_text()))
            try:
                urllib.request.urlopen(self.url + '/health', timeout=.2).close()
                return
            except OSError:
                time.sleep(.02)
        raise AssertionError('service did not start')

    def stop(self):
        if self.process and self.process.poll() is None:
            self.process.send_signal(signal.SIGINT)
            self.process.wait(timeout=10)
        self.log.close()

    def call(self, body=None, route='/v1/person-files', status=200, token=True):
        headers = {'Content-Type': 'application/json'}
        if token:
            headers['Authorization'] = 'Bearer fixture-token'
        request = urllib.request.Request(self.url + route, data=None if body is None else json.dumps(body).encode(), headers=headers)
        try:
            with urllib.request.urlopen(request, timeout=15) as response:
                code, value = response.status, json.load(response)
        except urllib.error.HTTPError as error:
            code, value = error.code, json.load(error)
        assert code == status, (code, status, value, self.root)
        return value

    def request(self, source=None, filename='renamed.mp4', vid=VID):
        source = source or self.source
        stat = source.stat()
        fingerprint = hashlib.sha256(json.dumps([str(source), str(stat.st_dev), str(stat.st_ino), str(stat.st_size), str(stat.st_mtime_ns)], separators=(',', ':'), ensure_ascii=False).encode()).hexdigest()
        return {'id': str(uuid.uuid4()), 'action': 'rename-file', 'person': '甲', 'files': [{'vid': vid, 'path': str(source), 'version': fingerprint}], 'filename': filename}

    def preview(self, request=None, status=200):
        return self.call({'mode': 'preview', 'request': request or self.request()}, status=status)

    def apply(self, plan, status=200):
        return self.call({'mode': 'apply', 'request': plan['request'], 'version': plan['version']}, status=status)

    def status(self, plan):
        return self.call(route='/v1/person-files?id=' + plan['id'])

    def fault(self, phase, mode='exit', **extra):
        write(self.root / 'rust-fault.json', {'phase': phase, 'mode': mode, **extra})

    def assert_complete(self, plan):
        status = self.status(plan)
        assert status['status'] == 'complete', status
        assert self.target.read_bytes() == PAYLOAD and not self.source.exists()
        assert not (self.base / '.review-write.lock').exists()
        with sqlite3.connect(self.base / 'faces.db') as db:
            assert db.execute('SELECT path,name FROM videos WHERE vid=?', (VID,)).fetchone() == (str(self.target), self.target.name)
            assert db.execute('SELECT signature FROM _face_admin_rust_commits WHERE id=?', (plan['id'],)).fetchone()[0] == plan['version']
        with sqlite3.connect(self.base / 'processing/processing.db') as db:
            path, stage, data = db.execute('SELECT path,stage,data FROM tasks WHERE vid=?', (VID,)).fetchone()
            assert path == str(self.target) and stage == 'excluded'
            data = json.loads(data)
            assert data['name'] == self.target.name and data['text'] == str(self.source) and data['vid'] == VID
        with sqlite3.connect(self.ledger) as db:
            assert db.execute('SELECT path,name FROM asset WHERE id=1').fetchone() == (self.win(self.target), 'preserved ledger title')
            assert db.execute('SELECT source FROM asset_entity ORDER BY source').fetchall() == [('import',), ('manual',)]
            assert db.execute('SELECT alias FROM entity_alias').fetchone()[0] == 'alias'
            assert db.execute('SELECT state FROM _face_admin_person_ops WHERE id=?', (plan['id'],)).fetchone()[0] == 'committed'
            assert db.execute('PRAGMA foreign_key_check').fetchall() == []
        report = json.loads((self.base / 'report.json').read_text())['identities'][0]['vids'][0]
        assert report['path'] == str(self.target) and report['name'] == self.target.name and report['text'] == str(self.source)
        assert json.loads((self.base / 'archive-history.json').read_text())[0]['original_vid'] == VID
        assert json.loads((self.base / 'video-path-overrides.json').read_text())[VID] == str(self.target)
        return status['result']


def success_and_guards():
    f = Fixture()
    try:
        f.call(route='/v1/person-files?id=' + str(uuid.uuid4()), status=401, token=False)
        f.call({'mode': 'preview', 'request': {'action': 'rename-file'}}, status=400)
        f.preview(f.request(filename='../bad.mp4'), 400)
        f.preview(f.request(filename='extension.txt'), 400)
        f.target.write_bytes(b'foreign target')
        f.preview(status=409)
        assert f.target.read_bytes() == b'foreign target'
        f.target.unlink()
        symlink = f.source.with_name('link.mp4')
        symlink.symlink_to(f.source)
        f.preview(f.request(source=symlink), 409)
        nested = f.source.parent / 'linked'
        nested.symlink_to(f.source.parent)
        f.preview(f.request(source=nested / f.source.name), 409)
        other = f.source.with_name('unindexed.mp4')
        other.write_bytes(PAYLOAD)
        f.preview(f.request(source=other, vid='file:unknown'), 409)
        outside = f.root / 'outside.mp4'
        outside.write_bytes(PAYLOAD)
        f.preview(f.request(source=outside), 400)
        with sqlite3.connect(f.ledger) as db:
            db.execute('INSERT INTO asset(location,path) VALUES(?,?)', ('another-location', f.win(f.source)))
        f.preview(status=409)
        with sqlite3.connect(f.ledger) as db:
            db.execute("DELETE FROM asset WHERE location='another-location'")
        related = f.preview()
        with sqlite3.connect(f.base / 'processing/processing.db') as db:
            db.execute('UPDATE tasks SET updated_at=1 WHERE vid=?', (VID,))
        f.apply(related, 409)
        assert f.source.read_bytes() == PAYLOAD and not f.target.exists()
        ledger_conflict = f.preview()
        with sqlite3.connect(f.ledger) as db:
            db.execute('INSERT INTO asset(location,path) VALUES(?,?)', ('fixture', f.win(f.target)))
        f.apply(ledger_conflict, 409)
        with sqlite3.connect(f.ledger) as db:
            db.execute('DELETE FROM asset WHERE path=?', (f.win(f.target),))
        stale = f.preview()
        f.source.write_bytes(PAYLOAD + b'changed')
        f.apply(stale, 409)
        f.source.write_bytes(PAYLOAD)
        late = f.preview()
        f.target.symlink_to(f.root / 'missing')
        f.apply(late, 409)
        assert f.target.is_symlink()
        f.target.unlink()
        plan = f.preview()
        f.call({'mode': 'preview', 'request': {**plan['request'], 'filename': 'changed.mp4'}}, status=409)
        lock = f.base / '.review-write.lock'
        lock.write_text('competing writer')
        f.apply(plan, 409)
        assert f.status(plan)['status'] == 'not_started' and lock.read_text() == 'competing writer'
        lock.unlink()
        # Another processing task can progress without invalidating this preview.
        with sqlite3.connect(f.base / 'processing/processing.db') as db:
            db.execute("UPDATE tasks SET stage='complete',updated_at=999 WHERE vid='unrelated'")
        result = f.apply(plan)
        assert result == f.assert_complete(plan) == f.apply(plan)
        f.stop()
        f.start()
        assert f.apply(plan) == result == f.status(plan)['result']
        # Process lifetime ownership rejects another native writer on the same state volume.
        duplicate = subprocess.run([str(BINARY)], env=f.env, capture_output=True, timeout=5)
        assert duplicate.returncode != 0 and '执行器'.encode() in duplicate.stderr
        print('PASS native constraints, metadata semantics, affected-task version, idempotency, restart receipt, process ownership')
    finally:
        f.stop()


def disconnected_client():
    f = Fixture()
    try:
        plan = f.preview()
        f.fault('destination_publish_intent', 'pause', milliseconds=400)
        client = http.client.HTTPConnection('127.0.0.1', f.port)
        client.request('POST', '/v1/person-files', json.dumps({'mode': 'apply', 'request': plan['request'], 'version': plan['version']}), {'Content-Type': 'application/json', 'Authorization': 'Bearer fixture-token'})
        for _ in range(100):
            if f.status(plan)['phase'] == 'destination_publish_intent':
                break
            time.sleep(.01)
        else:
            raise AssertionError('worker did not reach admitted checkpoint')
        client.close()
        for _ in range(100):
            if f.status(plan)['status'] == 'complete':
                break
            time.sleep(.02)
        assert f.apply(plan) == f.assert_complete(plan)
        print('PASS disconnected HTTP waiter does not cancel admitted native operation')
    finally:
        f.stop()


def interrupted(phase):
    f = Fixture()
    try:
        plan = f.preview()
        f.fault(phase)
        try:
            f.apply(plan)
            raise AssertionError('expected process termination')
        except (OSError, http.client.RemoteDisconnected):
            pass
        assert f.process.wait(timeout=5) == 86
        # Capture actual bytes and all three commit domains before restart.
        source_exists, target_exists = f.source.exists(), f.target.exists()
        assert source_exists or target_exists
        if source_exists:
            assert f.source.read_bytes() == PAYLOAD
        if target_exists:
            assert f.target.read_bytes() == PAYLOAD
        if phase in ('destination_published', 'destination_published_unrecorded'):
            assert source_exists and target_exists and f.source.stat().st_ino == f.target.stat().st_ino
        if phase in ('source_removed', 'source_removed_unrecorded'):
            assert not source_exists and target_exists
        (f.root / 'rust-fault.json').unlink()
        f.stop()
        f.start()
        status = f.status(plan)
        assert f.apply(plan) == f.assert_complete(plan)
        print('PASS crash checkpoint', phase, '->', status['status'])
    finally:
        f.stop()


def failed_commit():
    f = Fixture()
    try:
        plan = f.preview()
        # Real SQLite trigger rejection, after media/local records have committed.
        with sqlite3.connect(f.ledger) as db:
            db.executescript("CREATE TRIGGER reject_path BEFORE UPDATE OF path ON asset BEGIN SELECT RAISE(ABORT,'fixture ledger failure'); END;")
        failure = f.apply(plan, 503)
        assert failure['code'] == 'database_unavailable' and 'fixture ledger failure' in failure['error']
        assert f.status(plan)['status'] == 'rolled_back'
        assert f.source.read_bytes() == PAYLOAD and not f.target.exists()
        assert not (f.base / '.review-write.lock').exists()
        with sqlite3.connect(f.ledger) as db:
            assert db.execute('SELECT path FROM asset WHERE id=1').fetchone()[0] == f.win(f.source)
            assert db.execute('SELECT state FROM _face_admin_person_ops WHERE id=?', (plan['id'],)).fetchone()[0] == 'cancelled'
        assert f.apply(plan, 503) == failure
        print('PASS actual SQLite commit failure compensates with durable guarded receipts')
    finally:
        f.stop()


def publication_race():
    f = Fixture()
    try:
        plan = f.preview()
        f.fault('destination_publish_intent', 'pause', milliseconds=600)
        client = http.client.HTTPConnection('127.0.0.1', f.port)
        client.request('POST', '/v1/person-files', json.dumps({'mode': 'apply', 'request': plan['request'], 'version': plan['version']}), {'Content-Type': 'application/json', 'Authorization': 'Bearer fixture-token'})
        for _ in range(100):
            if f.status(plan)['phase'] == 'destination_publish_intent':
                break
            time.sleep(.01)
        else:
            raise AssertionError('publication checkpoint was not reached')
        f.target.write_bytes(b'late foreign content')
        response = client.getresponse()
        assert response.status == 503 and json.load(response)['code'] == 'recovery_required'
        client.close()
        assert f.source.read_bytes() == PAYLOAD and f.target.read_bytes() == b'late foreign content'
        assert (f.base / '.review-write.lock').exists()
        print('PASS late publication race never overwrites foreign destination')
    finally:
        f.stop()


if __name__ == '__main__':
    success_and_guards()
    disconnected_client()
    failed_commit()
    publication_race()
    for phase in ['destination_publish_intent', 'destination_published_unrecorded', 'destination_published', 'source_removed_unrecorded', 'source_removed', 'faces_commit_intent', 'faces_committed_unrecorded', 'faces_committed', 'jobs_commit_intent', 'jobs_committed_unrecorded', 'jobs_committed', 'json_replace_intent', 'json_replaced_unrecorded', 'json_replaced', 'ledger_commit_intent', 'ledger_committed_unrecorded', 'ledger_committed', 'complete']:
        interrupted(phase)
