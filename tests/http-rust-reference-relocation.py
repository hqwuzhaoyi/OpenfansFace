"""Marked synthetic-only reproduction, no application or production changes."""
import hashlib, importlib.util, json, os, sqlite3, sys
from pathlib import Path
from unittest.mock import patch
import numpy as np
repo=Path(__file__).resolve().parents[1]
spec=importlib.util.spec_from_file_location('fixture',repo/'tests/http-rust-file-operations.py')
f=importlib.util.module_from_spec(spec);spec.loader.exec_module(f)
sys.path.insert(0,str(repo/'maintenance'))
from processing_faces import FACE_VERSION,cache_path,save_npz,build_references,reference_features
x=f.Fixture()
queue_handle=sqlite3.connect(x.base/'processing/processing.db')
queue_handle.execute('SELECT COUNT(*) FROM tasks').fetchone()
try:
    cache=cache_path(x.base,f.VID)
    vector=np.zeros((6,512),np.float32);vector[:,0]=1
    stat=x.source.stat()
    source_identity={key:str(getattr(stat,'st_'+key)) for key in ('dev','ino','size','mtime_ns')}
    save_npz(cache,version=FACE_VERSION,path=str(x.source),size=stat.st_size,mtime_ns=str(stat.st_mtime_ns),embeddings=vector,thumbs=np.asarray([],dtype=str),timestamps=np.asarray([]))
    payload_before=cache.read_bytes();cache_sha=hashlib.sha256(payload_before).hexdigest()
    media_sha=hashlib.sha256(x.source.read_bytes()).hexdigest()
    with patch.dict(os.environ,{'FILE_OPERATIONS_RUST_URL':x.url,'FILE_OPERATIONS_RUST_TOKEN':'fixture-token'}):
        before=build_references(x.base,x.media)
        plan=x.preview()
        if os.environ.get('PROCESSING_COPY_RELOCATION')=='1':f.write(x.root/'rust-fault.json',{'force_copy':True})
        result=x.apply(plan);receipt=x.status(plan)
        assert result['ok'] and receipt['status']=='complete'
        after=build_references(x.base,x.media)
    current=x.target.stat()
    with sqlite3.connect(x.base/'faces.db') as db:
        faces_count=db.execute('SELECT count(*) FROM faces').fetchone()[0]
        source_row=db.execute('SELECT vid,path FROM videos WHERE vid=?',(f.VID,)).fetchone()
    with np.load(cache,allow_pickle=False) as saved:
        metadata={key:str(saved[key]) for key in ('version','path','size','mtime_ns')}
    with sqlite3.connect(x.base/'file-operations-rust/operations.sqlite3') as db:
        tables=[r[0] for r in db.execute("SELECT name FROM sqlite_master WHERE type='table'")]
    evidence={'fixture_root':str(x.root),'fixture_marker':(x.root/'.fileops-fixture').read_text(),'source_before':str(x.source),'source_after':str(x.target),'source_identity_before':source_identity,'source_identity_after':{key:str(getattr(current,'st_'+key)) for key in ('dev','ino','size','mtime_ns')},'source_bytes_same':hashlib.sha256(x.target.read_bytes()).hexdigest()==media_sha,'legacy_cache':str(cache),'legacy_cache_sha256_before':cache_sha,'legacy_cache_sha256_after':hashlib.sha256(cache.read_bytes()).hexdigest(),'legacy_cache_metadata':metadata,'face_rows':faces_count,'stable_video_row':source_row,'native_operation_id':plan['id'],'native_status':receipt['status'],'native_result':result,'references_before':before,'references_after':after,'native_store_tables':tables,'native_plan':plan,'native_receipt':receipt}
    assert before['people']==after['people']==1 and before['templates']==after['templates']==1
    assert evidence['legacy_cache_sha256_before']==evidence['legacy_cache_sha256_after']
    assert evidence['source_bytes_same']
    # The final file cannot be replaced even with equal bytes, size and mtime.
    with patch.dict(os.environ,{'FILE_OPERATIONS_RUST_URL':x.url,'FILE_OPERATIONS_RUST_TOKEN':'fixture-token'}):
        assert len(reference_features(x.base,f.VID,x.target))==6
        # A matching path in a preview/uncertain/rolled-back record is never proof.
        with sqlite3.connect(x.base/'file-operations-rust/operations.sqlite3') as journal:
            for status in ['not_started','running','recovery_needed','rolled_back','failed']:
                journal.execute('UPDATE operations SET status=? WHERE id=?',(status,plan['id']));journal.commit()
                assert not reference_features(x.base,f.VID,x.target),status
            journal.execute("UPDATE operations SET status='complete' WHERE id=?",[plan['id']]);journal.commit()
        assert len(reference_features(x.base,f.VID,x.target))==6
        # Model bytes remain guarded after relocation, without rewriting the NPZ.
        save_npz(cache,version='wrong-model',path=str(x.source),size=stat.st_size,mtime_ns=str(stat.st_mtime_ns),embeddings=vector)
        assert not reference_features(x.base,f.VID,x.target)
        cache.write_bytes(payload_before)
        assert len(reference_features(x.base,f.VID,x.target))==6
        old=x.target.stat();replacement=x.target.with_suffix('.replacement');replacement.write_bytes(x.target.read_bytes());os.utime(replacement,ns=(old.st_atime_ns,old.st_mtime_ns));os.replace(replacement,x.target)
        assert not reference_features(x.base,f.VID,x.target)
    print(json.dumps({key:evidence[key] for key in ('fixture_root','source_bytes_same','legacy_cache_sha256_before','legacy_cache_sha256_after','face_rows','native_operation_id','native_status','references_before','references_after')},ensure_ascii=False,indent=2))
finally:queue_handle.close();x.stop()
