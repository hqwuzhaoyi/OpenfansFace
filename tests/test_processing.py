import json
from pathlib import Path
import sqlite3
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'maintenance'))
from processing_fixture import finish
from processing_store import connect, set_setting, reconcile, claim, fail, recover
from processing_logic import candidates_from_text, can_skip_face, folder_hints, resolve_candidates
from processing_worker import discover, process_one as worker_process_one


def process_one(base, root, *args):
    def publish(task, base, data, next_stage):
        con = connect(base)
        try: finish(con, task, data, next_stage)
        finally: con.close()
    return worker_process_one(base, root, *args, preparer=lambda *a: None, publisher=publish, reference_loader=lambda *a: None)


class ProcessingTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.base = Path(self.temp.name) / 'base'; self.base.mkdir()
        self.root = Path(self.temp.name) / 'media'; self.root.mkdir()
        self.file = self.root / 'one.mp4'; self.file.write_bytes(b'fixture-video')
        self.item = dict(vid='one', path=str(self.file), name=self.file.name, size=self.file.stat().st_size, mtime_ns=str(self.file.stat().st_mtime_ns))
        (self.base / '.processing-fixture').write_text('isolated processing fixture')
        self.db = connect(self.base)
        import numpy as np
        np.savez(self.base / 'processing/references.npz', input_version='a'*64)
        with self.db:
            reconcile(self.db, [self.item], 'initial', False)

    def tearDown(self):
        self.db.close(); self.temp.cleanup()

    def test_folder_or_unconfirmed_handle_cannot_skip_faces(self):
        hints = folder_hints('/media/Alice/movie.mp4', ['Alice'])
        self.assertEqual(hints[0]['name'], 'Alice')
        self.assertFalse(can_skip_face(hints))
        mark = dict(candidates=[dict(key='a', text='@Alice', token='@Alice', platform='unknown', kind='account')])
        proposed = candidates_from_text(mark, {}, hints)
        self.assertFalse(can_skip_face(proposed))
        confirmed = candidates_from_text(mark, {'a': dict(kind='person', creator='Alice')}, hints)
        self.assertTrue(can_skip_face(confirmed))
        self.assertFalse(can_skip_face(confirmed, ['Bob']))
        self.assertFalse(can_skip_face(confirmed + [dict(name='Bob', source='folder')]))
        self.assertEqual(candidates_from_text(mark, {'a': dict(kind='source')}, []), [])

    def test_multiple_candidates_are_one_conflicted_video(self):
        result = resolve_candidates([dict(name='Alice', source='watermark'), dict(name='Alice', source='face', score=.6), dict(name='Bob', source='folder')])
        self.assertEqual(len(result['candidates']), 2)
        self.assertEqual(result['kind'], 'suggested'); self.assertEqual(result['suggested'], 'Alice')
        conflict = resolve_candidates([dict(name='Alice', source='watermark'), dict(name='Bob', source='watermark')])
        self.assertEqual(conflict['kind'], 'conflict'); self.assertIsNone(conflict['suggested'])
        self.assertEqual(resolve_candidates([])['kind'], 'none')

    def test_new_video_priority_and_pause(self):
        second = self.root / 'two.mp4'; second.write_bytes(b'two')
        item = dict(self.item, vid='two', path=str(second), name=second.name, size=3, mtime_ns=str(second.stat().st_mtime_ns))
        with self.db:
            reconcile(self.db, [self.item, item], 'new', True)
            set_setting(self.db, 'paused', True)
        self.assertIsNone(claim(self.db))
        with self.db: set_setting(self.db, 'paused', False)
        self.assertEqual(claim(self.db)['vid'], 'two')

    def test_failure_has_two_retries_not_an_infinite_loop(self):
        for attempt in range(1, 4):
            task = claim(self.db)
            self.assertEqual(task['text_attempts'], attempt)
            fail(self.db, task, 'decoder unavailable')
            state = self.db.execute('SELECT stage FROM tasks').fetchone()[0]
            self.assertEqual(state, 'retry' if attempt < 3 else 'error')
            with self.db: self.db.execute('UPDATE tasks SET next_retry=0')
        self.assertIsNone(claim(self.db))

    def test_media_outage_does_not_claim_or_exhaust_tasks(self):
        with patch('processing_worker.os.scandir', side_effect=OSError(5, 'Input/output error')):
            for _ in range(4):
                self.assertFalse(process_one(self.base, self.root))
        row = self.db.execute('SELECT * FROM tasks').fetchone()
        self.assertEqual(row['stage'], 'queued')
        self.assertEqual(row['text_attempts'], 0)
        self.assertEqual(self.db.execute("SELECT count(*) FROM events WHERE kind='media_unavailable'").fetchone()[0], 1)
        self.assertTrue(process_one(self.base, self.root, lambda *args: dict(candidates=[])))
        self.assertEqual(self.db.execute('SELECT stage FROM tasks').fetchone()[0], 'face_queued')
        self.assertEqual(self.db.execute("SELECT count(*) FROM events WHERE kind='media_recovered'").fetchone()[0], 1)

    def test_media_outage_during_stage_returns_attempt(self):
        def runner(*args):
            self.root.rename(self.root.with_name('offline'))
            raise OSError(5, 'Input/output error')
        process_one(self.base, self.root, runner)
        row = self.db.execute('SELECT * FROM tasks').fetchone()
        self.assertEqual(row['stage'], 'retry')
        self.assertEqual(row['text_attempts'], 0)

    def test_file_failure_with_healthy_mount_still_consumes_attempt(self):
        def runner(*args):
            raise OSError(5, 'Input/output error')
        process_one(self.base, self.root, runner)
        row = self.db.execute('SELECT * FROM tasks').fetchone()
        self.assertEqual(row['stage'], 'retry')
        self.assertEqual(row['text_attempts'], 1)

    def test_restart_does_not_bypass_retry_limit(self):
        with self.db: self.db.execute("UPDATE tasks SET stage='face',retry_stage='face',face_attempts=3")
        recover(self.db)
        self.assertEqual(self.db.execute('SELECT stage FROM tasks').fetchone()[0], 'error')

    def test_file_replacement_discards_stale_result_and_preserves_note(self):
        old = claim(self.db)
        with self.db:
            self.db.execute("UPDATE tasks SET note='secondary person',deferred=1")
            reconcile(self.db, [dict(self.item, size=999, mtime_ns='999')], 'changed', True)
        finish(self.db, old, dict(ready=True), 'review')
        row = self.db.execute('SELECT * FROM tasks').fetchone()
        self.assertEqual(row['stage'], 'queued'); self.assertEqual(row['data'], '{}')
        self.assertEqual(row['note'], 'secondary person'); self.assertEqual(row['deferred'], 0)

    def test_complete_text_then_no_face_is_review_ready_not_failure(self):
        calls = []
        def runner(task, base, root):
            calls.append(task['stage'])
            if task['stage'] == 'text': return dict(candidates=[], sampled_frames=8)
            return dict(matches=[], face_count=0, thumbs=[], reference_people=0)
        self.assertTrue(process_one(self.base, self.root, runner))
        self.assertEqual(self.db.execute('SELECT stage FROM tasks').fetchone()[0], 'face_queued')
        self.assertTrue(process_one(self.base, self.root, runner))
        row = self.db.execute('SELECT * FROM tasks').fetchone()
        self.assertEqual(calls, ['text', 'face']); self.assertEqual(row['stage'], 'review')
        self.assertEqual(json.loads(row['data'])['kind'], 'none')

    def test_stable_video_id_can_change_path_without_leaving_a_stale_task(self):
        renamed = self.root / 'renamed.mp4'
        self.file.rename(renamed)
        item = dict(self.item, path=str(renamed), name=renamed.name)
        with self.db: reconcile(self.db, [item], 'rename', True)
        row = self.db.execute('SELECT * FROM tasks WHERE vid=?', ('one',)).fetchone()
        self.assertEqual(row['path'], str(renamed))
        self.assertEqual(row['stage'], 'queued')

    def test_confirmed_watermark_skips_face_stage(self):
        (self.base / 'watermarks').mkdir()
        (self.base / 'watermarks/mappings.json').write_text(json.dumps({'mark': dict(kind='person', creator='Alice')}))
        def runner(task, base, root):
            self.assertEqual(task['stage'], 'text')
            return dict(candidates=[dict(key='mark', text='@alice', token='alice', platform='unknown', kind='account')])
        process_one(self.base, self.root, runner)
        row = self.db.execute('SELECT * FROM tasks').fetchone()
        self.assertEqual(row['stage'], 'review'); self.assertTrue(json.loads(row['data'])['face_skipped'])

    def test_discovery_registers_short_videos_but_excludes_disposed_and_organized(self):
        faces = sqlite3.connect(self.base / 'faces.db')
        faces.executescript('CREATE TABLE videos(vid TEXT PRIMARY KEY,path TEXT,name TEXT,size INT,duration REAL,zone TEXT,collection TEXT,status TEXT DEFAULT "pending"); CREATE TABLE faces(vid TEXT,score REAL,emb BLOB,thumb TEXT);')
        faces.close()
        disposed = self.root / 'disposed.mp4'; disposed.write_bytes(b'x')
        organized = self.root / 'pikpak/creators/A/known.mp4'; organized.parent.mkdir(parents=True); organized.write_bytes(b'x')
        assets = [dict(path=str(self.file), disposal=None, duration=3), dict(path=str(disposed), disposal='trash', duration=100), dict(path=str(organized), disposal=None, duration=100)]
        with patch('processing_faces.native',return_value={'version':'a'*64}):
            discover(self.base, self.root, '', assets)
        faces = sqlite3.connect(self.base / 'faces.db')
        self.assertEqual(faces.execute('SELECT COUNT(*) FROM videos').fetchone()[0], 0)
        self.assertTrue(self.db.execute('SELECT COUNT(*) FROM tasks').fetchone()[0] >= 1)
        faces.close()

    def test_truncated_ocr_watermark_prefixes_merge_into_longest_handle(self):
        def runner(task, base, root):
            if task['stage'] == 'text':
                return dict(candidates=[
                    dict(key='wm1', text='@xxapple_e2', token='xxapple_e2', platform='unknown', kind='account'),
                    dict(key='wm2', text='@xxapple_e', token='xxapple_e', platform='unknown', kind='account'),
                    dict(key='wm3', text='@xxapp', token='xxapp', platform='unknown', kind='account'),
                ])
            return dict(matches=[], face_count=0, thumbs=[], reference_people=0)
        self.assertTrue(process_one(self.base, self.root, runner))
        self.assertTrue(process_one(self.base, self.root, runner))
        data = json.loads(self.db.execute('SELECT data FROM tasks').fetchone()[0])
        self.assertEqual(data['kind'], 'suggested')
        self.assertEqual(data['suggested'], 'xxapple_e2')
        self.assertEqual([c['name'] for c in data['candidates']], ['xxapple_e2'])
        self.assertEqual(len(data['candidates'][0]['evidence']), 3)


if __name__ == '__main__': unittest.main()
