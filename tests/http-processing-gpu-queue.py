"""Synthetic HTTP GPU queue -> actual VM claim -> real native receipt, never production.

Uses the native fixture's --binary argument. No model packages or media are read.
"""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import sqlite3
import sys
import threading
from unittest.mock import patch

import numpy as np

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'maintenance'))
from processing_faces import FACE_VERSION, save_npz
from processing_gpu import Detector
from processing_publication import PublicationError, native, publish_result, recover_publications
from processing_store import connect, reconcile, recover
from processing_worker import process_one
from task_progress import stage_progress
from test_gpu_inference import Broker, Client, FakeDetector, HASH, Resources, Server, TOKEN, png
from services.gpu_inference.protocol import ProtocolError

spec = importlib.util.spec_from_file_location('native_fixture', Path(__file__).with_name('http-rust-file-operations.py'))
f = importlib.util.module_from_spec(spec)
spec.loader.exec_module(f)
x = f.Fixture()
model, resources = FakeDetector(), Resources()
resources.busy = True
broker = Broker(model, resources)
server = Server(('127.0.0.1', 0), broker, TOKEN)
thread = threading.Thread(target=server.serve_forever, kwargs={'poll_interval': .01}, daemon=True)
thread.start()
recover_resources = threading.Timer(.15, lambda: setattr(resources, 'busy', False))
try:
    source = x.media / 'pikpak/My Telegram/marked-gpu-queue.mp4'
    source.parent.mkdir(parents=True, exist_ok=True)
    source.write_bytes(b'marked synthetic GPU queue worker fixture')
    vid = 'gpu-queue-worker-fixture'
    with sqlite3.connect(x.ledger) as db:
        db.execute('INSERT INTO asset(location,path,name) VALUES(?,?,?)', ('fixture', x.win(source), source.name))
    with sqlite3.connect(x.base / 'processing/processing.db') as db:
        db.execute('DROP TABLE tasks')
    con = connect(x.base)
    with con:
        reconcile(con, [dict(vid=vid, path=str(source), name=source.name, size=source.stat().st_size,
                             mtime_ns=str(source.stat().st_mtime_ns))], 'gpu-queue-batch', False)
    recover(con)
    sends = []
    original_call = Client._call

    def lose_gpu_response(client, path, body=None, timeout=None):
        value = original_call(client, path, body, timeout)
        if path == '/v1/detect':
            sends.append(body)
            if len(sends) == 1:
                raise ProtocolError('transport_unavailable', 503, True)
        return value

    def runner(task, base, root):
        if task['stage'] == 'text':
            return {**{k: task[k] for k in ('vid', 'path', 'size', 'mtime_ns')}, 'status': 'done',
                    'pipeline': 'synthetic-ocr-v1', 'artifact_attempt': task['run_id'], 'candidates': [],
                    'sampled_frames': 8, 'execution': {'backend': 'cpu', 'stage': 'ocr'}}
        detector = Detector(task, progress=stage_progress(task, base))
        recover_resources.start()
        for index in range(8):
            assert detector.detect(np.zeros((8, 8, 3), np.uint8), png(), index, index) == ()
        assert detector.timings[0]['queue_ms'] >= 100
        cache = base / 'processing/attempts' / task['run_id'] / 'features.npz'
        stat = source.stat()
        # Recognition is an explicit fixture; only transport/queue/ownership is tested.
        vectors = np.zeros((6, 512), np.float32)
        vectors[:, 0] = 1
        save_npz(cache, embeddings=vectors, thumbs=np.asarray([], dtype=str), timestamps=np.asarray([]),
                 version=FACE_VERSION, path=str(source), size=stat.st_size, mtime_ns=str(stat.st_mtime_ns),
                 source_dev=stat.st_dev, source_ino=stat.st_ino)
        return {'matches': [], 'face_count': 6, 'thumbs': [], 'sampled_frames': 8,
                'reference_version': 'synthetic-reference', 'reference_people': 0, 'version': FACE_VERSION,
                'unknown_group': None, 'feature_cache': str(cache),
                'feature_cache_sha256': hashlib.sha256(cache.read_bytes()).hexdigest(),
                'execution': detector.execution}

    def lose_native_response(task, base, data, stage):
        def send_then_disconnect(command, body):
            native(command, body)
            raise PublicationError('lost synthetic committed response', operation_id=body['operation_id'], uncertain=True)
        with patch('processing_publication.native', side_effect=send_then_disconnect):
            return publish_result(task, base, data, stage)

    with patch.dict(os.environ, {'FILE_OPERATIONS_RUST_URL': x.url, 'FILE_OPERATIONS_RUST_TOKEN': 'fixture-token',
                                  'PROCESSING_GPU_URL': f'http://127.0.0.1:{server.server_address[1]}',
                                  'PROCESSING_GPU_TOKEN': TOKEN, 'PROCESSING_GPU_MODEL_SHA256': HASH}), \
            patch.object(Client, '_call', lose_gpu_response):
        assert process_one(x.base, x.media, runner)
        assert process_one(x.base, x.media, runner, publisher=lose_native_response)
        assert model.calls == 8
        assert len(sends) == 9 and sends[0] == sends[1]
        recover_publications(x.base)
        recover_publications(x.base)
        task = dict(con.execute('SELECT * FROM tasks WHERE vid=?', (vid,)).fetchone())
        assert task['stage'] == 'review' and task['face_attempts'] == 1, task
        assert con.execute('SELECT completed FROM batch_items').fetchone()[0] == 1
        assert con.execute("SELECT COUNT(*) FROM events WHERE kind='ready'").fetchone()[0] == 1
        assert con.execute("SELECT COUNT(*) FROM processing_runs WHERE status='done'").fetchone()[0] == 2
        assert con.execute('SELECT COUNT(*) FROM _face_admin_processing_results').fetchone()[0] == 2
        assert json.loads(task['data'])['face']['execution']['backend'] == 'gpu-detection'
        print('PASS synthetic busy -> 8 HTTP frames / 9 sends / 8 computations -> actual VM attempt 1 -> native acceptance; GPU and native lost-response replay keep completion at 1')
    con.close()
finally:
    recover_resources.cancel()
    server.shutdown()
    server.server_close()
    broker.close()
    thread.join(1)
    x.stop()
