"""#19 native attempt publication: real HTTP, marked files and frozen production queue schema."""
import importlib.util,json,sqlite3,uuid,time,copy,threading,struct,os,subprocess
from pathlib import Path
spec=importlib.util.spec_from_file_location('fixture',Path(__file__).with_name('http-rust-file-operations.py'))
f=importlib.util.module_from_spec(spec);spec.loader.exec_module(f)

class Fixture(f.Fixture):
    def __init__(self):
        super().__init__()
        self.video=self.media/'pikpak/My Telegram/analysis.mp4';self.video.parent.mkdir(parents=True);self.video.write_bytes(b'analysis fixture bytes')
        self.run=str(uuid.uuid4());self.generation=str(uuid.uuid4());self.batch='cohort';self.vid='analysis'
        with sqlite3.connect(self.base/'faces.db') as db:db.execute('INSERT INTO videos(vid,path,name) VALUES(?,?,?)',(self.vid,str(self.video),self.video.name))
        with self.db() as db:
            db.execute('DROP TABLE tasks');db.executescript((f.REPO/'tests/fixtures/processing-publication-schema.sql').read_text())
            db.execute('INSERT OR REPLACE INTO settings VALUES (?,?)',('coordinator_generation',json.dumps(self.generation)))
            now=time.time();self.expiry=now+600
            db.execute("INSERT INTO tasks(vid,path,name,size,mtime_ns,stage,retry_stage,text_attempts,batch_id,created_at,updated_at,run_id,lease_expires_at,coordinator_generation) VALUES(?,?,?,?,?,'text','text',1,?,?,?,?,?,?)",(self.vid,str(self.video),self.video.name,self.video.stat().st_size,str(self.video.stat().st_mtime_ns),self.batch,now,now,self.run,self.expiry,self.generation))
            db.execute('INSERT INTO batch_items(batch_id,vid,name,path,size,mtime_ns,added_at) SELECT batch_id,vid,name,path,size,mtime_ns,created_at FROM tasks')
            db.execute("INSERT INTO processing_runs(id,vid,name,batch_id,stage,started_at,status) VALUES(?,?,?,?,?,?,?)",(self.run,self.vid,self.video.name,self.batch,'text',now,'running'))
        f.write(self.base/'watermarks/results.json',{'unrelated':{'value':'preserve'}})
    def db(self):return sqlite3.connect(self.base/'processing/processing.db',timeout=3)
    def meta(self,command,body={},status=200):return self.call({'command':command,'body':body},'/v1/metadata',status)
    def body(self,stage='text',ready=False):
        with self.db() as db:
            db.row_factory=sqlite3.Row;t=dict(db.execute('SELECT * FROM tasks WHERE vid=?',(self.vid,)).fetchone())
        if stage=='text' and ready:
            f.write(self.base/'watermarks/mappings.json',{'f'*64:{'kind':'person','creator':'Confirmed'}})
        scope={'vids':[self.vid],'people':['Confirmed'] if ready else [],'watermark_keys':['f'*64] if ready else [],'all_watermark_rules':stage=='text'}
        data=json.loads(t['data'])
        if stage=='text':
            cands=[{'key':'f'*64,'text':'@confirmed','token':'confirmed','platform':'X','kind':'account'}] if ready else []
            text_cands=[{'name':'Confirmed','source':'watermark','confirmed':True,'key':'f'*64,'detail':'@confirmed'}] if ready else []
            resolved_cands=[{'name':'Confirmed','sources':['watermark'],'evidence':text_cands,'score':None}] if ready else []
            data={'watermark':{'vid':self.vid,'path':str(self.video),'size':self.video.stat().st_size,'mtime_ns':str(self.video.stat().st_mtime_ns),'status':'done','pipeline':'ocr-v1','artifact_attempt':t['run_id'],'candidates':cands,'execution':{'backend':'cpu','stage':'ocr'}},'folder':[],'text_candidates':text_cands,'text_complete':True,'face_skipped':ready,'ready':ready,'kind':'suggested' if ready else 'none','candidates':resolved_cands,'suggested':'Confirmed' if ready else None}
        else:data.update(face={'matches':[],'face_count':2,'version':'face-v1','reference_version':'reference-v1','unknown_group':'untrusted-id','unknown_group_input':{'id':'provisional','version':'face-v1','embedding':[1.]+[0.]*511},'thumbs':[],'feature_cache':None,'execution':{'backend':'cpu','recognition_provider':'CPUExecutionProvider'}},face_complete=True,ready=True)
        result={'operation_id':t['run_id'],'scope':scope,'version':self.meta('state',{'scope':scope})['version'],'task':{k:t[k] for k in ['vid','path','size','mtime_ns','stage','run_id','batch_id','lease_expires_at','coordinator_generation']},'data':data,'next_stage':'review' if ready or stage=='face' else 'face_queued'}
        if stage=='face':result['reference_input_version']=self.meta('analysis-state')['version']
        return result
    def face_attempt(self):
        self.meta('processing-result',self.body())
        self.run=str(uuid.uuid4())
        with self.db() as db:
            db.execute("UPDATE tasks SET stage='face',run_id=?,face_attempts=1 WHERE vid=?",(self.run,self.vid))
            db.execute("INSERT INTO processing_runs(id,vid,name,batch_id,stage,started_at,status) VALUES(?,?,?,?,?,?,?)",(self.run,self.vid,self.video.name,self.batch,'face',time.time(),'running'))
        return self.body('face')
    def counts(self):
        with self.db() as db:return db.execute('SELECT COUNT(*) FROM events').fetchone()[0],db.execute('SELECT SUM(completed) FROM batch_items').fetchone()[0],db.execute("SELECT COUNT(*) FROM processing_runs WHERE status='done'").fetchone()[0]

def text_and_replay():
    x=Fixture()
    try:
        body=x.body(ready=True);result=x.meta('processing-result',body);assert result['accepted'] and result['data']==body['data'];assert x.counts()==(1,1,1)
        assert json.loads((x.base/'watermarks/results.json').read_text())['unrelated']=={'value':'preserve'}
        assert x.meta('processing-result',body)==result and x.counts()==(1,1,1)
        with x.db() as db:db.execute("UPDATE tasks SET stage='error',manual=1,note='human',run_id='changed',data='{}' WHERE vid=?",(x.vid,));db.execute("UPDATE settings SET value='\"restarted\"' WHERE key='coordinator_generation'")
        assert x.meta('processing-result',body)==result
        with x.db() as db:assert db.execute('SELECT note,data FROM tasks').fetchone()==('human','{}')
        changed=copy.deepcopy(body);changed['data']['kind']='conflict';x.meta('processing-result',changed,409);assert x.counts()==(1,1,1)
        print('PASS text acceptance, atomic run/cohort/event receipt, duplicate and later human/progress preservation')
    finally:x.stop()

def stale_inputs():
    for kind in ['file','lease','generation','manual','rules','queue','run','batch','field','scope']:
        x=Fixture()
        try:
            body=x.body()
            with x.db() as db:
                if kind=='lease':db.execute('UPDATE tasks SET lease_expires_at=?',(time.time()-1,))
                if kind=='generation':db.execute("UPDATE settings SET value='\"new\"' WHERE key='coordinator_generation'")
                if kind=='manual':db.execute('UPDATE tasks SET manual=1')
                if kind=='run':db.execute("UPDATE processing_runs SET status='interrupted'")
                if kind=='batch':db.execute('UPDATE batch_items SET excluded=1')
            if kind=='file':x.video.write_bytes(b'replaced')
            if kind=='rules':f.write(x.base/'watermarks/mappings.json',{'b'*64:{'kind':'person','creator':'other'}})
            if kind=='queue':f.write(x.base/'integration-queue.json',[{'vid':x.vid,'creator':'human'}])
            if kind=='field':body['data']['manual']=1
            if kind=='scope':body['scope']['vids']=[]
            x.meta('processing-result',body,400 if kind in ('field','scope') else 409)
            assert x.counts()==(0,0,0);assert x.vid not in json.loads((x.base/'watermarks/results.json').read_text())
            print('PASS stale/malformed',kind)
        finally:x.stop()

def face_groups():
    x=Fixture()
    try:
        body=x.face_attempt();result=x.meta('processing-result',body)
        group=result['data']['face']['unknown_group'];assert group and group!='untrusted-id';assert 'unknown_group_input' not in result['data']['face'];assert x.counts()==(2,1,2)
        with x.db() as db:assert db.execute('SELECT length(embedding) FROM face_groups WHERE id=?',(group,)).fetchone()==(2048,)
        # A second current face attempt matches an active compatible group atomically.
        run=str(uuid.uuid4())
        with x.db() as db:
            db.execute("UPDATE tasks SET stage='face',run_id=? WHERE vid=?",(run,x.vid));db.execute("INSERT INTO processing_runs(id,vid,name,batch_id,stage,started_at,status) VALUES(?,?,?,?,?,?,?)",(run,x.vid,x.video.name,x.batch,'face',time.time(),'running'))
        body=x.body('face');again=x.meta('processing-result',body);assert again['data']['face']['unknown_group']==group
        assert x.counts()==(3,1,3)
        with x.db() as db:assert db.execute('SELECT COUNT(*) FROM face_groups').fetchone()==(1,)
        print('PASS atomic unknown-group insert/match, stripped provisional vector, first-valid cohort completion')
    finally:x.stop()
    for kind in ['vector','reference','text_rewrite','artifact']:
        x=Fixture()
        try:
            body=x.face_attempt()
            if kind=='vector':body['data']['face']['unknown_group_input']['embedding']=[0.]*512
            if kind=='reference':body['reference_input_version']='stale'
            if kind=='text_rewrite':body['data']['watermark']['pipeline']='forged'
            if kind=='artifact':body['data']['face']['thumbs']=['/tmp/foreign.jpg']
            x.meta('processing-result',body,409 if kind in ('reference','text_rewrite') else 400)
            with x.db() as db:assert db.execute('SELECT COUNT(*) FROM face_groups').fetchone()==(0,)
            print('PASS no pre-admission group/artifact writes',kind)
        finally:x.stop()

def interrupted():
    for phase in ['processing_admitted','processing_json_published','processing_db_committed','processing_final_receipt_intent']:
        x=Fixture()
        try:
            body=x.body(ready=True);x.fault(phase)
            try:x.meta('processing-result',body);raise AssertionError('expected process death')
            except (OSError,__import__('http.client').client.HTTPException):pass
            x.process.wait(5);assert x.process.returncode==86
            committed=phase in ('processing_db_committed','processing_final_receipt_intent')
            if committed:
                with x.db() as db:db.execute("UPDATE tasks SET stage='error',manual=1,note='later human',run_id='new' WHERE vid=?",(x.vid,))
            (x.root/'rust-fault.json').unlink();x.start()
            status=x.call(route='/v1/person-files?id='+body['operation_id']);assert status['status']==('complete' if committed else 'rolled_back'),status
            assert x.counts()==((1,1,1) if committed else (0,0,0))
            if committed:
                assert x.meta('processing-result',body)['ok']
                with x.db() as db:assert db.execute('SELECT note,manual FROM tasks').fetchone()==('later human',1)
            else:x.meta('processing-result',body,409);assert x.vid not in json.loads((x.base/'watermarks/results.json').read_text())
            assert not (x.base/'.review-write.lock').exists()
            print('PASS crash',phase,'committed' if committed else 'rolled_back')
        finally:x.stop()

def json_failure_and_foreign():
    x=Fixture()
    try:
        body=x.body();x.fault('processing_json_published','pause',milliseconds=700)
        result=[]
        thread=threading.Thread(target=lambda:result.append(x.meta('processing-result',body,409)));thread.start()
        for _ in range(200):
            if (x.root/'rust-fault-reached.json').exists():break
            time.sleep(.01)
        values=json.loads((x.base/'watermarks/results.json').read_text());values['foreign-other']={'keep':True};f.write(x.base/'watermarks/results.json',values)
        x.fault('processing_db_commit_intent','error');thread.join(10);assert result and result[0]['code']=='publication_aborted'
        assert json.loads((x.base/'watermarks/results.json').read_text())=={'unrelated':{'value':'preserve'},'foreign-other':{'keep':True}}
        assert x.counts()==(0,0,0)
        print('PASS late DB/JSON failure rolls back selected vid and preserves foreign unrelated keys')
    finally:x.stop()
    x=Fixture()
    try:
        body=x.body();x.fault('processing_json_published')
        try:x.meta('processing-result',body)
        except (OSError,__import__('http.client').client.HTTPException):pass
        x.process.wait(5);values=json.loads((x.base/'watermarks/results.json').read_text());values[x.vid]={'foreign':'keep'};f.write(x.base/'watermarks/results.json',values)
        (x.root/'rust-fault.json').unlink();x.start()
        status=x.call(route='/v1/person-files?id='+body['operation_id']);assert status['status']=='recovery_needed',status
        assert json.loads((x.base/'watermarks/results.json').read_text())[x.vid]=={'foreign':'keep'}
        assert x.counts()==(0,0,0) and (x.base/'.review-write.lock').exists()
        # Exact replay retries recovery only after the foreign value has been explicitly reconciled.
        values=json.loads((x.base/'watermarks/results.json').read_text());values[x.vid]=body['data']['watermark'];f.write(x.base/'watermarks/results.json',values)
        x.meta('processing-result',body,409);assert not (x.base/'.review-write.lock').exists()
        print('PASS selected-video foreign edit retains recovery fence, reconciled exact replay releases it without counts')
    finally:x.stop()


def cache_integrity_and_schema():
    import hashlib
    x=Fixture()
    try:
        body=x.face_attempt();directory=x.base/'processing/attempts'/body['operation_id'];directory.mkdir(parents=True)
        cache=directory/'features.npz';cache.write_bytes(b'fixture feature bytes')
        body['data']['face']['feature_cache']=str(cache);body['data']['face']['feature_cache_sha256']=hashlib.sha256(cache.read_bytes()).hexdigest()
        accepted=x.meta('processing-result',body);identity=accepted['data']['face']['source_identity'];assert int(identity['ino'])==x.video.stat().st_ino
        f.write(x.base/'integration-queue.json',[{'vid':x.vid,'creator':'human'}])
        before=x.meta('analysis-state')['version'];cache.write_bytes(b'tampered cache bytes');assert x.meta('analysis-state')['version']!=before
        cache.write_bytes(b'fixture feature bytes');before=x.meta('analysis-state')['version']
        legacy=x.base/'processing/faces'/(hashlib.sha256(x.vid.encode()).hexdigest()+'.npz');legacy.parent.mkdir(exist_ok=True);legacy.write_bytes(b'legacy fixture');assert x.meta('analysis-state')['version']!=before
        # Later replays do not expose new bytes as a freshly accepted result.
        assert x.meta('processing-result',body)==accepted
        print('PASS accepted feature digest/source identity and assigned native/legacy reference-byte tokens')
    finally:x.stop()
    x=Fixture()
    try:
        body=x.face_attempt();directory=x.base/'processing/attempts'/body['operation_id'];directory.mkdir(parents=True);cache=directory/'features.npz';cache.write_bytes(b'actual')
        body['data']['face'].update(feature_cache=str(cache),feature_cache_sha256='0'*64)
        x.meta('processing-result',body,409)
        with x.db() as db:assert db.execute('SELECT COUNT(*) FROM face_groups').fetchone()==(0,)
        print('PASS mismatched feature bytes reject before any group or completion')
    finally:x.stop()
    x=Fixture()
    try:
        x.stop();state=x.base/'file-operations-rust/operations.sqlite3'
        with sqlite3.connect(state) as db:db.execute('PRAGMA user_version=4')
        x.start()
        with sqlite3.connect(state) as db:assert db.execute('PRAGMA user_version').fetchone()==(9,)
        x.stop()
        old=os.environ.get('OLD_FILE_OPERATIONS_BINARY')
        if old:
            result=subprocess.run([old],env=x.env,capture_output=True,text=True,timeout=10);assert result.returncode!=0 and 'schema_version' in result.stderr,result.stderr
            with sqlite3.connect(state) as db:assert db.execute('PRAGMA user_version').fetchone()==(9,)
        with sqlite3.connect(state) as db:db.execute('PRAGMA user_version=10')
        result=subprocess.run([str(f.BINARY)],env=x.env,capture_output=True,text=True,timeout=10);assert result.returncode!=0 and 'schema_version' in result.stderr,result.stderr
        with sqlite3.connect(state) as db:assert db.execute('PRAGMA user_version').fetchone()==(10,);db.execute('PRAGMA user_version=9')
        x.start();print('PASS schema4 to9 upgrade, future refusal without downgrade'+(' and actual older binary refusal' if old else ''))
    finally:x.stop()


def task_only_source():
    x=Fixture()
    try:
        with sqlite3.connect(x.base/'faces.db') as db:db.execute('DELETE FROM videos WHERE vid=?',(x.vid,))
        body=x.body(ready=True);state=x.meta('state',{'scope':body['scope']});assert state['material']['files'][0]['path']==str(x.video)
        x.meta('processing-result',body)
        page=x.call({'command':'review-page','body':{}},'/v1/archive')
        row=next(row for row in page['rows'] if row['vid']==x.vid)
        stat=x.video.stat();result=x.call({'command':'queue-confirm','body':{'creator':'Confirmed','vids':[x.vid],'source':'review','review_session':page['session'],'review_tokens':{x.vid:row['review_token']},'review_group':{'key':page['group']['key'],'deferred':bool(row['deferred'])},'fingerprints':{x.vid:{'path':str(x.video),'size':stat.st_size,'mtime_ns':str(stat.st_mtime_ns)}}}},'/v1/archive')
        assert result['ok'] and result['added']==1
        print('PASS task-only stable ID source fingerprint and explicit native review confirmation without faces index')
    finally:x.stop()


def rule_change_between_stages():
    for stage in ['face_queued','face','retry','error','review','text','queued']:
        x=Fixture()
        try:
            body=x.body();candidate={'key':'a'*64,'text':'@source','token':'source','platform':'X','kind':'account','frames':2}
            body['data']['watermark']['candidates']=[candidate];x.meta('processing-result',body)
            run=str(uuid.uuid4())
            with x.db() as db:
                db.execute('UPDATE tasks SET stage=?,run_id=? WHERE vid=?',(stage,run,x.vid))
                db.execute("INSERT INTO processing_runs(id,vid,name,batch_id,stage,started_at,status) VALUES(?,?,?,?,?,?,?)",(run,x.vid,x.video.name,x.batch,'face',time.time(),'running'))
                db.execute("INSERT INTO tasks(vid,path,name,size,mtime_ns,stage,batch_id,created_at,updated_at,manual,data,run_id) SELECT 'manual','/fixture/manual','manual',size,mtime_ns,'face',batch_id,created_at,updated_at,1,data,'manual-run' FROM tasks WHERE vid=?",(x.vid,))
                prior=db.execute('SELECT data FROM tasks WHERE vid=?',(x.vid,)).fetchone()[0]
            late=x.body('face') if stage=='face' else None
            x.meta('watermark-set',{'key':'a'*64,'kind':'person','name':'Changed','candidate':candidate,'evidence_vid':x.vid})
            with x.db() as db:
                changed=db.execute('SELECT stage,retry_stage,run_id,lease_expires_at,data FROM tasks WHERE vid=?',(x.vid,)).fetchone()
                assert changed==('queued','text','',0.,prior),changed
                assert db.execute("SELECT stage,run_id FROM tasks WHERE vid='manual'").fetchone()==('face','manual-run')
            if late:x.meta('processing-result',late,409)
            print('PASS rule edit requeues stale text interpretation and fences',stage)
        finally:x.stop()

def authoritative_business_rules_and_raw_submission():
    x=Fixture()
    try:
        key='c'*64
        f.write(x.base/'people-manual.json',[{'name':'Telegram'}])
        f.write(x.base/'person-renames.json',{'OldAlias':'CanonicalFace'})
        with x.db() as db:
            db.row_factory=sqlite3.Row;t=dict(db.execute('SELECT * FROM tasks WHERE vid=?',(x.vid,)).fetchone())
        scope={'vids':[x.vid],'people':['Telegram'],'watermark_keys':[key],'all_watermark_rules':True}
        raw_text={
            'operation_id':t['run_id'],
            'scope':scope,
            'version':x.meta('state',{'scope':scope})['version'],
            'task':{k:t[k] for k in ['vid','path','size','mtime_ns','stage','run_id','batch_id','lease_expires_at','coordinator_generation']},
            'data':{'watermark':{'vid':x.vid,'path':str(x.video),'size':x.video.stat().st_size,'mtime_ns':str(x.video.stat().st_mtime_ns),'status':'done','pipeline':'ocr-v1','artifact_attempt':t['run_id'],'candidates':[{'key':key,'text':'@unconfirmed_acct','token':'@unconfirmed_acct','platform':'X','kind':'account'}],'execution':{'backend':'cpu','stage':'ocr'}}},
        }
        res1=x.meta('processing-result',raw_text)
        assert res1['next_stage']=='face_queued'
        assert res1['data']['face_skipped'] is False and res1['data']['ready'] is False
        assert res1['data']['kind']=='conflict' and res1['data']['suggested'] is None
        assert [c['name'] for c in res1['data']['candidates']]==['unconfirmed_acct','Telegram']

        run2=str(uuid.uuid4())
        with x.db() as db:
            db.execute("UPDATE tasks SET stage='face',run_id=?,face_attempts=1 WHERE vid=?",(run2,x.vid))
            db.execute("INSERT INTO processing_runs(id,vid,name,batch_id,stage,started_at,status) VALUES(?,?,?,?,?,?,?)",(run2,x.vid,x.video.name,x.batch,'face',time.time(),'running'))
            db.row_factory=sqlite3.Row;t2=dict(db.execute('SELECT * FROM tasks WHERE vid=?',(x.vid,)).fetchone())
        scope2={'vids':[x.vid],'people':['Telegram','CanonicalFace'],'watermark_keys':[key],'all_watermark_rules':False}
        raw_face={
            'operation_id':run2,
            'scope':scope2,
            'version':x.meta('state',{'scope':scope2})['version'],
            'reference_input_version':x.meta('analysis-state')['version'],
            'task':{k:t2[k] for k in ['vid','path','size','mtime_ns','stage','run_id','batch_id','lease_expires_at','coordinator_generation']},
            'data':{'face':{'matches':[{'name':'OldAlias','source':'face','score':0.88,'support':5,'detail':'人脸匹配'}],'face_count':1,'version':'face-v1','reference_version':'reference-v1','thumbs':[],'feature_cache':None,'execution':{'backend':'cpu','recognition_provider':'CPUExecutionProvider'}}},
        }
        res2=x.meta('processing-result',raw_face)
        assert res2['next_stage']=='review' and res2['data']['ready'] is True and res2['data']['face_complete'] is True
        assert res2['data']['kind']=='conflict'
        assert [c['name'] for c in res2['data']['candidates']]==['unconfirmed_acct','CanonicalFace','Telegram']
        print('PASS raw OCR/face submission evaluated authoritatively in Rust with folder/watermark/face/rename rules')
    finally:x.stop()

if __name__=='__main__':
    for test in [text_and_replay,stale_inputs,face_groups,interrupted,json_failure_and_foreign,cache_integrity_and_schema,task_only_source,rule_change_between_stages,authoritative_business_rules_and_raw_submission]:test()
