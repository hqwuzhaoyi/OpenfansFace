"""Tests for resident pure-calculation model runtime, write boundary enforcement, and daemon auto-restart."""
import hashlib
import json
import os
from pathlib import Path
import signal
import sqlite3
import tempfile
import unittest
from unittest.mock import patch

from processing_compute import (
    RUNTIME,
    ResidentComputeClient,
    compute_write_guard,
)
from processing_faces import MODEL_HASHES, extract_features
from processing_worker import text_stage


class _FakeOcrEngine:
    init_count = 0

    def __init__(self):
        type(self).init_count += 1


class ResidentComputeTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='marked-compute-test-')
        self.root = Path(self.temp.name)
        self.base = self.root / 'base'
        self.media = self.root / 'media'
        (self.base / 'processing/attempts').mkdir(parents=True)
        (self.base / 'processing/tmp').mkdir(parents=True)
        (self.base / 'jobs').mkdir(parents=True)
        (self.base / 'watermarks').mkdir(parents=True)
        (self.media / 'pikpak/unsorted').mkdir(parents=True)
        RUNTIME.reset()
        _FakeOcrEngine.init_count = 0

    def tearDown(self):
        RUNTIME.reset()
        self.temp.cleanup()

    def _make_video(self, name='clip.mp4', content=b'video-content-bytes'):
        video = self.media / 'pikpak/unsorted' / name
        video.write_bytes(content)
        stat = video.stat()
        return {
            'vid': f'vid-{name}',
            'path': str(video),
            'name': name,
            'size': stat.st_size,
            'mtime_ns': str(stat.st_mtime_ns),
            'stage': 'text',
            'run_id': '11111111-1111-4111-8111-111111111111',
            'batch_id': 'batch-1',
            'data': '{}',
        }

    def test_ocr_engine_stays_resident_across_multiple_tasks(self):
        def fake_scan(task, root, directory, engine, attempt_id, progress=None):
            self.assertIsInstance(engine, _FakeOcrEngine)
            return {'candidates': [], 'frames_sampled': 4}

        with patch('processing_worker.PaddleEngine', _FakeOcrEngine), patch(
            'processing_worker.scan_video', side_effect=fake_scan
        ):
            for idx in range(3):
                task = self._make_video(f'v{idx}.mp4', f'bytes-{idx}'.encode())
                task['run_id'] = f'11111111-1111-4111-8111-11111111111{idx}'
                out = text_stage(task, self.base, self.media)
                self.assertEqual(out['execution']['resident_load_count'], 1)
                self.assertEqual(out['execution']['resident_tasks_run'], idx + 1)

        snap = RUNTIME.health_snapshot()
        self.assertTrue(snap['ocr_loaded'])
        self.assertEqual(snap['ocr_load_count'], 1)
        self.assertEqual(snap['ocr_tasks_run'], 3)
        self.assertEqual(_FakeOcrEngine.init_count, 1)

    def test_face_runtime_verifies_model_hashes_and_reuses_loaded_instance(self):
        model_home = self.root / 'insightface/models/buffalo_l'
        model_home.mkdir(parents=True)
        det_bytes = b'fake-det-model'
        rec_bytes = b'fake-rec-model'
        (model_home / 'det_10g.onnx').write_bytes(det_bytes)
        (model_home / 'w600k_r50.onnx').write_bytes(rec_bytes)
        expected = {
            'det_10g.onnx': hashlib.sha256(det_bytes).hexdigest(),
            'w600k_r50.onnx': hashlib.sha256(rec_bytes).hexdigest(),
        }

        class FakeFaceAnalysis:
            loads = 0

            def __init__(self, **kwargs):
                type(self).loads += 1

            def prepare(self, **kwargs):
                pass

        with patch.dict('sys.modules', {
            'onnxruntime': type('ort', (), {'SessionOptions': lambda: type('opt', (), {'intra_op_num_threads': 1, 'inter_op_num_threads': 1})()}),
            'insightface': type('ins', (), {}),
            'insightface.app': type('app', (), {'FaceAnalysis': FakeFaceAnalysis}),
        }):
            r1 = RUNTIME.get_face_runtime('cpu', model_home, expected)
            r2 = RUNTIME.get_face_runtime('cpu', model_home, expected)
            self.assertIs(r1, r2)
            self.assertEqual(FakeFaceAnalysis.loads, 1)
            self.assertEqual(RUNTIME.face_load_count, 1)
            self.assertEqual(RUNTIME.face_tasks_run, 2)

            # Corrupting model hash must fail closed
            (model_home / 'w600k_r50.onnx').write_bytes(b'tampered')
            with self.assertRaises(ValueError):
                RUNTIME.get_face_runtime('cpu', model_home, expected)

    def test_compute_write_guard_allows_only_attempt_tmp_and_blocks_business_truth(self):
        run_id = '22222222-2222-4222-8222-222222222222'
        ro_db = self.base / 'faces.db'
        con = sqlite3.connect(ro_db)
        con.execute('CREATE TABLE videos (vid TEXT PRIMARY KEY, path TEXT)')
        con.commit()
        con.close()

        with compute_write_guard(self.base, self.media, run_id=run_id):
            # Allowed: attempt dir, processing/tmp, jobs
            attempt_file = self.base / 'processing/attempts' / run_id / 'features.npz'
            attempt_file.parent.mkdir(parents=True, exist_ok=True)
            attempt_file.write_bytes(b'ok-attempt')
            tmp_file = self.base / 'processing/tmp/staged.npz'
            tmp_file.write_bytes(b'ok-tmp')
            job_file = self.base / 'jobs' / f'{run_id}.json'
            job_file.write_text('{}')

            # Allowed: read-only SQLite URI
            ro = sqlite3.connect(f'file:{ro_db}?mode=ro', uri=True)
            ro.execute('SELECT * FROM videos').fetchall()
            ro.close()

            # Forbidden: writing to other attempts, processing.db, references.npz, business JSON, media root
            for forbidden in (
                self.base / 'processing/attempts/99999999-9999-4999-8999-999999999999/hack.txt',
                self.base / 'processing/references.npz',
                self.base / 'processing/processing.db',
                self.base / 'report.json',
                self.base / 'integration-queue.json',
                self.media / 'pikpak/unsorted/tamper.mp4',
            ):
                with self.assertRaises(PermissionError, msg=f'expected PermissionError for {forbidden}'):
                    forbidden.parent.mkdir(parents=True, exist_ok=True)
                    forbidden.write_text('forbidden')

            # Forbidden: writable SQLite connection
            with self.assertRaises(PermissionError):
                sqlite3.connect(str(ro_db))

    def test_resident_compute_daemon_health_and_crash_auto_restart(self):
        client = ResidentComputeClient()
        try:
            snap1 = client.ensure_alive()
            self.assertTrue(snap1['ok'])
            pid1 = snap1['pid']
            self.assertEqual(client.start_count, 1)
            self.assertEqual(client.restart_count, 0)

            # Second ensure_alive reuses the same resident process
            snap2 = client.ensure_alive()
            self.assertEqual(snap2['pid'], pid1)
            self.assertEqual(client.start_count, 1)

            # Simulate unexpected crash of resident daemon process
            os.kill(pid1, signal.SIGKILL)
            if client._process is not None:
                client._process.wait(timeout=3)

            # Next ensure_alive detects dead daemon and automatically restarts it
            snap3 = client.ensure_alive()
            self.assertTrue(snap3['ok'])
            self.assertNotEqual(snap3['pid'], pid1)
            self.assertEqual(client.start_count, 2)
            self.assertGreaterEqual(client.restart_count, 1)
        finally:
            client.close()


if __name__ == '__main__':
    unittest.main()
