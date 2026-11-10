"""Merging spelling variants and leftover alias residue; registration reuses an existing spelling.

Run: PYTHONPATH=maintenance python3 tests/http-rust-person-merge-variants.py
"""
import importlib.util,json,sqlite3
from pathlib import Path
spec=importlib.util.spec_from_file_location('merge',Path(__file__).with_name('http-rust-person-merge.py'))
m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
f=m.f

def manual(x):return json.loads((x.base/'people-manual.json').read_text())
def aliases(x):return json.loads((x.base/'person-renames.json').read_text())

class VariantFixture(m.MergeFixture):
    """Adds a real person `momo` (folder + Peach entity) and an empty registration `MOMO`."""
    def __init__(self):
        super().__init__()
        self.momo=self.media/'pikpak/creators/momo';self.momo.mkdir()
        (self.momo/'kept.mp4').write_bytes(b'momo video')
        f.write(self.base/'people-manual.json',manual(self)+[{'name':'momo','created_at':'real','rep_thumb':'momo-thumb'},{'name':'MOMO','created_at':'typo','rep_thumb':None}])
        with sqlite3.connect(self.ledger) as db:
            db.execute("INSERT INTO entity(id,kind,canonical_name,normalized_name,created_at,updated_at) VALUES(7,'creator','momo','momo','fixture','fixture')")
    def meta(self,command,body=None,status=200):return self.call({'command':command,'body':body or {}},'/v1/metadata',status)

def spelling_variant_registration():
    x=VariantFixture()
    try:
        plan=x.preview(x.merge_request(person='MOMO',target='momo'))
        assert plan['files']==[],plan['files']
        x.apply(plan);assert x.status(plan)['status']=='complete'
        names=[r['name'] for r in manual(x)];assert 'MOMO' not in names and names.count('momo')==1,names
        kept=next(r for r in manual(x) if r['name']=='momo');assert kept['created_at']=='real' and kept['rep_thumb']=='momo-thumb',kept
        assert (x.momo/'kept.mp4').read_bytes()==b'momo video'
        with sqlite3.connect(x.ledger) as db:
            assert db.execute("SELECT canonical_name FROM entity WHERE id=7").fetchone()==('momo',)
            assert not db.execute('PRAGMA foreign_key_check').fetchall()
        assert not (x.base/'.review-write.lock').exists()
        print('PASS spelling variant registration folds into the person that owns the folder')
    finally:x.stop()

def spelling_variant_with_folder_must_be_kept():
    x=VariantFixture()
    try:
        error=x.preview(x.merge_request(person='momo',target='MOMO'),409)
        assert 'momo 设为保留人物' in error['error'],error
        assert (x.momo/'kept.mp4').exists() and [r['name'] for r in manual(x)].count('MOMO')==1
        print('PASS spelling variant that owns a folder cannot be merged away')
    finally:x.stop()

def alias_residue():
    """`旧甲` already aliases to `甲`, but still owns an empty folder and its own Peach entity."""
    x=m.MergeFixture()
    try:
        residue=x.media/'pikpak/creators/旧甲';residue.mkdir()
        with sqlite3.connect(x.ledger) as db:
            db.execute("INSERT INTO entity(id,kind,canonical_name,normalized_name,created_at,updated_at) VALUES(9,'creator','旧甲','旧甲','fixture','fixture')")
            db.execute("INSERT INTO asset_entity(asset_id,entity_id,role,source,confidence) VALUES(2,9,'creator','manual',.5)")
        plan=x.preview(x.merge_request(person='旧甲',target='甲'))
        assert plan['person']=='旧甲' and plan['target_person']=='甲' and plan['files']==[],plan
        x.apply(plan);assert x.status(plan)['status']=='complete'
        assert not residue.exists() and x.source.exists()
        with sqlite3.connect(x.ledger) as db:
            assert db.execute("SELECT COUNT(*) FROM entity WHERE canonical_name='旧甲'").fetchone()[0]==0
            assert db.execute("SELECT COUNT(*) FROM asset_entity WHERE asset_id=2 AND entity_id=1").fetchone()[0]==1
            assert not db.execute('PRAGMA foreign_key_check').fetchall()
        assert aliases(x)['旧甲']=='甲'
        print('PASS alias residue (folder + entity) merges into the current person')
    finally:x.stop()

def alias_target_resolves_to_current_person():
    x=m.MergeFixture()
    try:
        (x.media/'pikpak/creators/旧甲').mkdir()
        # Picking the alias as the kept person still keeps the current person.
        x.preview(x.merge_request(person='甲',target='旧甲'),409)
        print('PASS alias cannot become the kept person over its current person')
    finally:x.stop()

def registration_reuses_spelling():
    x=VariantFixture()
    try:
        x.meta('person-remember',{'name':'Momo'})
        names=[r['name'] for r in manual(x)];assert 'Momo' not in names and names.count('momo')==1,names
        x.meta('person-remember',{'name':'全新人物'})
        assert '全新人物' in [r['name'] for r in manual(x)]
        print('PASS registration reuses an existing spelling instead of adding a variant')
    finally:x.stop()

def merge_scanner_in_place_reconciliation():
    import http.client
    x=m.MergeFixture(True)
    try:
        plan=x.preview(x.merge_request());x.fault('ledger_commit_intent')
        try:x.apply(plan)
        except (OSError,http.client.HTTPException):pass
        assert x.process.wait(timeout=5)==86
        (x.root/'rust-fault.json').unlink();x.stop()
        with sqlite3.connect(x.ledger) as db:
            for file in plan['files']:
                db.execute('UPDATE asset SET path=?,last_seen=? WHERE path=?',(x.win(Path(file['dst'])),'2026-11-10 01:59:11',x.win(Path(file['src']))))
        x.start();x.assert_success(plan)
        status=x.status(plan)
        assert status['result'].get('reconciled_scanner_assets')==[1],status
        print('PASS merge-person reconciles in-place Peach scanner asset move on recovery')
    finally:x.stop()

if __name__=='__main__':
    spelling_variant_registration();spelling_variant_with_folder_must_be_kept()
    alias_residue();alias_target_resolves_to_current_person();registration_reuses_spelling()
    merge_scanner_in_place_reconciliation()
