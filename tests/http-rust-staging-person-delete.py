"""Joint #11/#16: whole-person provenance, selective disposal and independent restore."""
import importlib.util
import json
import sqlite3
import uuid
from pathlib import Path

spec = importlib.util.spec_from_file_location('person_delete', Path(__file__).with_name('http-rust-person-delete.py'))
person = importlib.util.module_from_spec(spec)
spec.loader.exec_module(person)
f = person.DeleteFixture()
try:
    deleted = f.preview(f.request_delete())
    f.apply(deleted)
    rows = f.call(route='/v1/staging')['rows']
    source = next(row for row in rows if row['vid'] == person.f.VID)
    restore_row = next(row for row in rows if row['path'].endswith('/nested/scanless.mp4'))
    origin = source['origin'].copy()
    with sqlite3.connect(f.ledger) as db:
        links = db.execute('SELECT * FROM asset_entity ORDER BY asset_id,entity_id,role,source').fetchall()
        entity = db.execute('SELECT * FROM entity WHERE id=1').fetchone()
    request = {'id':str(uuid.uuid4()), 'action':'delete-staged', 'person':'', 'files':[{k:source[k] for k in ('vid','path','version')}]}
    plan = f.preview(request)
    assert plan['files'][0]['vids'] == [person.f.VID], plan
    result = f.call({'mode':'apply', 'request':plan['request'], 'version':plan['version'], 'confirmation':'永久删除'})
    assert result['completed'] == 1 and not Path(source['path']).exists(), result
    assert all(Path(row['path']).exists() for row in rows if row['path'] != source['path'])
    with sqlite3.connect(f.ledger) as db:
        assert db.execute('SELECT disposal FROM asset WHERE id=1').fetchone() == ('trash',)
        assert db.execute('SELECT * FROM entity WHERE id=1').fetchone() == entity
        assert db.execute('SELECT * FROM asset_entity ORDER BY asset_id,entity_id,role,source').fetchall() == links
    audit = json.loads((f.base/'staging-origins.json').read_text())[source['path']]
    assert all(audit[k] == value for k,value in origin.items())
    history = json.loads((f.base/'archive-history.json').read_text())[0]
    assert history['creator'] == '甲' and history['file_state'] == 'deleted'
    request = {'id':str(uuid.uuid4()), 'action':'restore-files', 'person':'', 'files':[{k:restore_row[k] for k in ('vid','path','version')}]}
    restore = f.preview(request)
    f.apply(restore)
    assert (f.source.parent/'nested/scanless.mp4').read_bytes() == b'new video'
    assert not f.source.exists()
    assert json.loads((f.base/'external-profiles/manual-links.json').read_text())['links'] == []
    assert json.loads((f.base/'watermarks/mappings.json').read_text()) == {}
    assert f.call(route='/v1/staging')['files'] == len(rows)-2
    assert not (f.base/'.review-write.lock').exists()
    print('PASS whole-person staging → selected permanent deletion + independent restore; provenance, creator relationships and withdrawn decisions retained')
finally:
    f.stop()
