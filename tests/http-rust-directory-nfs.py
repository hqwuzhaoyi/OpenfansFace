"""Opt-in owned NFS fixture: missing person directory, archive, and compensation.

The operator supplies a fresh /tmp/face-admin-person-fileops-* root with only its
media subdirectory mounted from a fresh marked NFS fixture. Never use real media.
"""
import importlib.util
import json
import os
from pathlib import Path
import sqlite3
from unittest.mock import patch

root = Path(os.environ['DIRECTORY_NFS_FIXTURE_ROOT']).resolve()
assert root.parent == Path('/tmp') and root.name.startswith('face-admin-person-fileops-')
assert (root/'media/.rust-nfs-fixture').read_text() == 'rust nfs acceptance\n'
assert not (root/'base').exists()
spec = importlib.util.spec_from_file_location('archive', Path(__file__).with_name('http-rust-archive.py'))
a = importlib.util.module_from_spec(spec); spec.loader.exec_module(a)
a.SCHEMA = (Path(__file__).parent/'fixtures/peach-full-schema.sql').read_text()+"\nINSERT INTO ledger_revision(tbl,n) SELECT name,0 FROM sqlite_master WHERE type='table';\n"
with patch.object(a.tempfile, 'mkdtemp', return_value=str(root)):
    f = a.Fixture(2)
try:
    f.confirm(); preview=f.preview(); assert preview['n_ok']==2
    if os.environ.get('DIRECTORY_NFS_ROLLBACK') == '1':
        f.fault('ledger_commit_intent','error')
    result=f.apply(preview)
    with sqlite3.connect(f.base/'file-operations-rust/operations.sqlite3') as db:
        proofs=[json.loads(x[0])['directory'] for x in db.execute("SELECT evidence FROM steps WHERE phase='directory_published'")]
    assert proofs and all(x.get('retained_destination') for x in proofs), proofs
    destination=f.creators/'Momoyih'
    assert destination.is_dir()
    assert not list(f.creators.glob('.face-dir-*'))
    if os.environ.get('DIRECTORY_NFS_ROLLBACK') == '1':
        assert result['rolled_back'] and not result['recovery_needed'], result
        assert len(f.queue())==2 and not list(destination.iterdir())
        assert all(p.read_bytes()==a.BYTES+str(i).encode() for i,p in enumerate(f.sources))
    else:
        assert result['ok'] and result['moved']==2 and result['ledger']==2 and not f.queue(), result
        assert all(not p.exists() and (destination/p.name).read_bytes()==a.BYTES+str(i).encode() for i,p in enumerate(f.sources))
    with sqlite3.connect(f.ledger) as db:
        assert db.execute('PRAGMA integrity_check').fetchall()==[('ok',)]
        assert not db.execute('PRAGMA foreign_key_check').fetchall()
    print(json.dumps({'passed':True,'uid':os.getuid(),'rollback':os.environ.get('DIRECTORY_NFS_ROLLBACK')=='1','files':2,'retained_directory':True,'temporary_directories_cleaned':True,'result':result},ensure_ascii=False))
finally:
    f.stop()
