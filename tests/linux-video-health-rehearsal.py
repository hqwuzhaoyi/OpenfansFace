#!/usr/bin/env python3
"""Linux-only #17 acceptance. No mount changes; NFS mode NEVER writes media.

Run against the final native binary, not the old ticket04 binary. Evidence is retained in
new marked /tmp/face-admin-person-fileops-health-linux-* directories. Python only seeds
local fixtures, drives HTTP, and observes/reaps processes; Rust executes health checks.
"""
import argparse
import ctypes
import hashlib
import json
import os
from pathlib import Path
import shlex
import signal
import socket
import sqlite3
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
import uuid


def bounded_wait(predicate, seconds=10, description='condition'):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        value = predicate()
        if value:
            return value
        time.sleep(.005)
    raise AssertionError('Timed out waiting for ' + description)


def proc(pid):
    try:
        # comm may contain spaces/parentheses; fields after the final ')' start at state.
        fields = Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()
        return {'pid': pid, 'state': fields[0], 'ppid': int(fields[1]), 'start': fields[19]}
    except (FileNotFoundError, ProcessLookupError):
        return None


def live(identity):
    current = proc(identity['pid'])
    return current and current['start'] == identity['start'] and current['state'] != 'Z'


def children(pid):
    result = set()
    for task in Path(f'/proc/{pid}/task').glob('*'):
        try:
            result.update(int(value) for value in (task / 'children').read_text().split())
        except FileNotFoundError:
            pass
    return result


def reap(pid, seconds=3):
    def wait():
        try:
            got, status = os.waitpid(pid, os.WNOHANG)
            return (got, os.waitstatus_to_exitcode(status)) if got else None
        except ChildProcessError:
            return None
    return bounded_wait(wait, seconds, f'adopted child {pid} waitpid')


class Service:
    def __init__(self, binary, schema, media=None, sample=None, expected_source=None):
        self.root = Path(tempfile.mkdtemp(prefix='face-admin-person-fileops-health-linux-', dir='/tmp')).resolve()
        (self.root / '.fileops-fixture').write_text('person file operations')
        self.base = self.root / 'base'
        self.base.mkdir()
        self.media = media or self.root / 'media'
        self.sample = sample or self.media / 'pikpak/creators/health-fixture/sample.mp4'
        if media is None:
            self.sample.parent.mkdir(parents=True)
            self.sample.write_bytes(b'controlled probe fixture only\n')
            telegram = self.media / 'pikpak/My Telegram/health-fixture'
            telegram.mkdir(parents=True)
            (telegram / 'fixture.txt').write_text('isolated fixture')
        with sqlite3.connect(self.base / 'faces.db') as db:
            db.executescript('CREATE TABLE videos(vid TEXT PRIMARY KEY,path TEXT,name TEXT);CREATE TABLE faces(vid TEXT,score REAL,thumb TEXT);')
            db.execute('INSERT INTO videos VALUES(?,?,?)', ('health-fixture', str(self.sample), self.sample.name))
        with sqlite3.connect(self.root / 'ledger.db') as db:
            db.executescript(schema.read_text())
        with socket.socket() as sock:
            sock.bind(('127.0.0.1', 0))
            port = sock.getsockname()[1]
        self.url = f'http://127.0.0.1:{port}'
        self.env = dict(os.environ)
        for key in ('FILE_OPERATIONS_FIXTURE_ROOT', 'VIDEO_HEALTH_FIXTURE_NOW', 'VIDEO_HEALTH_FIXTURE_PROBE', 'VIDEO_HEALTH_FIXTURE_RUN_ON_START'):
            self.env.pop(key, None)
        self.env.update(FACES_BASE=str(self.base), MEDIA2_ROOT=str(self.media), PEACH_LEDGER_PATH=str(self.root / 'ledger.db'),
                        FILE_OPERATIONS_BIND=f'127.0.0.1:{port}', FILE_OPERATIONS_RUST_TOKEN='health-fixture-' + str(uuid.uuid4()),
                        VIDEO_HEALTH_ENABLED='1', VIDEO_HEALTH_BUDGET='1', VIDEO_HEALTH_SECONDS='10')
        if media is None:
            self.env.update(FILE_OPERATIONS_FIXTURE_ROOT=str(self.root), VIDEO_HEALTH_FIXTURE_RUN_ON_START='1')
        else:
            # Production mount check path, deliberately no fixture bypass/clock/probe executable.
            self.env['MEDIA_NFS_SOURCE'] = expected_source
            (self.base / 'video-health.json').write_text(json.dumps({'schema': 2, 'status': 'never_run', 'records': {}, 'next_run_at': '2000-01-01T00:00:00Z'}))
        self.binary = binary
        self.process = None
        self.log = None
        self.owned = []
        print(json.dumps({'fixture': str(self.root), 'media': str(self.media), 'uid': os.getuid(), 'gid': os.getgid()}), flush=True)

    def script(self, body):
        path = self.root / 'controlled-ffprobe'
        path.write_text('#!/bin/sh\nif [ \"$1\" = \"--health-fixture-exec-check\" ]; then exit 0; fi\n' + body)
        path.chmod(0o755)
        self.env['VIDEO_HEALTH_FIXTURE_PROBE'] = str(path)
        try:
            subprocess.run([str(path), '--health-fixture-exec-check'], check=True, timeout=2)
        except PermissionError as error:
            mount = subprocess.run(['findmnt', '-n', '-T', str(path), '-o', 'TARGET,FSTYPE,OPTIONS'], capture_output=True, text=True, timeout=3)
            raise RuntimeError('Controlled probe cannot execute; use a dedicated executable persistent /tmp volume. Mount: ' + mount.stdout.strip()) from error

    def launch(self):
        self.log = (self.root / 'service.log').open('ab')
        self.process = subprocess.Popen([str(self.binary)], env=self.env, stdout=self.log, stderr=self.log, start_new_session=True)

    def call(self, route='/v1/video-health'):
        request = urllib.request.Request(self.url + route, headers={'Authorization': 'Bearer ' + self.env['FILE_OPERATIONS_RUST_TOKEN']})
        with urllib.request.urlopen(request, timeout=12) as response:
            return json.load(response)

    def ready(self):
        def available():
            assert self.process.poll() is None, (self.root / 'service.log').read_text()
            try:
                return self.call('/health')
            except OSError:
                return None
        bounded_wait(available, 10, 'native HTTP readiness')

    def done(self):
        return bounded_wait(lambda: (v if (v := self.call()).get('status') in ('complete', 'skipped', 'interrupted') else None), 20, 'complete scan report')

    def remember(self, pid):
        identity = proc(pid)
        assert identity, f'PID {pid} already exited before observation'
        self.owned.append(identity)
        return identity

    def stop(self):
        if sys.exc_info()[0] is not None:
            evidence = {'fixture': str(self.root), 'error': str(sys.exc_info()[1])}
            for name, path in [('service_log_tail', self.root / 'service.log'), ('report_prefix', self.base / 'video-health.json')]:
                try:
                    with path.open() as stream:
                        evidence[name] = stream.read(16384)
                except OSError as error:
                    evidence[name] = str(error)
            print(json.dumps({'health_fixture_failure': evidence}, ensure_ascii=False), flush=True)
        if self.process and self.process.poll() is None:
            self.process.send_signal(signal.SIGTERM)
            try:
                self.process.wait(12)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(3)
        # Cleanup touches only identities observed as children of this fixture service.
        for identity in self.owned:
            if live(identity):
                os.kill(identity['pid'], signal.SIGKILL)
            try:
                os.waitpid(identity['pid'], os.WNOHANG)
            except ChildProcessError:
                pass
        if self.log:
            self.log.close()


def group_timeout(binary, schema):
    service = Service(binary, schema)
    marker = service.root / 'probe-pids'
    service.script(f'/bin/sleep 30 &\necho "$$ $!" > {shlex.quote(str(marker))}\nwait\n')
    service.env['VIDEO_HEALTH_SECONDS'] = '2'
    try:
        service.launch()
        service.ready()
        bounded_wait(lambda: marker.exists() and marker.stat().st_size, 8, 'probe PID marker')
        leader_pid, child_pid = map(int, marker.read_text().split())
        leader, child = service.remember(leader_pid), service.remember(child_pid)
        assert os.getpgid(child_pid) == leader_pid, 'fixture child must share ffprobe lane process group'
        result = service.done()
        assert result['status'] == 'complete' and result['counts'].get('unavailable') == 1, result
        bounded_wait(lambda: not live(leader) and not live(child), 3, 'probe group exit')
        # Rust owns/reaps the direct probe. We adopt/reap its killed descendant.
        assert proc(leader_pid) is None, 'Rust did not reap the direct probe'
        waited, exit_code = reap(child_pid)
        assert waited == child_pid and exit_code == -signal.SIGKILL, (waited, exit_code)
        assert not list((service.base / 'file-operations-rust').glob('probe-*'))
        print(json.dumps({'case': 'probe_group_timeout', 'result': 'PASS', 'direct_probe_reaped': leader_pid, 'descendant_reaped': child_pid, 'descendant_exit': exit_code}), flush=True)
    finally:
        service.stop()


def helper_parent_death(binary, schema):
    service = Service(binary, schema)
    # Several thousand aliases point only at the same tiny isolated sample. This widens
    # the native helper's observation window without creating or touching other media.
    with sqlite3.connect(service.base / 'faces.db') as db:
        db.executemany('INSERT INTO videos VALUES(?,?,?)', [(f'helper-{n:05d}', str(service.sample), service.sample.name) for n in range(12000)])
    service.env['VIDEO_HEALTH_BUDGET'] = '0'
    helper = None
    try:
        service.launch()
        def find_helper():
            assert service.process.poll() is None, (service.root / 'service.log').read_text()
            for pid in children(service.process.pid):
                try:
                    args = Path(f'/proc/{pid}/cmdline').read_bytes().split(b'\0')
                    if b'--health-observe' not in args:
                        continue
                    identity = service.remember(pid)
                    os.kill(pid, signal.SIGSTOP)
                    return identity
                except (ProcessLookupError, FileNotFoundError):
                    continue
            return None
        helper = bounded_wait(find_helper, 10, 'native health observation helper')
        bounded_wait(lambda: (p := proc(helper['pid'])) and p['state'] in ('T', 't'), 2, 'helper stopped')
        # Kill only service PID, never its group/child. SIGKILL wakes even a stopped child
        # only because Rust installed PR_SET_PDEATHSIG before exec.
        service.process.kill()
        service.process.wait(3)
        bounded_wait(lambda: not live(helper), 3, 'native helper PDEATHSIG exit')
        waited, exit_code = reap(helper['pid'])
        assert waited == helper['pid'] and exit_code == -signal.SIGKILL, (waited, exit_code)
        service.log.close()
        service.env['VIDEO_HEALTH_ENABLED'] = '0'
        service.env['VIDEO_HEALTH_FIXTURE_RUN_ON_START'] = '0'
        service.launch()
        service.ready()
        report = service.call()
        assert report['status'] == 'interrupted' and report.get('last_success_at') is None, report
        print(json.dumps({'case': 'native_helper_parent_death', 'result': 'PASS', 'helper_reaped': waited, 'exit': exit_code, 'restart': report['status']}), flush=True)
    finally:
        service.stop()


def tree_evidence(root):
    evidence = {}
    for index, path in enumerate(root.rglob('*')):
        assert index < 32, 'NFS health fixture must contain fewer than 32 entries'
        assert not path.is_symlink(), f'symlink in fixture: {path}'
        if not path.is_file():
            continue
        stat = path.stat()
        assert stat.st_size <= 16 * 1024 * 1024, 'use small known sample only'
        evidence[str(path.relative_to(root))] = {'size': stat.st_size, 'mtime_ns': stat.st_mtime_ns, 'inode': stat.st_ino, 'sha256': hashlib.sha256(path.read_bytes()).hexdigest()}
    return evidence


def nfs_readonly(binary, schema, media, sample, expected_source):
    media = media.resolve(strict=True)
    assert '.face-admin-rust-fixtures' in media.parts, 'use only a marked isolated NFS subtree'
    assert (media / '.rust-nfs-fixture').read_text() == 'rust nfs acceptance\n'
    sample = sample.resolve(strict=True) if sample else media / 'pikpak/creators/health-fixture/sample.mp4'
    assert sample.is_file() and sample.is_relative_to(media / 'pikpak'), 'sample must be inside this fixture'
    mount = subprocess.run(['findmnt', '-n', '-T', str(media), '-o', 'FSTYPE,SOURCE,OPTIONS'], check=True, capture_output=True, text=True, timeout=5).stdout.split()
    assert len(mount) == 3 and mount[0] in ('nfs', 'nfs4') and mount[1] == expected_source, mount
    assert 'ro' in mount[2].split(','), 'bind the isolated subtree read-only for this rehearsal'
    before = tree_evidence(media)
    service = Service(binary, schema, media=media, sample=sample, expected_source=expected_source)
    try:
        service.launch()
        service.ready()
        report = service.done()
        assert report['status'] == 'complete' and report['probed'] == 1 and report['counts'] == {'healthy': 1}, report
        service.stop()
        after = tree_evidence(media)
        assert before == after, 'NFS fixture content/mtime/inode changed'
        evidence = {'case': 'real_nfs_readonly_health', 'result': 'PASS', 'mount': mount, 'media': str(media), 'sample': str(sample), 'before_after': before, 'summary': report}
        (service.root / 'nfs-health-evidence.json').write_text(json.dumps(evidence, ensure_ascii=False, indent=2))
        print(json.dumps(evidence, ensure_ascii=False), flush=True)
    finally:
        service.stop()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mode', choices=('processes', 'nfs'))
    parser.add_argument('--binary', type=Path, default=Path('/usr/local/bin/face-file-operations'))
    parser.add_argument('--schema', type=Path, default=Path(__file__).parent / 'fixtures/peach-contract-schema.sql')
    parser.add_argument('--nfs-media', type=Path)
    parser.add_argument('--sample', type=Path)
    parser.add_argument('--expected-source')
    args = parser.parse_args()
    assert sys.platform == 'linux', 'Linux /proc and PDEATHSIG are required'
    assert args.binary.is_file() and os.access(args.binary, os.X_OK)
    libc = ctypes.CDLL(None, use_errno=True)
    assert libc.prctl(36, 1, 0, 0, 0) == 0, f'PR_SET_CHILD_SUBREAPER failed: {ctypes.get_errno()}'
    def timed_out(signum, frame):
        raise TimeoutError(f'Linux health rehearsal interrupted: signal {signum}')
    signal.signal(signal.SIGALRM, timed_out)
    signal.signal(signal.SIGTERM, timed_out)
    signal.alarm(90)
    if args.mode == 'processes':
        group_timeout(args.binary.resolve(), args.schema.resolve())
        helper_parent_death(args.binary.resolve(), args.schema.resolve())
    else:
        assert args.nfs_media and args.expected_source, 'NFS mode requires explicit media and expected source'
        nfs_readonly(args.binary.resolve(), args.schema.resolve(), args.nfs_media, args.sample, args.expected_source)
    signal.alarm(0)

if __name__ == '__main__':
    main()
