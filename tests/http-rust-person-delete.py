"""#11 whole-person soft deletion HTTP acceptance; marked real media/SQLite fixtures only."""
import importlib.util,json,sqlite3,uuid,threading,time,http.client,os
from pathlib import Path
spec=importlib.util.spec_from_file_location('rename',Path(__file__).with_name('http-rust-person-rename.py'))
r=importlib.util.module_from_spec(spec);spec.loader.exec_module(r)
f=r.f
class DeleteFixture(r.RenameFixture):
    def request_delete(self,person='更旧甲'):
        return {'id':str(uuid.uuid4()),'action':'delete-person','person':person}
    def staged_root(self,plan):
        return self.media/'pikpak/.face-cleanup-staging/person-deletions'/plan['id']/plan['person']
    def assert_deleted(self,plan):
        target=self.staged_root(plan)
        assert not self.source.parent.exists()
        assert {str(p.relative_to(target)):p.read_bytes() for p in target.rglob('*') if p.is_file()}==self.contents
        assert {str(p.relative_to(target)) for p in target.rglob('*') if p.is_dir()}==self.directories
        assert self.status(plan)['status']=='complete'
        def doc(name):return json.loads((self.base/name).read_text())
        assert set(doc('people-deleted.json'))=={'甲','旧甲','更旧甲'}
        assert doc('person-renames.json')=={'旧甲':'甲','更旧甲':'旧甲'}
        assert doc('people-manual.json')==[] and doc('report.json')['identities']==[]
        assert doc('external-profiles/manual-links.json')['links']==[]
        assert doc('external-profiles/reviewed-links.json')['links']==[]
        assert doc('template-overrides.json')==doc('watermarks/mappings.json')==doc('person-rejections.json')=={}
        assert doc('integration-queue.json')==[{'vid':'unrelated','creator':'other'}]
        history=doc('archive-history.json')[0]
        assert history['creator']=='甲' and history['vid']==history['original_vid']==f.VID and history['file_state']=='staged'
        assert history['dst']==str(target/'original.mp4')
        with sqlite3.connect(self.base/'faces.db') as db:
            assert db.execute('SELECT path FROM videos WHERE vid=?',(f.VID,)).fetchone()[0]==str(target/'original.mp4')
        with sqlite3.connect(self.base/'processing/processing.db') as db:
            assert db.execute('SELECT message FROM events').fetchone()[0]=='甲 historical audit'
            row=db.execute("SELECT stage,data FROM tasks WHERE vid='candidate'").fetchone();assert row[0]=='queued'
            data=json.loads(row[1]);assert data['candidates']==[] and data['ocr']=='甲' and data['vid']=='甲' and data['suggested'] is None and data['ready'] is False
            row=db.execute('SELECT stage,data FROM tasks WHERE vid=?',(f.VID,)).fetchone();assert row[0]=='excluded' and json.loads(row[1])['text']==str(self.source)
        with sqlite3.connect(self.ledger) as db:
            assert db.execute('SELECT canonical_name FROM entity WHERE id=1').fetchone()[0]=='甲'
            assert all('person-deletions' in row[0] for row in db.execute('SELECT path FROM asset'))
            assert db.execute('SELECT count(*) FROM asset_entity').fetchone()[0]==2
            assert db.execute('SELECT alias FROM entity_alias').fetchone()[0]=='alias'
            assert not db.execute('PRAGMA foreign_key_check').fetchall()
        listing=self.call(route='/v1/staging');assert listing['files']==len(self.contents)
        for row in listing['rows']:
            assert row['origin']['person']=='甲' and row['origin']['action']=='delete-person'
            assert row['origin']['original_path']==str(self.source.parent/Path(row['path']).relative_to(target))
        assert not (self.base/'.review-write.lock').exists()

def success():
    x=DeleteFixture()
    try:
        plan=x.preview(x.request_delete());assert len(plan['files'])==4 and plan['total_bytes']==sum(map(len,x.contents.values()))
        assert all(not row['vids'] for row in plan['files'] if not row['src'].endswith('.mp4'))
        stop=threading.Event()
        def background():
            while not stop.is_set():
                with sqlite3.connect(x.base/'processing/processing.db') as db:db.execute("UPDATE tasks SET updated_at=updated_at+1,face_attempts=face_attempts+1 WHERE vid='unrelated'")
                time.sleep(.005)
        t=threading.Thread(target=background);t.start()
        try:
            result=x.apply(plan);x.assert_deleted(plan);assert result['staged'] and x.apply(plan)==result
        finally:stop.set();t.join()
        x.preview(x.request_delete(),404)
        rows=x.call(route='/v1/staging')['rows'];row=next(v for v in rows if v['vid']==f.VID)
        restore=x.preview({'id':str(uuid.uuid4()),'action':'restore-files','person':'','files':[{k:row[k] for k in ('vid','path','version')}]});x.apply(restore)
        assert x.source.read_bytes()==f.PAYLOAD
        assert '甲' not in json.loads((x.base/'people-deleted.json').read_text())
        assert json.loads((x.base/'external-profiles/manual-links.json').read_text())['links']==[]
        assert json.loads((x.base/'watermarks/mappings.json').read_text())=={}
        print('PASS whole tree/hidden attachments/empty dirs/alias deletion/history/Peach links/stable restore/no withdrawn decisions/idempotency')
    finally:x.stop()

def empty():
    for kind in ('attachments','empty','metadata','history'):
        x=DeleteFixture()
        try:
            person=x.source.parent.with_name(kind)
            if kind not in ('metadata','history'):(person/'nested/empty').mkdir(parents=True)
            if kind=='attachments':(person/'.notes').write_bytes(b'notes')
            if kind=='history':f.write(x.base/'archive-history.json',[{'creator':'history','vid':'historical-id','original_vid':'historical-id','raw':'untouched','candidates':[{'name':'history'}],'suggested':'history'}])
            else:f.write(x.base/'people-manual.json',[{'name':kind}])
            plan=x.preview(x.request_delete(kind));result=x.apply(plan)
            assert result['staged'] and result['changed']==(1 if kind=='attachments' else 0)
            assert not person.exists()
            if kind not in ('metadata','history'):assert (x.staged_root(plan)/'nested/empty').is_dir()
            if kind=='attachments':assert (x.staged_root(plan)/'.notes').read_bytes()==b'notes'
            assert kind in json.loads((x.base/'people-deleted.json').read_text())
            if kind=='history':
                history=json.loads((x.base/'archive-history.json').read_text())[0]
                assert history['candidates']==[{'name':'history'}] and history['suggested']=='history' and history['raw']=='untouched'
            print('PASS deletion',kind)
        finally:x.stop()

def guards():
    for kind in ('file','directory','candidate','candidate_insert','index','prefix_asset','target','account','source_ambiguity','symlink','dangling_symlink','fifo','alias','origin','thumbnail'):
        x=DeleteFixture()
        try:
            if kind in ('symlink','dangling_symlink','fifo'):
                if kind=='fifo':os.mkfifo(x.source.parent/'pipe')
                else:(x.source.parent/'link').symlink_to(x.source if kind=='symlink' else x.source.with_name('missing'))
                x.preview(x.request_delete(),409);print('PASS rejected special',kind);continue
            plan=x.preview(x.request_delete())
            if kind=='file':x.source.write_bytes(b'changed')
            elif kind=='directory':(x.source.parent/'late-empty').mkdir()
            elif kind=='candidate':
                with sqlite3.connect(x.base/'processing/processing.db') as db:db.execute("UPDATE tasks SET face_attempts=9 WHERE vid='candidate'")
            elif kind=='candidate_insert':
                with sqlite3.connect(x.base/'processing/processing.db') as db:db.execute("UPDATE tasks SET data=? WHERE vid='unrelated'",(json.dumps({'suggested':'旧甲'}),))
            elif kind=='index':
                with sqlite3.connect(x.base/'faces.db') as db:db.execute('UPDATE videos SET size=99 WHERE vid=?',(f.VID,))
            elif kind=='prefix_asset':
                with sqlite3.connect(x.ledger) as db:db.execute('INSERT INTO asset(location,path) VALUES(?,?)',('fixture',x.win(x.source.parent/'late.mp4')))
            elif kind=='target':x.staged_root(plan).mkdir(parents=True)
            elif kind=='account':f.write(x.base/'external-profiles/manual-links.json',{'links':[]})
            elif kind=='source_ambiguity':
                with sqlite3.connect(x.ledger) as db:db.execute("INSERT INTO entity(kind,canonical_name,normalized_name,created_at,updated_at) VALUES('creator','甲','different','fixture','fixture')")
            elif kind=='alias':f.write(x.base/'person-renames.json',{})
            elif kind=='thumbnail':
                with sqlite3.connect(x.base/'faces.db') as db:db.execute('INSERT INTO faces VALUES(?,?,?)',(f.VID,.9,'new-thumb'))
            elif kind=='origin':f.write(x.base/'staging-origins.json',{'foreign':{'operation':'foreign'}})
            x.apply(plan,409);assert x.source.exists();assert not (x.base/'.review-write.lock').exists()
            print('PASS stale/conflict guard',kind)
        finally:x.stop()

def rollback():
    for phase in ('destination_published:1','source_removed:1','faces_commit_intent','faces_committed_unrecorded','jobs_commit_intent','jobs_committed_unrecorded','json_replaced_unrecorded:people-deleted.json','json_replaced_unrecorded:external-profiles/manual-links.json','json_replaced_unrecorded:staging-origins.json','ledger_commit_intent'):
        x=DeleteFixture()
        try:
            plan=x.preview(x.request_delete());before={str(p.relative_to(x.base)):p.read_bytes() for p in x.base.rglob('*.json')}
            x.fault(phase,'error');x.apply(plan,503)
            assert x.status(plan)['status']=='rolled_back',x.status(plan)
            assert {str(p.relative_to(x.source.parent)):p.read_bytes() for p in x.source.parent.rglob('*') if p.is_file()}==x.contents
            assert not x.staged_root(plan).exists()
            for name,raw in before.items():assert (x.base/name).read_bytes()==raw,name
            assert not (x.base/'people-deleted.json').exists() and not (x.base/'staging-origins.json').exists()
            assert not (x.base/'.review-write.lock').exists()
            print('PASS complete rollback',phase)
        finally:x.stop()
    for store,table in [('faces','videos'),('jobs','tasks'),('ledger','asset')]:
        x=DeleteFixture()
        try:
            plan=x.preview(x.request_delete());path={'faces':x.base/'faces.db','jobs':x.base/'processing/processing.db','ledger':x.ledger}[store]
            with sqlite3.connect(path) as db:db.executescript(f"CREATE TRIGGER fixture_reject BEFORE UPDATE ON {table} BEGIN SELECT RAISE(ABORT,'fixture commit failure'); END;")
            x.apply(plan,503);assert x.status(plan)['status']=='rolled_back';assert x.source.exists()
            assert not (x.base/'people-deleted.json').exists();print('PASS actual transaction failure',store)
        finally:x.stop()

def crash(phase,foreign=False):
    x=DeleteFixture()
    try:
        plan=x.preview(x.request_delete());x.fault(phase)
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
        else:x.assert_deleted(plan)
        print('PASS restart',phase,'guarded foreign edit' if foreign else 'complete')
    finally:x.stop()

def hardlinks():
    x=DeleteFixture()
    try:
        other=x.source.parent/'hardlink.mp4';os.link(x.source,other);x.contents['hardlink.mp4']=f.PAYLOAD
        plan=x.preview(x.request_delete());x.apply(plan);x.assert_deleted(plan)
        print('PASS distinct hardlink names preserved')
    finally:x.stop()

def mixed_metadata():
    x=DeleteFixture()
    try:
        thumb='source-thumb.jpg'
        with sqlite3.connect(x.base/'faces.db') as db:db.execute('INSERT INTO faces VALUES(?,?,?)',(f.VID,.9,thumb))
        f.write(x.base/'report.json',{'identities':[{'name':'甲','vids':[]},{'name':'other','rep_thumb':'/thumb?p='+thumb,'tpl_faces':1,'vids':[{'vid':'other-video'}]}]})
        f.write(x.base/'claims-detail.json',[{'vid':'outside','name':'旧甲'},{'vid':f.VID,'name':'other'},{'vid':'other-video','name':'other'}])
        f.write(x.base/'clusters.json',[{'vids':[{'vid':f.VID},{'vid':'other-video'}],'candidates':[{'name':'甲'},{'name':'other'}],'matches':[{'name':'旧甲'}],'suggested':'甲','raw':'甲'}])
        f.write(x.base/'external-profiles/manual-links.json',{'links':[{'creator':'甲','profile':{'name':'other'}},{'creator':'other','profile':{'name':'甲','description':'旧甲'}}]})
        raw=b'{"raw_ocr":"untouched"}';(x.base/'watermarks/raw.json').write_bytes(raw)
        plan=x.preview(x.request_delete());x.apply(plan)
        report=json.loads((x.base/'report.json').read_text());assert report['identities']==[{'name':'other','rep_thumb':None,'tpl_faces':0,'vids':[{'vid':'other-video'}]}]
        assert json.loads((x.base/'claims-detail.json').read_text())==[{'vid':'other-video','name':'other'}]
        cluster=json.loads((x.base/'clusters.json').read_text())[0];assert cluster['vids']==[{'vid':'other-video'}] and cluster['candidates']==[{'name':'other'}] and cluster['matches']==[] and cluster['raw']=='甲'
        assert json.loads((x.base/'external-profiles/manual-links.json').read_text())['links']==[{'creator':'other','profile':{'name':'甲','description':'旧甲'}}]
        assert (x.base/'watermarks/raw.json').read_bytes()==raw
        print('PASS scoped decisions/other thumbnail invalidation/unrelated profile snapshots/raw OCR preserved')
    finally:x.stop()

def committed_and_cleanup():
    x=DeleteFixture()
    try:
        plan=x.preview(x.request_delete());x.fault('ledger_committed_unrecorded','error');x.apply(plan,503)
        assert x.status(plan)['status']=='recovery_needed' and not x.source.exists()
        (x.root/'rust-fault.json').unlink()
        result=x.call({'mode':'reconcile','request':plan['request'],'version':plan['version']})
        assert result['status']=='complete';x.assert_deleted(plan)
        print('PASS confirmed ledger commit never rolls files back; reconciliation finalizes')
    finally:x.stop()
    x=DeleteFixture()
    try:
        plan=x.preview(x.request_delete());x.fault('ledger_committed','pause',milliseconds=800)
        client=http.client.HTTPConnection('127.0.0.1',x.port,timeout=15)
        client.request('POST','/v1/person-files',json.dumps({'mode':'apply','request':plan['request'],'version':plan['version']}),{'Content-Type':'application/json','Authorization':'Bearer fixture-token'})
        for _ in range(1000):
            if (x.root/'rust-fault-reached.json').exists():break
            time.sleep(.01)
        else:raise AssertionError('checkpoint missing')
        (x.source.parent/'late-foreign').write_bytes(b'preserve')
        response=client.getresponse();result=json.load(response);assert response.status==200 and result['warning'],result
        assert x.status(plan)['status']=='complete' and (x.source.parent/'late-foreign').read_bytes()==b'preserve'
        assert '甲' in json.loads((x.base/'people-deleted.json').read_text()) and not (x.base/'.review-write.lock').exists()
        print('PASS cleanup warning preserves late foreign files; deleted marker prevents ghost')
    finally:x.stop()

if __name__=='__main__':
    success();empty();guards();rollback();hardlinks();mixed_metadata();committed_and_cleanup()
    for phase in ('directory_published','source_removed:1','faces_committed_unrecorded','jobs_committed_unrecorded','json_replaced_unrecorded:people-deleted.json','ledger_committed_unrecorded','source_directory_cleanup_intent','source_directory_removed_unrecorded','final_receipt_intent'):crash(phase)
    crash('jobs_committed_unrecorded',True)
