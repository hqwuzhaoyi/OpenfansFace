import json
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import sys
import tempfile
import threading
import unittest
from unittest.mock import patch
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'maintenance'))
from processing_store import connect, reconcile, set_setting, claim, finish, fail, recover, release_task

from task_progress import stage_progress


class ConcurrencyTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.base = Path(self.temp.name)
        self.db = connect(self.base)
        items = [dict(vid=str(i),path=str(self.base/f'{i}.mp4'),name=f'{i}.mp4',size=10,mtime_ns='1') for i in range(8)]
        with self.db:
            reconcile(self.db,items,'batch-one',False)
            set_setting(self.db,'concurrency',2)

    def tearDown(self):
        self.db.close();self.temp.cleanup()

    def test_atomic_claim_caps_parallelism_and_never_duplicates_a_video(self):
        barrier=threading.Barrier(8)
        def take():
            con=connect(self.base)
            try:barrier.wait();return claim(con, capacity=2)
            finally:con.close()
        with ThreadPoolExecutor(max_workers=8) as pool: results=list(pool.map(lambda _:take(),range(8)))
        taken=[r for r in results if r]
        self.assertEqual(len(taken),2)
        self.assertEqual(len({r['vid'] for r in taken}),2)
        self.assertEqual(self.db.execute("SELECT COUNT(*) FROM processing_runs WHERE status='running'").fetchone()[0],2)

    def test_lowering_limit_drains_without_cancelling_work(self):
        first,second=claim(self.db, capacity=2),claim(self.db, capacity=2)
        with self.db:set_setting(self.db,'concurrency',1)
        self.assertIsNone(claim(self.db, capacity=2))
        finish(self.db,first,dict(text_complete=True,ready=True,face_skipped=True),'review')
        self.assertIsNone(claim(self.db, capacity=2))
        finish(self.db,second,dict(text_complete=True,ready=True,face_skipped=True),'review')
        self.assertIsNotNone(claim(self.db, capacity=2))
        self.assertIsNone(claim(self.db, capacity=2))

    def test_stale_lease_cannot_complete_or_fail_a_new_attempt(self):
        old=claim(self.db, capacity=2)
        with self.db:self.db.execute("UPDATE tasks SET stage='queued',run_id='',priority=10 WHERE vid=?",(old['vid'],))
        new=claim(self.db, capacity=2)
        self.assertEqual(new['vid'],old['vid']);self.assertNotEqual(new['run_id'],old['run_id'])
        finish(self.db,old,dict(ready=True),'review')
        fail(self.db,old,'late failure')
        current=self.db.execute('SELECT * FROM tasks WHERE vid=?',(new['vid'],)).fetchone()
        self.assertEqual(current['stage'],'text');self.assertEqual(current['run_id'],new['run_id'])
        self.assertEqual(self.db.execute('SELECT status FROM processing_runs WHERE id=?',(old['run_id'],)).fetchone()[0],'superseded')

    def test_completed_cohort_survives_archive_and_reference_refresh(self):
        task=claim(self.db, capacity=2)
        finish(self.db,task,dict(text_complete=True,face_skipped=True,ready=True),'review')
        original=self.db.execute('SELECT * FROM batch_items WHERE vid=?',(task['vid'],)).fetchone()
        with self.db:self.db.execute("UPDATE tasks SET stage='face_queued',priority=20 WHERE vid=?",(task['vid'],))
        refresh=claim(self.db, capacity=2)
        finish(self.db,refresh,dict(text_complete=True,face_complete=True,ready=True),'review')
        with self.db:reconcile(self.db,[],'later',True)
        after=self.db.execute('SELECT * FROM batch_items WHERE vid=?',(task['vid'],)).fetchone()
        self.assertEqual(after['completed'],1);self.assertEqual(after['face_skipped'],1)
        self.assertEqual(after['completed_at'],original['completed_at'])
        self.assertEqual(self.db.execute('SELECT COUNT(*) FROM batch_items').fetchone()[0],8)

    def test_retry_does_not_count_as_completed(self):
        task=claim(self.db, capacity=2);fail(self.db,task,'read error')
        member=self.db.execute('SELECT * FROM batch_items WHERE vid=?',(task['vid'],)).fetchone()
        self.assertEqual(member['completed'],0)
        self.assertEqual(self.db.execute('SELECT status FROM processing_runs WHERE id=?',(task['run_id'],)).fetchone()[0],'retry')

    def test_mount_wait_returns_budget_and_closes_its_run(self):
        task=claim(self.db, capacity=2);release_task(self.db,task,'mount unavailable')
        row=self.db.execute('SELECT * FROM tasks WHERE vid=?',(task['vid'],)).fetchone()
        self.assertEqual(row['text_attempts'],0);self.assertEqual(row['stage'],'retry')
        self.assertEqual(self.db.execute('SELECT status FROM processing_runs WHERE id=?',(task['run_id'],)).fetchone()[0],'waiting')

    def test_recovery_closes_all_interrupted_runs(self):
        claim(self.db, capacity=2);claim(self.db, capacity=2);recover(self.db)
        self.assertEqual(self.db.execute("SELECT COUNT(*) FROM processing_runs WHERE status='interrupted'").fetchone()[0],2)
        self.assertEqual(self.db.execute("SELECT COUNT(*) FROM tasks WHERE stage IN ('text','face')").fetchone()[0],0)

    def test_parallel_unknown_faces_defer_group_writes_until_native_acceptance(self):
        import numpy as np
        from processing_faces import analyze
        np.savez(self.base/'processing/references.npz',names=np.asarray([],dtype=str),embeddings=np.empty((0,512),dtype=np.float32),version='test')
        with self.db:self.db.execute("UPDATE tasks SET stage='face_queued',priority=10,data=? WHERE vid IN ('0','1')",(json.dumps(dict(kind='none',text_complete=True,ready=False)),))
        tasks=[claim(self.db, capacity=2),claim(self.db, capacity=2)]
        vector=np.zeros(512,dtype=np.float32);vector[0]=1
        features=dict(embeddings=np.stack([vector,vector]),thumbs=np.asarray(['a','b']),timestamps=np.asarray([0.,1.]))
        with patch('processing_faces.extract_features',return_value=(features,False,8)):
            with ThreadPoolExecutor(max_workers=2) as pool:results=list(pool.map(lambda t:analyze(t,self.base,self.base),tasks))
        self.assertIsNotNone(results[0]['unknown_group'])
        self.assertEqual(len(results[0]['unknown_group_input']['embedding']),512)
        self.assertEqual(self.db.execute('SELECT COUNT(*) FROM face_groups').fetchone()[0],0)

    def test_refresh_does_not_change_reviewable_face_evidence_mid_stage(self):
        import numpy as np
        from processing_faces import analyze,FACE_VERSION
        np.savez(self.base/'processing/references.npz',names=np.asarray([],dtype=str),embeddings=np.empty((0,512),dtype=np.float32),version='test')
        vector=np.zeros(512,dtype=np.float32);vector[0]=1
        original=dict(kind='none',text_complete=True,ready=True,face={'unknown_group':'old'})
        with self.db:
            self.db.execute("UPDATE tasks SET stage='face_queued',priority=10,data=? WHERE vid='0'",(json.dumps(original),))
            self.db.execute('INSERT INTO face_groups VALUES (?,?,?)',('old',FACE_VERSION,(-vector).tobytes()))
        task=claim(self.db, capacity=2)
        features=dict(embeddings=np.stack([vector,vector]),thumbs=np.asarray(['a','b']),timestamps=np.asarray([0.,1.]))
        with patch('processing_faces.extract_features',return_value=(features,False,8)):result=analyze(task,self.base,self.base)
        stored=json.loads(self.db.execute("SELECT data FROM tasks WHERE vid='0'").fetchone()[0])
        self.assertEqual(stored,original)
        self.assertNotEqual(result['unknown_group'],'old')

    def test_parallel_stage_progress_does_not_overwrite_same_kind(self):
        a=stage_progress(dict(stage='text',vid='a',run_id='run-a'),self.base,attach=False)
        b=stage_progress(dict(stage='text',vid='b',run_id='run-b'),self.base,attach=False)
        a_child=stage_progress(dict(stage='text',vid='a',run_id='run-a'),self.base)
        b.update(done=3,total=8);a_child.update(done=7,total=8);a.finish()
        self.assertNotEqual(a.file,b.file)
        self.assertEqual(json.loads(a.file.read_text())['done'],7)
        self.assertEqual(json.loads(b.file.read_text())['done'],3)
        self.assertEqual(json.loads(b.file.read_text())['status'],'running')

if __name__=='__main__':unittest.main()
