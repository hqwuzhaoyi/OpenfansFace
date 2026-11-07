"""#16 irreversible native staging acceptance. Only marked disposable fixtures."""
import importlib.util,json,sqlite3,uuid,signal,urllib.error,time,threading,os
from pathlib import Path
spec=importlib.util.spec_from_file_location('fixture',Path(__file__).with_name('http-rust-file-operations.py'))
m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
class Fixture(m.Fixture):
    def call(self, body=None, route='/v1/person-files', status=200, token=True):
        headers={'Content-Type':'application/json'}
        if token:headers['Authorization']='Bearer fixture-token'
        request=m.urllib.request.Request(self.url+route,data=None if body is None else json.dumps(body).encode(),headers=headers)
        try:
            with m.urllib.request.urlopen(request,timeout=120) as response:code,value=response.status,json.load(response)
        except m.urllib.error.HTTPError as error:code,value=error.code,json.load(error)
        assert code==status,(code,status,value,self.root)
        return value


def stage(f):
    r=f.request();r['action']='delete-files';r.pop('filename');p=f.preview(r);f.apply(p)
    return Path(p['files'][0]['dst'])
def preview(f,clear=False,rows=None):
    rows=rows if rows is not None else f.call(route='/v1/staging')['rows']
    return f.preview({'id':str(uuid.uuid4()),'action':'clear-staging' if clear else 'delete-staged','person':'','files':[] if clear else [{k:r[k] for k in ('vid','path','version')} for r in rows]})
def apply(f,p,status=200,confirmation=None):
    return f.call({'mode':'apply','request':p['request'],'version':p['version'],'confirmation':confirmation if confirmation is not None else ('清空回收站' if p['request']['action']=='clear-staging' else '永久删除')},status=status)
def reconcile(f,p):return f.call({'mode':'reconcile','request':p['request'],'version':p['version']})
def success():
    f=Fixture()
    try:
        dst=stage(f)
        m.write(f.base/'video-health.json',{'records':{'selected':{'path':str(dst),'status':'ok'},'unrelated':{'path':'elsewhere','status':'unchecked'}},'progress':1})
        p=preview(f);assert p['files'][0]['src']==str(dst)
        health=json.loads((f.base/'video-health.json').read_text());health['records']['unrelated']['status']='ok';health['progress']=2;m.write(f.base/'video-health.json',health)
        history=json.loads((f.base/'archive-history.json').read_text());history.append({'vid':'unrelated','dst':'elsewhere','progress':123});m.write(f.base/'archive-history.json',history)
        apply(f,p,400,'删除');assert dst.read_bytes()==m.PAYLOAD
        with sqlite3.connect(f.base/'processing/processing.db') as db:db.execute("UPDATE tasks SET updated_at=42 WHERE vid='unrelated'")
        result=apply(f,p);assert result['completed']==1 and not dst.exists(),result
        assert apply(f,p)==result
        s=f.status(p);assert s['committed']==1 and s['files'][0]['status']=='complete',s
        with sqlite3.connect(f.ledger) as db:
            assert db.execute('SELECT disposal FROM asset WHERE id=1').fetchone()==('trash',)
            assert db.execute('SELECT count(*) FROM asset_entity').fetchone()==(2,)
        assert json.loads((f.base/'archive-history.json').read_text())[0]['file_state']=='deleted'
        health=json.loads((f.base/'video-health.json').read_text());assert health['progress']==2 and health['records']['unrelated']['status']=='ok' and health['records']['selected']['status']=='missing'
        assert json.loads((f.base/'archive-history.json').read_text())[-1]['progress']==123
        assert not (f.base/'.review-write.lock').exists()
        f.stop();f.start();assert apply(f,p)==result
        print('PASS selected deletion explicit confirmation real bytes native stores idempotency restart')
    finally:f.stop()
def clear_scope():
    f=Fixture()
    try:
        root=f.media/'pikpak/.face-cleanup-staging';root.mkdir()
        for i in range(102):(root/f'{i:03d}.mp4').write_bytes(b'isolated')
        listing=f.call(route='/v1/staging?q=001');assert listing['matched']==1 and listing['files']==102
        p=preview(f,True);assert len(p['files'])==102
        extra=root/'late.mp4';extra.write_bytes(b'late');apply(f,p,409);assert len(list(root.iterdir()))==103
        p=preview(f,True);assert len(p['files'])==103
        result=apply(f,p);assert result['completed']==103 and not list(root.iterdir()),result
        print('PASS full clear beyond 100-row listing independent search and stale additions')
    finally:f.stop()
def safety():
    for kind in ['symlink','fifo','file','mode','ledger','jobs','origin','late-source']:
        f=Fixture()
        try:
            dst=stage(f)
            if kind in ('symlink','fifo'):
                special=dst.parent/'unsafe';special.symlink_to(f.source.parent) if kind=='symlink' else os.mkfifo(special)
                f.preview({'id':str(uuid.uuid4()),'action':'clear-staging','person':'','files':[]},409)
                assert dst.exists();print('PASS clear rejects',kind);continue
            p=preview(f)
            if kind=='file':dst.write_bytes(b'changed')
            elif kind=='mode':dst.chmod(0o400)
            elif kind=='ledger':
                with sqlite3.connect(f.ledger) as db:db.execute("UPDATE asset SET name='late' WHERE id=1")
            elif kind=='jobs':
                with sqlite3.connect(f.base/'processing/processing.db') as db:db.execute("UPDATE tasks SET data='{}' WHERE vid=?",(m.VID,))
            elif kind=='origin':
                v=json.loads((f.base/'staging-origins.json').read_text());v[str(dst)]['original_path']='changed';m.write(f.base/'staging-origins.json',v)
            elif kind=='late-source':dst.unlink();dst.symlink_to(f.root/'ledger.db')
            apply(f,p,409);assert dst.exists()
            print('PASS stale guard',kind)
        finally:f.stop()
def boundary_errors():
    for phase in ['delete_item_intent','delete_ledger_reserved','delete_ledger_committed','delete_before_unlink','delete_after_unlink','delete_faces_committed','delete_jobs_committed','delete_json_replaced','delete_item_complete','delete_result_committed']:
        f=Fixture()
        try:
            dst=stage(f);p=preview(f);f.fault(phase,'error')
            apply(f,p,200 if phase in ('delete_item_intent','delete_ledger_reserved') else 503)
            state=f.status(p);(f.root/'rust-fault.json').unlink()
            if phase in ('delete_item_intent','delete_ledger_reserved'):
                assert state['status']=='complete' and state['result']['status']=='partial' and dst.exists(),state
            elif phase=='delete_before_unlink':
                state=reconcile(f,p);assert state['status']=='recovery_needed' and dst.exists() and (f.base/'.review-write.lock').exists(),state
            elif phase=='delete_result_committed':
                assert state['status']=='complete';f.stop();f.start();assert not (f.base/'.review-write.lock').exists()
            else:
                state=reconcile(f,p);assert state['status']=='complete' and not dst.exists(),state
            print('PASS native error boundary',phase)
        finally:f.stop()
def crash():
    for phase in ['delete_item_intent','delete_ledger_reserved','delete_ledger_committed','delete_before_unlink','delete_after_unlink','delete_faces_committed','delete_jobs_committed','delete_json_replaced','delete_item_complete','delete_result_committed']:
        f=Fixture()
        try:
            dst=stage(f);p=preview(f);f.fault(phase,'pause',milliseconds=10000)
            result=[]
            def call():
                try:apply(f,p)
                except Exception as error:result.append(error)
            t=threading.Thread(target=call);t.start()
            for _ in range(400):
                if (f.root/'rust-fault-reached.json').exists():break
                time.sleep(.02)
            else:raise AssertionError(('not reached',phase))
            f.process.kill();f.process.wait();t.join();f.log.close();(f.root/'rust-fault.json').unlink();f.start()
            s=f.status(p)
            if phase=='delete_before_unlink':assert s['status']=='recovery_needed' and dst.exists(),s
            else:assert s['status']=='complete' and not dst.exists(),s
            print('PASS actual SIGKILL/restart',phase)
        finally:f.stop()
def unknown_mutations():
    for mode in ('mutation_error_after','mutation_error_delayed'):
        for errno in ('EIO','ESTALE','ETIMEDOUT'):
            f=Fixture()
            try:
                dst=stage(f);p=preview(f);f.fault('mutation:staging_unlink',mode,errno=errno)
                apply(f,p,503);s=f.status(p);assert s['failure']['mutation_outcome']=='unknown',s
                assert dst.exists()==(mode=='mutation_error_delayed')
                (f.root/'rust-fault.json').unlink();f.stop();f.start()
                for _ in range(2):
                    s=reconcile(f,p);assert s['status']=='recovery_needed' and (f.base/'.review-write.lock').exists(),s
                assert dst.exists()==(mode=='mutation_error_delayed')
                print('PASS held ambiguous native unlink',mode,errno)
            finally:f.stop()
def partial():
    f=Fixture()
    try:
        root=f.media/'pikpak/.face-cleanup-staging';root.mkdir();a=root/'a.mp4';b=root/'b.mp4';c=root/'c.mp4'
        for p in [a,b,c]:p.write_bytes(m.PAYLOAD)
        with sqlite3.connect(f.ledger) as db:
            db.execute('INSERT INTO asset(id,location,path,name) VALUES(2,?,?,?)',('fixture',f.win(b),'second'))
            db.executescript("CREATE TRIGGER fail_disposal BEFORE UPDATE ON asset WHEN NEW.id=2 BEGIN SELECT RAISE(ABORT,'isolated fail second'); END")
        p=preview(f,True);result=apply(f,p);assert result['status']=='partial' and result['completed']==1 and result['files'][1]['status']=='failed' and result['files'][2]['status']=='not_started',result
        assert not a.exists() and b.read_bytes()==m.PAYLOAD and c.read_bytes()==m.PAYLOAD
        assert apply(f,p)==result and not (f.base/'.review-write.lock').exists()
        print('PASS per-item receipt stop partial failure preserves later bytes')
    finally:f.stop()

def duplicate_and_legacy():
    for origin in ['unknown','native','legacy']:
        f=Fixture()
        try:
            root=f.media/'pikpak/.face-cleanup-staging';root.mkdir();duplicate=root/'duplicate.mp4';os.link(f.source,duplicate)
            with sqlite3.connect(f.base/'faces.db') as db:db.execute('INSERT INTO faces VALUES(?,?,?)',(m.VID,0.9,'keep-thumb'))
            old_history=json.loads((f.base/'archive-history.json').read_text());old_history[0]['staged_path']=str(duplicate);m.write(f.base/'archive-history.json',old_history)
            if origin=='native':m.write(f.base/'staging-origins.json',{str(duplicate):{'original_path':str(f.source),'source':'native-receipt','action':'stage-duplicate','vids':[m.VID]}})
            if origin=='legacy':m.write(f.base/'manifests/apply-old.json',{'status':'partial','operation_id':'old-archive','moves':[{'src':str(f.source),'dst':str(duplicate)}]})
            rows=f.call(route='/v1/staging')['rows'];assert (rows[0]['origin'] is None)==(origin=='unknown')
            p=preview(f,rows=rows);assert p['files'][0]['vids']==[],p['files']
            result=apply(f,p);assert result['completed']==1 and not duplicate.exists() and f.source.read_bytes()==m.PAYLOAD
            with sqlite3.connect(f.base/'faces.db') as db:assert db.execute('SELECT path FROM videos WHERE vid=?',(m.VID,)).fetchone()==(str(f.source),);assert db.execute('SELECT thumb FROM faces WHERE vid=?',(m.VID,)).fetchone()==('keep-thumb',)
            with sqlite3.connect(f.ledger) as db:assert db.execute('SELECT path,disposal FROM asset WHERE id=1').fetchone()==(f.win(f.source),None);assert db.execute('SELECT count(*) FROM asset_entity').fetchone()==(2,)
            row=json.loads((f.base/'archive-history.json').read_text())[0];assert row['dst']==str(f.source) and row['staged_state']=='deleted' and row.get('file_state')!='deleted'
            print('PASS canonical hardlink/asset/thumbnail/stable index preserved',origin)
        finally:f.stop()
    f=Fixture()
    try:
        dst=stage(f);old=str(uuid.uuid4());journal={'state':'running','plan':{'action':'delete','rows':[{'path':str(dst)}]}}
        m.write(f.base/'staging-operations'/old/'operation.json',journal)
        response=f.call(route='/v1/staging?id='+old);assert response['legacy'] and response['status']=='running'
        r={'id':str(uuid.uuid4()),'action':'clear-staging','person':'','files':[]};f.preview(r,503)
        journal['state']='finished';journal['result']={'status':'complete'};m.write(f.base/'staging-operations'/old/'operation.json',journal)
        f.preview({**r,'id':old},409)
        unknown={**r,'id':str(uuid.uuid4())};f.call({'mode':'reconcile','request':unknown});f.preview(unknown,409)
        p=preview(f);changed={**p['request'],'files':[]};f.call({'mode':'preview','request':{**changed,'action':'clear-staging'}},status=409)
        assert dst.exists();print('PASS legacy pending guard/read-only receipt ownership and late request fence')
    finally:f.stop()
def real_store_failures():
    for target in ['jobs','journal']:
        f=Fixture()
        try:
            dst=stage(f);p=preview(f)
            dbpath=f.base/('processing/processing.db' if target=='jobs' else 'file-operations-rust/operations.sqlite3')
            with sqlite3.connect(dbpath) as db:
                if target=='jobs':db.executescript("CREATE TRIGGER fixture_failure BEFORE UPDATE ON tasks BEGIN SELECT RAISE(ABORT,'fixture local fail'); END")
                else:db.executescript("CREATE TRIGGER fixture_failure BEFORE INSERT ON steps WHEN NEW.phase LIKE 'unlink_done_%' BEGIN SELECT RAISE(ABORT,'fixture journal fail'); END")
            apply(f,p,503);assert not dst.exists();state=f.status(p);assert state['status']=='recovery_needed' and (f.base/'.review-write.lock').exists(),state
            with sqlite3.connect(dbpath) as db:db.execute('DROP TRIGGER fixture_failure')
            state=reconcile(f,p);assert state['status']=='complete' and state['committed']==1 and not dst.exists(),state
            print('PASS actual SQLite failure after unlink forward recovery, no fake byte rollback',target)
        finally:f.stop()
def downgrade():
    binary=os.environ.get('FILEOPS_OLD_BINARY')
    if not binary:return
    f=Fixture()
    try:
        dst=stage(f);p=preview(f);f.fault('delete_faces_committed','error');apply(f,p,503);(f.root/'rust-fault.json').unlink();f.stop()
        def state():
            with sqlite3.connect(f.base/'file-operations-rust/operations.sqlite3') as db:return db.execute('PRAGMA user_version').fetchone(),db.execute('SELECT id,status,phase,revision FROM operations ORDER BY id').fetchall()
        before=state();assert before[0]==(4,)
        gate=(f.base/'.review-write.lock').read_bytes();old=m.subprocess.run([binary],env=f.env,capture_output=True,timeout=10)
        assert old.returncode!=0 and b'schema_version' in old.stderr,(old.returncode,old.stderr)
        assert state()==before and (f.base/'.review-write.lock').read_bytes()==gate and not dst.exists()
        f.start();assert f.status(p)['status']=='complete';print('PASS actual schema3 writer downgrade refused without data/journal/gate mutation')
    finally:f.stop()

def disconnected_waiter():
    f=Fixture()
    try:
        dst=stage(f);p=preview(f);f.fault('delete_faces_committed','pause',milliseconds=600)
        body={'mode':'apply','request':p['request'],'version':p['version'],'confirmation':'永久删除'}
        req=m.urllib.request.Request(f.url+'/v1/person-files',data=json.dumps(body).encode(),headers={'Content-Type':'application/json','Authorization':'Bearer fixture-token'})
        try:m.urllib.request.urlopen(req,timeout=.03).close()
        except (TimeoutError,OSError):pass
        else:raise AssertionError('fixture waiter should disconnect before receipt')
        for _ in range(100):
            state=f.status(p)
            if state['status']=='complete':break
            time.sleep(.03)
        else:raise AssertionError(state)
        (f.root/'rust-fault.json').unlink()
        assert state['result']['completed']==1 and not dst.exists() and apply(f,p)==state['result']
        with sqlite3.connect(f.ledger) as db:assert db.execute('SELECT count(*) FROM _face_admin_person_ops WHERE id=?',(p['id']+':delete:0',)).fetchone()==(1,)
        print('PASS disconnected HTTP waiter completes once and replays original receipt')
    finally:f.stop()
if __name__=='__main__':
    success();clear_scope();safety();boundary_errors();crash();unknown_mutations();partial();duplicate_and_legacy();real_store_failures();downgrade();disconnected_waiter()
