"""#12 batches use the native worker, original previews and independent child receipts."""
import importlib.util,json,sqlite3,uuid,threading,time,concurrent.futures,signal,urllib.error,os
from pathlib import Path
spec=importlib.util.spec_from_file_location('merge',Path(__file__).with_name('http-rust-person-merge.py'))
m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
f=m.f
class BatchFixture(m.MergeFixture):
    def __init__(self):
        super().__init__()
        self.third=self.kept.with_name('丙');(self.third/'empty').mkdir(parents=True)
        (self.third/'original.mp4').write_bytes(b'third same-name video')
        self.empty=self.kept.with_name('空人物');self.empty.mkdir()
    def batch(self,action='merge',people=None,target='乙',status=200,id=None):
        request={'action':action,'people':people or ['丙','甲','乙']}
        if action=='merge':request['target']=target
        return self.call({'mode':'preview','id':id or str(uuid.uuid4()),'request':request},route='/v1/person-batch',status=status)
    def run(self,plan,status=200):return self.call({'mode':'apply','id':plan['id'],'version':plan['version']},route='/v1/person-batch',status=status)
    def progress(self,plan):return self.call(route='/v1/person-batch?id='+plan['id'])
    def sql(self,command):
        with sqlite3.connect(self.ledger) as db:return db.execute(command).fetchall()
    def pause(self,phase):
        self.fault(phase,'pause',milliseconds=3000)
        reached=self.root/'rust-fault-reached.json'
        if reached.exists():reached.unlink()
    def reached(self):
        for _ in range(500):
            if (self.root/'rust-fault-reached.json').exists():return
            time.sleep(.01)
        raise AssertionError(('checkpoint timeout',self.root))

def basic():
    x=BatchFixture()
    try:
        x.batch(people=['甲'],target='甲',status=400)
        x.batch(people=['甲','旧甲'],target='甲',status=400)
        x.batch(target='空人物',status=400)
        p=x.batch();assert len(p['plans'])==2
        x.call({'mode':'preview','id':p['id'],'request':dict(p['request'],people=['甲','乙'])},route='/v1/person-batch',status=409)
        for mode in ('preview','apply','reconcile'):
            x.call({'mode':mode,'request':p['plans'][0]['request'],'version':p['plans'][0]['version']},status=409)
        x.run(dict(p,version='wrong'),409)
        x.source.write_bytes(b'changed second source');x.run(p,409)
        assert x.third.exists() and x.progress(p)['status']=='preview'
        x.source.write_bytes(f.PAYLOAD)
        p=x.batch();x.pause('batch_before_child:1')
        with concurrent.futures.ThreadPoolExecutor() as pool:
            pending=pool.submit(x.run,p);x.reached();progress=x.progress(p)
            assert progress['completed']==['丙'] and progress['remaining']==['甲'] and progress['active_person']=='甲',progress
            result=pending.result()
        assert result['ok'] and [v['person'] for v in result['completed']]==['丙','甲'],result
        assert x.run(p)==result and x.progress(p)['result']==result
        assert not x.third.exists() and not x.source.parent.exists()
        assert len([p for p in x.kept.iterdir() if p.is_file() and p.name.startswith('original')])==3
        x.call({'mode':'preview','id':p['id'],'request':p['request']},route='/v1/person-batch')
        print('PASS selection/original whole-preview/child ownership/version binding/real progress/collisions/idempotence')
    finally:x.stop()

def partial():
    for action in ('merge','delete'):
        x=BatchFixture()
        try:
            people=['丙','甲','乙'] if action=='merge' else ['丙','甲','空人物']
            p=x.batch(action,people)
            trigger="CREATE TRIGGER fail_batch BEFORE DELETE ON entity WHEN old.id=1 BEGIN SELECT RAISE(ABORT,'batch second'); END" if action=='merge' else "CREATE TRIGGER fail_batch BEFORE UPDATE OF path ON asset BEGIN SELECT RAISE(ABORT,'batch second'); END"
            x.sql(trigger);result=x.run(p)
            assert not result['ok'] and [v['person'] for v in result['completed']]==['丙'] and result['remaining']==people[1:-1 if action=='merge' else None],result
            assert not x.third.exists() and x.source.exists() and not (x.base/'.review-write.lock').exists()
            assert x.run(p)==result
            x.sql('DROP TRIGGER fail_batch')
            next_people=result['remaining']+(['乙'] if action=='merge' else [])
            assert x.run(x.batch(action,next_people))['ok']
            if action=='delete':assert not x.empty.exists()
            print('PASS independent person compensation, retained first receipt, fresh remaining preview',action)
        finally:x.stop()

def later_changes():
    for kind in ('target_conflict','source_file','source_empty_dir','background'):
        x=BatchFixture()
        try:
            p=x.batch();x.pause('batch_before_child:1')
            with concurrent.futures.ThreadPoolExecutor() as pool:
                pending=pool.submit(x.run,p);x.reached()
                if kind=='target_conflict':
                    late=Path(p['plans'][1]['files'][0]['dst']);late.parent.mkdir(parents=True,exist_ok=True);late.write_bytes(b'late target')
                if kind=='source_file':(x.source.parent/'late.mp4').write_bytes(b'late source')
                if kind=='source_empty_dir':(x.source.parent/'late-empty').mkdir()
                if kind=='background':
                    stop=threading.Event()
                    def worker():
                        while not stop.is_set():
                            with sqlite3.connect(x.base/'processing/processing.db') as db:db.execute("UPDATE tasks SET updated_at=updated_at+1 WHERE vid='unrelated'")
                            time.sleep(.005)
                    thread=threading.Thread(target=worker);thread.start()
                try:result=pending.result()
                finally:
                    if kind=='background':stop.set();thread.join()
            assert result['ok']==(kind in ('target_conflict','background')),result
            if kind=='target_conflict':assert late.read_bytes()==b'late target'
            if not result['ok']:assert result['remaining']==['甲'] and len(result['completed'])==1 and x.source.exists()
            print('PASS later replan preserves source proof',kind)
        finally:x.stop()

def crashes():
    for phase,expected in [('batch_validated',[]),('batch_child_prepared:0',[]),('batch_child_returned:0',['丙']),('batch_child_recorded:0',['丙']),('batch_before_child:1',['丙']),('batch_child_prepared:1',['丙']),('batch_child_returned:1',['丙','甲'])]:
        x=BatchFixture()
        try:
            p=x.batch();x.pause(phase)
            with concurrent.futures.ThreadPoolExecutor() as pool:
                pending=pool.submit(x.run,p);x.reached();x.process.kill();x.process.wait()
                try:pending.result()
                except (OSError,Exception):pass
            (x.root/'rust-fault.json').unlink();x.start()
            progress=x.progress(p);result=x.run(p)
            assert progress['status']=='complete' and progress['completed']==expected,(phase,progress)
            assert [r['person'] for r in result['completed']]==expected and x.run(p)==result
            assert not (x.base/'.review-write.lock').exists(),(phase,x.root)
            if '丙' in expected:assert not x.third.exists()
            if '甲' not in expected:assert x.source.exists()
            print('PASS real SIGKILL parent/child receipt transition',phase)
        finally:x.stop()


def deferred_failures():
    for mode in ('mid_file','ambiguous_after','ambiguous_delayed'):
        x=BatchFixture()
        try:
            p=x.batch();other=x.batch('delete',['空人物']);x.pause('batch_before_child:1')
            with concurrent.futures.ThreadPoolExecutor() as pool:
                pending=pool.submit(x.run,p);x.reached()
                if mode=='mid_file':x.fault('source_removed:1','error')
                else:x.fault('mutation:destination_publish','mutation_error_after' if mode=='ambiguous_after' else 'mutation_error_delayed',errno='ESTALE')
                result=pending.result()
            assert not result['ok'] and [r['person'] for r in result['completed']]==['丙'] and result['remaining']==['甲'],result
            assert not x.third.exists()
            child=p['plans'][1]
            with sqlite3.connect(x.base/'file-operations-rust/operations.sqlite3') as db:
                current=json.loads(db.execute('SELECT plan FROM operations WHERE id=?',(child['id'],)).fetchone()[0])
                first_receipt=db.execute('SELECT result FROM operations WHERE id=?',(p['plans'][0]['id'],)).fetchone()[0]
            if mode=='mid_file':
                assert x.source.exists() and not (x.base/'.review-write.lock').exists()
                assert x.status(child)['status']=='rolled_back'
            else:
                assert result['recovery_needed'] and x.status(child)['status']=='recovery_needed'
                x.run(other,503)
                file=current['files'][0]
                if mode=='ambiguous_delayed':os.link(file['src'],file['dst'])
                positions={q:Path(q).exists() for row in current['files'] for q in (row['src'],row['dst'])}
                (x.root/'rust-fault.json').unlink();x.stop();x.start()
                reconciled=x.call({'mode':'reconcile','id':p['id'],'version':p['version']},route='/v1/person-batch')
                assert reconciled['recovery_needed'] and [r['person'] for r in reconciled['completed']]==['丙']
                assert positions=={q:Path(q).exists() for q in positions} and (x.base/'.review-write.lock').exists()
                x.run(other,503)
            with sqlite3.connect(x.base/'file-operations-rust/operations.sqlite3') as db:
                assert db.execute('SELECT result FROM operations WHERE id=?',(p['plans'][0]['id'],)).fetchone()[0]==first_receipt
            print('PASS deferred second-person failure protects committed first/different IDs',mode)
        finally:x.stop()

def unknown_stores(stores=('faces','jobs','ledger','json','gate')):
    for store in stores:
        x=BatchFixture()
        try:
            p=x.batch();x.pause('batch_before_child:1')
            with concurrent.futures.ThreadPoolExecutor() as pool:
                pending=pool.submit(x.run,p);x.reached();x.pause('ledger_committed_unrecorded');x.reached()
                x.process.kill();x.process.wait()
                try:pending.result()
                except Exception:pass
            (x.root/'rust-fault.json').unlink();child=p['plans'][1]
            if store in ('json','gate'):
                path=x.base/('people-manual.json' if store=='json' else '.review-write.lock');before=path.read_bytes();path.write_text('[{"name":"foreign"}]' if store=='json' else '{broken')
            else:
                path={'faces':x.base/'faces.db','jobs':x.base/'processing/processing.db','ledger':x.ledger}[store]
                table='_face_admin_person_ops' if store=='ledger' else '_face_admin_rust_commits'
                with sqlite3.connect(path) as db:
                    before=db.execute('SELECT signature FROM '+table+' WHERE id=?',(child['id'],)).fetchone()[0]
                    db.execute('UPDATE '+table+' SET signature=? WHERE id=?',('foreign-signature',child['id']))
            x.start();progress=x.progress(p);result=x.run(p)
            assert progress['completed']==['丙'] and result['remaining']==['甲'] and result['recovery_needed'],(store,progress,result)
            assert (x.base/'.review-write.lock').exists() and not x.third.exists()
            if store in ('json','gate'):path.write_bytes(before)
            else:
                with sqlite3.connect(path) as db:db.execute('UPDATE '+table+' SET signature=? WHERE id=?',(before,child['id']))
            result=x.call({'mode':'reconcile','id':p['id'],'version':p['version']},route='/v1/person-batch')
            if store=='gate':
                # Rewriting a marker changes its identity proof. Even restored bytes
                # cannot authorize taking over a potentially foreign gate.
                assert result['recovery_needed'] and result['remaining']==['甲'] and (x.base/'.review-write.lock').exists()
                print('PASS corrupted/rewritten gate remains protected while parent status is readable')
                continue
            assert result['ok'] and len(result['completed'])==2 and not result.get('recovery_needed'),(store,result)
            assert not (x.base/'.review-write.lock').exists() and x.run(p)==result
            print('PASS per-store unknown protects committed previous person and explicit parent reconciliation',store)
        finally:x.stop()

if __name__=='__main__':basic();partial();later_changes();crashes();deferred_failures();unknown_stores()
