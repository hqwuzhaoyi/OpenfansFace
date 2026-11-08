"""HTTP contract, lease/timeout/retry, concurrency draining and control tests for Rust processing scheduler."""
import importlib.util
import json
import os
from pathlib import Path
import sqlite3
import sys
import time

spec = importlib.util.spec_from_file_location('native_fixture', Path(__file__).with_name('http-rust-file-operations.py'))
f = importlib.util.module_from_spec(spec)
spec.loader.exec_module(f)

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'maintenance'))
from processing_store import connect, reconcile, claim, fail, release_task, recover
from processing_worker import discover


def meta(x, command, body=None, expected=200):
    return x.call({'command': command, 'body': body or {}}, '/v1/metadata', status=expected)


x = f.Fixture()
try:
    db_file = x.base / 'processing/processing.db'
    for suffix in ('', '-wal', '-shm'):
        candidate = Path(str(db_file) + suffix)
        if candidate.exists():
            candidate.unlink()

    # 1. Before initialization, operator control / settings endpoints fail with 503 and do NOT create processing.db
    meta(x, 'processing-settings', {}, expected=503)
    meta(x, 'processing-control', {'action': 'pause'}, expected=503)
    meta(x, 'processing-concurrency', {'concurrency': 1}, expected=503)
    meta(x, 'processing-retry-errors', {}, expected=503)
    assert not db_file.exists()

    # 2. Discovery from Peach ledger + deduplication + exclusions + mass-drop guard
    pack = x.media / 'pikpak/My Pack/ scheduler'
    pack.mkdir(parents=True, exist_ok=True)
    v_a = pack / 'a.mp4'; v_a.write_bytes(b'video-a-bytes')
    v_b = pack / 'b.mp4'; v_b.write_bytes(b'video-b-bytes')
    v_trash = pack / 'trash.mp4'; v_trash.write_bytes(b'video-trash-bytes')
    v_queued = pack / 'queued.mp4'; v_queued.write_bytes(b'video-queued-bytes')
    v_creator = x.media / 'pikpak/creators/甲/organized.mp4'; v_creator.write_bytes(b'organized-bytes')

    with sqlite3.connect(x.ledger) as ldb:
        ldb.execute('DELETE FROM asset')
        ldb.execute(
            "INSERT INTO asset(id,location,path,name,size,disposal,medium) VALUES (10,'fixture',?,?,?,NULL,'video')",
            (x.win(v_a), 'a.mp4', v_a.stat().st_size),
        )
        ldb.execute(
            "INSERT INTO asset(id,location,path,name,size,disposal,medium) VALUES (11,'fixture',?,?,?,NULL,'video')",
            (x.win(v_b), 'b.mp4', v_b.stat().st_size),
        )
        ldb.execute(
            "INSERT INTO asset(id,location,path,name,size,disposal,medium) VALUES (12,'fixture',?,?,?,'trash','video')",
            (x.win(v_trash), 'trash.mp4', v_trash.stat().st_size),
        )
        ldb.execute(
            "INSERT INTO asset(id,location,path,name,size,disposal,medium) VALUES (13,'fixture',?,?,?,NULL,'video')",
            (x.win(v_queued), 'queued.mp4', v_queued.stat().st_size),
        )
        ldb.execute(
            "INSERT INTO asset(id,location,path,name,size,disposal,medium) VALUES (14,'fixture',?,?,?,NULL,'video')",
            (x.win(v_creator), 'organized.mp4', v_creator.stat().st_size),
        )

    with sqlite3.connect(x.base / 'faces.db') as fdb:
        fdb.execute('DELETE FROM videos')
        fdb.execute('INSERT INTO videos(vid,path) VALUES (?,?)', ('vid-a', str(v_a)))
        fdb.execute('INSERT INTO videos(vid,path) VALUES (?,?)', ('vid-queued', str(v_queued)))

    (x.base / 'integration-queue.json').write_text(
        json.dumps([{'vid': 'vid-queued', 'creator': '甲'}], ensure_ascii=False)
    )
    (x.base / 'archive-history.json').write_text('[]')

    inv = meta(x, 'processing-inventory')
    assert len(inv['assets']) == 5

    disc = meta(x, 'processing-discover', {'batch_id': 'batch-init', 'references': {'version': 'ref-v1', 'people': 1, 'images': 1}})
    assert disc['eligible'] == 2, disc
    assert disc['registered'] == 1, disc

    # Mass-drop guard: when >= 20 eligible tasks exist, a sudden drop below 20% is rejected with 503
    many_assets = []
    for idx in range(22):
        vf = pack / f'extra-{idx}.mp4'
        vf.write_bytes(f'extra-{idx}'.encode())
        many_assets.append({'id': 100 + idx, 'path': x.win(vf), 'name': vf.name, 'size': vf.stat().st_size, 'disposal': None})
    disc_many = meta(x, 'processing-discover', {'assets': many_assets, 'batch_id': 'batch-many'})
    assert disc_many['eligible'] == 23, disc_many
    drop = meta(x, 'processing-discover', {'assets': [many_assets[0]], 'batch_id': 'batch-drop'}, expected=503)
    assert '媒体可读数量突然下降' in drop['error'], drop

    # Reconcile back to 3 known tasks for lease/concurrency/control testing
    v_c = pack / 'c.mp4'; v_c.write_bytes(b'video-c-bytes')
    items = [
        {'vid': 'vid-a', 'path': str(v_a), 'name': 'a.mp4', 'size': v_a.stat().st_size, 'mtime_ns': str(v_a.stat().st_mtime_ns)},
        {'vid': 'vid-b', 'path': str(v_b), 'name': 'b.mp4', 'size': v_b.stat().st_size, 'mtime_ns': str(v_b.stat().st_mtime_ns)},
        {'vid': 'vid-c', 'path': str(v_c), 'name': 'c.mp4', 'size': v_c.stat().st_size, 'mtime_ns': str(v_c.stat().st_mtime_ns)},
    ]
    rec = meta(x, 'processing-reconcile', {'items': items, 'batch_id': 'batch-test', 'bootstrapped': True})
    assert rec['eligible'] == 3

    # 3. Concurrency settings & validation + 2 -> 1 smooth draining
    hb = meta(x, 'processing-heartbeat', {
        'capacity': 2,
        'heartbeat': {'protocol': 3, 'max_concurrency': 2, 'native_ready': True, 'waiting_native_admission': False},
    })
    assert hb['ok'] is True

    for invalid in [0, 3, '2', True, 1.5, None]:
        err = meta(x, 'processing-concurrency', {'concurrency': invalid}, expected=400)
        assert '并发数必须为 1 至 2 的整数' in err['error'], err

    s2 = meta(x, 'processing-concurrency', {'concurrency': 2})
    assert s2['effective'] == 2 and s2['requested'] == 2

    # Claim 2 tasks concurrently
    t1 = meta(x, 'processing-claim', {'capacity': 2})['task']
    t2 = meta(x, 'processing-claim', {'capacity': 2})['task']
    assert t1 is not None and t2 is not None and t1['vid'] != t2['vid']
    assert meta(x, 'processing-claim', {'capacity': 2})['task'] is None

    # Lower concurrency from 2 -> 1 while 2 are in-flight
    s1 = meta(x, 'processing-concurrency', {'concurrency': 1})
    assert s1['effective'] == 1

    # Both in-flight leases can still heartbeat
    hb_renew = meta(x, 'processing-heartbeat', {'run_ids': [t1['run_id'], t2['run_id']], 'capacity': 2})
    assert set(hb_renew['renewed']) == {t1['run_id'], t2['run_id']}
    assert hb_renew['active'] == 2 and hb_renew['concurrency'] == 1
    meta(x, 'processing-heartbeat', {'run_id': 'non-existent-run'}, expected=409)

    # Release t2 due to mount issue -> does not consume attempt budget, and with 1 still active (t1) and concurrency=1, no new task is claimed
    rel = meta(x, 'processing-release', {'task': t2, 'reason': '媒体挂载不可读，等待恢复'})
    assert rel['matched'] is True and rel['stage'] == 'retry'
    assert meta(x, 'processing-claim', {'capacity': 2})['task'] is None

    with sqlite3.connect(db_file) as con:
        row_t2 = con.execute('SELECT stage, text_attempts FROM tasks WHERE vid=?', (t2['vid'],)).fetchone()
        assert row_t2 == ('retry', 0), row_t2

    # 4. Lease timeout reaping frees the slot and transitions expired task to retry
    with sqlite3.connect(db_file) as con:
        con.execute('UPDATE tasks SET lease_expires_at=1 WHERE vid=?', (t1['vid'],))
        con.commit()

    claimed_after_reap = meta(x, 'processing-claim', {'capacity': 2})
    assert claimed_after_reap['reaped'] == 1
    t_next = claimed_after_reap['task']
    assert t_next is not None

    # 5. Single-file failure retry budget (3 attempts -> error)
    # Fail t_next up to 3 attempts
    while True:
        attempt = t_next['text_attempts']
        f_res = meta(x, 'processing-fail', {'task': t_next, 'error': '模拟解码失败'})
        if attempt < 3:
            assert f_res['stage'] == 'retry'
            with sqlite3.connect(db_file) as con:
                con.execute('UPDATE tasks SET next_retry=0 WHERE vid=?', (t_next['vid'],))
                con.commit()
            # Ensure t_next has highest priority so we claim it again
            meta(x, 'processing-control', {'action': 'priority', 'vids': [t_next['vid']]})
            t_next = meta(x, 'processing-claim', {'capacity': 2})['task']
            assert t_next['vid'] == f_res and True or t_next is not None
        else:
            assert f_res['stage'] == 'error'
            break

    # 6. Operator control actions: pause, resume, priority, retry, takeover, defer, restore, note, batch retry-errors
    err_vid = t_next['vid']
    assert meta(x, 'processing-control', {'action': 'pause'})['paused'] is True
    assert meta(x, 'processing-claim', {'capacity': 2})['task'] is None
    assert meta(x, 'processing-control', {'action': 'resume'})['paused'] is False

    # Takeover on error task succeeds; takeover on non-error task fails with 409
    other_vid = next(v for v in ('vid-a', 'vid-b', 'vid-c') if v != err_vid)
    meta(x, 'processing-control', {'action': 'takeover', 'vids': [other_vid]}, expected=409)
    meta(x, 'processing-control', {'action': 'takeover', 'vids': [err_vid]})

    # Defer / restore / note on manual (or review) task succeeds; on queued task fails with 409
    meta(x, 'processing-control', {'action': 'defer', 'vids': [other_vid]}, expected=409)
    meta(x, 'processing-control', {'action': 'defer', 'vids': [err_vid]})
    meta(x, 'processing-control', {'action': 'restore', 'vids': [err_vid]})
    meta(x, 'processing-control', {'action': 'note', 'vids': [err_vid], 'note': '操作台备注核对'})

    # Put err_vid back into non-manual error state and test batch retry-errors
    with sqlite3.connect(db_file) as con:
        con.execute("UPDATE tasks SET stage='error', manual=0, batch_id='batch-err' WHERE vid=?", (err_vid,))
        con.commit()
    meta(x, 'processing-retry-errors', {'batch': ''}, expected=400)
    retried = meta(x, 'processing-retry-errors', {'batch': 'batch-err'})
    assert retried['retried'] == 1

    # 7. Verify overview, dashboard, and home-tasks queries in Rust
    ov = meta(x, 'processing-overview', {'vid': err_vid})
    assert ov['all_count'] >= 1 and ov['task']['vid'] == err_vid
    dash = meta(x, 'processing-dashboard', {})
    assert dash['schema_version'] == 1 and dash['ledger_ready'] is True
    assert dash['totals']['total'] >= 3 and len(dash['history']) >= 1
    ht = meta(x, 'processing-home-tasks', {})
    assert ht['initialized'] is True and ht['total'] >= 1 and len(ht['jobs']) >= 2

    # 8. Verify Python processing_store / processing_worker integration with FILE_OPERATIONS_RUST_URL
    old_env = os.environ.get('FILE_OPERATIONS_RUST_URL'), os.environ.get('FILE_OPERATIONS_RUST_TOKEN')
    os.environ['FILE_OPERATIONS_RUST_URL'] = x.url
    os.environ['FILE_OPERATIONS_RUST_TOKEN'] = 'fixture-token'
    try:
        con = connect(x.base)
        try:
            recover(con)
            claimed_py = claim(con, capacity=1)
            assert claimed_py is not None and claimed_py['run_id']
            release_task(con, claimed_py, '测试释放', retry_delay=0)
            claimed_py2 = claim(con, capacity=1)
            assert claimed_py2 is not None
            fail(con, claimed_py2, '测试失败')
        finally:
            con.close()

        # 9. Verify pure compute reference staging + Rust validation/publication + artifact check & GC
        from processing_faces import build_references
        ref_meta = build_references(x.base, x.media)
        assert ref_meta['version'] and (x.base / 'processing/references.npz').is_file()

        unaccepted_run = '99999999-9999-4999-8999-999999999999'
        unaccepted_dir = x.base / 'processing/attempts' / unaccepted_run / 'thumbs'
        unaccepted_dir.mkdir(parents=True, exist_ok=True)
        (unaccepted_dir / 'leak.jpg').write_bytes(b'unaccepted')
        chk = meta(x, 'processing-artifact-check', {'attempt': unaccepted_run, 'relative': 'thumbs/leak.jpg'})
        assert chk['accepted'] is False
        evicted = meta(x, 'processing-evict-artifacts', {'grace_seconds': 0})
        assert unaccepted_run in evicted['evicted_attempts']
        assert not (x.base / 'processing/attempts' / unaccepted_run).exists()
    finally:
        if old_env[0] is None:
            os.environ.pop('FILE_OPERATIONS_RUST_URL', None)
        else:
            os.environ['FILE_OPERATIONS_RUST_URL'] = old_env[0]
        if old_env[1] is None:
            os.environ.pop('FILE_OPERATIONS_RUST_TOKEN', None)
        else:
            os.environ['FILE_OPERATIONS_RUST_TOKEN'] = old_env[1]

    print('PASS Rust processing scheduler HTTP contract: discovery, lease, timeout, retry, 2->1 draining, and console controls')
finally:
    x.stop()
