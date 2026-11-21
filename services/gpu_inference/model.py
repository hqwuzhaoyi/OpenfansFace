"""Detection only. No database, media paths, identity lookup or recognition model.

The operator mounts one model read-only. CUDA readiness requires profiler evidence,
not just a listed provider. No automatic CPU fallback is implemented.
"""
from __future__ import annotations

import collections
import hashlib
import inspect
import io
import json
import pathlib
import subprocess
import tempfile
from dataclasses import dataclass

from .protocol import MAX_FACES, MAX_PIXELS, ProtocolError


RECOGNIZER_SHA256 = '4c06341c33c2ca1f86781dab0e829f88ad5b64be9fba56e56bc9ebdefc619e43'


@dataclass(frozen=True)
class ResourcePolicy:
    min_free_mib: int = 600
    max_utilization_pct: int = 85

    def sample(self, timeout=3):
        try:
            output = subprocess.run(
                ['nvidia-smi', '--query-gpu=memory.used,memory.free,utilization.gpu',
                 '--format=csv,noheader,nounits', '--id=0'],
                capture_output=True, text=True, timeout=min(3, max(.001, timeout)), check=True).stdout.strip()
            used, free, utilization = (int(x.strip()) for x in output.split(','))
            if min(used, free, utilization) < 0 or utilization > 100:
                raise ValueError('invalid counters')
            return {'available': True, 'memory_used_mib': used, 'memory_free_mib': free,
                    'utilization_pct': utilization,
                    'busy': free < self.min_free_mib or utilization > self.max_utilization_pct,
                    'reason': 'memory_low' if free < self.min_free_mib else
                              'utilization_high' if utilization > self.max_utilization_pct else None}
        except (OSError, ValueError, subprocess.SubprocessError):
            return {'available': False, 'busy': True, 'reason': 'telemetry_unavailable'}


class Detector:
    def __init__(self, model_path: str, expected_sha256: str, size=640, arena_mib=512, recognizer_path: str | None = None):
        import numpy as np
        import onnxruntime as ort
        from insightface.model_zoo.scrfd import SCRFD

        path = pathlib.Path(model_path)
        if not path.is_file() or hashlib.sha256(path.read_bytes()).hexdigest() != expected_sha256:
            raise ProtocolError('model_hash_mismatch', 503)
        if size not in (160, 640) or not 64 <= arena_mib <= 512:
            raise ValueError('unsupported detector size or CUDA arena limit')
        self.model_sha256 = expected_sha256
        self.preprocess_version = f'scrfd-{size}-bgr8-det0.5-nms0.4-v1'
        self.size = size
        ort.preload_dlls(directory='')
        options = ort.SessionOptions()
        options.intra_op_num_threads = options.inter_op_num_threads = 1
        options.enable_profiling = True
        options.log_severity_level = 3
        provider = {'device_id': 0, 'gpu_mem_limit': arena_mib * 1024 * 1024,
                    'arena_extend_strategy': 'kSameAsRequested', 'cudnn_conv_algo_search': 'HEURISTIC',
                    'cudnn_conv_use_max_workspace': '1', 'do_copy_in_default_stream': '1'}
        # Only the readiness profile is written, to an automatically removed private temp dir.
        with tempfile.TemporaryDirectory(prefix='gpu-readiness-') as folder:
            options.profile_file_prefix = folder + '/profile'
            session = ort.InferenceSession(str(path), sess_options=options,
                                           providers=[('CUDAExecutionProvider', provider), 'CPUExecutionProvider'])
            session.disable_fallback()
            if 'CUDAExecutionProvider' not in session.get_providers():
                raise ProtocolError('cuda_unavailable', 503)
            kwargs = {'model_file': str(path), 'session': session}
            # New InsightFace versions can create derived sessions; those must not bypass
            # our explicit provider, profiling, memory cap or single-session budget.
            if 'static_shape_sessions' in inspect.signature(SCRFD).parameters:
                kwargs['static_shape_sessions'] = False
            self.detector = SCRFD(**kwargs)
            self.detector.prepare(ctx_id=0, input_size=(size, size), det_thresh=.5, nms_thresh=.4)
            self.detector.detect(np.zeros((size, size, 3), dtype=np.uint8))
            profile = json.loads(pathlib.Path(session.end_profiling()).read_text())
        counts = collections.Counter(event.get('args', {}).get('provider') for event in profile
                                     if event.get('cat') == 'Node')
        if counts['CUDAExecutionProvider'] <= 0:
            raise ProtocolError('cuda_execution_not_proven', 503)

        self.recognizer = None
        rec_candidate = pathlib.Path(recognizer_path) if recognizer_path else (path.parent.parent / RECOGNIZER_SHA256 / 'w600k_r50.onnx')
        if rec_candidate.is_file():
            if hashlib.sha256(rec_candidate.read_bytes()).hexdigest() != RECOGNIZER_SHA256:
                raise ProtocolError('model_hash_mismatch', 503)
            from insightface.app.common import Face
            from insightface.model_zoo.arcface_onnx import ArcFaceONNX
            rec_options = ort.SessionOptions()
            rec_options.intra_op_num_threads = rec_options.inter_op_num_threads = 1
            rec_options.log_severity_level = 3
            rec_session = ort.InferenceSession(str(rec_candidate), sess_options=rec_options,
                                               providers=[('CUDAExecutionProvider', provider), 'CPUExecutionProvider'])
            rec_session.disable_fallback()
            if 'CUDAExecutionProvider' not in rec_session.get_providers():
                raise ProtocolError('cuda_unavailable', 503)
            self.recognizer = ArcFaceONNX(model_file=str(rec_candidate), session=rec_session)
            self.recognizer.prepare(ctx_id=0)
            warmup_face = Face(
                bbox=np.array([100, 100, 300, 300], dtype=np.float32),
                kps=np.array([[150, 180], [250, 180], [200, 220], [160, 260], [240, 260]], dtype=np.float32),
                det_score=0.9,
            )
            self.recognizer.get(np.zeros((size, size, 3), dtype=np.uint8), warmup_face)

        self.execution = {'stage': 'detection', 'model_sha256': expected_sha256,
                          'preprocess_version': self.preprocess_version, 'runtime_version': ort.__version__,
                          'provider_proof': 'startup_profile',
                          'providers': session.get_providers(), 'cuda_kernel_events': counts['CUDAExecutionProvider'],
                          'cpu_kernel_events': counts['CPUExecutionProvider']}

    def infer(self, request):
        import cv2
        import numpy as np
        from PIL import Image, UnidentifiedImageError

        if min(request.width, request.height) * self.size < max(request.width, request.height):
            raise ProtocolError('unsupported_aspect_ratio')
        # Inspect dimensions before decoding. A tiny compressed image must not allocate
        # an unbounded raster; do not trust the caller's declared dimensions.
        try:
            with Image.open(io.BytesIO(request.image)) as source:
                if source.format not in ('PNG', 'JPEG') or getattr(source, 'n_frames', 1) != 1:
                    raise ProtocolError('unsupported_image')
                if source.size != (request.width, request.height) or source.width * source.height > MAX_PIXELS:
                    raise ProtocolError('image_dimensions_mismatch')
                source.verify()
        except ProtocolError:
            raise
        except (UnidentifiedImageError, OSError, ValueError, SyntaxError, Image.DecompressionBombError) as exc:
            raise ProtocolError('invalid_image') from exc
        # Ignore EXIF orientation: returned geometry must map to transmitted raster.
        try:
            image = cv2.imdecode(np.frombuffer(request.image, dtype=np.uint8), cv2.IMREAD_COLOR | cv2.IMREAD_IGNORE_ORIENTATION)
        except cv2.error as exc:
            raise ProtocolError('invalid_image') from exc
        if image is None or image.shape[:2] != (request.height, request.width):
            raise ProtocolError('invalid_image')
        boxes, points = self.detector.detect(image)
        if len(boxes) > MAX_FACES or (len(boxes) and (points is None or len(points) != len(boxes))):
            raise ProtocolError('invalid_model_output', 503)
        results = []
        recognized_count = 0
        from insightface.app.common import Face
        for index, box in enumerate(boxes):
            score = float(box[4])
            item = {
                'bbox': box[:4].astype(float).tolist(),
                'score': score,
                'keypoints': points[index].astype(float).tolist(),
            }
            if self.recognizer is not None and score >= 0.7 and recognized_count < 16:
                face_obj = Face(
                    bbox=box[:4].astype(np.float32),
                    kps=points[index].astype(np.float32),
                    det_score=score,
                )
                self.recognizer.get(image, face_obj)
                emb = getattr(face_obj, 'normed_embedding', None)
                if emb is not None and emb.shape == (512,) and np.isfinite(emb).all():
                    item['embedding'] = [round(float(x), 7) for x in emb]
                    recognized_count += 1
            results.append(item)
        return results
