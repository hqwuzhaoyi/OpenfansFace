"""#9 full-person native HTTP acceptance against marked real file/full core schema fixtures."""
import importlib.util,json,sqlite3,uuid,threading,time,http.client
from pathlib import Path
spec=importlib.util.spec_from_file_location('rename_fixture',Path(__file__).with_name('http-rust-file-operations.py'))
f=importlib.util.module_from_spec(spec);spec.loader.exec_module(f)

class RenameFixture(f.Fixture):
    def __init__(self):
        super().__init__()
        root=self.source.parent
        (root/'nested/empty/deep').mkdir(parents=True)
        (root/'nested/scanless.mp4').write_bytes(b'new video')
        (root/'.hidden-attachment').write_bytes(b'attachment')
        (root/'nested/readme.txt').write_bytes(b'document')
        self.contents={str(p.relative_to(root)):p.read_bytes() for p in root.rglob('*') if p.is_file()}
        self.directories={str(p.relative_to(root)) for p in root.rglob('*') if p.is_dir()}
        f.write(self.base/'person-renames.json',{'旧甲':'甲','更旧甲':'旧甲'})
        f.write(self.base/'people-manual.json',[{'name':'甲','at':'audit'}])
        f.write(self.base/'template-overrides.json',{'甲':{'thumb':'original-thumb','name':'甲'}})
        f.write(self.base/'external-profiles/manual-links.json',{'links':[{'id':'link','creator':'旧甲','profile':{'id':'profile','name':'甲','description':'甲 original'}}]})
        f.write(self.base/'external-profiles/reviewed-links.json',{'links':[{'person':'甲','confidence':'candidate','profile_id':'profile'}]})
        f.write(self.base/'integration-queue.json',[{'vid':'outside','creator':'旧甲','source':'person','source_id':'甲','return_href':'/persons/%E7%94%B2','note':'甲 raw audit'},{'vid':'unrelated','creator':'other'}])
        f.write(self.base/'watermarks/mappings.json',{'token':{'name':'甲','token':'甲'}})
        f.write(self.base/'person-rejections.json',{'oldkey':{'name':'旧甲','vid':f.VID,'fingerprint':'original'}})
        # Use the complete production task schema, including retry/counter/manual state.
        spec2=importlib.util.spec_from_file_location('processing_store',f.REPO/'maintenance/processing_store.py')
        mod=importlib.util.module_from_spec(spec2);spec2.loader.exec_module(mod)
        with sqlite3.connect(self.base/'processing/processing.db') as db:
            old=db.execute('SELECT * FROM tasks').fetchall();db.execute('DROP TABLE tasks');db.executescript(mod.SCHEMA)
            for row in old:db.execute("INSERT INTO tasks(vid,path,name,size,mtime_ns,stage,updated_at,data,batch_id,created_at) VALUES(?,?,?,?,?,?,?,?,'fixture',0)",row)
            db.execute("INSERT INTO events(at,vid,kind,message) VALUES(0,'audit','fixture','甲 historical audit')")
            db.execute("INSERT INTO tasks(vid,path,name,size,mtime_ns,stage,updated_at,data,batch_id,created_at) VALUES('candidate','outside.mp4','outside.mp4',1,'0','face',0,?,'fixture',0)",(json.dumps({'candidates':[{'name':'旧甲','score':.9}],'ocr':'甲','vid':'甲','source':'person','source_id':'甲','return_href':'/persons/%E7%94%B2'}),))
        with sqlite3.connect(self.ledger) as db:
            db.execute('INSERT INTO asset(id,location,path,name,disposal) VALUES(2,?,?,?,?)',('fixture',self.win(root/'missing-historical.mp4'),'historical','retained'))
    def rename_request(self,person='甲',target='新甲'):
        return {'id':str(uuid.uuid4()),'action':'rename-person','person':person,'new_name':target}
    def assert_success(self,plan):
        target=self.media/'pikpak/creators'/plan['target_person'];assert not self.source.parent.exists()
        assert {str(p.relative_to(target)):p.read_bytes() for p in target.rglob('*') if p.is_file()}==self.contents
        assert {str(p.relative_to(target)) for p in target.rglob('*') if p.is_dir()}==self.directories
        assert self.status(plan)['status']=='complete'
        aliases=json.loads((self.base/'person-renames.json').read_text());assert aliases['甲']=='新甲' and aliases['旧甲']=='新甲'
        links=json.loads((self.base/'external-profiles/manual-links.json').read_text())['links'];assert links[0]['creator']=='新甲' and links[0]['profile']['name']=='甲'
        queue=json.loads((self.base/'integration-queue.json').read_text());assert queue[0]['creator']==queue[0]['source_id']=='新甲' and queue[0]['note']=='甲 raw audit'
        assert queue[0]['return_href']=='/persons/%E6%96%B0%E7%94%B2'
        with sqlite3.connect(self.base/'processing/processing.db') as db:
            assert db.execute('SELECT message FROM events').fetchone()[0]=='甲 historical audit'
            row=db.execute("SELECT stage,data FROM tasks WHERE vid='candidate'").fetchone();assert row[0]=='queued'
            data=json.loads(row[1]);assert data['candidates'][0]['name']=='新甲' and data['ocr']=='甲' and data['vid']=='甲'
        with sqlite3.connect(self.ledger) as db:
            assert db.execute('SELECT canonical_name FROM entity WHERE id=1').fetchone()[0]=='新甲'
            assert all('新甲' in row[0] for row in db.execute('SELECT path FROM asset'))
            assert db.execute('SELECT alias FROM entity_alias').fetchone()[0]=='alias'
            assert not db.execute('PRAGMA foreign_key_check').fetchall()
        assert not (self.base/'.review-write.lock').exists()

def success():
    x=RenameFixture()
    try:
        plan=x.preview(x.rename_request());assert len(plan['files'])==4
        assert all(not row['vids'] for row in plan['files'] if not row['src'].endswith('.mp4'))
        stop=threading.Event()
        def background():
            while not stop.is_set():
                with sqlite3.connect(x.base/'processing/processing.db') as db:db.execute("UPDATE tasks SET updated_at=updated_at+1,face_attempts=face_attempts+1 WHERE vid='unrelated'")
                time.sleep(.005)
        t=threading.Thread(target=background);t.start()
        try:result=x.apply(plan);x.assert_success(plan);assert x.apply(plan)==result
        finally:stop.set();t.join()
        back=x.preview(x.rename_request('更旧甲','甲'));x.apply(back);assert x.source.exists()
        print('PASS complete tree/attachments/empty dirs/full tasks/identity refs/Peach prefix/idempotency/alias rename back')
    finally:x.stop()

def guards():
    for kind in ('file','directory','candidate','candidate_insert','index','prefix_asset','target','template','source_ambiguity','target_ledger','symlink','dangling_symlink','target_index'):
        x=RenameFixture()
        try:
            if kind in ('symlink','dangling_symlink'):
                (x.source.parent/'link').symlink_to(x.source if kind=='symlink' else x.source.with_name('missing'));x.preview(x.rename_request(),409);continue
            plan=x.preview(x.rename_request())
            if kind=='file':x.source.write_bytes(b'changed')
            elif kind=='directory':(x.source.parent/'late-empty').mkdir()
            elif kind=='candidate':
                with sqlite3.connect(x.base/'processing/processing.db') as db:db.execute("UPDATE tasks SET face_attempts=9 WHERE vid='candidate'")
            elif kind=='candidate_insert':
                with sqlite3.connect(x.base/'processing/processing.db') as db:db.execute("UPDATE tasks SET data=? WHERE vid='unrelated'",(json.dumps({'suggested':'旧甲'}),))
            elif kind=='index':
                with sqlite3.connect(x.base/'faces.db') as db:db.execute("UPDATE videos SET size=99 WHERE vid=?",(f.VID,))
            elif kind=='prefix_asset':
                with sqlite3.connect(x.ledger) as db:db.execute('INSERT INTO asset(location,path) VALUES(?,?)',('fixture',x.win(x.source.parent/'late.mp4')))
            elif kind=='target_index':
                with sqlite3.connect(x.base/'faces.db') as db:db.execute('INSERT INTO videos VALUES(?,?,?,?,?)',('target-index',str(x.source.parent.with_name('新甲')/'missing.mp4'),'missing.mp4','target',0))
            elif kind=='target':(x.source.parent.with_name('新甲')).mkdir()
            elif kind=='template':f.write(x.base/'template-overrides.json',{'新甲':{'thumb':'foreign'}})
            elif kind=='source_ambiguity':
                with sqlite3.connect(x.ledger) as db:db.execute("INSERT INTO entity(kind,canonical_name,normalized_name,created_at,updated_at) VALUES('creator','甲','different','fixture','fixture')")
            elif kind=='target_ledger':
                with sqlite3.connect(x.ledger) as db:db.execute("INSERT INTO entity(kind,canonical_name,normalized_name,created_at,updated_at) VALUES('creator','新甲','新甲','fixture','fixture')")
            x.apply(plan,409);assert x.source.exists();assert not (x.base/'.review-write.lock').exists()
            print('PASS relevant stale/conflict guard',kind)
        finally:x.stop()

def rollback():
    for phase in ('faces_commit_intent','faces_committed_unrecorded','jobs_commit_intent','jobs_committed_unrecorded','json_replaced_unrecorded:person-renames.json','ledger_commit_intent'):
        x=RenameFixture()
        try:
            plan=x.preview(x.rename_request());before={str(p.relative_to(x.base)):p.read_bytes() for p in x.base.rglob('*.json')}
            x.fault(phase,'error');x.apply(plan,503)
            status=x.status(plan);assert status['status']=='rolled_back',status
            assert {str(p.relative_to(x.source.parent)):p.read_bytes() for p in x.source.parent.rglob('*') if p.is_file()}==x.contents
            assert not x.source.parent.with_name('新甲').exists()
            for name,raw in before.items():assert (x.base/name).read_bytes()==raw,name
            with sqlite3.connect(x.ledger) as db:assert db.execute('SELECT canonical_name FROM entity WHERE id=1').fetchone()[0]=='甲'
            assert not (x.base/'.review-write.lock').exists()
            print('PASS full-tree guarded rollback',phase)
        finally:x.stop()
    x=RenameFixture()
    try:
        plan=x.preview(x.rename_request())
        with sqlite3.connect(x.ledger) as db:db.executescript("CREATE TRIGGER fixture_reject BEFORE UPDATE ON entity BEGIN SELECT RAISE(ABORT,'fixture identity commit failure'); END;")
        x.apply(plan,503);assert x.status(plan)['status']=='rolled_back';assert x.source.exists()
        print('PASS actual Peach entity transaction failure restores all stores and tree')
    finally:x.stop()

def crash(phase,foreign=False):
    x=RenameFixture()
    try:
        plan=x.preview(x.rename_request());x.fault(phase)
        try:x.apply(plan)
        except (OSError,http.client.HTTPException):pass
        assert x.process.wait(timeout=5)==86
        (x.root/'rust-fault.json').unlink();x.stop()
        if foreign:
            with sqlite3.connect(x.base/'processing/processing.db') as db:db.execute("UPDATE tasks SET note='foreign edit' WHERE vid='candidate'")
        x.start();status=x.status(plan)
        if foreign:
            assert status['status']=='recovery_needed',status
            with sqlite3.connect(x.base/'processing/processing.db') as db:assert db.execute("SELECT note FROM tasks WHERE vid='candidate'").fetchone()[0]=='foreign edit'
            assert (x.base/'.review-write.lock').exists()
        else:
            assert status['status']=='complete',status;x.assert_success(plan)
        print('PASS crash/restart',phase,'guarded external edit' if foreign else 'complete')
    finally:x.stop()

def cleanup_warning():
    x=RenameFixture()
    try:
        plan=x.preview(x.rename_request());x.fault('ledger_committed','pause',milliseconds=800)
        client=http.client.HTTPConnection('127.0.0.1',x.port,timeout=15)
        client.request('POST','/v1/person-files',json.dumps({'mode':'apply','request':plan['request'],'version':plan['version']}),{'Content-Type':'application/json','Authorization':'Bearer fixture-token'})
        for _ in range(1000):
            if (x.root/'rust-fault-reached.json').exists():break
            time.sleep(.01)
        else:raise AssertionError('checkpoint missing')
        (x.source.parent/'late-foreign').write_bytes(b'preserve')
        response=client.getresponse();result=json.load(response);assert response.status==200 and result['warning'],result
        assert x.status(plan)['status']=='complete';assert (x.source.parent/'late-foreign').read_bytes()==b'preserve';assert not (x.base/'.review-write.lock').exists()
        print('PASS committed cleanup warning preserves foreign files and releases write gate')
    finally:x.stop()

def hardlinks():
    x=RenameFixture()
    try:
        import os
        duplicate=x.source.parent/'hardlink.mp4';os.link(x.source,duplicate);x.contents[duplicate.name]=duplicate.read_bytes()
        plan=x.preview(x.rename_request());x.apply(plan);x.assert_success(plan)
        destination=Path(plan['files'][0]['dst']).parent
        assert (x.source.parent.with_name('新甲')/'original.mp4').stat().st_ino==(x.source.parent.with_name('新甲')/'hardlink.mp4').stat().st_ino
        print('PASS distinct hardlink names both retained without duplicate-ID moves')
    finally:x.stop()

def empty():
    x=RenameFixture()
    try:
        root=x.media/'pikpak/creators/empty';(root/'nested/deep').mkdir(parents=True)
        plan=x.preview(x.rename_request('empty','empty-new'));assert plan['files']==[];result=x.apply(plan);assert result['changed']==0
        assert not root.exists() and (root.with_name('empty-new')/'nested/deep').is_dir()
        print('PASS directory-only person rename')
    finally:x.stop()

def identity_conflicts():
    for existing,target in [('TARGET','target'),('e\u0301','é')]:
        x=RenameFixture()
        try:
            (x.source.parent.with_name(existing)).mkdir();x.preview(x.rename_request(target=target),409);assert x.source.exists()
            print('PASS normalized directory target conflict',repr(existing))
        finally:x.stop()
    x=RenameFixture()
    try:
        with sqlite3.connect(x.ledger) as db:db.execute('INSERT INTO asset(location,path) VALUES(?,?)',('other-location',x.win(x.source)))
        x.preview(x.rename_request(),409);assert x.source.exists()
        print('PASS duplicate Peach source prefix path rejects before media effects')
    finally:x.stop()
    x=RenameFixture()
    try:
        first=x.preview(x.rename_request(target='Source'));x.apply(first)
        with sqlite3.connect(x.ledger) as db:db.execute("UPDATE entity SET canonical_name='SOURCE',normalized_name='source' WHERE id=1")
        second=x.preview(x.rename_request('Source','Final'));x.apply(second)
        with sqlite3.connect(x.ledger) as db:assert db.execute('SELECT id,canonical_name FROM entity WHERE id=1').fetchone()==(1,'Final')
        print('PASS normalized source entity reused in place')
    finally:x.stop()

def ambiguous_cleanup():
    for errno in ('EIO', 'ESTALE', 'ETIMEDOUT'):
        x = RenameFixture()
        try:
            plan = x.preview(x.rename_request())
            x.fault('mutation:source_directory_remove', 'mutation_error_after', errno=errno)
            error = x.apply(plan, 503)
            assert error['mutation_outcome'] == 'unknown' and error['step'] == 'source_directory_remove', error
            target = x.media / 'pikpak/creators' / plan['target_person']
            assert {str(p.relative_to(target)):p.read_bytes() for p in target.rglob('*') if p.is_file()} == x.contents
            with sqlite3.connect(x.ledger) as db:
                assert db.execute('SELECT canonical_name FROM entity WHERE id=1').fetchone()[0] == '新甲'
            with sqlite3.connect(x.base/'file-operations-rust/operations.sqlite3') as db:
                attempts = db.execute("SELECT COUNT(*) FROM steps WHERE operation_id=? AND phase='filesystem_mutation_intent'", (plan['id'],)).fetchone()[0]
            (x.root/'rust-fault.json').unlink()
            x.stop(); x.start()
            for _ in range(2):
                state = x.call({'mode':'reconcile', 'request':plan['request'], 'version':plan['version']})
                assert state['status'] == 'recovery_needed' and state['io_state'] == 'unknown', state
                assert (x.base/'.review-write.lock').exists()
                x.apply(plan, 503)
            with sqlite3.connect(x.base/'file-operations-rust/operations.sqlite3') as db:
                assert attempts == db.execute("SELECT COUNT(*) FROM steps WHERE operation_id=? AND phase='filesystem_mutation_intent'", (plan['id'],)).fetchone()[0]
                assert db.execute("SELECT COUNT(*) FROM steps WHERE operation_id=? AND phase='source_directory_cleanup_warning'", (plan['id'],)).fetchone()[0] == 0
            print('PASS committed directory cleanup ambiguity stays protected across restart', errno)
        finally:
            x.stop()


if __name__=='__main__':
    success();guards();rollback();empty();hardlinks();cleanup_warning();identity_conflicts();ambiguous_cleanup()
    for phase in ('directory_published','source_removed:1','faces_committed_unrecorded','jobs_committed_unrecorded','ledger_committed_unrecorded','source_directory_cleanup_intent','source_directory_removed_unrecorded','final_receipt_intent'):crash(phase)
    crash('jobs_committed_unrecorded',True)
