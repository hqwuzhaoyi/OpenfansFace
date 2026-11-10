"""Scanner coordination, atomic adoption of pure blank scanner rows, strict conflict rejection, and crash/rollback safety."""
import concurrent.futures
import importlib.util
import json
from pathlib import Path
import sqlite3
import uuid

spec=importlib.util.spec_from_file_location('race',Path(__file__).with_name('archive-scanner-fixture.py'))
r=importlib.util.module_from_spec(spec);spec.loader.exec_module(r)
a=r.a


def preview_isolates_one_collision():
    f=a.Fixture(3)
    try:
        before=r.seed_refs(f);f.confirm();dst=f.creators/'Momoyih'/f.sources[0].name
        scanner=r.insert_scanner_row(f,dst);preview=f.preview()
        assert preview['n_ok']==2 and preview['rows'][0]['status']=='invalid',preview
        assert '账本' in preview['rows'][0]['reason'] and preview['rows'][0]['dst']==str(dst)
        result=f.apply(preview,ids=f.ids[1:]);assert result['ok'] and result['moved']==2,result
        assert f.sources[0].exists() and not dst.exists() and [x['vid'] for x in f.queue()]==f.ids[:1]
        assert r.rows(f.ledger,'SELECT * FROM asset WHERE id=1001')==[scanner]
        assert r.rows(f.ledger,'SELECT * FROM asset WHERE id=1')==before['assets'][:1]
        r.integrity(f)
        print('PASS preview isolates blocked row; explicitly selected other two archive',flush=True)
    finally:f.stop()


def seed_peach_state(f, dst):
    state_dir = f.ledger.parent / 'state'
    state_dir.mkdir(parents=True, exist_ok=True)
    (state_dir / 'library-new-assets.json').write_text(json.dumps([1001, 2002]))
    (state_dir / 'local-file-identities.json').write_text(json.dumps({
        '1001': {
            'id': 1001,
            'location': 'fixture',
            'path': f.win(dst),
            'native': str(dst),
            'identity': [1, 2, dst.stat().st_size if dst.exists() else 0, 4, 5],
        }
    }))
    if not (state_dir / 'library-processing-control.json').exists():
        (state_dir / 'library-processing-control.json').write_text(json.dumps({'paused': False}))
    return state_dir


def late_case(kind='pure',crash=None,after_crash=None):
    f=a.Fixture(3)
    try:
        state_dir=f.ledger.parent/'state'
        state_dir.mkdir(parents=True,exist_ok=True)
        (state_dir/'library-processing-control.json').write_text(json.dumps({'paused':False}))
        original=r.seed_refs(f);f.confirm();preview=f.preview();op=str(uuid.uuid4())
        destinations=[f.creators/'Momoyih'/p.name for p in f.sources]
        if kind=='before_reserve':
            phase='destination_published:2'
        else:
            phase='ledger_commit_intent'
        f.fault(phase,milliseconds=600)
        with concurrent.futures.ThreadPoolExecutor() as pool:
            work=pool.submit(f.apply,preview,None,op);a.wait_fault(f)
            with sqlite3.connect(f.ledger) as db:
                lease=db.execute('SELECT state FROM _face_admin_scanner_leases WHERE id=?',(op,)).fetchone()
                assert lease==('active',),lease
            control=json.loads((state_dir/'library-processing-control.json').read_text())
            assert control.get('paused') is True and control.get('archive_lease',{}).get('operation_id')==op,control
            scanner=r.insert_scanner_row(f,destinations[0])
            seed_peach_state(f,destinations[0])
            with sqlite3.connect(f.ledger) as db:
                if kind=='manual':db.execute('UPDATE asset SET rating=5 WHERE id=1001')
                elif kind=='field_owner':db.execute("UPDATE asset SET field_owners='{\"name\":\"manual\"}' WHERE id=1001")
                elif kind=='unexpected_reference':
                    db.execute('CREATE TABLE asset_search_manual_notes(asset_id INTEGER, note TEXT)');db.execute("INSERT INTO asset_search_manual_notes VALUES(1001,'keep user note')")
                elif kind in ('uppercase_reference','mixed_reference','plural_reference'):
                    column={'uppercase_reference':'ASSET_ID','mixed_reference':'Acquired_Asset_Id','plural_reference':'Asset_Ids'}[kind]
                    db.execute(f'CREATE TABLE external_manual_notes("{column}" TEXT, note TEXT)')
                    db.execute(f'INSERT INTO external_manual_notes("{column}",note) VALUES(?,?)',('1001','keep user evidence'))
                elif kind=='fake_fts_reference':
                    db.execute('DROP TABLE asset_search')
                    db.execute('CREATE TABLE asset_search(asset_id INTEGER, name TEXT, code TEXT, entities TEXT, search_terms TEXT, note TEXT)')
                    db.execute("INSERT INTO asset_search(asset_id,note) SELECT id,CASE WHEN id=1001 THEN 'keep user evidence' END FROM asset")
                elif kind=='source_links':db.execute("INSERT INTO asset_entity(asset_id,entity_id,role,source) VALUES(1,7,'creator','new external link')")
                elif kind=='receipt_signature':db.execute("UPDATE _face_admin_person_ops SET signature='changed' WHERE id=?",(op,))
                elif kind=='foreign_key':db.execute("INSERT INTO asset_tag(asset_id,tag,source) VALUES(1001,'manual','user')")
                elif kind=='non_fk':db.execute("INSERT INTO want_item(origin,state,acquired_asset_id,created_at,updated_at) VALUES('code','acquired',1001,'fixture','fixture')")
                elif kind=='source_changed':db.execute("UPDATE asset SET feedback='new external edit' WHERE id=1")
                elif kind=='missing_receipt':db.execute('DELETE FROM _face_admin_person_ops WHERE id=?',(op,))
                elif kind=='committed_receipt':db.execute("UPDATE _face_admin_person_ops SET state='committed' WHERE id=?",(op,))
                elif kind=='rollback_pure':
                    db.execute("CREATE TRIGGER force_rollback BEFORE UPDATE OF path ON asset WHEN OLD.id=3 BEGIN SELECT RAISE(ABORT,'forced rollback'); END")
            if kind=='file_identity':
                file=destinations[0];old=file.read_bytes();file.rename(file.with_suffix('.foreign-backup'));file.write_bytes(old)
            preserved=r.snapshot(f);foreign=r.rows(f.ledger,'SELECT * FROM asset WHERE id=1001')
            if crash:f.fault(crash,'exit')
            try:result=work.result()
            except Exception:
                if not crash:raise
                f.process.wait(timeout=10);assert f.process.returncode==86
                (f.root/'rust-fault.json').unlink();f.stop()
                if after_crash:
                    with sqlite3.connect(f.ledger) as db:
                        if after_crash=='remove_index':db.execute('DELETE FROM asset WHERE id=1001')
                        elif after_crash=='manual_reference':db.execute("INSERT INTO asset_tag(asset_id,tag,source) VALUES(1001,'late manual relation','user')")
                        elif after_crash=='post_commit_tamper':db.execute("DELETE FROM asset_entity WHERE asset_id=1 AND role='creator'")
                    preserved=r.snapshot(f);foreign=r.rows(f.ledger,'SELECT * FROM asset WHERE id=1001')
                f.start();result=f.call(id=op)['result']
        reconciled = kind in ('pure','before_reserve') and after_crash is None
        safe_rollback = kind == 'rollback_pure' and after_crash is None
        if reconciled:
            assert result['ok'] and result['moved']==3,result
            assert result.get('reconciled_scanner_assets')==[1001],result
            assert not result.get('rolled_back') and not result.get('recovery_needed'),result
            assert not any(p.exists() for p in f.sources)
            assert all(p.read_bytes()==a.BYTES+str(i).encode() for i,p in enumerate(destinations))
            assert not f.queue() and not (f.base/'.review-write.lock').exists()
            expected = {
                **original,
                'assets': [
                    {**row, 'path': f.win(destinations[i])}
                    for i, row in enumerate(original['assets'])
                ],
            }
            assert r.snapshot(f) == expected
            with sqlite3.connect(f.ledger) as db:
                assert db.execute('SELECT * FROM asset WHERE id=1001').fetchall()==[]
                assert db.execute('SELECT state FROM _face_admin_person_ops WHERE id=?',(op,)).fetchone()==('committed',)
                assert db.execute('SELECT state FROM _face_admin_scanner_leases WHERE id=?',(op,)).fetchone()==('committed',)
            assert json.loads((state_dir/'library-new-assets.json').read_text())==[2002]
            identities=json.loads((state_dir/'local-file-identities.json').read_text())
            assert '1001' not in identities and identities['1']['path']==f.win(destinations[0])
            control=json.loads((state_dir/'library-processing-control.json').read_text())
            assert control.get('paused') is False and 'archive_lease' not in control
            assert f.apply(preview,op=op)==result
            f.stop();f.start();assert f.call(id=op)['result']==result
        elif safe_rollback:
            assert result.get('rolled_back') is True and not result.get('recovery_needed'),result
            assert r.snapshot(f)==preserved and r.rows(f.ledger,'SELECT * FROM asset WHERE id=1001')==foreign
            assert all(p.read_bytes()==a.BYTES+str(i).encode() for i,p in enumerate(f.sources))
            assert not any(p.exists() for p in destinations) and len(f.queue())==3
            assert not (f.base/'.review-write.lock').exists()
            assert result['external_index_conflicts'] and not result['ok'] and result['moved']==0
            assert all(row['retryable'] is False for row in result['failures'])
            assert r.snapshot(f)==original
            with sqlite3.connect(f.ledger) as db:
                assert db.execute('SELECT state FROM _face_admin_person_ops WHERE id=?',(op,)).fetchone()==('cancelled',)
                assert db.execute('SELECT state FROM _face_admin_scanner_leases WHERE id=?',(op,)).fetchone()==('rolled_back',)
            control=json.loads((state_dir/'library-processing-control.json').read_text())
            assert control.get('paused') is False and 'archive_lease' not in control
            assert f.apply(preview,op=op)==result
            f.stop();f.start();assert f.call(id=op)['result']==result
            current=f.preview();assert current['n_ok']==2
        else:
            assert result.get('rolled_back') is False and result.get('recovery_needed') is True,result
            if after_crash!='post_commit_tamper':
                assert r.snapshot(f)==preserved and r.rows(f.ledger,'SELECT * FROM asset WHERE id=1001')==foreign
            assert (f.base/'.review-write.lock').exists()
            assert all(row['retryable'] is False for row in result['failures'])
            with sqlite3.connect(f.ledger) as db:
                assert db.execute('SELECT state FROM _face_admin_scanner_leases WHERE id=?',(op,)).fetchone()==('recovery_needed',)
            control=json.loads((state_dir/'library-processing-control.json').read_text())
            assert control.get('paused') is False and 'archive_lease' not in control
        r.integrity(f)
        print('PASS',kind,crash or 'no-crash',after_crash or '',flush=True)
    finally:f.stop()


def scanner_lease_expiry_and_user_pause():
    f=a.Fixture(1)
    try:
        state_dir=f.ledger.parent/'state'
        state_dir.mkdir(parents=True,exist_ok=True)
        # 1. Stale active lease from a crashed process is recovered on startup without blocking Peach or archive.
        with sqlite3.connect(f.ledger) as db:
            db.execute("""CREATE TABLE IF NOT EXISTS _face_admin_scanner_leases (
                id TEXT PRIMARY KEY, action TEXT NOT NULL, state TEXT NOT NULL,
                scope TEXT NOT NULL, acquired_at INTEGER NOT NULL, expires_at INTEGER NOT NULL, updated_at INTEGER NOT NULL
            )""")
            db.execute("INSERT INTO _face_admin_scanner_leases VALUES('stale-op','archive','active','{}',1,2,1)")
        (state_dir/'library-processing-control.json').write_text(json.dumps({
            'paused': True,
            'archive_lease': {'holder': 'face-file-operations', 'operation_id': 'stale-op', 'acquired_at': 1, 'expires_at': 2},
        }))
        (f.base/'file-operations-rust/scanner-coordination.json').write_text(json.dumps({
            'operation_id': 'stale-op', 'state': 'active', 'acquired_at': 1, 'expires_at': 2,
        }))
        f.stop();f.start()
        with sqlite3.connect(f.ledger) as db:
            state=db.execute("SELECT state FROM _face_admin_scanner_leases WHERE id='stale-op'").fetchone()[0]
            assert state in ('expired','recovered'),state
        control=json.loads((state_dir/'library-processing-control.json').read_text())
        assert control.get('paused') is False and 'archive_lease' not in control,control
        # 2. User-initiated pause in Peach control file is preserved across archive execution.
        (state_dir/'library-processing-control.json').write_text(json.dumps({'paused': True, 'job_id': 'user-manual-pause'}))
        f.confirm();preview=f.preview();result=f.apply(preview)
        assert result['ok'] and result['moved']==1,result
        control=json.loads((state_dir/'library-processing-control.json').read_text())
        assert control=={'paused': True, 'job_id': 'user-manual-pause'},control
        print('PASS stale scanner lease recovery and user pause preservation',flush=True)
    finally:f.stop()


if __name__=='__main__':
    preview_isolates_one_collision()
    late_case('pure')
    late_case('before_reserve')
    for phase in ('ledger_committed_unrecorded','final_receipt_unreleased'):
        late_case('pure',crash=phase)
        late_case('before_reserve',crash=phase)
    late_case('pure',crash='ledger_committed_unrecorded',after_crash='post_commit_tamper')
    for kind in ('manual','field_owner','foreign_key','non_fk','unexpected_reference','uppercase_reference','mixed_reference','plural_reference','fake_fts_reference','source_links','receipt_signature','source_changed','missing_receipt','committed_receipt','file_identity'):late_case(kind)
    late_case('rollback_pure')
    for phase in ('compensation_intent','compensation_ledger_fenced','compensation_file_restored'):late_case('rollback_pure',crash=phase)
    for changed in ('remove_index','manual_reference'):late_case('rollback_pure',crash='compensation_ledger_fenced',after_crash=changed)
    scanner_lease_expiry_and_user_pause()
