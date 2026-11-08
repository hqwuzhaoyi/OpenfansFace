"""Joint merge/staging restore: current identity follows merge; provenance stays historical."""
import importlib.util
import json
import sqlite3
import uuid
from pathlib import Path

spec = importlib.util.spec_from_file_location('merge', Path(__file__).with_name('http-rust-person-merge.py'))
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)
for target_entity in (True, False):
    x = m.MergeFixture(target_entity)
    try:
        request = x.request()
        request['action'] = 'delete-files'
        request.pop('filename')
        staged = x.preview(request)
        x.apply(staged)
        row = x.call(route='/v1/staging')['rows'][0]
        origin = row['origin'].copy()
        merge = x.preview(x.merge_request())
        x.apply(merge)
        row = x.call(route='/v1/staging')['rows'][0]
        assert row['origin'] == origin and row['vid'] == m.f.VID
        with sqlite3.connect(x.ledger) as db:
            kept_id = db.execute("SELECT id FROM entity WHERE kind='creator' AND canonical_name='乙'").fetchone()[0]
            assert kept_id != 1
            assert {r[0] for r in db.execute("SELECT entity_id FROM asset_entity WHERE asset_id=1 AND role='creator'")} == {kept_id}
            other_roles = db.execute("SELECT * FROM asset_entity WHERE asset_id=1 AND role!='creator' ORDER BY entity_id,role,source").fetchall()
        restore = x.preview({'id':str(uuid.uuid4()), 'action':'restore-files', 'person':'', 'target_person':'丙', 'files':[{k:row[k] for k in ('vid','path','version')}]})
        assert restore['person'] == '乙' and restore['target_person'] == '丙', restore
        x.apply(restore)
        assert Path(restore['files'][0]['dst']).read_bytes() == m.f.PAYLOAD
        with sqlite3.connect(x.ledger) as db:
            names = {r[0] for r in db.execute("SELECT e.canonical_name FROM asset_entity ae JOIN entity e ON e.id=ae.entity_id WHERE ae.asset_id=1 AND ae.role='creator'")}
            assert names == {'丙'}, names
            assert db.execute("SELECT * FROM asset_entity WHERE asset_id=1 AND role!='creator' ORDER BY entity_id,role,source").fetchall() == other_roles
            assert not db.execute('PRAGMA foreign_key_check').fetchall()
        audit = json.loads((x.base/'staging-origins.json').read_text())[row['path']]
        assert all(audit[k] == v for k,v in origin.items())
        assert json.loads((x.base/'person-renames.json').read_text())['甲'] == '乙'
        assert not (x.base/'.review-write.lock').exists()
        print('PASS staged source → whole-person merge → restore uses current creator; historical origin retained; existing target', target_entity)
    finally:
        x.stop()
