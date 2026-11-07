"""#8 native delete/restore/list acceptance. Isolated media + full Peach schema; no external executables."""
import importlib.util,json,sqlite3,uuid,time,signal,http.client,os
from pathlib import Path
spec=importlib.util.spec_from_file_location('fixture',Path(__file__).with_name('http-rust-file-operations.py'))
fmod=importlib.util.module_from_spec(spec);spec.loader.exec_module(fmod)
Fixture,PAYLOAD,VID=fmod.Fixture,fmod.PAYLOAD,fmod.VID

def delete(f,paths=None):
    r=f.request();r['action']='delete-files';r.pop('filename')
    if paths:r['files']=[f.request(path,vid=vid)['files'][0] for path,vid in paths]
    return f.preview(r)
def listing(f):return f.call(route='/v1/staging')
def restore(f,row=None,target=None):
    row=row or listing(f)['rows'][0]
    r={'id':str(uuid.uuid4()),'action':'restore-files','person':'','files':[{k:row[k] for k in ('vid','path','version')}]}
    if target:r.update(target_person=target,target_folder='恢复/子目录')
    return f.preview(r)
def staged(f,plan):
    dst=Path(plan['files'][0]['dst']);assert dst.read_bytes()==PAYLOAD and not f.source.exists()
    rows=listing(f);assert rows['files']==1 and rows['rows'][0]['origin']['original_path']==str(f.source)
    assert rows['rows'][0]['origin']['operation']==plan['id']
    assert rows['rows'][0]['vid']==VID
    with sqlite3.connect(f.base/'faces.db') as db:assert db.execute('SELECT path FROM videos WHERE vid=?',(VID,)).fetchone()[0]==str(dst)
    with sqlite3.connect(f.ledger) as db:
        assert db.execute('SELECT path FROM asset WHERE id=1').fetchone()[0]==f.win(dst)
        assert db.execute('SELECT count(*) FROM asset_entity').fetchone()[0]==2
    assert json.loads((f.base/'archive-history.json').read_text())[0]['file_state']=='staged'
    assert not json.loads((f.base/'report.json').read_text())['identities'][0]['vids']
    assert not json.loads((f.base/'integration-queue.json').read_text())
    return dst

def success():
    f=Fixture()
    try:
        fmod.write(f.base/'integration-queue.json',[{'vid':VID,'creator':'甲'}])
        fmod.write(f.base/'claims-detail.json',[{'vid':VID,'name':'甲'}])
        plan=delete(f);result=f.apply(plan);dst=staged(f,plan);assert f.apply(plan)==result
        assert f.status(plan)['committed']==1 and f.status(plan)['files'][0]['status']=='complete'
        before=(f.base/'external-profiles/manual-links.json').read_bytes()
        restored=restore(f);assert restored['files'][0]['dst']==str(f.source)
        f.apply(restored);assert f.source.read_bytes()==PAYLOAD and not dst.exists() and listing(f)['files']==0
        assert (f.base/'external-profiles/manual-links.json').read_bytes()==before
        assert f.status(plan)['result']==result
        with sqlite3.connect(f.ledger) as db:assert not db.execute('PRAGMA foreign_key_check').fetchall()
        print('PASS delete/list/origin/restore/bytes/stable ID/Peach/receipts')
    finally:f.stop()

def guards():
    for mutation in ['target','source','ledger','origin','symlink']:
        f=Fixture()
        try:
            f.apply(delete(f));plan=restore(f);src=Path(plan['files'][0]['src'])
            if mutation=='target':f.source.write_bytes(b'late foreign file')
            elif mutation=='source':src.write_bytes(b'changed source')
            elif mutation=='ledger':
                with sqlite3.connect(f.ledger) as db:db.execute('UPDATE asset SET name=? WHERE id=1',('late change',))
            elif mutation=='origin':
                value=json.loads((f.base/'staging-origins.json').read_text());value[str(src)]['original_path']=str(f.source.with_name('changed.mp4'));fmod.write(f.base/'staging-origins.json',value)
            elif mutation=='symlink':f.source.parent.rmdir();f.source.parent.symlink_to(f.root,target_is_directory=True)
            f.apply(plan,409);assert src.exists()
            if mutation=='target':assert f.source.read_bytes()==b'late foreign file'
            print('PASS restore guard',mutation)
        finally:f.stop()

def unknown_and_deleted():
    f=Fixture()
    try:
        stage=f.media/'pikpak/.face-cleanup-staging';stage.mkdir();unknown=stage/'unknown.mp4';unknown.write_bytes(PAYLOAD)
        row=listing(f)['rows'][0];assert row['origin'] is None and row['vid']==str(unknown)
        req={'id':str(uuid.uuid4()),'action':'restore-files','person':'','files':[{k:row[k] for k in ('vid','path','version')}]}
        f.preview(req,409)
        fmod.write(f.base/'people-deleted.json',{'复建':{'operation':'historical'},'other':{'operation':'keep'}})
        fmod.write(f.base/'watermarks/mappings.json',{'kept':{'name':'other'}})
        fmod.write(f.base/'external-profiles/manual-links.json',{'links':[]})
        plan=restore(f,row,'复建');f.apply(plan)
        assert Path(plan['files'][0]['dst']).read_bytes()==PAYLOAD
        assert plan['files'][0]['vids'][0].startswith('file:')
        assert json.loads((f.base/'people-deleted.json').read_text())=={'other':{'operation':'keep'}}
        assert json.loads((f.base/'external-profiles/manual-links.json').read_text())=={'links':[]}
        assert json.loads((f.base/'watermarks/mappings.json').read_text())=={'kept':{'name':'other'}}
        print('PASS unknown origin requires explicit target, missing directories, deleted marker, no withdrawn decisions restored')
    finally:f.stop()

def rollback():
    for restore_op in [False,True]:
        for phase in ['source_removed','faces_committed','jobs_committed','json_replaced','ledger_committed']:
            f=Fixture()
            try:
                if restore_op:f.apply(delete(f));plan=restore(f)
                else:plan=delete(f)
                before={p.relative_to(f.base):p.read_bytes() for p in f.base.rglob('*.json') if 'file-operation-routing' not in str(p)}
                f.fault(phase,'error');f.apply(plan,503)
                state=f.status(plan)
                if phase=='ledger_committed':
                    assert state['status']=='recovery_needed' and Path(plan['files'][0]['dst']).read_bytes()==PAYLOAD
                    (f.root/'rust-fault.json').unlink()
                    reconciled=f.call({'mode':'reconcile','request':plan['request'],'version':plan['version']})
                    assert reconciled['status']=='complete',reconciled
                    assert not (f.base/'.review-write.lock').exists()
                    print('PASS committed result remains moved until reconciliation',restore_op)
                    continue
                assert state['status']=='rolled_back',(phase,state)
                assert Path(plan['files'][0]['src']).read_bytes()==PAYLOAD and not Path(plan['files'][0]['dst']).exists()
                for path,value in before.items():assert (f.base/path).read_bytes()==value,(phase,path)
                with sqlite3.connect(f.ledger) as db:assert db.execute('SELECT path FROM asset WHERE id=1').fetchone()[0]==f.win(Path(plan['files'][0]['src']))
                print('PASS compensation',restore_op,phase)
            finally:f.stop()

def legacy():
    f=Fixture()
    try:
        op=str(uuid.uuid4());dst=f.media/'pikpak/.face-cleanup-staging/person-deletions'/op/'甲/original.mp4';dst.parent.mkdir(parents=True);f.source.rename(dst)
        receipt={'status':'complete','plan':{'action':'delete-person','person':'甲','files':[{'src':str(f.source),'dst':str(dst),'vids':[VID]}]},'result':{'ok':True}}
        file=f.base/'person-operations'/op/'operation.json';fmod.write(file,receipt);before=file.read_bytes()
        fmod.write(f.base/'archive-history.json',[{'vid':VID,'dst':str(dst),'win_dst':f.win(dst),'file_state':'staged'}]);fmod.write(f.base/'video-path-overrides.json',{VID:str(dst)})
        with sqlite3.connect(f.base/'faces.db') as db:db.execute('UPDATE videos SET path=? WHERE vid=?',(str(dst),VID))
        with sqlite3.connect(f.ledger) as db:db.execute('UPDATE asset SET path=? WHERE id=1',(f.win(dst),))
        row=listing(f)['rows'][0];assert row['origin']['source']=='legacy-receipt'
        f.apply(restore(f,row));assert f.source.read_bytes()==PAYLOAD and file.read_bytes()==before
        print('PASS legacy person-delete receipt origin and untouched old audit')
    finally:f.stop()

def crashes():
    for restore_op in [False,True]:
        for phase in ['destination_published:0','source_removed:0','faces_committed','ledger_committed_unrecorded']:
            f=Fixture()
            try:
                if restore_op:f.apply(delete(f));plan=restore(f)
                else:plan=delete(f)
                f.fault(phase,'pause',milliseconds=10000)
                client=http.client.HTTPConnection('127.0.0.1',f.port,timeout=15)
                client.request('POST','/v1/person-files',json.dumps({'mode':'apply','request':plan['request'],'version':plan['version']}),{'Content-Type':'application/json','Authorization':'Bearer fixture-token'})
                reached=f.root/'rust-fault-reached.json'
                for _ in range(1000):
                    if reached.exists():break
                    time.sleep(.01)
                else:raise AssertionError(('checkpoint missing',phase))
                assert f.status(plan)['committed']==0
                f.process.send_signal(signal.SIGKILL);f.process.wait(timeout=5);client.close()
                (f.root/'rust-fault.json').unlink();f.stop();f.start()
                state=f.status(plan);assert state['status'] in ('complete','rolled_back'),state
                current=Path(plan['files'][0]['dst'] if state['status']=='complete' else plan['files'][0]['src'])
                assert current.read_bytes()==PAYLOAD
                with sqlite3.connect(f.ledger) as db:assert db.execute('SELECT path FROM asset WHERE id=1').fetchone()[0]==f.win(current)
                with sqlite3.connect(f.base/'faces.db') as db:assert db.execute('SELECT path FROM videos WHERE vid=?',(VID,)).fetchone()[0]==str(current)
                assert not (f.base/'.review-write.lock').exists()
                print('PASS SIGKILL restart',restore_op,phase,state['status'])
            finally:f.stop()

def nested_and_native_receipt():
    f=Fixture()
    try:
        nested=f.source.parent/'nested'/f.source.name;nested.parent.mkdir();nested.write_bytes(b'separate same name')
        fmod.write(f.base/'integration-queue.json',[])
        with sqlite3.connect(f.base/'faces.db') as db:db.execute('INSERT INTO faces VALUES(?,?,?)',(VID,1,'a b.jpg'))
        fmod.write(f.base/'report.json',{'identities':[{'name':'甲','rep_thumb':'/thumb?p=a%20b.jpg','vids':[{'vid':VID},{'vid':'keep'}]}]})
        plan=delete(f,[(f.source,VID),(nested,str(nested))]);f.apply(plan)
        assert len({item['dst'] for item in plan['files']})==2
        assert json.loads((f.base/'report.json').read_text())['identities'][0]['rep_thumb'] is None
        assert [Path(item['dst']).read_bytes() for item in plan['files']]==[PAYLOAD,b'separate same name']
        # Prove native receipts can reconstruct origins for later delete-person domains too.
        (f.base/'staging-origins.json').unlink()
        row=next(row for row in listing(f)['rows'] if row['vid']==VID);assert row['origin']['source']=='native-receipt'
        nested.parent.rmdir();f.source.parent.rmdir()
        restored=restore(f,row);f.apply(restored);assert f.source.read_bytes()==PAYLOAD
        print('PASS same basename preserved, selected thumbnail invalidated, native receipt origin, missing original parent recreated')
    finally:f.stop()

def database_failures():
    for kind in ['faces','jobs','ledger']:
        f=Fixture()
        try:
            f.apply(delete(f));plan=restore(f)
            path,table={'faces':(f.base/'faces.db','videos'),'jobs':(f.base/'processing/processing.db','tasks'),'ledger':(f.ledger,'asset')}[kind]
            with sqlite3.connect(path) as db:db.executescript(f"CREATE TRIGGER fixture_reject BEFORE UPDATE OF path ON {table} BEGIN SELECT RAISE(ABORT,'staging actual SQL failure'); END;")
            f.apply(plan,503);assert f.status(plan)['status']=='rolled_back'
            assert Path(plan['files'][0]['src']).read_bytes()==PAYLOAD and not Path(plan['files'][0]['dst']).exists()
            with sqlite3.connect(f.ledger) as db:assert db.execute('SELECT path FROM asset WHERE id=1').fetchone()[0]==f.win(Path(plan['files'][0]['src']))
            assert not (f.base/'.review-write.lock').exists()
            print('PASS actual SQLite rejection compensates restore',kind)
        finally:f.stop()

def special_files():
    f=Fixture()
    try:
        stage=f.media/'pikpak/.face-cleanup-staging';stage.mkdir();fifo=stage/'pipe.mp4';os.mkfifo(fifo)
        assert listing(f)['files']==0
        request={'id':str(uuid.uuid4()),'action':'restore-files','person':'','target_person':'甲','files':[{'vid':str(fifo),'path':str(fifo),'version':'0'*64}]}
        f.preview(request,409)
        # The shared writer still serves a real operation immediately after rejection.
        assert f.apply(delete(f))['changed']==1
        assert fifo.exists()
        print('PASS FIFO excluded/rejected without blocking shared writer')
    finally:f.stop()

if __name__=='__main__':
    success();guards();unknown_and_deleted();rollback();legacy();crashes();nested_and_native_receipt();database_failures();special_files()
