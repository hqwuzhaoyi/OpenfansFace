"""Resident pure-calculation model runtime and daemon for OCR and face feature extraction.

Architecture contract (Ticket #33):
- Keeps OCR (PaddleEngine) and Face (FaceAnalysis / recognizer) models resident across tasks
  with health self-check and automatic restart resilience.
- Strictly restricted to pure calculation: accepts explicit versioned inputs, writes only
  unaccepted scratch artifacts under `processing/attempts/<run_id>` or `processing/tmp`,
  and never claims tasks, mutates person/account ownership, writes `processing.db` / `faces.db`
  / `ledger.db` / business JSONs / `processing/references.npz`, or decides business retries.
"""
import argparse
import builtins
from contextlib import contextmanager
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import io
import json
import os
from pathlib import Path
import shutil
import signal
import socket
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request
import uuid


_GUARD_STATE = threading.local()
_GUARD_LOCK = threading.Lock()
_ACTIVE_GUARDS = 0
_ORIG_OPEN = builtins.open
_ORIG_IO_OPEN = io.open
_ORIG_OS_OPEN = os.open
_ORIG_OS_REPLACE = os.replace
_ORIG_OS_RENAME = os.rename
_ORIG_OS_REMOVE = os.remove
_ORIG_OS_UNLINK = os.unlink
_ORIG_SQLITE_CONNECT = sqlite3.connect


def _is_write_mode(mode: str) -> bool:
    return any(ch in str(mode) for ch in ('w', 'a', 'x', '+'))


def _is_write_flags(flags: int) -> bool:
    write_mask = os.O_WRONLY | os.O_RDWR | os.O_CREAT | os.O_TRUNC | os.O_APPEND
    return bool(flags & write_mask)


def assert_compute_writable(path, base, root, run_id=None):
    """Verify `path` is within allowed scratch/attempt boundaries for pure compute."""
    if isinstance(path, int):
        return
    try:
        raw = os.fsdecode(path)
    except Exception:
        return
    if raw in ('/dev/null', 'nul'):
        return
    target = Path(raw).resolve(strict=False)
    base_path = Path(base).resolve(strict=False)
    root_path = Path(root).resolve(strict=False)

    allowed_under_base = [
        (base_path / 'processing/tmp').resolve(strict=False),
        (base_path / 'jobs').resolve(strict=False),
    ]
    if run_id:
        allowed_under_base.append((base_path / 'processing/attempts' / str(run_id)).resolve(strict=False))
    else:
        allowed_under_base.append((base_path / 'processing/attempts').resolve(strict=False))

    for allowed in allowed_under_base:
        if target == allowed or target.is_relative_to(allowed):
            return

    if target == base_path or target.is_relative_to(base_path):
        raise PermissionError(f'Pure compute boundary forbade write inside base directory: {target}')
    if target == root_path or target.is_relative_to(root_path):
        raise PermissionError(f'Pure compute boundary forbade write inside media root: {target}')

    sys_tmp = Path(tempfile.gettempdir()).resolve(strict=False)
    for tmp_root in (sys_tmp, Path('/tmp').resolve(strict=False), Path('/private/tmp').resolve(strict=False)):
        if target == tmp_root or target.is_relative_to(tmp_root):
            return

    raise PermissionError(f'Pure compute boundary forbade write outside scratch directories: {target}')


def _guarded_open(file, mode='r', *args, **kwargs):
    ctx = getattr(_GUARD_STATE, 'context', None)
    if ctx is not None and _is_write_mode(mode):
        assert_compute_writable(file, ctx['base'], ctx['root'], ctx.get('run_id'))
    return _ORIG_OPEN(file, mode, *args, **kwargs)


def _guarded_os_open(path, flags, *args, **kwargs):
    ctx = getattr(_GUARD_STATE, 'context', None)
    if ctx is not None and _is_write_flags(flags):
        assert_compute_writable(path, ctx['base'], ctx['root'], ctx.get('run_id'))
    return _ORIG_OS_OPEN(path, flags, *args, **kwargs)


def _guarded_os_replace(src, dst, *args, **kwargs):
    ctx = getattr(_GUARD_STATE, 'context', None)
    if ctx is not None:
        assert_compute_writable( src, ctx['base'], ctx['root'], ctx.get('run_id'))
        assert_compute_writable(dst, ctx['base'], ctx['root'], ctx.get('run_id'))
    return _ORIG_OS_REPLACE(src, dst, *args, **kwargs)


def _guarded_os_rename(src, dst, *args, **kwargs):
    ctx = getattr(_GUARD_STATE, 'context', None)
    if ctx is not None:
        assert_compute_writable(src, ctx['base'], ctx['root'], ctx.get('run_id'))
        assert_compute_writable(dst, ctx['base'], ctx['root'], ctx.get('run_id'))
    return _ORIG_OS_RENAME(src, dst, *args, **kwargs)


def _guarded_os_unlink(path, *args, **kwargs):
    ctx = getattr(_GUARD_STATE, 'context', None)
    if ctx is not None:
        assert_compute_writable(path, ctx['base'], ctx['root'], ctx.get('run_id'))
    return _ORIG_OS_UNLINK(path, *args, **kwargs)


def _guarded_sqlite_connect(database, *args, **kwargs):
    ctx = getattr(_GUARD_STATE, 'context', None)
    if ctx is not None:
        db_str = str(database)
        uri = bool(kwargs.get('uri', False))
        if not (uri and db_str.startswith('file:') and 'mode=ro' in db_str):
            raise PermissionError(f'Pure compute boundary forbade writable SQLite connection: {db_str}')
    return _ORIG_SQLITE_CONNECT(database, *args, **kwargs)


@contextmanager
def compute_write_guard(base, root, run_id=None):
    """Enforce at runtime that Python compute writes only to attempt/tmp scratch directories."""
    global _ACTIVE_GUARDS
    previous = getattr(_GUARD_STATE, 'context', None)
    _GUARD_STATE.context = {'base': Path(base), 'root': Path(root), 'run_id': run_id}
    with _GUARD_LOCK:
        if _ACTIVE_GUARDS == 0:
            builtins.open = _guarded_open
            io.open = _guarded_open
            os.open = _guarded_os_open
            os.replace = _guarded_os_replace
            os.rename = _guarded_os_rename
            os.unlink = _guarded_os_unlink
            os.remove = _guarded_os_unlink
            sqlite3.connect = _guarded_sqlite_connect
        _ACTIVE_GUARDS += 1
    try:
        yield
    finally:
        with _GUARD_LOCK:
            _ACTIVE_GUARDS -= 1
            if _ACTIVE_GUARDS == 0:
                builtins.open = _ORIG_OPEN
                io.open = _ORIG_IO_OPEN
                os.open = _ORIG_OS_OPEN
                os.replace = _ORIG_OS_REPLACE
                os.rename = _ORIG_OS_RENAME
                os.unlink = _ORIG_OS_UNLINK
                os.remove = _ORIG_OS_REMOVE
                sqlite3.connect = _ORIG_SQLITE_CONNECT
        _GUARD_STATE.context = previous


class ComputeRuntime:
    """Process-wide resident holder for OCR and Face models with health and reuse counters."""

    def __init__(self):
        self._lock = threading.Lock()
        self.started_at = time.time()
        self.ocr_engine = None
        self._ocr_cls = None
        self.ocr_load_count = 0
        self.ocr_tasks_run = 0
        self.last_ocr_at = None

        self.face_runtime = None
        self._face_key = None
        self.face_load_count = 0
        self.face_tasks_run = 0
        self.last_face_at = None

    def reset(self):
        with self._lock:
            self.ocr_engine = None
            self._ocr_cls = None
            self.ocr_load_count = 0
            self.ocr_tasks_run = 0
            self.last_ocr_at = None
            self.face_runtime = None
            self._face_key = None
            self.face_load_count = 0
            self.face_tasks_run = 0
            self.last_face_at = None

    def get_ocr_engine(self, engine_cls=None):
        if engine_cls is None:
            from watermark_worker import PaddleEngine as engine_cls
        with self._lock:
            if self.ocr_engine is None or self._ocr_cls is not engine_cls:
                self.ocr_engine = engine_cls()
                self._ocr_cls = engine_cls
                self.ocr_load_count += 1
            self.ocr_tasks_run += 1
            self.last_ocr_at = time.time()
            return self.ocr_engine

    def get_face_runtime(self, backend_mode: str, model_home: Path, expected_hashes: dict):
        model_home = Path(model_home)
        model_hashes = {
            name: hashlib.sha256((model_home / name).read_bytes()).hexdigest()
            for name in ('det_10g.onnx', 'w600k_r50.onnx')
        }
        if model_hashes != expected_hashes:
            raise ValueError('人脸模型内容与已登记版本不符，暂停该次分析')

        key = (backend_mode, str(model_home), json.dumps(model_hashes, sort_keys=True))
        with self._lock:
            if self.face_runtime is None or self._face_key != key:
                import onnxruntime as ort
                options = ort.SessionOptions()
                options.intra_op_num_threads = 1
                options.inter_op_num_threads = 1
                insight_root = str(model_home.parent.parent)
                if backend_mode == 'gpu-detection':
                    from insightface.model_zoo import get_model
                    recognition_path = model_home / 'w600k_r50.onnx'
                    recognizer = get_model(
                        str(recognition_path),
                        providers=['CPUExecutionProvider'],
                        sess_options=options,
                    )
                    recognizer.prepare(ctx_id=-1)
                    self.face_runtime = {
                        'mode': 'gpu-detection',
                        'recognizer': recognizer,
                        'engine': None,
                        'model_hashes': model_hashes,
                    }
                else:
                    from insightface.app import FaceAnalysis
                    engine = FaceAnalysis(
                        name='buffalo_l',
                        root=insight_root,
                        allowed_modules=['detection', 'recognition'],
                        providers=['CPUExecutionProvider'],
                        sess_options=options,
                    )
                    engine.prepare(ctx_id=-1, det_size=(640, 640))
                    self.face_runtime = {
                        'mode': 'cpu',
                        'recognizer': None,
                        'engine': engine,
                        'model_hashes': model_hashes,
                    }
                self._face_key = key
                self.face_load_count += 1
            self.face_tasks_run += 1
            self.last_face_at = time.time()
            return self.face_runtime

    def health_snapshot(self) -> dict:
        with self._lock:
            return {
                'ok': True,
                'pid': os.getpid(),
                'started_at': self.started_at,
                'uptime_sec': round(max(0.0, time.time() - self.started_at), 3),
                'ocr_loaded': self.ocr_engine is not None,
                'ocr_load_count': self.ocr_load_count,
                'ocr_tasks_run': self.ocr_tasks_run,
                'last_ocr_at': self.last_ocr_at,
                'face_loaded': self.face_runtime is not None,
                'face_load_count': self.face_load_count,
                'face_tasks_run': self.face_tasks_run,
                'last_face_at': self.last_face_at,
                'face_backend': self.face_runtime.get('mode') if self.face_runtime else None,
                'write_boundary': 'attempt_and_tmp_only',
            }


RUNTIME = ComputeRuntime()


class _ComputeHandler(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):
        return

    def _send_json(self, status: int, payload: dict):
        body = json.dumps(payload, ensure_ascii=False).encode('utf-8')
        self.send_response(status)
        self.send_header('Content-Type', 'application/json; charset=utf-8')
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path == '/health':
            self._send_json(200, RUNTIME.health_snapshot())
            return
        self._send_json(404, {'ok': False, 'error': 'not_found'})

    def do_POST(self):
        length = int(self.headers.get('Content-Length', '0') or '0')
        raw = self.rfile.read(length) if length > 0 else b'{}'
        try:
            body = json.loads(raw.decode('utf-8'))
        except Exception as error:
            self._send_json(400, {'ok': False, 'error': f'invalid_json: {error}'})
            return

        if self.path == '/v1/compute/stage':
            stage = body.get('stage')
            task = body.get('task')
            base = Path(body.get('base', ''))
            root = Path(body.get('root', ''))
            if stage not in ('text', 'face') or not isinstance(task, dict):
                self._send_json(400, {'ok': False, 'error': 'stage and task are required'})
                return
            try:
                from processing_worker import text_stage
                from processing_faces import analyze
                with compute_write_guard(base, root, run_id=task.get('run_id')):
                    if stage == 'text':
                        result = text_stage(task, base, root)
                    else:
                        result = analyze(task, base, root)
                self._send_json(200, {
                    'ok': True,
                    'result': result,
                    'runtime': RUNTIME.health_snapshot(),
                })
            except Exception as error:
                self._send_json(500, {
                    'ok': False,
                    'error': str(error),
                    'error_type': type(error).__name__,
                })
            return

        if self.path == '/v1/compute/references':
            base = Path(body.get('base', ''))
            root = Path(body.get('root', ''))
            state = body.get('state')
            try:
                from processing_faces import compute_staged_references
                with compute_write_guard(base, root):
                    staged = compute_staged_references(base, root, state=state)
                self._send_json(200, {
                    'ok': True,
                    'result': staged,
                    'runtime': RUNTIME.health_snapshot(),
                })
            except Exception as error:
                self._send_json(500, {
                    'ok': False,
                    'error': str(error),
                    'error_type': type(error).__name__,
                })
            return

        if self.path == '/v1/compute/reset':
            RUNTIME.reset()
            self._send_json(200, {'ok': True, 'runtime': RUNTIME.health_snapshot()})
            return

        self._send_json(404, {'ok': False, 'error': 'not_found'})


class ResidentComputeClient:
    """Supervises a resident `processing_compute.py` daemon with health checks and auto-restart."""

    def __init__(self, external_url: str | None = None, startup_timeout: float = 15.0):
        self.external_url = external_url or os.environ.get('PROCESSING_COMPUTE_URL')
        self.startup_timeout = startup_timeout
        self._lock = threading.Lock()
        self._process: subprocess.Popen | None = None
        self._port: int | None = None
        self.start_count = 0
        self.restart_count = 0

    @property
    def base_url(self) -> str:
        if self.external_url:
            return self.external_url.rstrip('/')
        if self._port is None:
            raise RuntimeError('Resident compute daemon is not started')
        return f'http://127.0.0.1:{self._port}'

    def _pick_free_port(self) -> int:
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
            sock.bind(('127.0.0.1', 0))
            return int(sock.getsockname()[1])

    def _request(self, method: str, path: str, payload: dict | None = None, timeout: float = 10.0) -> dict:
        data = None if payload is None else json.dumps(payload, ensure_ascii=False).encode('utf-8')
        headers = {'Content-Type': 'application/json'} if data is not None else {}
        req = urllib.request.Request(self.base_url + path, data=data, headers=headers, method=method)
        try:
            with urllib.request.urlopen(req, timeout=timeout) as resp:
                return json.loads(resp.read().decode('utf-8'))
        except urllib.error.HTTPError as error:
            raw = error.read().decode('utf-8', errors='replace')
            try:
                parsed = json.loads(raw)
            except Exception:
                parsed = {'ok': False, 'error': raw or str(error)}
            raise RuntimeError(parsed.get('error') or f'HTTP {error.code}') from error

    def health(self, timeout: float = 3.0) -> dict:
        return self._request('GET', '/health', timeout=timeout)

    def ensure_alive(self) -> dict:
        with self._lock:
            if self.external_url:
                return self.health()
            if self._process is not None and self._process.poll() is None:
                try:
                    snap = self.health(timeout=2.0)
                    if snap.get('ok') is True:
                        return snap
                except Exception:
                    pass
                self._terminate_locked()
                self.restart_count += 1
            elif self._process is not None:
                self._terminate_locked()
                self.restart_count += 1

            port = self._pick_free_port()
            cmd = [
                sys.executable,
                str(Path(__file__).resolve()),
                '--serve',
                '--host',
                '127.0.0.1',
                '--port',
                str(port),
            ]
            self._process = subprocess.Popen(
                cmd,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.PIPE,
                text=True,
                start_new_session=True,
            )
            self._port = port
            self.start_count += 1

            deadline = time.time() + self.startup_timeout
            last_error = None
            while time.time() < deadline:
                if self._process.poll() is not None:
                    stderr = (self._process.stderr.read() if self._process.stderr else '')[-400:]
                    raise RuntimeError(f'Resident compute daemon exited early: {stderr}')
                try:
                    snap = self.health(timeout=1.0)
                    if snap.get('ok') is True:
                        return snap
                except Exception as error:
                    last_error = error
                    time.sleep(0.05)
            self._terminate_locked()
            raise RuntimeError(f'Resident compute daemon failed health check: {last_error}')

    def _terminate_locked(self):
        if self._process is None:
            return
        proc = self._process
        self._process = None
        self._port = None
        if proc.poll() is None:
            try:
                os.killpg(proc.pid, signal.SIGTERM)
                proc.wait(timeout=3)
            except Exception:
                try:
                    os.killpg(proc.pid, signal.SIGKILL)
                    proc.wait(timeout=2)
                except Exception:
                    pass
        if proc.stderr:
            try:
                proc.stderr.close()
            except Exception:
                pass

    def close(self):
        with self._lock:
            self._terminate_locked()

    def run_stage(self, task: dict, base: Path, root: Path, timeout: float = 600.0) -> dict:
        self.ensure_alive()
        payload = {
            'stage': task['stage'],
            'task': {**task, '_stage_deadline_ms': int((time.time() + timeout) * 1000)},
            'base': str(base),
            'root': str(root),
        }
        try:
            response = self._request('POST', '/v1/compute/stage', payload, timeout=timeout)
            return response['result']
        except (urllib.error.URLError, ConnectionError, BrokenPipeError, EOFError, TimeoutError, socket.timeout):
            # If the resident daemon crashed or hung mid-flight, terminate and surface error
            # so the next task automatically restarts a healthy resident daemon.
            with self._lock:
                self._terminate_locked()
                self.restart_count += 1
            raise

    def compute_references(self, base: Path, root: Path, state: dict | None = None, timeout: float = 120.0) -> dict:
        self.ensure_alive()
        payload = {
            'base': str(base),
            'root': str(root),
            'state': state,
        }
        response = self._request('POST', '/v1/compute/references', payload, timeout=timeout)
        return response['result']


_DEFAULT_CLIENT: ResidentComputeClient | None = None
_CLIENT_LOCK = threading.Lock()


def get_resident_client() -> ResidentComputeClient:
    global _DEFAULT_CLIENT
    with _CLIENT_LOCK:
        if _DEFAULT_CLIENT is None:
            _DEFAULT_CLIENT = ResidentComputeClient()
        return _DEFAULT_CLIENT


def close_resident_client():
    global _DEFAULT_CLIENT
    with _CLIENT_LOCK:
        if _DEFAULT_CLIENT is not None:
            _DEFAULT_CLIENT.close()
            _DEFAULT_CLIENT = None


def serve(host: str = '127.0.0.1', port: int = 0):
    server = ThreadingHTTPServer((host, port), _ComputeHandler)
    stop = threading.Event()

    def _shutdown(*_):
        stop.set()
        threading.Thread(target=server.shutdown, daemon=True).start()

    signal.signal(signal.SIGTERM, _shutdown)
    signal.signal(signal.SIGINT, _shutdown)
    try:
        server.serve_forever(poll_interval=0.2)
    finally:
        server.server_close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serve', action='store_true')
    parser.add_argument('--host', default='127.0.0.1')
    parser.add_argument('--port', type=int, default=int(os.environ.get('PROCESSING_COMPUTE_PORT', '0')))
    args = parser.parse_args()
    if args.serve:
        serve(args.host, args.port)


if __name__ == '__main__':
    main()
