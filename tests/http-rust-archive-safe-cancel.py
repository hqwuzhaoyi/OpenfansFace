"""Narrow own-archive rollback, never delete/coalesce scanner rows or retry production."""
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


def late_case(kind='pure',crash=None,after_crash=None):
    f=a.Fixture(3)
    try:
        original=r.seed_refs(f);f.confirm();preview=f.preview();op=str(uuid.uuid4())
        destinations=[f.creators/'Momoyih'/p.name for p in f.sources]
        phase='destination_published:2' if kind=='before_reserve' else 'ledger_commit_intent'
        f.fault(phase,milliseconds=600)
        with concurrent.futures.ThreadPoolExecutor() as pool:
            work=pool.submit(f.apply,preview,None,op);a.wait_fault(f)
            scanner=r.insert_scanner_row(f,destinations[0])
            with sqlite3.connect(f.ledger) as db:
                if kind=='manual':db.execute('UPDATE asset SET rating=5 WHERE id=1001')
                elif kind=='field_owner':db.execute("UPDATE asset SET field_owners='{\"name\":\"manual\"}' WHERE id=1001")
                elif kind=='unexpected_reference':
                    db.execute('CREATE TABLE asset_search_manual_notes(asset_id INTEGER, note TEXT)');db.execute("INSERT INTO asset_search_manual_notes VALUES(1001,'keep user note')")
                elif kind=='source_links':db.execute("INSERT INTO asset_entity(asset_id,entity_id,role,source) VALUES(1,7,'creator','new external link')")
                elif kind=='receipt_signature':db.execute("UPDATE _face_admin_person_ops SET signature='changed' WHERE id=?",(op,))
                elif kind=='foreign_key':db.execute("INSERT INTO asset_tag(asset_id,tag,source) VALUES(1001,'manual','user')")
                elif kind=='non_fk':db.execute("INSERT INTO want_item(origin,state,acquired_asset_id,created_at,updated_at) VALUES('code','acquired',1001,'fixture','fixture')")
                elif kind=='source_changed':db.execute("UPDATE asset SET feedback='new external edit' WHERE id=1")
                elif kind=='missing_receipt':db.execute('DELETE FROM _face_admin_person_ops WHERE id=?',(op,))
                elif kind=='committed_receipt':db.execute("UPDATE _face_admin_person_ops SET state='committed' WHERE id=?",(op,))
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
                    preserved=r.snapshot(f);foreign=r.rows(f.ledger,'SELECT * FROM asset WHERE id=1001')
                f.start();result=f.call(id=op)['result']
        safe=kind=='pure' and after_crash is None
        assert result.get('rolled_back') is safe,result
        assert result.get('recovery_needed') is (not safe),result
        assert r.snapshot(f)==preserved and r.rows(f.ledger,'SELECT * FROM asset WHERE id=1001')==foreign
        if safe:
            assert all(p.read_bytes()==a.BYTES+str(i).encode() for i,p in enumerate(f.sources))
            assert not any(p.exists() for p in destinations) and len(f.queue())==3
            assert not (f.base/'.review-write.lock').exists()
            assert result['external_index_conflicts'] and not result['ok'] and result['moved']==0
            assert all(row['retryable'] is False for row in result['failures'])
            assert r.snapshot(f)==original
            with sqlite3.connect(f.ledger) as db:assert db.execute('SELECT state FROM _face_admin_person_ops WHERE id=?',(op,)).fetchone()==('cancelled',)
            # No new archive: exact same operation returns its terminal rollback receipt.
            assert f.apply(preview,op=op)==result
            f.stop();f.start();assert f.call(id=op)['result']==result
            current=f.preview();assert current['n_ok']==2
        else:
            assert (f.base/'.review-write.lock').exists()
            assert all(row['retryable'] is False for row in result['failures'])
        r.integrity(f)
        print('PASS',kind,crash or 'no-crash',after_crash or '',flush=True)
    finally:f.stop()


if __name__=='__main__':
    preview_isolates_one_collision()
    late_case()
    for kind in ('manual','field_owner','foreign_key','non_fk','unexpected_reference','source_links','receipt_signature','source_changed','missing_receipt','committed_receipt','file_identity','before_reserve'):late_case(kind)
    for phase in ('compensation_intent','compensation_ledger_fenced','compensation_file_restored'):late_case(crash=phase)

    for changed in ('remove_index','manual_reference'):late_case(crash='compensation_ledger_fenced',after_crash=changed)
