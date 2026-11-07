"""Explicit remote CUDA detection; CPU recognition and identity references stay on VM."""
import hashlib
import os
from pathlib import Path
import sys
import uuid

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from services.gpu_inference.client import Client
from services.gpu_inference.protocol import Binding, Fingerprint, Request, now_ms

DETECTOR_SHA = '5838f7fe053675b1c7a08b633df49e7af5495cee0493c7dcf6697200b85b5b91'
PREPROCESS = 'scrfd-640-bgr8-det0.5-nms0.4-v1'


def backend():
    value = os.environ.get('PROCESSING_FACE_BACKEND', 'cpu')
    if value not in ('cpu', 'gpu-detection'):
        raise ValueError('PROCESSING_FACE_BACKEND 必须为 cpu 或 gpu-detection')
    return value


def client():
    return Client(os.environ.get('PROCESSING_GPU_URL', ''), os.environ.get('PROCESSING_GPU_TOKEN', ''),
                  timeout=60, allow_private_http=os.environ.get('PROCESSING_GPU_PRIVATE_HTTP') == '1')


def inference_status():
    selected = backend()
    if selected == 'cpu':
        return {'backend': 'cpu', 'available': None, 'model_ready': None, 'name': '本机 CPU', 'fallback': False}
    state = {'backend': selected, 'available': False, 'name': 'GPU 检测 + 本机 CPU 识别', 'fallback': False}
    try:
        ready = client().ready()
        state.update(available=ready.get('model_ready') is True and ready.get('accepting') is True,
                     health=ready, error=None if ready.get('accepting') is True else 'GPU 暂不可接单，等待资源恢复')
    except Exception as error:
        state['error'] = str(error)[:250]
    return state


def binding(task):
    opaque = hashlib.sha256(task['vid'].encode()).hexdigest()
    return Binding(task_id=opaque, video_id=opaque,
        batch_id=hashlib.sha256(task['batch_id'].encode()).hexdigest(), attempt_id=task['run_id'],
        lease_id=task['run_id'], coordinator_generation=task['coordinator_generation'],
        lease_expires_at_ms=int(task['lease_expires_at'] * 1000),
        fingerprint=Fingerprint(task['size'], task['mtime_ns']),
        material_version=task['_material']['version'], pipeline_version='vm-face-det640-cpu-rec-v1')


class Detector:
    def __init__(self, task, transport=None):
        self.task, self.binding = task, binding(task)
        self.client = transport or client()
        self.execution = None
        self.timings = []

    def detect(self, image, encoded, index, timestamp):
        request = Request(str(uuid.uuid4()), self.binding,
            os.environ.get('PROCESSING_GPU_MODEL_SHA256', DETECTOR_SHA),
            os.environ.get('PROCESSING_GPU_PREPROCESS_VERSION', PREPROCESS),
            min(now_ms() + 60_000, self.binding.lease_expires_at_ms), index, round(timestamp * 1000),
            hashlib.sha256(encoded).hexdigest(), image.shape[1], image.shape[0], encoded)
        result = self.client.detect(request)
        result.require_current(self.binding)
        self.execution = {**result.execution, 'backend': 'gpu-detection', 'recognition_provider': 'CPUExecutionProvider',
                          'service_instance': result.service_instance}
        self.timings.append({**result.timing, 'bytes': len(encoded)})
        return result.faces
