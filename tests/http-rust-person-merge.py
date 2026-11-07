"""#10 native two-whole-person merge acceptance; marked isolated files and actual Peach schema."""
import importlib.util,json,sqlite3,uuid,threading,time,http.client,os
from pathlib import Path
spec=importlib.util.spec_from_file_location('rename',Path(__file__).with_name('http-rust-person-rename.py'))
r=importlib.util.module_from_spec(spec);spec.loader.exec_module(r)
f=r.f
class MergeFixture(r.RenameFixture):
    def __init__(self,target_entity=True):
        super().__init__()
        self.kept=self.media/'pikpak/creators/乙'
        (self.kept/'nested/empty').mkdir(parents=True)
        (self.kept/'original.mp4').write_bytes(b'target video')
        (self.kept/'nested/scanless.mp4').write_bytes(b'target scanless')
        (self.kept/'nested/readme.txt').write_bytes(b'target document')
        (self.source.parent/'file-v-dir').write_bytes(b'source file')
        (self.kept/'file-v-dir').mkdir()
        (self.source.parent/'dir-v-file/empty').mkdir(parents=True)
        (self.source.parent/'dir-v-file/a.bin').write_bytes(b'source attachment')
        (self.kept/'dir-v-file').write_bytes(b'target file')
        (self.source.parent/'NFC-Caf\u00e9.mp4').write_bytes(b'source unicode')
        (self.kept/'nfc-cafe\u0301.MP4').write_bytes(b'target unicode')
        self.contents={str(p.relative_to(self.source.parent)):p.read_bytes() for p in self.source.parent.rglob('*') if p.is_file()}
        self.target_contents={str(p.relative_to(self.kept)):p.read_bytes() for p in self.kept.rglob('*') if p.is_file()}
        f.write(self.base/'person-renames.json',{'旧甲':'甲','更旧甲':'旧甲','旧乙':'乙'})
        f.write(self.base/'people-manual.json',[{'name':'甲','created_at':'source'},{'name':'乙','created_at':'target','rep_thumb':'target-thumb'}])
        f.write(self.base/'template-overrides.json',{'甲':{'thumb':'source-thumb','name':'甲'},'乙':{'thumb':'target-thumb','name':'乙'}})
        f.write(self.base/'external-profiles/manual-links.json',{'links':[{'id':'source-link','creator':'旧甲','profile':{'id':'profile','name':'甲'}},{'id':'target-link','creator':'乙','profile':{'id':'profile','name':'external kept'}},{'id':'another-source','creator':'甲','profile':{'id':'second','name':'甲'}}]})
        f.write(self.base/'external-profiles/reviewed-links.json',{'links':[{'person':'甲','profile_id':'profile','confidence':'candidate','evidence':'source uncertainty'},{'person':'旧乙','profile_id':'profile','confidence':'reference','evidence':'target reference'}]})
        f.write(self.base/'person-rejections.json',{'source':{'name':'甲','vid':'same','at':'source'},'target':{'name':'旧乙','vid':'same','at':'target'}})
        f.write(self.base/'claims-detail.json',[{'name':'甲','vid':'same','s':.99},{'name':'乙','vid':'same','s':.5}])
        f.write(self.base/'integration-queue.json',[{'vid':'same','creator':'甲','at':'source','source':'person','source_id':'旧甲','return_href':'/persons/%E6%97%A7%E7%94%B2'},{'vid':'same','creator':'旧乙','at':'target'},{'vid':'unrelated','creator':'other'}])
        report=json.loads((self.base/'report.json').read_text());report['identities'][0].update(rep_thumb='source-thumb',aliases=['historic-source']);report['identities'].append({'name':'乙','rep_thumb':'target-thumb','tpl_faces':7,'vids':[{'vid':'target-id','path':str(self.kept/'original.mp4')}],'aliases':['historic-target']});f.write(self.base/'report.json',report)
        with sqlite3.connect(self.base/'processing/processing.db') as db:
            db.execute("UPDATE tasks SET data=? WHERE vid='candidate'",(json.dumps({'kind':'conflict','candidates':[{'name':'旧甲','score':.8,'support':3,'sources':['a'],'evidence':['a']},{'name':'乙','score':.9,'support':2,'sources':['b'],'evidence':['b']}],'ocr':'甲','vid':'甲','source':'person','source_id':'甲'}),))
        with sqlite3.connect(self.ledger) as db:
            db.execute("UPDATE entity SET metadata_json=?,region='source-region' WHERE id=1",(json.dumps({'source':'source-metadata'}),))
            db.execute('INSERT INTO asset(id,location,path,name) VALUES(3,?,?,?)',('fixture',self.win(self.kept/'original.mp4'),'target name'))
            db.execute("INSERT INTO asset_entity(asset_id,entity_id,role,source,confidence,metadata_json) VALUES(3,1,'performer','manual',.7,'{\"audit\":\"source role\"}')")
            if target_entity:
                db.execute("INSERT INTO entity(id,kind,canonical_name,normalized_name,metadata_json,created_at,updated_at,region) VALUES(2,'creator','乙','乙','{\"keep\":\"target\"}','fixture','fixture','target-region')")
                db.execute("INSERT INTO asset_entity(asset_id,entity_id,role,source,confidence) VALUES(1,2,'creator','manual',.42),(3,2,'creator','manual',.77)")
                db.execute("INSERT INTO entity_alias(entity_id,alias,normalized_alias,source,confidence) VALUES(2,'alias','alias','fixture',.42)")
        self.target_entity=target_entity
    def merge_request(self,person='甲',target='乙'):
        return {'id':str(uuid.uuid4()),'action':'merge-person','person':person,'target_person':target}
    def assert_success(self,plan):
        assert not self.source.parent.exists()
        for file in plan['files']:assert Path(file['dst']).read_bytes()==self.contents[str(Path(file['src']).relative_to(self.source.parent))]
        for name,data in self.target_contents.items():assert (self.kept/name).read_bytes()==data
        for directory in plan['execution']['data']['directory_inventory']['directories']:assert Path(directory['dst']).is_dir()
        assert self.status(plan)['status']=='complete'
        aliases=json.loads((self.base/'person-renames.json').read_text());assert all(aliases[n]=='乙' for n in ['甲','旧甲','更旧甲','旧乙'])
        assert json.loads((self.base/'template-overrides.json').read_text())=={'乙':{'thumb':'target-thumb','name':'乙'}}
        manual=json.loads((self.base/'people-manual.json').read_text());assert len(manual)==1 and manual[0]['created_at']=='target'
        links=json.loads((self.base/'external-profiles/manual-links.json').read_text())['links'];assert {v['id'] for v in links}=={'target-link','another-source'} and all(v['creator']=='乙' for v in links)
        assert next(v for v in links if v['id']=='another-source')['profile']['name']=='甲'
        reviewed=json.loads((self.base/'external-profiles/reviewed-links.json').read_text())['links'];assert len(reviewed)==1 and reviewed[0]['confidence']=='candidate' and reviewed[0]['person']=='乙'
        rejection=json.loads((self.base/'person-rejections.json').read_text());assert len(rejection)==1 and next(iter(rejection.values()))['at']=='target'
        claims=json.loads((self.base/'claims-detail.json').read_text());assert claims==[{'name':'乙','vid':'same','s':.99}]
        queue=json.loads((self.base/'integration-queue.json').read_text());assert len(queue)==2 and next(v for v in queue if v['vid']=='same')['at']=='target'
        report=json.loads((self.base/'report.json').read_text())['identities'];assert len(report)==1 and report[0]['rep_thumb']=='target-thumb' and report[0]['tpl_faces']==7 and len(report[0]['vids'])==2 and '甲' in report[0]['aliases']
        with sqlite3.connect(self.base/'processing/processing.db') as db:
            stage,data=db.execute("SELECT stage,data FROM tasks WHERE vid='candidate'").fetchone();data=json.loads(data);assert stage=='queued' and data['kind']=='suggested' and data['suggested']=='乙',data
            candidate=data['candidates'][0];assert len(data['candidates'])==1 and candidate['score']==.9 and candidate['support']==3 and candidate['sources']==['a','b'] and candidate['evidence']==['a','b']
            assert data['ocr']=='甲' and data['vid']=='甲' and db.execute('SELECT message FROM events').fetchone()[0]=='甲 historical audit'
        with sqlite3.connect(self.ledger) as db:
            entity=db.execute("SELECT id,metadata_json,region FROM entity WHERE kind='creator'").fetchall();assert len(entity)==1 and entity[0][0]!=1,entity
            eid,raw,region=entity[0];meta=json.loads(raw);assert meta['face_admin_merged_people'][0]['entity']['region']=='source-region'
            if self.target_entity:assert eid==2 and meta['keep']=='target' and region=='target-region'
            assert db.execute("SELECT confidence FROM asset_entity WHERE asset_id=1 AND entity_id=? AND role='creator' AND source='manual'",(eid,)).fetchone()[0]==(.42 if self.target_entity else 1.)
            assert db.execute("SELECT confidence FROM asset_entity WHERE asset_id=3 AND entity_id=? AND role='performer'",(eid,)).fetchone()[0]==.7
            assert db.execute('SELECT COUNT(*) FROM entity_alias').fetchone()[0]==1
            for aid,path in db.execute('SELECT id,path FROM asset'):assert 'creators\\乙\\' in path,(aid,path)
            assert not db.execute('PRAGMA foreign_key_check').fetchall()
        assert not (self.base/'.review-write.lock').exists()

def success():
    for target_entity in [True,False]:
        x=MergeFixture(target_entity)
        try:
            plan=x.preview(x.merge_request());assert len(plan['files'])==7
            assert len(plan['execution']['data']['target_preview']['files'])==5
            result=x.apply(plan);x.assert_success(plan);assert x.apply(plan)==result
            print('PASS full tree/collisions/target preferences/raw evidence/actual Peach relationships',target_entity)
        finally:x.stop()

def stores(x):
    result={'json':{str(p.relative_to(x.base)):p.read_bytes() for p in x.base.rglob('*.json') if 'file-operation-routing' not in str(p)}}
    for name,path,tables in [('faces',x.base/'faces.db',['videos','faces']),('jobs',x.base/'processing/processing.db',['tasks','events']),('ledger',x.ledger,['asset','entity','asset_entity','entity_alias','entity_redirect','entity_classification','entity_identity_link'])]:
        with sqlite3.connect(path) as db:result[name]={t:sorted(db.execute('SELECT * FROM '+t).fetchall(),key=repr) for t in tables}
    return result

def guards():
    for kind in ('same_person','missing_target','source_ambiguity','target_ambiguity','source_duplicate_asset','occupied_asset','unknown_fk','metadata_shape','symbolic_link','target_file','target_directory','source_file','source_directory','target_template','candidate','new_candidate','source_relation','target_relation','source_alias','target_alias','prefix_asset','override'):
        x=MergeFixture()
        try:
            if kind=='same_person':x.preview(x.merge_request(target='旧甲'),409);continue
            if kind=='missing_target':x.preview(x.merge_request(target='不存在'),404);continue
            before=stores(x)
            immediate=kind in ('source_ambiguity','target_ambiguity','source_duplicate_asset','occupied_asset','unknown_fk','metadata_shape','symbolic_link')
            plan=None if immediate else x.preview(x.merge_request())
            if kind=='symbolic_link':(x.source.parent/'unsafe').symlink_to(x.source)
            elif kind=='target_file':(x.kept/'original.mp4').write_bytes(b'foreign')
            elif kind=='target_directory':(x.kept/'late-empty').mkdir()
            elif kind=='source_file':x.source.write_bytes(b'foreign')
            elif kind=='source_directory':(x.source.parent/'late-empty').mkdir()
            elif kind=='target_template':f.write(x.base/'template-overrides.json',{'乙':{'thumb':'changed'}})
            elif kind=='override':f.write(x.base/'video-path-overrides.json',{f.VID:str(x.kept/'original.mp4')})
            elif kind in ('candidate','new_candidate'):
                with sqlite3.connect(x.base/'processing/processing.db') as db:
                    if kind=='candidate':db.execute("UPDATE tasks SET face_attempts=8 WHERE vid='candidate'")
                    else:db.execute("UPDATE tasks SET data=? WHERE vid='unrelated'",(json.dumps({'suggested':'甲'}),))
            else:
                with sqlite3.connect(x.ledger) as db:
                    if kind in ('source_ambiguity','target_ambiguity'):
                        name='甲' if kind=='source_ambiguity' else '乙';db.execute("INSERT INTO entity(kind,canonical_name,normalized_name,created_at,updated_at) VALUES('creator',?,'ambiguous','fixture','fixture')",(name,))
                    elif kind=='source_duplicate_asset':db.execute('INSERT INTO asset(location,path) VALUES(?,?)',('another-location',x.win(x.source)))
                    elif kind=='occupied_asset':db.execute('INSERT INTO asset(location,path) VALUES(?,?)',('fixture',x.win(x.kept/'missing-historical.mp4')))
                    elif kind=='unknown_fk':db.execute("INSERT INTO entity_redirect VALUES(99,1,'fixture','now')")
                    elif kind=='metadata_shape':db.execute("UPDATE entity SET metadata_json='[]' WHERE id=1")
                    elif kind.endswith('_relation'):db.execute("INSERT INTO asset_entity(asset_id,entity_id,role,source) VALUES(2,?,'observer','foreign')",(1 if kind.startswith('source') else 2,))
                    elif kind.endswith('_alias'):db.execute("INSERT INTO entity_alias(entity_id,alias,normalized_alias,source) VALUES(?,'late','late','foreign')",(1 if kind.startswith('source') else 2,))
                    elif kind=='prefix_asset':db.execute('INSERT INTO asset(location,path) VALUES(?,?)',('fixture',x.win(x.source.parent/'late-history.mp4')))
            if immediate:x.preview(x.merge_request(),409)
            else:x.apply(plan,409)
            assert x.source.exists() and (x.kept/'original.mp4').exists() and not (x.base/'.review-write.lock').exists()
            print('PASS stale/unsafe scope rejected before effects',kind)
        finally:x.stop()

def rollback():
    for target_entity in (True,False):
        for phase in ('destination_published:1','source_removed_unrecorded:1','faces_commit_intent','faces_committed_unrecorded','jobs_commit_intent','jobs_committed_unrecorded','json_replaced_unrecorded:person-renames.json','ledger_commit_intent'):
            x=MergeFixture(target_entity)
            try:
                plan=x.preview(x.merge_request());before=stores(x);x.fault(phase,'error');x.apply(plan,503)
                status=x.status(plan);assert status['status']=='rolled_back',status
                assert stores(x)==before
                assert {str(p.relative_to(x.source.parent)):p.read_bytes() for p in x.source.parent.rglob('*') if p.is_file()}==x.contents
                assert {str(p.relative_to(x.kept)):p.read_bytes() for p in x.kept.rglob('*') if p.is_file()}==x.target_contents
                assert not (x.base/'.review-write.lock').exists()
                print('PASS full-store compensation',target_entity,phase)
            finally:x.stop()
    for trigger in ('BEFORE DELETE ON entity','BEFORE INSERT ON entity_alias'):
        x=MergeFixture(False)
        try:
            plan=x.preview(x.merge_request());before=stores(x)
            with sqlite3.connect(x.ledger) as db:db.executescript("CREATE TRIGGER fixture_reject "+trigger+" BEGIN SELECT RAISE(ABORT,'fixture Peach failure'); END;")
            x.apply(plan,503);assert x.status(plan)['status']=='rolled_back';assert stores(x)==before
            print('PASS actual Peach constraint/transaction rollback',trigger)
        finally:x.stop()

def crash(phase,foreign=None,target_entity=True):
    x=MergeFixture(target_entity)
    try:
        plan=x.preview(x.merge_request());x.fault(phase)
        try:x.apply(plan)
        except (OSError,http.client.HTTPException):pass
        assert x.process.wait(timeout=5)==86
        (x.root/'rust-fault.json').unlink();x.stop()
        if foreign=='task':
            with sqlite3.connect(x.base/'processing/processing.db') as db:db.execute("UPDATE tasks SET note='foreign' WHERE vid='candidate'")
        if foreign=='ledger':
            with sqlite3.connect(x.ledger) as db:db.execute("UPDATE entity SET metadata_json='{\"foreign\":true}' WHERE canonical_name='乙'")
        if foreign=='file':Path(plan['files'][1]['dst']).write_bytes(b'foreign changed content')
        x.start();status=x.status(plan)
        if foreign:
            assert status['status']=='recovery_needed',status
            assert (x.base/'.review-write.lock').exists()
            if foreign=='task':
                with sqlite3.connect(x.base/'processing/processing.db') as db:assert db.execute("SELECT note FROM tasks WHERE vid='candidate'").fetchone()[0]=='foreign'
            if foreign=='ledger':
                with sqlite3.connect(x.ledger) as db:assert json.loads(db.execute("SELECT metadata_json FROM entity WHERE canonical_name='乙'").fetchone()[0])=={'foreign':True}
            if foreign=='file':assert Path(plan['files'][1]['dst']).read_bytes()==b'foreign changed content'
        else:x.assert_success(plan)
        print('PASS crash/restart',target_entity,phase,foreign or 'complete')
    finally:x.stop()

def extras():
    x=MergeFixture()
    try:
        staged=x.media/'pikpak/.face-cleanup-staging/held.mp4';staged.parent.mkdir(parents=True);staged.write_bytes(b'held staged')
        f.write(x.base/'video-path-overrides.json',{'held-source':str(x.source),'held-canonical':str(x.kept/'original.mp4'),'held-staged':str(staged)})
        old_history=json.loads((x.base/'archive-history.json').read_text());old_history.extend([{'vid':'held-staged','dst':str(x.kept/'original.mp4'),'original_dst':str(x.source),'creator':'甲'},{'vid':'held-canonical','dst':str(x.source),'creator':'甲'}]);f.write(x.base/'archive-history.json',old_history)
        plan=x.preview(x.merge_request());assert 'held-source' in next(v for v in plan['files'] if v['src']==str(x.source))['vids'];assert not any('held-staged' in v['vids'] or 'held-canonical' in v['vids'] for v in plan['files'])
        x.apply(plan);overrides=json.loads((x.base/'video-path-overrides.json').read_text());assert overrides['held-staged']==str(staged) and overrides['held-canonical']==str(x.kept/'original.mp4') and staged.read_bytes()==b'held staged'
        print('PASS independently held source/canonical/staged aliases retain locations')
    finally:x.stop()
    x=MergeFixture()
    try:
        duplicate=x.source.parent/'hardlink.mp4';os.link(x.source,duplicate);x.contents[duplicate.name]=duplicate.read_bytes()
        plan=x.preview(x.merge_request());x.apply(plan);x.assert_success(plan)
        print('PASS separate hardlink names preserved')
    finally:x.stop()
    x=MergeFixture()
    try:
        empty=x.media/'pikpak/creators/empty';(empty/'deep/empty').mkdir(parents=True)
        plan=x.preview(x.merge_request('empty'));assert not plan['files'];x.apply(plan);assert not empty.exists() and (x.kept/'deep/empty').is_dir()
        print('PASS empty-directory-only merge without synthetic media')
    finally:x.stop()
    x=MergeFixture()
    try:
        plan=x.preview(x.merge_request());stop=threading.Event()
        def worker():
            while not stop.is_set():
                with sqlite3.connect(x.base/'processing/processing.db') as db:db.execute("UPDATE tasks SET updated_at=updated_at+1,face_attempts=face_attempts+1 WHERE vid='unrelated'")
                time.sleep(.005)
        thread=threading.Thread(target=worker);thread.start()
        try:x.apply(plan);x.assert_success(plan)
        finally:stop.set();thread.join()
        print('PASS unrelated worker progress does not invalidate merge')
    finally:x.stop()

if __name__=='__main__':
    success();guards();rollback();extras()
    for phase in ('directory_published','destination_published:1','source_removed_unrecorded:1','faces_committed_unrecorded','jobs_committed_unrecorded','json_replaced_unrecorded:person-renames.json','ledger_committed_unrecorded','ledger_committed','source_directory_removed_unrecorded','complete'):
        crash(phase)
    crash('ledger_committed_unrecorded',target_entity=False)
    crash('jobs_committed_unrecorded','task')
    crash('ledger_committed_unrecorded','ledger')
    crash('destination_published:1','file')
