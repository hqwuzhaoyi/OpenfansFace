"""Reproduce late scanner insertion against real Rust and full Peach schema.

Investigation only: seeds isolated files/SQLite, never repairs/deletes a foreign
row. Existing ledger_commit_intent checkpoint brackets actual file publication
and transaction commit. Successful exit means the known race was reproduced.
"""
import concurrent.futures
import hashlib
import importlib.util
import json
from pathlib import Path
import socket
import sqlite3
import sys
import urllib.error
import urllib.request
import uuid

ROOT=Path(__file__).resolve().parents[1]
# The fixture parser supplies --binary/--port. Port zero selects an unused local port.
spec=importlib.util.spec_from_file_location('archive_fixture',Path(__file__).with_name('http-rust-archive.py'))
a=importlib.util.module_from_spec(spec);spec.loader.exec_module(a)
if not a.args.port:
    with socket.socket() as sock:sock.bind(('127.0.0.1',0));a.args.port=sock.getsockname()[1]
a.SCHEMA=(ROOT/'tests/fixtures/peach-full-schema.sql').read_text()+"\nINSERT INTO ledger_revision(tbl,n) SELECT name,0 FROM sqlite_master WHERE type='table';\n"


def rows(file,sql,args=()):
    with sqlite3.connect(file) as db:
        db.row_factory=sqlite3.Row
        return [dict(row) for row in db.execute(sql,args)]


def seed_refs(f):
    with sqlite3.connect(f.ledger) as db:
        db.execute('PRAGMA foreign_keys=ON')
        for index,source in enumerate(f.sources,1):
            db.execute("UPDATE asset SET rating=5,play_count=7,feedback='keep user feedback',size=?,hash_kind='sha256',hash=? WHERE id=?",(source.stat().st_size,hashlib.sha256(source.read_bytes()).hexdigest(),index))
            db.execute("INSERT INTO asset_entity(asset_id,entity_id,role,source,metadata_json) VALUES(?,7,'creator','fixture-manual','{\"keep\":true}')",(index,))
            db.execute("INSERT INTO asset_tag(asset_id,tag,source) VALUES(?,'keep-tag','fixture-user')",(index,))
            db.execute("INSERT INTO activity_event(asset_id,kind,occurred_at,source,event_key) VALUES(?,'play','fixture-time','fixture-user',?)",(index,'fixture-event-'+str(index)))
    return snapshot(f)


def snapshot(f):
    return {'assets':rows(f.ledger,'SELECT * FROM asset WHERE id<=? ORDER BY id',(len(f.ids),)),
            'links':rows(f.ledger,'SELECT * FROM asset_entity ORDER BY asset_id,entity_id,role,source'),
            'tags':rows(f.ledger,'SELECT * FROM asset_tag ORDER BY asset_id,tag'),
            'activities':rows(f.ledger,'SELECT * FROM activity_event ORDER BY id')}


def insert_scanner_row(f,target):
    # Separate SQLite connection. This is an observed filesystem path and basic
    # scan metadata; no old asset IDs/relations are copied or modified.
    with sqlite3.connect(f.ledger,timeout=2) as scanner:
        scanner.execute('PRAGMA foreign_keys=ON')
        scanner.execute("INSERT INTO asset(id,location,path,name,medium,size,mtime,first_seen,last_seen) VALUES(1001,'fixture',?,?,'video',?,'fixture-mtime','fixture-scanner','fixture-scanner')",(f.win(target),target.name,target.stat().st_size if target.exists() else 123))
    return rows(f.ledger,'SELECT * FROM asset WHERE id=1001')[0]


def integrity(f):
    with sqlite3.connect(f.ledger) as db:
        assert db.execute('PRAGMA integrity_check').fetchall()==[('ok',)]
        assert not db.execute('PRAGMA foreign_key_check').fetchall()
        assert db.execute('SELECT count(*) FROM asset_search').fetchone()[0]==db.execute('SELECT count(*) FROM asset').fetchone()[0]


def replay(f,preview,operation):
    body={'command':'archive-apply','body':{'vids':f.ids,'version':preview['version'],'operation_id':operation}}
    request=urllib.request.Request(f.url+'/v1/archive',data=json.dumps(body).encode(),headers={'Content-Type':'application/json','Authorization':'Bearer archive-fixture'})
    try:
        with urllib.request.urlopen(request,timeout=20) as response:return response.status,json.load(response)
    except urllib.error.HTTPError as error:return error.code,json.load(error)


def race():
    f=a.Fixture(3)
    try:
        before=seed_refs(f);f.confirm();preview=f.preview();operation=str(uuid.uuid4())
        destinations=[f.creators/'Momoyih'/source.name for source in f.sources]
        f.fault('ledger_commit_intent',milliseconds=2500)
        with concurrent.futures.ThreadPoolExecutor(max_workers=1) as pool:
            future=pool.submit(f.apply,preview,None,operation);a.wait_fault(f)
            assert all(dst.is_file() and not src.exists() for src,dst in zip(f.sources,destinations))
            assert rows(f.ledger,'SELECT path FROM asset ORDER BY id')==[{'path':vid} for vid in f.ids]
            scanner=insert_scanner_row(f,destinations[0])
            result=future.result()
        assert not result['ok'],result
        assert result['recovery_needed'] and not result['rolled_back'],result
        assert snapshot(f)==before
        assert rows(f.ledger,'SELECT * FROM asset WHERE id=1001')==[scanner]
        assert all(dst.read_bytes()==a.BYTES+str(i).encode() for i,dst in enumerate(destinations))
        assert all(not source.exists() for source in f.sources)
        assert (f.base/'.review-write.lock').exists()
        integrity(f)
        observed={'source_files_present':sum(p.exists() for p in f.sources),'target_files_present':sum(p.exists() for p in destinations),'queue_count':len(f.queue()),'original_asset_rows_and_refs_unchanged':snapshot(f)==before,'scanner_row_preserved':True,'shared_gate_retained':True}
        (f.root/'rust-fault.json').unlink()
        retry_status,retry_result=replay(f,preview,operation)
        assert snapshot(f)==before and rows(f.ledger,'SELECT * FROM asset WHERE id=1001')==[scanner]
        f.stop();f.start()
        receipt=f.call(id=operation)
        assert receipt['result']['recovery_needed']
        assert snapshot(f)==before and rows(f.ledger,'SELECT * FROM asset WHERE id=1001')==[scanner]
        integrity(f)
        report={'case':'late_scanner_after_publish_before_ledger_commit','fixture_root':str(f.root),'count':3,'scanner_insertions':1,'checkpoint':'ledger_commit_intent','file_publish_observed_before_insert':True,'result':result,'state':observed,'same_operation_retry':{'http_status':retry_status,'result':retry_result},'after_restart':receipt}
        (f.root/'scanner-race-evidence.json').write_text(json.dumps(report,ensure_ascii=False,indent=2))
        return report
    finally:f.stop()


def controls():
    reports=[]
    for collision in (False,True):
        f=a.Fixture(1)
        try:
            before=seed_refs(f);f.confirm();dst=f.creators/'Momoyih'/f.sources[0].name
            if collision:scanner=insert_scanner_row(f,dst)
            preview=f.preview()
            if collision:
                result=f.call('archive-apply',{'vids':f.ids,'version':preview['version'],'operation_id':str(uuid.uuid4())},status=409)
                assert f.sources[0].exists() and not dst.exists() and len(f.queue())==1
                assert snapshot(f)==before and rows(f.ledger,'SELECT * FROM asset WHERE id=1001')==[scanner]
            else:
                result=f.apply(preview);assert result['ok'] and result['moved']==1,result
                after=snapshot(f)
                assert after['links']==before['links'] and after['tags']==before['tags'] and after['activities']==before['activities']
                assert after['assets'][0]['id']==1 and after['assets'][0]['path']==f.win(dst)
                assert after['assets'][0]['rating']==5 and after['assets'][0]['feedback']=='keep user feedback'
            integrity(f);reports.append({'case':'preexisting_target_collision' if collision else 'no_scanner_control','fixture_root':str(f.root),'result':result})
        finally:f.stop()
    return reports


if __name__=='__main__':
    results=controls()+[race()]
    print(json.dumps({'reproduced':True,'scope':'isolated real Rust/full Peach schema; no production or repair',
      'source_head':'a6a2c1bb','binary_sha256':hashlib.sha256(a.args.binary.read_bytes()).hexdigest(),
      'schema_sha256':hashlib.sha256((ROOT/'tests/fixtures/peach-full-schema.sql').read_bytes()).hexdigest(),'cases':results},ensure_ascii=False,indent=2))
