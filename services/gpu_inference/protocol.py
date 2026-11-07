"""Versioned wire contract shared by the GPU adapter and VM client.

Opaque identifiers only: never send a filesystem path, database name, or identity.
A matching response is evidence, not authorization to publish business state.
"""
from __future__ import annotations

import base64
import hashlib
import json
import math
import re
import time
from dataclasses import asdict, dataclass
from typing import Any

SCHEMA = 1
MAX_IMAGE_BYTES = 1_500_000
MAX_BODY_BYTES = 2_010_000
MAX_RESPONSE_BYTES = 300_000
MAX_WIDTH = 1280
MAX_HEIGHT = 4096
MAX_DIMENSION = MAX_HEIGHT
MAX_PIXELS = MAX_WIDTH * MAX_HEIGHT
MAX_FACES = 256
MAX_DEADLINE_MS = 120_000
TOKEN_RE = re.compile(r'[A-Za-z0-9_.:-]{1,128}\Z')
HASH_RE = re.compile(r'[a-f0-9]{64}\Z')


class ProtocolError(ValueError):
    def __init__(self, code: str, status: int = 400, retryable: bool = False):
        super().__init__(code)
        self.code, self.status, self.retryable = code, status, retryable


def now_ms() -> int:
    return time.time_ns() // 1_000_000


def canonical(value: Any) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(',', ':'), allow_nan=False).encode()


def digest(value: Any) -> str:
    return hashlib.sha256(canonical(value)).hexdigest()


def decode_json(data: bytes) -> dict:
    def pairs(items):
        result = {}
        for key, value in items:
            if key in result:
                raise ProtocolError('duplicate_json_key')
            result[key] = value
        return result
    try:
        result = json.loads(data, object_pairs_hook=pairs,
                            parse_constant=lambda _: (_ for _ in ()).throw(ProtocolError('nonfinite_json')))
    except (ValueError, UnicodeError, RecursionError) as exc:
        raise ProtocolError('invalid_json') from exc
    if type(result) is not dict:
        raise ProtocolError('invalid_json_object')
    return result


def fields(obj, names):
    if type(obj) is not dict or set(obj) != set(names.split()):
        raise ProtocolError('invalid_fields')


def integer(value, minimum=0, maximum=2**63 - 1):
    if type(value) is not int or not minimum <= value <= maximum:
        raise ProtocolError('invalid_integer')
    return value


def token(value):
    if type(value) is not str or not TOKEN_RE.fullmatch(value):
        raise ProtocolError('invalid_identifier')
    return value


def sha(value):
    if type(value) is not str or not HASH_RE.fullmatch(value):
        raise ProtocolError('invalid_sha256')
    return value


@dataclass(frozen=True)
class Fingerprint:
    size: int
    mtime_ns: str
    content_sha256: str | None = None

    @classmethod
    def parse(cls, obj):
        fields(obj, 'size mtime_ns content_sha256')
        integer(obj['size'])
        if type(obj['mtime_ns']) is not str or not re.fullmatch(r'[0-9]{1,20}', obj['mtime_ns']):
            raise ProtocolError('invalid_mtime')
        if obj['content_sha256'] is not None:
            sha(obj['content_sha256'])
        return cls(**obj)


@dataclass(frozen=True)
class Binding:
    task_id: str
    video_id: str
    batch_id: str
    attempt_id: str
    lease_id: str
    coordinator_generation: str
    lease_expires_at_ms: int
    fingerprint: Fingerprint
    material_version: str
    pipeline_version: str

    @classmethod
    def parse(cls, obj):
        fields(obj, 'task_id video_id batch_id attempt_id lease_id coordinator_generation lease_expires_at_ms fingerprint material_version pipeline_version')
        for key in ('task_id', 'video_id', 'batch_id', 'attempt_id', 'lease_id', 'coordinator_generation', 'pipeline_version'):
            token(obj[key])
        integer(obj['lease_expires_at_ms'], 1)
        sha(obj['material_version'])
        return cls(**{**obj, 'fingerprint': Fingerprint.parse(obj['fingerprint'])})


@dataclass(frozen=True)
class Request:
    request_id: str
    binding: Binding
    model_sha256: str
    preprocess_version: str
    deadline_ms: int
    frame_index: int
    timestamp_ms: int
    image_sha256: str
    width: int
    height: int
    image: bytes

    def wire(self):
        return {'schema_version': SCHEMA, 'stage': 'detection', 'request_id': self.request_id,
                'binding': asdict(self.binding), 'model_sha256': self.model_sha256,
                'preprocess_version': self.preprocess_version, 'deadline_ms': self.deadline_ms,
                'frame': {'index': self.frame_index, 'timestamp_ms': self.timestamp_ms,
                          'sha256': self.image_sha256, 'width': self.width, 'height': self.height,
                          'data_base64': base64.b64encode(self.image).decode()}}

    def context(self):
        result = self.wire()
        del result['frame']['data_base64']
        return result

    @property
    def request_hash(self):
        return digest(self.context())

    @classmethod
    def parse(cls, obj, at_ms=None, check_deadline=True):
        fields(obj, 'schema_version stage request_id binding model_sha256 preprocess_version deadline_ms frame')
        if type(obj['schema_version']) is not int or obj['schema_version'] != SCHEMA or obj['stage'] != 'detection':
            raise ProtocolError('unsupported_protocol')
        binding = Binding.parse(obj['binding'])
        token(obj['request_id']); sha(obj['model_sha256']); token(obj['preprocess_version'])
        integer(obj['deadline_ms'], 1)
        at_ms = now_ms() if at_ms is None else at_ms
        if check_deadline and not at_ms < obj['deadline_ms'] <= min(at_ms + MAX_DEADLINE_MS, binding.lease_expires_at_ms):
            raise ProtocolError('expired_or_invalid_deadline', 408)
        frame = obj['frame']
        fields(frame, 'index timestamp_ms sha256 width height data_base64')
        integer(frame['index'], 0, 10_000); integer(frame['timestamp_ms'])
        integer(frame['width'], 1, MAX_WIDTH); integer(frame['height'], 1, MAX_HEIGHT)
        if frame['width'] * frame['height'] > MAX_PIXELS:
            raise ProtocolError('image_too_large', 413)
        sha(frame['sha256'])
        data = frame['data_base64']
        if type(data) is not str or len(data) > (MAX_IMAGE_BYTES + 2) // 3 * 4:
            raise ProtocolError('image_too_large', 413)
        try:
            image = base64.b64decode(data, validate=True)
        except (ValueError, TypeError) as exc:
            raise ProtocolError('invalid_image_encoding') from exc
        if not image or len(image) > MAX_IMAGE_BYTES or hashlib.sha256(image).hexdigest() != frame['sha256']:
            raise ProtocolError('image_hash_mismatch')
        return cls(obj['request_id'], binding, obj['model_sha256'], obj['preprocess_version'], obj['deadline_ms'],
                   frame['index'], frame['timestamp_ms'], frame['sha256'], frame['width'], frame['height'], image)


@dataclass(frozen=True)
class Result:
    context: dict
    request_hash: str
    service_instance: str
    execution: dict
    faces: tuple[dict, ...]
    timing: dict

    def require_current(self, current: Binding, at_ms=None):
        """Preflight only; VM must still perform atomic publication via #13."""
        at_ms = now_ms() if at_ms is None else at_ms
        if self.context['binding'] != asdict(current) or at_ms >= min(current.lease_expires_at_ms, self.context['deadline_ms']):
            raise ProtocolError('superseded_result', 409)

    @classmethod
    def parse(cls, value, request: Request):
        fields(value, 'schema_version context request_hash service_instance execution faces timing')
        if type(value['schema_version']) is not int or value['schema_version'] != SCHEMA or value['context'] != request.context() or value['request_hash'] != request.request_hash:
            raise ProtocolError('response_context_mismatch', 502)
        token(value['service_instance'])
        execution = value['execution']
        fields(execution, 'stage model_sha256 preprocess_version runtime_version providers provider_proof cuda_kernel_events cpu_kernel_events')
        if (execution['provider_proof'] != 'startup_profile' or execution['stage'] != 'detection' or execution['model_sha256'] != request.model_sha256 or
                execution['preprocess_version'] != request.preprocess_version or
                type(execution['providers']) is not list or 'CUDAExecutionProvider' not in execution['providers']):
            raise ProtocolError('unexpected_execution', 502)
        for provider in execution['providers']:
            token(provider)
        token(execution['runtime_version'])
        integer(execution['cuda_kernel_events'], 1); integer(execution['cpu_kernel_events'])
        faces = value['faces']
        if type(faces) is not list or len(faces) > MAX_FACES:
            raise ProtocolError('invalid_faces', 502)
        for face in faces:
            fields(face, 'bbox score keypoints')
            box, score, points = face['bbox'], face['score'], face['keypoints']
            if (type(box) is not list or len(box) != 4 or type(points) is not list or len(points) != 5 or
                    any(type(p) is not list or len(p) != 2 for p in points)):
                raise ProtocolError('invalid_geometry', 502)
            numbers = box + [score] + [n for point in points for n in point]
            if any(type(n) not in (int, float) or not math.isfinite(n) for n in numbers):
                raise ProtocolError('invalid_geometry', 502)
            # Model geometry may extend outside an image. Bound it, preserve original coordinates
            # for VM alignment, and require an actual intersection with the supplied frame.
            if (not 0 <= score <= 1 or not box[0] < box[2] or not box[1] < box[3] or
                    box[2] <= 0 or box[3] <= 0 or box[0] >= request.width or box[1] >= request.height or
                    any(abs(n) > MAX_DIMENSION * 4 for n in box + [n for p in points for n in p])):
                raise ProtocolError('invalid_geometry', 502)
        timing = value['timing']
        fields(timing, 'queue_ms inference_ms total_ms')
        for val in timing.values():
            if type(val) not in (float, int) or not math.isfinite(val) or val < 0:
                raise ProtocolError('invalid_timing', 502)
        result = cls(value['context'], value['request_hash'], value['service_instance'], execution, tuple(faces), timing)
        result.require_current(request.binding)
        return result
