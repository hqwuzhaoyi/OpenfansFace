"""Calibrator computation handoff. Rust is the sole business metadata writer.

The private deployed identify_faces.py stays outside this repository; its supported
F_OUT isolates report output. No failed native call falls back to writing BASE.
"""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import threading
import time
import urllib.error
import urllib.request
import uuid


class MetadataError(RuntimeError):
    def __init__(self, message, status=503, operation_id=None):
        super().__init__(message)
        self.status = status
        self.operation_id = operation_id


def native(command, body):
    base = os.environ.get('FILE_OPERATIONS_RUST_URL')
    if not base:
        raise MetadataError('Rust metadata coordinator is not configured')
    headers = {'Content-Type': 'application/json'}
    token = os.environ.get('FILE_OPERATIONS_RUST_TOKEN')
    if token:
        headers['Authorization'] = 'Bearer ' + token
    data = json.dumps({'command': command, 'body': body}, ensure_ascii=False).encode()
    request = urllib.request.Request(base.rstrip('/') + '/v1/metadata', data=data, headers=headers)
    try:
        with urllib.request.urlopen(request, timeout=120) as response:
            value = json.load(response)
    except urllib.error.HTTPError as error:
        try:
            message = json.load(error).get('error', str(error))
        except (ValueError, OSError):
            message = str(error)
        raise MetadataError(message, error.code, body.get('operation_id')) from error
    except (OSError, ValueError) as error:
        raise MetadataError('Metadata outcome unknown; query original operation receipt', operation_id=body.get('operation_id')) from error
    return value


class Rebuilder:
    def __init__(self, base, python, state):
        self.base, self.python, self.state = base, python, state
        self.lock = threading.Lock()
        self.pending = False

    def pick(self, payload):
        operation_id = payload.get('operation_id') or str(uuid.uuid4())
        saved = native('template-set', {'operation_id': operation_id, 'name': payload.get('name'), 'groups': payload.get('groups')})
        with self.lock:
            self.pending = True
            if self.state['running']:
                return {'queued': True, 'running': True, 'operation_id': saved['id']}
            self.state.update(running=True, ok=None, msg='starting', done_at=None)
            threading.Thread(target=self.run, daemon=True).start()
        return {'started': True, 'operation_id': saved['id']}

    def run(self):
        while True:
            with self.lock:
                self.pending = False
            try:
                version = native('analysis-state', {})['version']
                with tempfile.TemporaryDirectory(prefix='face-analysis-output-') as directory:
                    result = subprocess.run([self.python, str(Path(self.base) / 'identify_faces.py')],
                        capture_output=True, text=True, timeout=900, cwd=self.base,
                        env={**os.environ, 'F_OUT': directory, 'F_INPUT_VERSION': version})
                    if result.returncode:
                        raise RuntimeError(((result.stdout or '') + (result.stderr or ''))[-500:])
                    outputs = {}
                    for name in ('report.json', 'claims-detail.json', 'promoted.json'):
                        target = Path(directory) / name
                        if target.exists():
                            if target.is_symlink() or target.stat().st_size > 8 * 1024 * 1024:
                                raise RuntimeError('Invalid or oversized analysis output')
                            outputs[name] = json.loads(target.read_text())
                    if 'report.json' not in outputs:
                        raise RuntimeError('Analysis did not produce an isolated report')
                    operation_id = str(uuid.uuid4())
                    self.state['publish_operation_id'] = operation_id
                    native('analysis-publish', {'operation_id': operation_id, 'version': version, 'outputs': outputs})
                    self.state.update(ok=True, msg=(result.stdout or '')[-500:])
            except Exception as error:
                self.state.update(ok=False, msg=str(error))
            with self.lock:
                if self.pending:
                    continue
                self.state.update(running=False, done_at=time.strftime('%T'))
                return
