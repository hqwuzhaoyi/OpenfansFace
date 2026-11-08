"""#13: authenticated HTTP, real isolated JSON/SQLite/media, shared authority and crash receipts."""
import importlib.util,json,sqlite3,uuid,threading,time,concurrent.futures,sys,os
from pathlib import Path
spec=importlib.util.spec_from_file_location('fixture',Path(__file__).with_name('http-rust-file-operations.py'))
f=importlib.util.module_from_spec(spec);spec.loader.exec_module(f)

class Fixture(f.Fixture):
    def __init__(self):
        super().__init__()
        f.write(self.base/'external-profiles/manual-links.json',{'version':1,'source':'nv-pu-sa','links':[]})
        f.write(self.base/'external-profiles/nv-pu-sa.json',{'fetched_at':'2026-11-08T00:00:00Z','data':[{'id':'123','screen_name':'creator','name':'甲 upstream','description':'original evidence','followers_count':5,'last_synced_at':'2026-11-08T00:00:00Z'},{'id':'124','screen_name':'second','name':'乙','description':'','followers_count':None,'last_synced_at':None}]})
        f.write(self.base/'person-renames.json',{'旧甲':'甲'})
        self.video=self.media/'pikpak/My Telegram/candidate.mp4';self.video.parent.mkdir(parents=True);self.video.write_bytes(b'candidate')
        with sqlite3.connect(self.base/'faces.db') as db:db.execute('INSERT INTO videos(vid,path,name) VALUES(?,?,?)',('candidate',str(self.video),self.video.name))
        with sqlite3.connect(self.base/'processing/processing.db') as db:
            db.execute('ALTER TABLE tasks ADD COLUMN manual INTEGER DEFAULT 0');db.execute('ALTER TABLE tasks ADD COLUMN deferred INTEGER DEFAULT 0');db.execute("ALTER TABLE tasks ADD COLUMN note TEXT DEFAULT ''")
        report=json.loads((self.base/'report.json').read_text());report['identities'][0]['vids'].append({'vid':'candidate','path':str(self.video),'name':self.video.name});f.write(self.base/'report.json',report)
        self.scope={'people':['旧甲'],'vids':[f.VID],'watermark_keys':['a'*64]}
    def meta(self,command,body=None,status=200):return self.call({'command':command,'body':body or {}},'/v1/metadata',status)
    def link(self,**kw):return self.meta('account-link',{'name':'旧甲','profile_id':'123','handle':'creator',**kw})
    def rename(self):return self.preview({'id':str(uuid.uuid4()),'action':'rename-person','person':'甲','new_name':'新甲'})
    def links(self):return json.loads((self.base/'external-profiles/manual-links.json').read_text())['links']
    def material(self):return self.meta('state',{'scope':self.scope})

def accounts_and_concurrency():
    x=Fixture()
    try:
        plan=x.rename();oid=str(uuid.uuid4());result=x.link(operation_id=oid)
        assert result==x.link(operation_id=oid);assert len(x.links())==1
        assert x.links()[0]['profile']['name']=='甲 upstream'
        x.meta('account-link',{'operation_id':oid,'name':'甲','profile_id':'124','handle':'second'},409)
        x.apply(plan,409)
        # Fresh rename incorporates account edits; old alias resolves after rename.
        plan=x.rename();assert x.apply(plan)['ok'];assert x.links()[0]['creator']=='新甲'
        x.link();assert len(x.links())==1
        x.meta('account-unlink',{'name':'旧甲','link_id':oid});assert x.links()==[]
        assert not (x.base/'integration-queue.json').exists(), 'account association is not a claim'
        # Both HTTP callers are admitted to one writer lane; unrelated subjects survive.
        x.meta('person-remember',{'name':'乙'})
        with concurrent.futures.ThreadPoolExecutor(2) as pool:
            results=list(pool.map(lambda b:x.meta('account-link',b),[{'name':'旧甲','profile_id':'123','handle':'creator'},{'name':'乙','profile_id':'124','handle':'second'}]))
        assert len(x.links())==2 and all(r['ok'] for r in results)
        saved=x.links();broken=json.loads((x.base/'external-profiles/manual-links.json').read_text());broken['links'][0]['linked_at']='invalid';f.write(x.base/'external-profiles/manual-links.json',broken)
        x.meta('account-unlink',{'name':'旧甲','link_id':saved[0]['id']},503)
        assert json.loads((x.base/'external-profiles/manual-links.json').read_text())==broken
        print('PASS accounts, aliases, stale rename, concurrent saves, no implicit claim, corrupt-store protection')
    finally:x.stop()

def material_versions():
    x=Fixture()
    try:
        version=x.material()['version']
        with sqlite3.connect(x.base/'processing/processing.db') as db:db.execute("UPDATE tasks SET stage='review',updated_at=777,data='{}'")
        report=json.loads((x.base/'report.json').read_text());report['identities'][0]['candidate_score']=.123;f.write(x.base/'report.json',report)
        x.meta('person-remember',{'name':'甲'})
        f.write(x.base/'integration-queue.json',[{'vid':'unrelated','creator':'甲'}])
        assert x.material()['version']==version,'nonmaterial progress reordered evidence or another confirmation invalidated scope'
        x.meta('check-state',{'scope':x.scope,'version':version})
        x.link();assert x.material()['version']!=version
        assert x.meta('check-state',{'scope':x.scope,'version':version},409)['code']=='material_changed'
        stale=x.material()['version']
        with sqlite3.connect(x.base/'processing/processing.db') as db:db.execute('UPDATE tasks SET deferred=1 WHERE vid=?',(f.VID,))
        x.meta('template-set',{'name':'旧甲','groups':[0],'scope':x.scope,'expected_version':stale},409)
        current=x.material()['version'];x.source.write_bytes(b'replaced');assert x.material()['version']!=current
        print('PASS scoped material vs analysis/progress/other confirmation; file and manual state conflicts')
    finally:x.stop()

def rules_rejections_templates():
    x=Fixture()
    try:
        key='a'*64;candidate={'key':key,'text':'@creator','token':'creator','platform':'X','kind':'account','frames':3}
        stat=x.video.stat();f.write(x.base/'watermarks/results.json',{'candidate':{'path':str(x.video),'size':stat.st_size,'mtime_ns':str(stat.st_mtime_ns),'candidates':[candidate]}})
        x.meta('watermark-set',{'name':'旧甲','kind':'person','key':key,'candidate':candidate,'evidence_vid':'candidate'})
        mapping=json.loads((x.base/'watermarks/mappings.json').read_text())[key];assert mapping['creator']=='甲'
        assert x.links()==[]
        x.meta('watermark-set',{'kind':'source','key':key,'candidate':candidate,'evidence_vid':'candidate'});assert 'creator' not in json.loads((x.base/'watermarks/mappings.json').read_text())[key]
        x.meta('watermark-set',{'kind':'remove','key':key});assert not json.loads((x.base/'watermarks/mappings.json').read_text())
        x.video.write_bytes(b'changed after evidence');x.meta('watermark-set',{'name':'甲','kind':'person','key':key,'candidate':candidate,'evidence_vid':'candidate'},409)
        stat=x.video.stat();body={'name':'旧甲','vid':'candidate','path':str(x.video),'size':stat.st_size,'mtime_ns':str(stat.st_mtime_ns)}
        rejected=x.meta('recommendation-reject',body);x.meta('recommendation-undo',{'name':'旧甲','vid':'candidate','rejection_id':'foreign'},409)
        x.meta('recommendation-undo',{'name':'甲','vid':'candidate','rejection_id':rejected['id']});assert json.loads((x.base/'person-rejections.json').read_text())=={}
        x.meta('template-set',{'name':'旧甲','groups':[1,2]});assert json.loads((x.base/'template-overrides.json').read_text())['甲']['groups']==[1,2]
        x.meta('template-set',{'name':'甲','groups':[-1]},400);x.meta('arbitrary-write',{'name':'甲','path':'../outside'},400)
        print('PASS mapping save/source/remove, rejection/undo, template validation')
    finally:x.stop()

def publish_and_foreign():
    x=Fixture()
    try:
        f.write(x.base/'claims-detail.json',[{'audit':'preserve'}]);f.write(x.base/'promoted.json',{'audit':'preserve'})
        version=x.meta('analysis-state')['version'];x.meta('template-set',{'name':'甲','groups':[0]})
        x.meta('analysis-publish',{'version':version,'outputs':{'report.json':{'identities':[]}}},409)
        version=x.meta('analysis-state')['version'];x.meta('analysis-publish',{'version':version,'outputs':{'report.json':{'identities':[]}}})
        assert json.loads((x.base/'claims-detail.json').read_text())==[{'audit':'preserve'}]
        assert json.loads((x.base/'promoted.json').read_text())=={'audit':'preserve'}
        version=x.meta('analysis-state')['version'];x.video.write_bytes(b'changed input bytes')
        x.meta('analysis-publish',{'version':version,'outputs':{'report.json':{'identities':[]}}},409)
        # Native preview then foreign modification must never replace it.
        request={'id':str(uuid.uuid4()),'action':'metadata-write','person':'','files':[],'payload':{'command':'template-set','name':'甲','groups':[2],'at':'2026-11-08T00:00:00Z'}}
        plan=x.preview(request);f.write(x.base/'template-overrides.json',{'甲':{'groups':[9]},'foreign':{'groups':[8]}})
        x.apply(plan,409);assert json.loads((x.base/'template-overrides.json').read_text())['foreign']['groups']==[8]
        print('PASS guarded F_OUT publication, optional reports preserved, file/foreign changes fenced')
    finally:x.stop()

def crash_receipt():
    x=Fixture()
    try:
        oid=str(uuid.uuid4());x.fault('json_replaced_unrecorded:template-overrides.json')
        try:x.meta('template-set',{'operation_id':oid,'name':'甲','groups':[0]});raise AssertionError('expected process death')
        except (OSError, __import__('http.client').client.HTTPException):pass
        x.process.wait(5);assert x.process.returncode==86
        (x.root/'rust-fault.json').unlink();x.start()
        receipt=x.call(route='/v1/person-files?id='+oid)
        assert receipt['status'] in ('complete','rolled_back'),receipt
        if receipt['status']=='complete':assert x.meta('template-set',{'operation_id':oid,'name':'甲','groups':[0]})['id']==oid
        else:assert not (x.base/'template-overrides.json').exists()
        assert not (x.base/'.review-write.lock').exists()
        # A real legacy lock cannot be stolen even by a metadata-only mutation.
        (x.base/'.review-write.lock').write_text('{"pid":99999}')
        x.meta('template-set',{'name':'甲','groups':[1]},409)
        assert (x.base/'.review-write.lock').exists()
        print('PASS interrupted publication receipt/restart and shared legacy lock')
    finally:x.stop()


def simultaneous_rename_save():
    x=Fixture()
    try:
        plan=x.rename();x.fault('gate_acquired','pause',milliseconds=250)
        with concurrent.futures.ThreadPoolExecutor(2) as pool:
            rename=pool.submit(x.apply,plan)
            for _ in range(300):
                if (x.root/'rust-fault-reached.json').exists():break
                time.sleep(.01)
            save=pool.submit(x.link)
            assert rename.result()['ok'] and save.result()['ok']
        assert x.links()[0]['creator']=='新甲'
        assert (x.media/'pikpak/creators/新甲/original.mp4').read_bytes()==f.PAYLOAD
        print('PASS overlapping HTTP rename and account save serializes with preserved alias')
    finally:x.stop()

def calibrator_handoff():
    x=Fixture();saved_env={key:os.environ.get(key) for key in ['FILE_OPERATIONS_RUST_URL','FILE_OPERATIONS_RUST_TOKEN']}
    try:
        sys.path.insert(0,str(f.REPO/'sidecar'))
        from native_metadata import Rebuilder
        os.environ['FILE_OPERATIONS_RUST_URL']=x.url;os.environ['FILE_OPERATIONS_RUST_TOKEN']='fixture-token'
        # A fixture computation only; no private deployed model script is copied.
        (x.base/'identify_faces.py').write_text("import os,json,time,pathlib\nbase=pathlib.Path.cwd()\nout=pathlib.Path(os.environ['F_OUT'])\nassert out!=base\n(base/'fixture-analysis-started').write_text(str(out))\ntime.sleep(.2)\n(out/'report.json').write_text(json.dumps({'identities':[], 'fixture':True}))\n")
        state={'running':False};runner=Rebuilder(str(x.base),sys.executable,state)
        f.write(x.base/'claims-detail.json',[{'prior':True}])
        result=runner.pick({'name':'甲','groups':[0]});assert result['started']
        for _ in range(300):
            if not state['running']:break
            time.sleep(.01)
        assert state['ok'],state
        assert json.loads((x.base/'report.json').read_text())['fixture']
        assert json.loads((x.base/'claims-detail.json').read_text())==[{'prior':True}]
        assert not Path((x.base/'fixture-analysis-started').read_text()).exists(), 'temporary F_OUT leaked'
        (x.base/'fixture-analysis-started').unlink();runner.pick({'name':'甲','groups':[1]})
        for _ in range(300):
            if (x.base/'fixture-analysis-started').exists():break
            time.sleep(.01)
        x.meta('template-set',{'name':'甲','groups':[9]})
        for _ in range(300):
            if not state['running']:break
            time.sleep(.01)
        assert state['ok'] is False and '变化' in state['msg'],state
        receipt=x.call(route='/v1/person-files?id='+state['publish_operation_id'],status=404)
        assert receipt['code']=='not_found',receipt
        print('PASS real calibrator transport, isolated output, optional preservation and stale publish refusal')
    finally:
        for key,value in saved_env.items():
            if value is None:os.environ.pop(key,None)
            else:os.environ[key]=value
        x.stop()

def interrupted_foreign_report():
    x=Fixture()
    try:
        oid=str(uuid.uuid4());version=x.meta('analysis-state')['version']
        x.fault('json_replaced_unrecorded:claims-detail.json')
        try:x.meta('analysis-publish',{'operation_id':oid,'version':version,'outputs':{'claims-detail.json':[], 'report.json':{'identities':[]}}});raise AssertionError('expected crash')
        except (OSError, __import__('http.client').client.HTTPException):pass
        x.process.wait(5);assert x.process.returncode==86
        f.write(x.base/'claims-detail.json',[{'foreign':'retain'}]);(x.root/'rust-fault.json').unlink();x.start()
        receipt=x.call(route='/v1/person-files?id='+oid);assert receipt['status']=='recovery_needed',receipt
        assert json.loads((x.base/'claims-detail.json').read_text())==[{'foreign':'retain'}]
        x.meta('template-set',{'name':'甲','groups':[0]},503)
        x.meta('state',{'scope':x.scope},503)
        assert (x.base/'.review-write.lock').exists()
        print('PASS interrupted multifile publish retains foreign writes and fences later mutations')
    finally:x.stop()


def interrupted_optional_reports():
    x=Fixture()
    try:
        oid=str(uuid.uuid4());version=x.meta('analysis-state')['version'];outputs={'claims-detail.json':[{'candidate':'fresh'}],'promoted.json':[{'suggestion':'still unconfirmed'}],'report.json':{'identities':[]}}
        x.fault('json_replaced_unrecorded:promoted.json')
        try:x.meta('analysis-publish',{'operation_id':oid,'version':version,'outputs':outputs});raise AssertionError('expected crash')
        except (OSError, __import__('http.client').client.HTTPException):pass
        x.process.wait(5);assert x.process.returncode==86
        (x.root/'rust-fault.json').unlink();x.start()
        receipt=x.call(route='/v1/person-files?id='+oid);assert receipt['status']=='complete',receipt
        for name,value in outputs.items():assert json.loads((x.base/name).read_text())==value
        assert x.meta('analysis-publish',{'operation_id':oid,'version':version,'outputs':outputs})['id']==oid
        assert not (x.base/'integration-queue.json').exists()
        print('PASS optional claims/promoted array publication crash recovery and replay without confirmation')
    finally:x.stop()

if __name__=='__main__':
    for test in [accounts_and_concurrency,material_versions,rules_rejections_templates,publish_and_foreign,crash_receipt,simultaneous_rename_save,calibrator_handoff,interrupted_foreign_report,interrupted_optional_reports]:test()
