"""Control-file adversarial acceptance; real FIFOs/symlinks/sparse files, isolated fixtures only."""
import importlib.util
import json
import os
from pathlib import Path
import signal
import stat
import subprocess
import time

spec = importlib.util.spec_from_file_location('native_fixture', Path(__file__).with_name('http-rust-file-operations.py'))
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)
Fixture, PAYLOAD = fixture.Fixture, fixture.PAYLOAD


def metadata_and_routes():
    f = Fixture()
    try:
        path = f.base / 'report.json'
        original = path.read_bytes()
        foreign = f.root / 'foreign-report.json'
        foreign.write_bytes(original)
        for kind in ('fifo', 'symlink', 'oversize'):
            path.unlink()
            if kind == 'fifo': os.mkfifo(path)
            elif kind == 'symlink': path.symlink_to(foreign)
            else:
                with path.open('wb') as out: out.truncate(256 * 1024 * 1024 + 1)
            start = time.monotonic()
            error = f.preview(status=409 if kind == 'symlink' else 503)
            assert error['path'] == str(path), error
            if kind != 'symlink': assert error['step'] == 'control_read', error
            assert time.monotonic() - start < 2
            assert f.source.read_bytes() == PAYLOAD and not f.target.exists()
            assert foreign.read_bytes() == original
            path.unlink()
            path.write_bytes(original)
        for kind in ('fifo', 'symlink'):
            request = f.request()
            marker = f.base / 'file-operation-routing' / (request['id'] + '.json')
            if kind == 'fifo': os.mkfifo(marker)
            else: marker.symlink_to(foreign)
            start = time.monotonic()
            error = f.preview(request, 409 if kind == 'symlink' else 503)
            assert error['path'] == str(marker) and time.monotonic() - start < 2
            assert marker.is_symlink() or stat.S_ISFIFO(marker.stat().st_mode)
        # A real legacy-sized metadata document remains supported, not limited to marker size.
        report = json.loads(original)
        report['legacy_audit'] = 'x' * (2 * 1024 * 1024)
        path.write_text(json.dumps(report))
        f.stop()
        f.env['FILE_OPERATIONS_CONTROL_MAX_BYTES'] = str(1024 * 1024)
        f.start()
        assert f.preview(status=503)['code'] == 'control_file_too_large'
        f.stop()
        f.env['FILE_OPERATIONS_CONTROL_MAX_BYTES'] = str(3 * 1024 * 1024)
        f.start()
        plan = f.preview()
        assert f.apply(plan)['ok']
        f.stop(); f.start()
        assert f.apply(plan) == f.assert_complete(plan)
        assert json.loads(path.read_text())['legacy_audit'] == report['legacy_audit']
        print('PASS FIFO/symlink/oversize metadata + routing refused promptly; large legacy JSON and normal restart preserved')
    finally: f.stop()


def gate_restart():
    for kind in ('fifo', 'symlink', 'oversize'):
        f = Fixture()
        try:
            plan = f.preview()
            f.fault('destination_publish_intent', 'exit')
            try: f.apply(plan)
            except OSError: pass
            assert f.process.wait(timeout=5) == 86
            (f.root / 'rust-fault.json').unlink()
            f.stop()
            marker = f.base / '.review-write.lock'
            saved = f.root / 'original-gate.json'
            marker.rename(saved)
            if kind == 'fifo': os.mkfifo(marker)
            elif kind == 'symlink': marker.symlink_to(saved)
            else:
                with marker.open('wb') as out: out.truncate(1024 * 1024 + 1)
            start = time.monotonic()
            f.start()
            assert time.monotonic() - start < 2
            state = f.status(plan)
            assert state['status'] == 'recovery_needed', state
            assert f.source.read_bytes() == PAYLOAD and not f.target.exists()
            f.apply(plan, 503)
            assert marker.lstat() and saved.exists()
            print('PASS restart preserves foreign', kind, 'gate and keeps recovery protection')
        finally: f.stop()


def executor_and_fixture_marker():
    f = Fixture()
    try:
        f.stop()
        for path in (f.base / 'file-operations-rust/executor.lock', f.root / '.fileops-fixture'):
            saved = path.with_name(path.name + '.saved')
            path.rename(saved)
            for kind in ('fifo', 'symlink'):
                if kind == 'fifo': os.mkfifo(path)
                else: path.symlink_to(saved)
                start = time.monotonic()
                result = subprocess.run([str(fixture.BINARY)], env=f.env, capture_output=True, timeout=3)
                assert result.returncode != 0 and time.monotonic() - start < 2, result
                assert path.lstat() and saved.exists()
                path.unlink()
            saved.rename(path)
        f.start()
        plan = f.preview()
        assert f.apply(plan)['ok']
        print('PASS FIFO/symlink executor lock and fixture marker fail startup promptly; original authority restarts normally')
    finally: f.stop()


metadata_and_routes()
gate_restart()
executor_and_fixture_marker()
