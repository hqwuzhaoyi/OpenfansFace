"""Isolated full-Peach-schema fixtures for the archive safety regressions.

Historical failing reproduction is retained in commit b0f2292 and private
context/archive-scanner-race-*.json; this helper has no failure-as-success test.
Run http-rust-archive-safe-cancel.py for current behavior.
"""
import hashlib
import importlib.util
import json
from pathlib import Path
import socket
import sqlite3

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


if __name__=='__main__':
    raise SystemExit('Fixture helper only; run tests/http-rust-archive-safe-cancel.py')
