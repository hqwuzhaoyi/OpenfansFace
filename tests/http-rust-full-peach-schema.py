"""Replay native operations against sanitized 275-object Peach schema, no live rows."""
import importlib.util
from pathlib import Path
import sqlite3
spec=importlib.util.spec_from_file_location('batch',Path(__file__).with_name('http-rust-person-batch.py'))
m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
m.f.SCHEMA=(Path(__file__).parent/'fixtures/peach-full-schema.sql').read_text()+"\nINSERT INTO ledger_revision(tbl,n) SELECT name,0 FROM sqlite_master WHERE type='table';\n"
for name,cls,action,check in [
 ('rename',m.m.r.RenameFixture,lambda x:x.rename_request(),lambda x,p:x.assert_success(p)),
 ('merge',m.m.MergeFixture,lambda x:x.merge_request(),lambda x,p:x.assert_success(p)),
]:
 x=cls()
 try:
  p=x.preview(action(x));x.apply(p);check(x,p)
  with sqlite3.connect(x.ledger) as db:
   assert not db.execute('PRAGMA foreign_key_check').fetchall()
   assert db.execute('PRAGMA integrity_check').fetchall()==[('ok',)]
   assert db.execute("SELECT n FROM ledger_revision WHERE tbl='asset'").fetchone()[0]>0
   assert db.execute("SELECT count(*) FROM asset_search").fetchone()[0]==db.execute("SELECT count(*) FROM asset").fetchone()[0]
  print('PASS full Peach schema/FTS/triggers/revisions/foreign keys',name,flush=True)
 finally:x.stop()
m.basic();m.partial()
