"""Ambiguous filesystem RPC responses, using marked local fixtures only.

No shared NFS disruption. `mutation_error_after` returns the selected errno after
real publication/removal. `mutation_error_delayed` returns it at the native write
boundary; this harness then releases that simulated remote effect after response
and restart. Neither mode is an HTTP timeout or cancellation.
"""
import importlib.util
import os
import http.client
from pathlib import Path
import sqlite3

spec = importlib.util.spec_from_file_location('native_fixture', Path(__file__).with_name('http-rust-file-operations.py'))
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)


def assert_protected(f, plan, position):
    state = f.status(plan)
    assert state['status'] == 'recovery_needed' and state['io_state'] == 'unknown', state
    assert state['failure']['mutation_outcome'] == 'unknown', state
    assert (f.base / '.review-write.lock').exists()
    assert (f.source.exists(), f.target.exists()) == position
    for path in [f.source, f.target]:
        if path.exists():
            assert path.read_bytes() == fixture.PAYLOAD
    with sqlite3.connect(f.base / 'file-operations-rust/operations.sqlite3') as db:
        assert db.execute("SELECT COUNT(*) FROM steps WHERE operation_id=? AND phase='compensation_intent'", (plan['id'],)).fetchone()[0] == 0
        assert db.execute("SELECT COUNT(*) FROM steps WHERE operation_id=? AND phase IN ('faces_commit_intent','jobs_commit_intent')", (plan['id'],)).fetchone()[0] == 0
    with sqlite3.connect(f.ledger) as db:
        assert db.execute('SELECT state FROM _face_admin_person_ops WHERE id=?', (plan['id'],)).fetchone()[0] == 'prepared'
        assert db.execute('SELECT path FROM asset WHERE id=1').fetchone()[0] == f.win(f.source)
    with sqlite3.connect(f.base / 'faces.db') as db:
        assert db.execute('SELECT path FROM videos WHERE vid=?', (fixture.VID,)).fetchone()[0] == str(f.source)
    return state


def ambiguous(step, errno, delayed=False):
    f = fixture.Fixture()
    try:
        plan = f.preview()
        # A second accepted preview must remain unable to mutate under the held gate.
        other = f.preview(f.request(filename='other.mp4'))
        f.fault('mutation:' + step, 'mutation_error_delayed' if delayed else 'mutation_error_after', errno=errno)
        error = f.apply(plan, 503)
        assert error['code'] == 'recovery_required' and error['mutation_outcome'] == 'unknown', error
        position = (True, step == 'source_remove') if delayed else (step != 'source_remove', True)
        assert_protected(f, plan, position)
        (f.root / 'rust-fault.json').unlink()
        f.stop(); f.start()
        assert_protected(f, plan, position)
        if delayed:
            # Release exactly one delayed server-side syscall, only within the
            # disposable fixture. The application must not race it with rollback.
            if step == 'destination_publish':
                os.link(f.source, f.target)
                position = (True, True)
            else:
                f.source.unlink()
                position = (False, True)
        for _ in range(3):
            state = f.call({'mode': 'reconcile', 'request': plan['request'], 'version': plan['version']})
            assert state['status'] == 'recovery_needed'
            f.apply(plan, 503)
            f.apply(other, 503)
            assert_protected(f, plan, position)
        f.stop(); f.start()
        assert_protected(f, plan, position)
        # No further native mutation attempts, including destructive repeated unlink.
        with sqlite3.connect(f.base / 'file-operations-rust/operations.sqlite3') as db:
            starts = db.execute("SELECT COUNT(*) FROM steps WHERE operation_id=? AND phase='filesystem_mutation_intent'", (plan['id'],)).fetchone()[0]
            assert starts == (1 if step == 'destination_publish' else 2), starts
        assert not f.source.with_name('other.mp4').exists()
        print('PASS ambiguous native mutation', step, errno, 'delayed' if delayed else 'after effect', flush=True)
    finally:
        f.stop()


def unresolved_crash():
    f = fixture.Fixture()
    try:
        plan = f.preview()
        # Simulate death after durable intent but before a syscall result can be
        # journaled. Even unchanged current paths do not prove remote quiescence.
        f.fault('filesystem_mutation_intent', 'exit')
        try:
            f.apply(plan)
        except (OSError, http.client.RemoteDisconnected):
            pass
        assert f.process.wait(timeout=5) == 86
        (f.root / 'rust-fault.json').unlink()
        f.stop(); f.start()
        assert_protected(f, plan, (True, False))
        assert f.call({'mode': 'reconcile', 'request': plan['request'], 'version': plan['version']})['status'] == 'recovery_needed'
        assert_protected(f, plan, (True, False))
        print('PASS unresolved syscall journal on restart', flush=True)
    finally:
        f.stop()


def ambiguous_compensation():
    f = fixture.Fixture()
    try:
        plan = f.preview()
        with sqlite3.connect(f.ledger) as db:
            db.executescript("CREATE TRIGGER fixture_reject BEFORE UPDATE OF path ON asset BEGIN SELECT RAISE(ABORT,'fixture rollback'); END;")
        f.fault('mutation:compensation_destination_remove', 'mutation_error_after', errno='ESTALE')
        assert f.apply(plan, 503)['mutation_outcome'] == 'unknown'
        # The reverse link and destination unlink actually ran. Their apparent
        # rollback position does not license declaring rollback or releasing gate.
        assert f.source.read_bytes() == fixture.PAYLOAD and not f.target.exists()
        (f.root / 'rust-fault.json').unlink()
        with sqlite3.connect(f.base / 'file-operations-rust/operations.sqlite3') as db:
            starts = db.execute("SELECT COUNT(*) FROM steps WHERE operation_id=? AND phase='filesystem_mutation_intent'", (plan['id'],)).fetchone()[0]
        f.stop(); f.start()
        for _ in range(2):
            state = f.call({'mode': 'reconcile', 'request': plan['request'], 'version': plan['version']})
            assert state['status'] == 'recovery_needed' and state['failure']['mutation_outcome'] == 'unknown'
            assert (f.base / '.review-write.lock').exists()
            assert f.source.read_bytes() == fixture.PAYLOAD and not f.target.exists()
            f.apply(plan, 503)
        with sqlite3.connect(f.base / 'file-operations-rust/operations.sqlite3') as db:
            assert starts == db.execute("SELECT COUNT(*) FROM steps WHERE operation_id=? AND phase='filesystem_mutation_intent'", (plan['id'],)).fetchone()[0]
        print('PASS ambiguous compensation unlink cannot become false rolled_back', flush=True)
    finally:
        f.stop()


for step in ['destination_publish', 'source_remove']:
    for errno in ['EIO', 'ESTALE', 'ETIMEDOUT']:
        ambiguous(step, errno)
    ambiguous(step, 'ETIMEDOUT', delayed=True)
unresolved_crash()
ambiguous_compensation()
# Existing pre-effect errors and disconnected HTTP clients keep their semantics.
fixture.proven_storage_failure()
fixture.disconnected_client()
