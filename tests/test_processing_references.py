"""Synthetic vectors only: preserve cache-only reference people across valid moves."""
import hashlib,json,sqlite3,sys,tempfile,unittest,uuid
from pathlib import Path
from unittest.mock import patch
import numpy as np
sys.path.insert(0,str(Path(__file__).resolve().parents[1]/'maintenance'))
from processing_store import connect
from processing_faces import FACE_VERSION,save_npz,cache_path,build_references,reference_features

class ReferenceTests(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory(prefix='marked-reference19-')
        self.base=Path(self.tmp.name)/'base';self.base.mkdir()
        self.root=Path(self.tmp.name)/'media';self.root.mkdir()
        self.file=self.root/'before.mp4';self.file.write_bytes(b'synthetic-media')
        self.vid='stable-fixture-video';self.run=str(uuid.uuid4())
        self.con=connect(self.base)
        self.con.execute('CREATE TABLE _face_admin_processing_results(id TEXT PRIMARY KEY,signature TEXT,result TEXT)')
        self.con.commit()
        with sqlite3.connect(self.base/'faces.db') as db:
            db.executescript('CREATE TABLE videos(vid TEXT PRIMARY KEY,path TEXT); CREATE TABLE faces(vid TEXT,score REAL,emb BLOB,thumb TEXT)')
            db.execute('INSERT INTO videos VALUES (?,?)',(self.vid,str(self.file)))
        (self.base/'integration-queue.json').write_text(json.dumps([{'vid':self.vid,'creator':'Cache Only Person'}]))
        self.vectors=np.zeros((6,512),np.float32);self.vectors[:,0]=1
        self.cache=self.base/'processing/attempts'/self.run/'features.npz'
        self.stat=self.file.stat()
        save_npz(self.cache,embeddings=self.vectors,thumbs=np.asarray([],dtype=str),timestamps=np.asarray([]),
                 version=FACE_VERSION,path=str(self.file),size=self.stat.st_size,mtime_ns=str(self.stat.st_mtime_ns),source_dev=self.stat.st_dev,source_ino=self.stat.st_ino)
        self.receipt={'ok':True,'accepted':True,'vid':self.vid,'data':{'face':{'version':FACE_VERSION,'feature_cache':str(self.cache),'feature_cache_sha256':hashlib.sha256(self.cache.read_bytes()).hexdigest(),
            'source_identity':{'dev':str(self.stat.st_dev),'ino':str(self.stat.st_ino)}}}}
        self.con.execute('INSERT INTO _face_admin_processing_results VALUES (?,?,?)',(self.run,'fixture',json.dumps(self.receipt)));self.con.commit()

    def tearDown(self):self.con.close();self.tmp.cleanup()

    def build(self):
        with patch('processing_faces.native',return_value={'version':'a'*64}):
            return build_references(self.base,self.root)

    def test_cache_only_reference_requires_native_proof_for_a_changed_path(self):
        self.assertEqual(self.build()['people'],1)
        archived=self.root/'pikpak/creators/Cache Only Person/after.mp4';archived.parent.mkdir(parents=True)
        self.file.rename(archived)
        with sqlite3.connect(self.base/'faces.db') as db:db.execute('UPDATE videos SET path=? WHERE vid=?',(str(archived),self.vid))
        (self.base/'integration-queue.json').write_text('[]')
        (self.base/'archive-history.json').write_text(json.dumps([{'vid':self.vid,'creator':'Cache Only Person','dst':str(archived)}]))
        # A manual path rewrite has no committed native receipt. Valid native
        # rename/archive/copy chains are covered by the actual HTTP fixtures.
        self.assertEqual(self.build()['people'],0)
        self.assertEqual(len(reference_features(self.base,self.vid,archived)),0)
        # Same size and preserved mtime still cannot turn a replaced inode into the old file.
        archived.unlink();archived.write_bytes(b'synthetic-media')
        import os
        os.utime(archived,ns=(self.stat.st_atime_ns,self.stat.st_mtime_ns))
        self.assertEqual(self.build()['people'],0)

    def test_tampered_or_unaccepted_cache_cannot_promote(self):
        self.cache.write_bytes(self.cache.read_bytes()+b'tamper')
        self.assertEqual(self.build()['people'],0)
        self.con.execute('DELETE FROM _face_admin_processing_results');self.con.commit()
        self.assertEqual(self.build()['people'],0)

    def test_legacy_cache_requires_exact_current_fingerprint_path_and_model(self):
        self.con.execute('DELETE FROM _face_admin_processing_results');self.con.commit()
        legacy=cache_path(self.base,self.vid)
        def save(**overrides):
            data=dict(embeddings=self.vectors,version=FACE_VERSION,path=str(self.file),size=self.stat.st_size,mtime_ns=str(self.stat.st_mtime_ns));data.update(overrides);save_npz(legacy,**data)
        save();self.assertEqual(self.build()['people'],1)
        for changed in ({'size':999},{'mtime_ns':'1'},{'path':'/old/path.mp4'},{'version':'old-model'}):
            with self.subTest(changed=changed):save(**changed);self.assertEqual(self.build()['people'],0)

if __name__=='__main__':unittest.main()
