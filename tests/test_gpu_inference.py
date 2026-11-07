"""Isolated contract/HTTP tests. No GPU, media, business DB or external network."""
import copy
import hashlib
import http.client
import pathlib
import socket
import struct
import sys
import threading
import tempfile
import shutil
import time
import unittest
import uuid
import zlib
from dataclasses import replace
from unittest.mock import patch

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
from services.gpu_inference.broker import Broker
from services.gpu_inference.client import Client
from services.gpu_inference.protocol import (
    Binding, Fingerprint, MAX_BODY_BYTES, ProtocolError, Request, Result, canonical,
    decode_json, now_ms,
)
from services.gpu_inference.server import Server

HASH = 'a' * 64
TOKEN = 'isolated-test-token-' + 'x' * 32


def png(width=8, height=8):
    def chunk(kind, payload):
        return struct.pack('!I', len(payload)) + kind + payload + struct.pack('!I', zlib.crc32(kind + payload))
    return b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('!IIBBBBB', width, height, 8, 2, 0, 0, 0)) + chunk(b'IDAT', zlib.compress(b''.join(b'\x00' + bytes(width*3) for _ in range(height)))) + chunk(b'IEND', b'')


def request(**changes):
    image = png()
    return replace(Request(str(uuid.uuid4()), Binding('task-fixture', 'video-fixture', 'batch-fixture',
                   'attempt-fixture', 'lease-fixture', 'generation-fixture', now_ms()+60_000,
                   Fingerprint(123, '12345678', HASH), HASH, 'pipeline-fixture'), HASH,
                   'scrfd-640-bgr8-det0.5-nms0.4-v1', now_ms()+30_000,
                   0, 1200, hashlib.sha256(image).hexdigest(), 8, 8, image), **changes)


class Resources:
    busy = False
    def sample(self):
        return {'available': True, 'busy': self.busy, 'memory_free_mib': 800}


class FakeDetector:
    model_sha256 = HASH
    preprocess_version = 'scrfd-640-bgr8-det0.5-nms0.4-v1'
    execution = {'stage': 'detection', 'model_sha256': HASH, 'preprocess_version': preprocess_version,
                 'runtime_version': 'fixture', 'provider_proof': 'startup_profile', 'providers': ['CUDAExecutionProvider', 'CPUExecutionProvider'],
                 'cuda_kernel_events': 1, 'cpu_kernel_events': 1}
    def __init__(self):
        self.calls = 0
        self.entered = threading.Event()
        self.release = threading.Event()
        self.release.set()
        self.error = None
        self.faces = []
    def infer(self, req):
        self.calls += 1
        self.entered.set()
        if not self.release.wait(3):
            raise RuntimeError('fixture wait timed out')
        if self.error:
            raise self.error
        return self.faces


class ContractTests(unittest.TestCase):
    def test_roundtrip_and_hash_include_material_attempt_frame(self):
        first = request()
        self.assertEqual(first, Request.parse(first.wire()))
        metadata_only=replace(first,binding=replace(first.binding,fingerprint=replace(first.binding.fingerprint,content_sha256=None)))
        self.assertEqual(metadata_only,Request.parse(metadata_only.wire()))
        for changed in [replace(first, frame_index=1), replace(first, timestamp_ms=0),
                        replace(first, binding=replace(first.binding, attempt_id='attempt-2')),
                        replace(first, binding=replace(first.binding, material_version='b'*64))]:
            self.assertNotEqual(first.request_hash, changed.request_hash)

    def test_portrait_retains_original_raster_with_bounded_height(self):
        image=png(1080,1920)
        portrait=request(width=1080,height=1920,image=image,image_sha256=hashlib.sha256(image).hexdigest())
        self.assertEqual(Request.parse(portrait.wire()),portrait)
        tall=png(1280,4096)
        bound=request(width=1280,height=4096,image=tall,image_sha256=hashlib.sha256(tall).hexdigest())
        self.assertEqual(Request.parse(bound.wire()).height,4096)
        for width,height in [(1281,4096),(1280,4097)]:
            with self.assertRaises(ProtocolError):Request.parse(replace(bound,width=width,height=height).wire())

    def test_strict_json_and_bad_fields(self):
        for data in [b'{"x":1,"x":2}', b'{"x":NaN}', b'[]', b'\xff']:
            with self.assertRaises(ProtocolError): decode_json(data)
        valid = request().wire()
        variants = []
        for field, value in [('stage', 'recognition'), ('schema_version', True), ('model_sha256', 'bad'), ('request_id', '/private/path'), ('deadline_ms', now_ms()-1)]:
            wrong = copy.deepcopy(valid); wrong[field] = value; variants.append(wrong)
        wrong = copy.deepcopy(valid); wrong['frame']['data_base64'] = 'AA=='; variants.append(wrong)
        wrong = copy.deepcopy(valid); wrong['frame']['width'] = 100000; variants.append(wrong)
        wrong = copy.deepcopy(valid); wrong['binding']['fingerprint']['path'] = '/media'; variants.append(wrong)
        for wrong in variants:
            with self.subTest(wrong=wrong.keys()), self.assertRaises(ProtocolError): Request.parse(wrong)

    def test_client_disallows_credential_redirect_proxy_and_unsafe_origin(self):
        for origin in ['file:///etc/passwd', 'http://public.example', 'http://u:p@localhost', 'https://host/a']:
            with self.assertRaises(ValueError): Client(origin, TOKEN)
        Client('http://127.0.0.1:1', TOKEN)
        Client('https://private.example', TOKEN)


class BrokerTests(unittest.TestCase):
    def setUp(self):
        self.model, self.resources = FakeDetector(), Resources()
        self.broker = Broker(self.model, self.resources)
    def tearDown(self):
        self.model.release.set()
        self.broker.close()

    def test_duplicate_computes_once_conflicting_body_rejected(self):
        req = request(); first = self.broker.submit(req)
        value = self.broker.wait(first)
        self.assertEqual(value, self.broker.wait(self.broker.submit(req)))
        self.assertEqual(self.model.calls, 1)
        self.assertIsNone(first.request)
        with self.assertRaisesRegex(ProtocolError, 'idempotency_conflict'):
            self.broker.submit(replace(req, frame_index=1))

    def test_single_execution_bounded_queue_and_inflight_duplicate(self):
        self.model.release.clear()
        req = request(); first = self.broker.submit(req)
        self.assertTrue(self.model.entered.wait(1))
        second = self.broker.submit(request())
        with self.assertRaisesRegex(ProtocolError, 'queue_full'): self.broker.submit(request())
        with self.assertRaisesRegex(ProtocolError, 'request_in_progress'): self.broker.submit(req)
        health = self.broker.health()
        self.assertEqual((health['running'], health['queued'], health['concurrency']), (1, 1, 1))
        self.assertFalse(health['accepting'])
        self.model.release.set()
        self.broker.wait(first); self.broker.wait(second)
        self.assertEqual(self.model.calls, 2)

    def test_cancellation_preserves_running_slot_and_discards_late_result(self):
        self.model.release.clear()
        req = request(); first = self.broker.submit(req)
        self.assertTrue(self.model.entered.wait(1))
        with self.assertRaisesRegex(ProtocolError, 'attempt_mismatch'): self.broker.cancel(req.request_id, 'other')
        self.assertFalse(self.broker.cancel(req.request_id, req.binding.attempt_id)['execution_finished'])
        self.assertEqual(self.broker.health()['running'], 1)
        self.model.release.set()
        with self.assertRaisesRegex(ProtocolError, 'cancelled'): self.broker.wait(first)
        self.assertIsNone(first.result)
        with self.assertRaisesRegex(ProtocolError, 'cancelled'): self.broker.wait(self.broker.submit(req))

    def test_timeout_fences_result_without_pretending_kernel_stopped(self):
        self.model.release.clear()
        first = self.broker.submit(request(deadline_ms=now_ms()+50))
        with self.assertRaisesRegex(ProtocolError, 'deadline_exceeded'): self.broker.wait(first)
        self.assertEqual(self.broker.health()['running'], 1)
        self.model.release.set(); self.assertTrue(first.done.wait(1))
        self.assertIsNone(first.result)

    def test_resource_and_model_readiness_are_distinct(self):
        self.resources.busy = True
        health = self.broker.health()
        self.assertTrue(health['model_ready']); self.assertTrue(health['busy']); self.assertFalse(health['accepting'])
        with self.assertRaisesRegex(ProtocolError, 'resource_busy'): self.broker.submit(request())
        self.resources.busy = False
        with self.assertRaisesRegex(ProtocolError, 'model_version_mismatch'):
            self.broker.submit(request(model_sha256='b'*64))

    def test_model_error_disables_session_and_drops_queued_work(self):
        self.model.release.clear(); self.model.error = RuntimeError('private CUDA details')
        first = self.broker.submit(request()); self.assertTrue(self.model.entered.wait(1))
        second = self.broker.submit(request()); self.model.release.set()
        with self.assertRaisesRegex(ProtocolError, 'inference_failed'): self.broker.wait(first)
        with self.assertRaisesRegex(ProtocolError, 'model_not_ready'): self.broker.wait(second)
        self.assertEqual(self.model.calls, 1)
        self.assertFalse(self.broker.health()['model_ready'])

    def test_drain_refuses_new_work_but_completes_owned_jobs(self):
        self.model.release.clear()
        first=self.broker.submit(request());self.assertTrue(self.model.entered.wait(1))
        second=self.broker.submit(request())
        state=self.broker.begin_drain()
        self.assertFalse(state['drained'])
        with self.assertRaisesRegex(ProtocolError,'service_draining'):self.broker.submit(request())
        self.model.release.set();self.broker.wait(first);self.broker.wait(second)
        health=self.broker.health()
        self.assertTrue(health['drained']);self.assertFalse(health['accepting'])
        self.assertEqual(self.model.calls,2)

    def test_current_binding_gate_rejects_manual_change_new_lease_or_expiry(self):
        req = request(); response = self.broker.wait(self.broker.submit(req)); result = Result.parse(response, req)
        result.require_current(req.binding)
        for binding in [replace(req.binding, attempt_id='next'), replace(req.binding, material_version='b'*64),
                        replace(req.binding, coordinator_generation='restarted'),
                        replace(req.binding, fingerprint=replace(req.binding.fingerprint, mtime_ns='999'))]:
            with self.assertRaisesRegex(ProtocolError, 'superseded_result'): result.require_current(binding)
        with self.assertRaisesRegex(ProtocolError, 'superseded_result'): result.require_current(req.binding, req.deadline_ms)

    def test_invalid_results_never_return_as_success(self):
        self.model.faces = [{'bbox':[0,0,2,2],'score':float('nan'),'keypoints':[[1,1]]*5}]
        with self.assertRaises(ProtocolError): self.broker.wait(self.broker.submit(request()))
        self.model.faces = []
        req=request(); response=self.broker.wait(self.broker.submit(req))
        response['execution'] = {**response['execution'], 'providers':['CPUExecutionProvider']}
        with self.assertRaisesRegex(ProtocolError, 'unexpected_execution'): Result.parse(response, req)

    def test_restart_has_new_generation_no_durable_computation_claim(self):
        before = self.broker.instance
        other = Broker(FakeDetector(), Resources())
        try: self.assertNotEqual(before, other.instance)
        finally: other.close()


class HTTPTests(unittest.TestCase):
    def setUp(self):
        self.model, self.resources = FakeDetector(), Resources()
        self.broker = Broker(self.model, self.resources)
        self.server = Server(('127.0.0.1', 0), self.broker, TOKEN)
        self.thread = threading.Thread(target=self.server.serve_forever, kwargs={'poll_interval':.01}, daemon=True)
        self.thread.start()
        self.port = self.server.server_address[1]
        self.client = Client(f'http://127.0.0.1:{self.port}', TOKEN)
    def tearDown(self):
        self.model.release.set()
        self.server.shutdown(); self.server.server_close(); self.broker.close(); self.thread.join(1)

    def raw(self, method, path, body=b'', headers=None):
        con=http.client.HTTPConnection('127.0.0.1',self.port,timeout=2)
        try:
            con.request(method,path,body=body,headers=headers or {})
            response=con.getresponse(); return response.status,response.read()
        finally: con.close()

    def test_authenticated_roundtrip_and_replay(self):
        req=request(); result=self.client.detect(req)
        self.assertEqual(result.context,req.context())
        self.assertEqual(self.client.detect(req).request_hash,result.request_hash)
        self.assertEqual(self.model.calls,1)
        self.assertTrue(self.client.ready()['model_ready'])

    def test_bounded_handlers_reject_more_connections(self):
        sockets=[]
        try:
            for _ in range(8):
                sock=socket.create_connection(('127.0.0.1',self.port),timeout=2)
                sock.sendall(b'GET /health/live HTTP/1.0\r\n')
                sockets.append(sock)
            deadline=time.monotonic()+1
            while self.server.slots._value and time.monotonic()<deadline:time.sleep(.005)
            status,_=self.raw('GET','/health/live')
            self.assertEqual(status,503)
        finally:
            for sock in sockets:sock.close()

    def test_auth_applies_to_health_and_inference(self):
        for method,path in [('GET','/health/live'),('GET','/health/ready'),('POST','/v1/detect')]:
            status,body=self.raw(method,path)
            self.assertEqual(status,401)
            self.assertNotIn(TOKEN.encode(),body)
        self.assertEqual(self.model.calls,0)

    def test_body_limit_and_no_resource_fetch_endpoint(self):
        headers={'Authorization':'Bearer '+TOKEN,'Content-Type':'application/json','Content-Length':str(MAX_BODY_BYTES+1)}
        status,_=self.raw('POST','/v1/detect',headers=headers)
        self.assertEqual(status,413)
        status,_=self.raw('GET','/file?path=/media',headers={'Authorization':'Bearer '+TOKEN})
        self.assertEqual(status,404)

    def test_network_failure_is_explicit_and_no_retry(self):
        self.client.timeout=.05; self.model.release.clear()
        with self.assertRaisesRegex(ProtocolError,'transport_unavailable'): self.client.detect(request())
        self.assertEqual(self.model.calls,1)
        self.model.release.set()

    def test_cancel_does_not_publish_and_returns_execution_state(self):
        req=request(); self.client.detect(req)
        result=self.client.cancel(req.request_id,req.binding.attempt_id)
        self.assertTrue(result['execution_finished'])
        with self.assertRaisesRegex(ProtocolError,'cancelled'): self.client.detect(req)

    def test_drain_state_and_release_survive_http(self):
        state=self.client.drain()
        self.assertTrue(state['drained'])
        health=self.client.ready()
        self.assertTrue(health['draining'])
        self.assertEqual(health['release'],{'managed':False})
        with self.assertRaisesRegex(ProtocolError,'service_draining'):self.client.detect(request())

    def test_busy_status_survives_http(self):
        self.resources.busy=True
        health=self.client.ready()
        self.assertTrue(health['model_ready'])
        self.assertTrue(health['busy'])
        self.assertFalse(health['accepting'])
        with self.assertRaises(ProtocolError) as failure:self.client.detect(request())
        self.assertEqual((failure.exception.code,failure.exception.status),('resource_busy',503))


class ReleaseTests(unittest.TestCase):
    def test_manifest_rejects_changed_code_lock_and_policy(self):
        from services.gpu_inference.release import create_manifest,verify_manifest
        package=pathlib.Path(__file__).resolve().parents[1]/'services/gpu_inference'
        with tempfile.TemporaryDirectory(prefix='isolated-gpu-release-') as folder:
            copied=pathlib.Path(folder)/'package';shutil.copytree(package,copied)
            manifest=create_manifest('isolated-release','a'*40,HASH,copied)
            path=pathlib.Path(folder)/'release.json';path.write_bytes(canonical(manifest))
            self.assertEqual(verify_manifest(path,copied)['revision'],'a'*40)
            changed={**manifest,'admission':{**manifest['admission'],'min_free_mib':1}}
            path.write_bytes(canonical(changed))
            with self.assertRaisesRegex(ValueError,'release_manifest_mismatch'):verify_manifest(path,copied)
            path.write_bytes(canonical(manifest))
            with (copied/'model.py').open('a') as out:out.write('\n# changed\n')
            with self.assertRaisesRegex(ValueError,'release_manifest_mismatch'):verify_manifest(path,copied)

    def test_release_health_requires_expected_instance_code_and_cuda_proof(self):
        from services.gpu_inference.release import create_manifest,health_release
        from services.gpu_inference.deploy.control import verify_release_health
        manifest=create_manifest('isolated-release','a'*40,HASH)
        manifest['manifest_sha256']='b'*64
        health={'release':health_release(manifest),'model_ready':True,'execution':dict(FakeDetector.execution)}
        verify_release_health(health,manifest)
        changed={**health,'release':{**health['release'],'revision':'c'*40}}
        with self.assertRaisesRegex(RuntimeError,'remote_release_mismatch'):verify_release_health(changed,manifest)
        changed={**health,'execution':{**health['execution'],'providers':['CPUExecutionProvider']}}
        with self.assertRaisesRegex(RuntimeError,'remote_model_readiness_mismatch'):verify_release_health(changed,manifest)

    def test_lock_has_only_gpu_runtime_and_pinned_versions(self):
        from services.gpu_inference.environment import lock_versions,LOCK
        versions=lock_versions()
        self.assertNotIn('onnxruntime',versions)
        self.assertEqual(versions['onnxruntime-gpu'],'1.23.2')
        text=LOCK.read_text()
        for block in text.split('\n'):
            if block and not block.startswith(('#',' ')):
                self.assertRegex(block,r'^[a-zA-Z0-9_.-]+==[^ ]+ \\$')
        self.assertEqual(len(versions),31)


if __name__=='__main__':
    unittest.main()
