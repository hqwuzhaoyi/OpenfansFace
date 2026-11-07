"""Rust #7 HTTP acceptance. Only marked isolated native schema/media fixtures; no production writes.
python3 tests/http-rust-person-move.py --port 18985
"""
import importlib.util
import json
from pathlib import Path
import sqlite3
import uuid
import http.client
import time
import signal

spec=importlib.util.spec_from_file_location('move_fixture',Path(__file__).with_name('http-rust-file-operations.py'))
fixture=importlib.util.module_from_spec(spec);spec.loader.exec_module(fixture)
Fixture,PAYLOAD,VID=fixture.Fixture,fixture.PAYLOAD,fixture.VID

class MoveFixture(Fixture):
    def __init__(self):
        super().__init__()
        self.other=self.source.with_name('second.mp4');self.other.write_bytes(PAYLOAD+b'second')
        self.untouched=self.source.with_name('unselected.mp4');self.untouched.write_bytes(b'keep me')
        self.destination=self.media/'pikpak/creators/乙';self.destination.mkdir()
        self.vid2='stable-second'
        with sqlite3.connect(self.base/'faces.db') as db:
            db.execute('INSERT INTO videos VALUES(?,?,?,?,?)',(self.vid2,str(self.other),self.other.name,str(self.other.parent),self.other.stat().st_size))
            db.execute('INSERT INTO faces VALUES(?,?,?)',(VID,1,'selected-thumb.jpg'))
        fixture.write(self.base/'report.json',{'identities':[{'name':'甲','rep_thumb':'selected-thumb.jpg','tpl_faces':2,'vids':[{'vid':VID,'path':str(self.source),'creator':'甲','original_vid':VID,'ocr':'甲 original.mp4'},{'vid':self.vid2,'path':str(self.other)},{'vid':'keep','path':str(self.untouched)}]},{'name':'乙','vids':[]}]})
        fixture.write(self.base/'integration-queue.json',[{'vid':VID,'creator':'甲','source':'person','source_id':'甲','note':'original evidence'},{'vid':'unrelated','creator':'甲'}])
        with sqlite3.connect(self.ledger) as db:
            db.execute('INSERT INTO asset(id,location,path,name) VALUES(2,?,?,?)',('fixture',self.win(self.other),'preserved second title'))
            db.execute("INSERT INTO asset_entity(asset_id,entity_id,role,source) VALUES(2,1,'creator','manual')")
            db.execute("INSERT INTO asset_entity(asset_id,entity_id,role,source) VALUES(1,1,'performer','manual')")

    def move_request(self,files=None,folder='nested/中文'):
        files=files or [(self.source,VID),(self.other,self.vid2)]
        return {'id':str(uuid.uuid4()),'action':'move-files','person':'甲','target_person':'乙','target_folder':folder,'files':[self.request(path,vid=vid)['files'][0] for path,vid in files]}

    def assert_moved(self,plan):
        result=self.status(plan);assert result['status']=='complete',result
        assert result['result']['changed']==len(plan['files'])
        assert result['committed']==len(plan['files']) and result['processed']==len(plan['files'])
        assert [item['file_index'] for item in result['files']]==list(range(len(plan['files'])))
        assert all(item['status']=='complete' for item in result['files'])
        for file in plan['files']:
            assert Path(file['dst']).exists() and not Path(file['src']).exists()
            for vid in file['vids']:
                assert json.loads((self.base/'video-path-overrides.json').read_text())[vid]==file['dst']
        assert self.untouched.read_bytes()==b'keep me'
        with sqlite3.connect(self.ledger) as db:
            assert not db.execute('PRAGMA foreign_key_check').fetchall()
            assert db.execute("SELECT source FROM asset_entity WHERE asset_id=1 AND entity_id=1 AND role='performer'").fetchall()==[('manual',)]
            assert db.execute("SELECT COUNT(*) FROM asset_entity WHERE entity_id=1 AND role='creator'").fetchone()[0]==0
            target=db.execute("SELECT id FROM entity WHERE normalized_name='乙'").fetchone()[0]
            assert db.execute("SELECT COUNT(*) FROM asset_entity WHERE entity_id=? AND role='creator'",(target,)).fetchone()[0]==2
            assert db.execute('SELECT name FROM asset ORDER BY id').fetchall()==[('preserved ledger title',),('preserved second title',)]
        report=json.loads((self.base/'report.json').read_text())['identities'];old,target=report
        assert [r['vid'] for r in old['vids']]==['keep'] and old['rep_thumb'] is None
        assert {r['vid'] for r in target['vids']}=={VID,self.vid2}
        assert target['vids'][0]['ocr']=='甲 original.mp4' and target['vids'][0]['original_vid']==VID
        queue=json.loads((self.base/'integration-queue.json').read_text())
        assert queue[0]['creator']=='乙' and queue[0]['source_id']=='甲' and queue[1]=={'vid':'unrelated','creator':'甲'}
        assert json.loads((self.base/'external-profiles/manual-links.json').read_text())['links'][0]['creator']=='甲'
        assert not (self.base/'.review-write.lock').exists()


def success():
    f=MoveFixture()
    try:
        plan=f.preview(f.move_request())
        assert len(plan['files'])==2 and plan['total_bytes']==len(PAYLOAD)*2+6
        with sqlite3.connect(f.base/'processing/processing.db') as db:
            db.execute("UPDATE tasks SET stage='ocr',updated_at=100 WHERE vid='unrelated'")
        result=f.apply(plan);f.assert_moved(plan);assert f.apply(plan)==result
        assert f.status(plan)['total']==2
        print('PASS selected cross-person nested move, real ledger roles, metadata, unrelated task and replay')
    finally:f.stop()


def guards():
    for mutation in ('target','source','index','links','permission'):
        f=MoveFixture()
        try:
            plan=f.preview(f.move_request(folder=''))
            if mutation=='target':(f.destination/f.other.name).write_bytes(b'foreign')
            elif mutation=='source':f.other.write_bytes(b'changed')
            elif mutation=='index':
                with sqlite3.connect(f.base/'faces.db') as db:db.execute('UPDATE videos SET name=? WHERE vid=?',('changed',f.vid2))
            elif mutation=='links':
                with sqlite3.connect(f.ledger) as db:db.execute("INSERT INTO asset_entity(asset_id,entity_id,role,source) VALUES(1,1,'creator','late')")
            elif mutation=='permission':f.destination.chmod(0o555)
            failure=f.apply(plan,403 if mutation=='permission' else 409)
            assert f.source.exists() and f.other.exists(),failure
            assert not (f.destination/f.source.name).exists()
            if mutation=='target':assert (f.destination/f.other.name).read_bytes()==b'foreign'
            print('PASS stale guard',mutation)
        finally:
            f.destination.chmod(0o755);f.stop()
    f=MoveFixture()
    try:
        for change in ({'files':[]},{'target_person':'missing'},{'target_folder':'../outside'},{'target_folder':'a//b'}):
            f.preview({**f.move_request(),**change},404 if change.get('target_person') else 400)
        duplicate=f.move_request();duplicate['files'].append(duplicate['files'][0]);f.preview(duplicate,400)
        outside=f.media/'pikpak/My Telegram/else.mp4';outside.parent.mkdir();outside.write_bytes(PAYLOAD)
        f.preview(f.move_request([(outside,str(outside))]),400)
        link=f.source.parent/'link.mp4';link.symlink_to(f.other)
        f.preview(f.move_request([(link,str(link))]),409)
        same=f.other.parent/'inner'/f.source.name;same.parent.mkdir();same.write_bytes(PAYLOAD)
        f.preview(f.move_request([(f.source,VID),(same,str(same))]),409)
        print('PASS scope, traversal, symlink, selection and same-name plan guards')
    finally:f.stop()


def registration():
    f=MoveFixture()
    try:
        source=f.source.with_name('unindexed.mp4');source.write_bytes(PAYLOAD)
        plan=f.preview(f.request(source,filename='first.mp4',vid=str(source)))
        stable=plan['files'][0]['vids'][0];assert stable.startswith('file:')
        f.apply(plan);first=Path(plan['files'][0]['dst'])
        plan2=f.preview(f.move_request([(first,stable)],folder='registered'))
        assert plan2['files'][0]['vids']==[stable]
        f.apply(plan2);current=Path(plan2['files'][0]['dst'])
        request=f.request(current,filename='last.mp4',vid=stable);request['person']='乙'
        plan3=f.preview(request);f.apply(plan3)
        assert json.loads((f.base/'video-path-overrides.json').read_text())[stable]==plan3['files'][0]['dst']
        with sqlite3.connect(f.base/'faces.db') as db:assert not db.execute('SELECT 1 FROM videos WHERE vid=?',(stable,)).fetchall()
        print('PASS unindexed rename registration, repeated move+rename retains stable playback reference without fake face index')
    finally:f.stop()


def rollback():
    f=MoveFixture()
    try:
        plan=f.preview(f.move_request())
        originals={name:(f.base/name).read_bytes() for name in ['report.json','archive-history.json','integration-queue.json']}
        with sqlite3.connect(f.ledger) as db:db.executescript("CREATE TRIGGER fixture_reject BEFORE UPDATE OF path ON asset BEGIN SELECT RAISE(ABORT,'fixture move ledger failure'); END;")
        f.apply(plan,503)
        assert f.status(plan)['status']=='rolled_back'
        assert f.source.read_bytes()==PAYLOAD and f.other.read_bytes()==PAYLOAD+b'second'
        assert all(not Path(file['dst']).exists() for file in plan['files'])
        assert not (f.destination/'nested').exists()
        assert all((f.base/name).read_bytes()==raw for name,raw in originals.items())
        assert not (f.base/'video-path-overrides.json').exists()
        with sqlite3.connect(f.ledger) as db:assert db.execute('SELECT COUNT(*) FROM entity').fetchone()[0]==1
        print('PASS aggregate native ledger failure restores entire selection, local stores and owned directories')
    finally:f.stop()


def boundary():
    f=MoveFixture()
    try:
        files=[]
        for index in range(500):
            path=f.source.with_name(f'boundary-{index:03}.mp4');path.write_bytes(b'x');files.append((path,str(path)))
        request=f.move_request(files,folder='boundary');plan=f.preview(request)
        assert len(plan['files'])==500
        result=f.apply(plan);assert result['changed']==500 and f.status(plan)['total']==500
        assert all(Path(file['dst']).read_bytes()==b'x' for file in plan['files'])
        overflow={**request,'id':str(uuid.uuid4()),'files':request['files']+[request['files'][0]]};f.preview(overflow,400)
        print('PASS 500-file HTTP boundary (body over old 64 KiB), explicit 501 rejection')
    finally:f.stop()


def crash_checkpoint(phase,replace=False):
    f=MoveFixture()
    try:
        plan=f.preview(f.move_request())
        f.fault(phase,'pause',milliseconds=10000)
        client=http.client.HTTPConnection('127.0.0.1',f.port,timeout=15)
        client.request('POST','/v1/person-files',json.dumps({'mode':'apply','request':plan['request'],'version':plan['version']}),{'Content-Type':'application/json','Authorization':'Bearer fixture-token'})
        reached=f.root/'rust-fault-reached.json'
        for _ in range(1000):
            if reached.exists() and json.loads(reached.read_text())['phase']==phase:break
            time.sleep(.01)
        else:raise AssertionError(('checkpoint not reached',phase,f.root))
        observed=f.status(plan);assert len(observed['files'])==2 and observed['committed']==0
        if phase.endswith(':1'):assert observed['files'][0]['status']=='moved_pending_commit' and observed['files'][1]['file_index']==1
        f.process.send_signal(signal.SIGKILL);f.process.wait(timeout=5);client.close()
        (f.root/'rust-fault.json').unlink();f.stop()
        if replace:
            foreign=Path(plan['files'][1]['dst']);foreign.unlink();foreign.write_bytes(b'foreign replacement')
        f.start();status=f.status(plan)
        if replace:
            assert status['status']=='recovery_needed',status
            assert foreign.read_bytes()==b'foreign replacement' and (f.base/'.review-write.lock').exists()
            assert f.call({'mode':'apply','request':plan['request'],'version':plan['version']},status=503)['code']=='recovery_required'
        else:
            assert status['status'] in ('complete','rolled_back'),status
            if status['status']=='complete':f.assert_moved(plan)
            else:
                assert f.source.read_bytes()==PAYLOAD and f.other.read_bytes()==PAYLOAD+b'second'
                assert all(not Path(file['dst']).exists() for file in plan['files'])
            assert not (f.base/'.review-write.lock').exists()
        print('PASS multi-file process kill',phase,'foreign replacement' if replace else status['status'])
    finally:f.stop()


def identity_resolution():
    f=MoveFixture()
    try:
        # Case-normalized existing identity, multiple link sources and unrelated roles survive.
        f.destination=f.destination.with_name('Target');f.destination.mkdir()
        with sqlite3.connect(f.ledger) as db:
            db.execute("INSERT INTO entity(id,kind,canonical_name,normalized_name,created_at,updated_at) VALUES(2,'creator','TARGET','target','fixture','fixture')")
            db.execute("INSERT INTO asset_entity(asset_id,entity_id,role,source) VALUES(1,2,'creator','existing-account')")
        request=f.move_request(folder='');request['target_person']='Target'
        plan=f.preview(request);f.apply(plan)
        with sqlite3.connect(f.ledger) as db:
            assert db.execute('SELECT COUNT(*) FROM entity').fetchone()[0]==2
            assert db.execute("SELECT source FROM asset_entity WHERE asset_id=1 AND entity_id=2 AND role='creator'").fetchall()==[('existing-account',)]
        print('PASS normalized existing creator reuse preserves target link sources')
    finally:f.stop()
    f=MoveFixture()
    try:
        plan=f.preview(f.move_request())
        with sqlite3.connect(f.ledger) as db:db.execute("INSERT INTO entity(kind,canonical_name,normalized_name,created_at,updated_at) VALUES('creator','unrelated','unrelated','fixture','fixture')")
        f.apply(plan);f.assert_moved(plan)
        print('PASS unrelated creator allocation does not stale absent-target preview')
    finally:f.stop()


def source_identity_resolution():
    for ambiguous in (False,True):
        f=MoveFixture()
        try:
            old=f.source.parent;source_root=old.with_name('Source');old.rename(source_root)
            f.source=source_root/f.source.name;f.other=source_root/f.other.name;f.untouched=source_root/f.untouched.name
            fixture.write(f.base/'archive-history.json',[])
            with sqlite3.connect(f.base/'faces.db') as db:
                db.execute('UPDATE videos SET path=? WHERE vid=?',(str(f.source),VID));db.execute('UPDATE videos SET path=? WHERE vid=?',(str(f.other),f.vid2))
            with sqlite3.connect(f.ledger) as db:
                db.execute("UPDATE entity SET canonical_name='SOURCE',normalized_name='source' WHERE id=1")
                db.execute('UPDATE asset SET path=? WHERE id=1',(f.win(f.source),));db.execute('UPDATE asset SET path=? WHERE id=2',(f.win(f.other),))
                if ambiguous:db.execute("INSERT INTO entity(kind,canonical_name,normalized_name,created_at,updated_at) VALUES('creator','Source','other','fixture','fixture')")
            request=f.move_request();request['person']='Source'
            if ambiguous:
                f.preview(request,409);assert f.source.exists() and f.other.exists()
            else:
                plan=f.preview(request);f.apply(plan)
                with sqlite3.connect(f.ledger) as db:
                    assert db.execute("SELECT COUNT(*) FROM asset_entity WHERE entity_id=1 AND role='creator'").fetchone()[0]==0
                    assert db.execute("SELECT COUNT(*) FROM asset_entity WHERE entity_id=1 AND role='performer'").fetchone()[0]==1
            print('PASS normalized source identity', 'ambiguity rejects' if ambiguous else 'old creator links removed, unrelated role retained')
        finally:f.stop()

if __name__=='__main__':
    success();guards();registration();rollback();identity_resolution();source_identity_resolution();boundary()
    for phase in ('destination_published:1','source_removed:1','faces_committed','ledger_committed_unrecorded'):
        crash_checkpoint(phase)
    crash_checkpoint('destination_published:1',replace=True)
