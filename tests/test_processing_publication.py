"""CPU/mock transport tests. Real CUDA performance is a separate release gate."""
import json
import os
from pathlib import Path
import sqlite3
import sys
import tempfile
import time
import unittest
import uuid
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'maintenance'))
from processing_store import connect, reconcile, claim, recover, fail
from processing_publication import publish_result, recover_publications, PublicationError
from processing_gpu import binding, Detector, inference_status
from services.gpu_inference.protocol import ProtocolError


class PublicationTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix='marked-worker19-')
        self.base = Path(self.tmp.name)
        self.con = connect(self.base)
        with self.con:
            reconcile(self.con, [{'vid': 'R:\\media\\private-name.mp4', 'path': '/media/private-name.mp4', 'name': 'private-name.mp4', 'size': 100, 'mtime_ns': '123'}], 'private-batch', False)
        recover(self.con)
        self.task = claim(self.con)
        self.task['_material'] = {'scope': {'vids': [self.task['vid']]}, 'version': 'a' * 64}

    def tearDown(self):
        self.con.close(); self.tmp.cleanup()

    def test_default_capacity_is_one_and_restart_fences_attempt(self):
        old = self.task['run_id']; generation = self.task['coordinator_generation']
        self.assertIsNone(claim(self.con))
        recover(self.con)
        new = claim(self.con)
        self.assertNotEqual(old, new['run_id'])
        self.assertNotEqual(generation, new['coordinator_generation'])
        self.assertGreater(new['lease_expires_at'], time.time())

    def test_uncertain_result_replays_exact_operation_after_restart(self):
        calls = []
        def transport(command, body):
            calls.append((command, body.copy()))
            if len(calls) == 1:
                raise PublicationError('connection lost after commit', operation_id=body['operation_id'], uncertain=True)
            return {'ok': True, 'id': body['operation_id']}
        with patch('processing_publication.native', side_effect=transport):
            with self.assertRaises(PublicationError):
                publish_result(self.task, self.base, {'ready': True}, 'review')
            recover_publications(self.base)
            recover_publications(self.base)
        self.assertEqual(len(calls), 2)
        self.assertEqual(calls[0], calls[1])
        self.assertEqual(len(list(self.base.glob('processing/attempts/*/receipt.json'))), 1)

    def test_changed_body_cannot_reuse_operation_id(self):
        with patch('processing_publication.native', return_value={'ok': True}):
            publish_result(self.task, self.base, {'ready': True}, 'review')
            with self.assertRaises(PublicationError):
                publish_result(self.task, self.base, {'ready': False}, 'face_queued')

    def test_late_result_rejection_is_retained_without_retrying_new_id(self):
        with patch('processing_publication.native', side_effect=PublicationError('expired', 409)):
            with self.assertRaises(PublicationError):
                publish_result(self.task, self.base, {'ready': True}, 'review')
            recover_publications(self.base)
        receipt = json.loads(next(self.base.glob('processing/attempts/*/receipt.json')).read_text())
        self.assertFalse(receipt['ok'])
        self.assertEqual(self.con.execute('SELECT completed FROM batch_items').fetchone()[0], 0)

    def test_late_failure_does_not_change_manual_takeover_or_reclaim_it(self):
        with self.con:
            self.con.execute("UPDATE tasks SET manual=1,stage='error',note='human' WHERE vid=?",(self.task['vid'],))
        fail(self.con,self.task,'late model error')
        row=self.con.execute('SELECT stage,manual,note FROM tasks').fetchone()
        self.assertEqual(tuple(row),('error',1,'human'))
        with self.con:self.con.execute("UPDATE tasks SET stage='retry'")
        self.assertIsNone(claim(self.con))

    def test_legacy_watermark_writer_is_disabled_but_attempt_computation_can_publish(self):
        from watermark_worker import publish,main
        target=self.base/'watermarks/results.json'
        with patch.dict(os.environ,{'FILE_OPERATIONS_BACKEND':'rust'}):
            with self.assertRaises(RuntimeError):publish(target,{'would':'bypass'})
            with self.assertRaises(SystemExit):main()
            local=self.base/'processing/attempts'/self.task['run_id']/'result.json'
            publish(local,{'local':'computation'})
            self.assertEqual(json.loads(local.read_text()),{'local':'computation'})
        self.assertFalse(target.exists())

    def test_native_read_outage_waits_without_spending_model_budget(self):
        from processing_worker import process_one
        source=self.base/'marked.mp4';source.write_bytes(b'fixture')
        recover(self.con)
        with self.con:
            self.con.execute('UPDATE tasks SET path=?,size=?,mtime_ns=?,text_attempts=0',(str(source),source.stat().st_size,str(source.stat().st_mtime_ns)))
        with patch('processing_publication.native',side_effect=PublicationError('native offline',503)):
            self.assertTrue(process_one(self.base,self.base,lambda *args:self.fail('model must not run')))
        row=self.con.execute('SELECT stage,text_attempts,next_retry FROM tasks').fetchone()
        self.assertEqual(row['stage'],'retry');self.assertEqual(row['text_attempts'],0)
        self.assertGreater(row['next_retry'],time.time()+20)

    def test_remote_binding_contains_no_media_paths_names_or_identities(self):
        from dataclasses import asdict
        encoded = json.dumps(asdict(binding(self.task)))
        self.assertNotIn('private', encoded)
        self.assertNotIn('/media', encoded)
        self.assertEqual(binding(self.task).attempt_id, self.task['run_id'])

    def test_network_oom_backpressure_are_explicit_no_cpu_fallback(self):
        for code in ('transport_unavailable', 'cuda_out_of_memory', 'queue_full'):
            with self.subTest(code=code), patch.dict(os.environ, {'PROCESSING_FACE_BACKEND': 'gpu-detection'}), patch('processing_gpu.client') as factory:
                factory.return_value.ready.side_effect = ProtocolError(code, 503, True)
                result = inference_status()
                self.assertFalse(result['available']); self.assertFalse(result['fallback'])
                self.assertEqual(result['backend'], 'gpu-detection')
                self.assertEqual(result['error'], code)

    def test_remote_frame_deadline_is_clipped_to_stage_and_lease_reserve(self):
        class Image:
            shape = (8, 8, 3)
        requests = []
        class Transport:
            def detect(self, request):
                requests.append(request)
                raise ProtocolError('synthetic-stop', 503)
        self.task['_stage_deadline_ms'] = int(time.time()*1000)+1234
        detector = Detector(self.task, Transport())
        with self.assertRaisesRegex(ProtocolError, 'synthetic-stop'):
            detector.detect(Image(), b'marked-synthetic', 0, 0)
        self.assertEqual(requests[0].deadline_ms, self.task['_stage_deadline_ms'])
        self.assertEqual(requests[0].binding.lease_expires_at_ms, int(self.task['lease_expires_at']*1000))
        self.task['_stage_deadline_ms'] += 900_000
        detector = Detector(self.task, Transport())
        self.assertEqual(detector.stage_deadline_ms, int(self.task['lease_expires_at']*1000)-60_000)

    def test_remote_failure_is_never_converted_to_no_faces(self):
        class Image:
            shape = (160, 160, 3)
        class Transport:
            def detect(self, request):
                raise ProtocolError('cuda_out_of_memory', 503, True)
        detector = Detector(self.task, Transport())
        with self.assertRaises(ProtocolError):
            detector.detect(Image(), b'encoded-fixture', 0, 0.5)
        self.assertIsNone(detector.execution)


if __name__ == '__main__':
    unittest.main()
