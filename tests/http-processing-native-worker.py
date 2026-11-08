"""Mock model stages, real VM claims/native HTTP commits/SQLite and marked file fixtures."""
import importlib.util,json,os,sqlite3,sys,time,hashlib,uuid
import numpy as np
from pathlib import Path
from unittest.mock import patch
spec=importlib.util.spec_from_file_location('native_fixture',Path(__file__).with_name('http-rust-file-operations.py'))
f=importlib.util.module_from_spec(spec);spec.loader.exec_module(f)
sys.path.insert(0,str(Path(__file__).resolve().parents[1]/'maintenance'))
from processing_store import connect,reconcile,recover
from processing_worker import process_one
from processing_faces import save_npz,build_references,FACE_VERSION
from processing_publication import publish_result,recover_publications,PublicationError,native

x=f.Fixture()
try:
    file=x.media/'pikpak/My Telegram/mock-worker.mp4';file.parent.mkdir(parents=True,exist_ok=True);file.write_bytes(b'marked mock-model video')
    vid='worker-joint'
    # Newly discovered analysis task intentionally has no legacy faces.db video row.
    with sqlite3.connect(x.ledger) as db:db.execute('INSERT INTO asset(location,path,name) VALUES(?,?,?)',('fixture',x.win(file),file.name))
    with sqlite3.connect(x.base/'processing/processing.db') as db:db.execute('DROP TABLE tasks')
    con=connect(x.base)
    with con:reconcile(con,[dict(vid=vid,path=str(file),name=file.name,size=file.stat().st_size,mtime_ns=str(file.stat().st_mtime_ns))],'joint-batch',False)
    recover(con)
    calls=[]
    def runner(task,base,root):
        calls.append(task['stage'])
        if task['stage']=='text':
            return {**{k:task[k] for k in ('vid','path','size','mtime_ns')},'status':'done','pipeline':'mock-ocr-v1','artifact_attempt':task['run_id'],'candidates':[],'sampled_frames':8,'execution':{'backend':'cpu','stage':'ocr'}}
        cache=base/'processing/attempts'/task['run_id']/'features.npz';stat=file.stat()
        vectors=np.zeros((6,512),np.float32);vectors[:,0]=1
        save_npz(cache,embeddings=vectors,thumbs=np.asarray([],dtype=str),timestamps=np.asarray([]),version=FACE_VERSION,path=str(file),size=stat.st_size,mtime_ns=str(stat.st_mtime_ns),source_dev=stat.st_dev,source_ino=stat.st_ino)
        return {'matches':[],'face_count':6,'thumbs':[],'sampled_frames':8,'reference_version':'mock-reference','reference_people':0,'version':FACE_VERSION,'unknown_group':None,'feature_cache':str(cache),'feature_cache_sha256':hashlib.sha256(cache.read_bytes()).hexdigest(),'execution':{'backend':'cpu','recognition_provider':'CPUExecutionProvider'}}
    with patch.dict(os.environ,{'FILE_OPERATIONS_RUST_URL':x.url,'FILE_OPERATIONS_RUST_TOKEN':'fixture-token'}):
        assert process_one(x.base,x.media,runner)
        task=dict(con.execute('SELECT * FROM tasks WHERE vid=?',(vid,)).fetchone());assert task['stage']=='face_queued',task
        assert json.loads((x.base/'watermarks/results.json').read_text())[vid]['status']=='done'
        # Lose response AFTER native commits. The exact journal body resolves on restart.
        def lost_after_commit(task,base,data,stage):
            def send_then_disconnect(command,body):
                native(command,body)
                raise PublicationError('lost committed response',operation_id=body['operation_id'],uncertain=True)
            with patch('processing_publication.native',side_effect=send_then_disconnect):
                return publish_result(task,base,data,stage)
        assert process_one(x.base,x.media,runner,publisher=lost_after_commit)
        task=dict(con.execute('SELECT * FROM tasks WHERE vid=?',(vid,)).fetchone());assert task['stage']=='review',task
        assert con.execute('SELECT completed FROM batch_items').fetchone()[0]==1
        events=con.execute("SELECT COUNT(*) FROM events WHERE kind='ready'").fetchone()[0]
        assert events==1
        assert not (x.base/'processing/attempts'/task['run_id']/'receipt.json').exists()
        # A legitimate later human decision must survive the committed receipt replay.
        with con:con.execute("UPDATE tasks SET manual=1,note='keep human decision' WHERE vid=?",(vid,))
        recover_publications(x.base);recover_publications(x.base)
        assert con.execute("SELECT COUNT(*) FROM events WHERE kind='ready'").fetchone()[0]==events
        assert con.execute('SELECT note FROM tasks').fetchone()[0]=='keep human decision'
        assert con.execute("SELECT COUNT(*) FROM processing_runs WHERE status='done'").fetchone()[0]==2
        assert con.execute('SELECT COUNT(*) FROM _face_admin_processing_results').fetchone()[0]==2
        assert calls==['text','face']
        print('PASS real VM claim -> native text/face publish, OCR selected merge, unknown transport committed replay, human preservation and no double count')
        with con:con.execute('UPDATE tasks SET manual=0 WHERE vid=?',(vid,))
        page=x.call({'command':'review-page','body':{}},'/v1/archive')
        original=next(row for row in page['rows'] if row['vid']==vid)
        confirmation_scope={'review_session':page['session'],'review_tokens':{vid:original['review_token']}}
        if os.environ.get('PROCESSING_REVIEW_JOINT')=='1':
            with con:con.execute("UPDATE tasks SET stage='face_queued' WHERE vid=?",(vid,))
            def refreshed(task,base,root):
                face=runner(task,base,root)
                face['matches']=[{'name':'甲','source':'face','score':.8,'support':6,'detail':'synthetic fixture candidate'}]
                return face
            assert process_one(x.base,x.media,refreshed)
            assert json.loads(con.execute('SELECT data FROM tasks WHERE vid=?',(vid,)).fetchone()[0])['kind']=='suggested'
            status=x.call({'command':'review-status','body':{'session':page['session']}},'/v1/archive')
            assert status['has_updates']
            same=x.call({'command':'review-page','body':{'session':page['session']}},'/v1/archive')
            assert [row['vid'] for row in same['rows']]==[row['vid'] for row in page['rows']]
            assert next(row for row in same['rows'] if row['vid']==vid)['data']==original['data']
            assert same['group']==page['group']
            assert con.execute('SELECT completed FROM batch_items').fetchone()[0]==1
            print('PASS #18 viewed evidence/group retained after actual native refreshed suggestion; old delivered token still admits explicit decision; completion remains1')
        confirmed=x.call({'command':'queue-confirm','body':{'creator':'甲','vids':[vid],'source':'review','fingerprints':{vid:{'path':str(file),'size':file.stat().st_size,'mtime_ns':str(file.stat().st_mtime_ns)}},'review_group':{'key':'none','deferred':False},**confirmation_scope}},'/v1/archive')
        assert build_references(x.base,x.media)['people']==1
        preview=x.call({'command':'archive-preview','body':{}},'/v1/archive')
        if os.environ.get('PROCESSING_COPY_RELOCATION')=='1':f.write(x.root/'rust-fault.json',{'force_copy':True})
        result=x.call({'command':'archive-apply','body':{'vids':[vid],'version':preview['version'],'operation_id':str(uuid.uuid4())}},'/v1/archive')
        destination=Path(json.loads((x.base/'video-path-overrides.json').read_text())[vid])
        assert destination!=file and destination.is_file(),result
        assert build_references(x.base,x.media)['people']==1
        # A second native move must bridge through the now-absent intermediate path.
        request=x.request(destination,filename='renamed-reference.mp4',vid=vid)
        plan=x.call({'mode':'preview','request':request})
        x.call({'mode':'apply','request':request,'version':plan['version']})
        destination=destination.with_name('renamed-reference.mp4')
        assert build_references(x.base,x.media)['people']==1
        stored=json.loads(con.execute('SELECT data FROM tasks WHERE vid=?',[vid]).fetchone()[0])
        cache=Path(stored['face']['feature_cache']);payload=cache.read_bytes();current=destination.stat()
        with np.load(cache,allow_pickle=False) as saved:
            origin={'path':str(saved['path']),'size':str(int(saved['size'])),'mtime_ns':str(saved['mtime_ns']),'dev':str(int(saved['source_dev'])),'ino':str(int(saved['source_ino']))}
        binding={'vid':vid,'source':str(destination),'cache':str(cache),'cache_sha256':hashlib.sha256(payload).hexdigest(),'origin':origin,'current_identity':{k:str(getattr(current,'st_'+k)) for k in ('dev','ino','size','mtime_ns')}}
        proof=native('reference-relocation',binding)
        unrelated=x.preview(x.request(filename='unrelated-renamed.mp4'));x.apply(unrelated)
        assert native('reference-relocation',binding)['version']==proof['version']
        accepted_caches={Path(json.loads(row[0])['data']['face']['feature_cache']) for row in con.execute("SELECT result FROM _face_admin_processing_results WHERE json_extract(result,'$.vid')=? AND json_type(result,'$.data.face.feature_cache')='text'",[vid])}
        originals={path:path.read_bytes() for path in accepted_caches}
        for path,raw in originals.items():path.write_bytes(raw+b'changed-cache')
        assert build_references(x.base,x.media)['people']==0
        for path,raw in originals.items():path.write_bytes(raw)
        assert build_references(x.base,x.media)['people']==1
        old=destination.stat();replacement=destination.with_suffix('.replacement');replacement.write_bytes(destination.read_bytes());os.utime(replacement,ns=(old.st_atime_ns,old.st_mtime_ns));os.replace(replacement,destination)
        assert build_references(x.base,x.media)['people']==0
        print('PASS cache-only confirmed reference after native archive move; same-size/mtime inode replacement rejected')
    con.close()
finally:x.stop()
