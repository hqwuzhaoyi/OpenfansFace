"""Atomic, restart-visible progress records; separate from evidence and task decisions."""
import hashlib
import json
import os
from pathlib import Path
import tempfile
import time
import uuid

from processing_compute import in_compute_write_guard
from processing_publication import native


class TaskProgress:
    def __init__(self, base, kind, title, current=None, vid=None, job_id=None, attach=False, isolated=False):
        self.base = Path(base)
        self.isolated = bool(isolated)
        identifier = job_id or str(uuid.uuid4())
        digest = hashlib.sha256(identifier.encode()).hexdigest() + '.json'
        self.job_file = (self.base / 'jobs' / 'stages' / digest
                         if isolated else self.base / 'jobs' / (kind + '.json'))
        self.scratch_file = self.base / 'processing/tmp' / ('progress-' + digest)
        self.file = self.scratch_file if in_compute_write_guard() else self.job_file
        self.file.parent.mkdir(parents=True, exist_ok=True)
        now = time.time()
        self.data = dict(id=identifier, kind=kind, title=title, status='running',
                         stage='准备中', current=current, vid=vid, done=None, total=None, unit='', failed=0,
                         started_at=now, updated_at=now, heartbeat_at=now, href='/')
        if attach:
            for candidate in (self.job_file, self.scratch_file):
                try:
                    saved = json.loads(candidate.read_text())
                    if saved['id'] == job_id:
                        self.data = saved
                        break
                except (OSError, ValueError, KeyError):
                    pass
        self.write()

    def write(self):
        if os.environ.get('FILE_OPERATIONS_RUST_URL'):
            try:
                native('processing-stage-progress', {
                    'isolated': self.isolated,
                    'data': self.data,
                }, timeout=5)
            except Exception:
                pass
        target = self.scratch_file if in_compute_write_guard() else self.job_file
        target.parent.mkdir(parents=True, exist_ok=True)
        fd, temp = tempfile.mkstemp(dir=target.parent, prefix='.' + target.name)
        try:
            with os.fdopen(fd, 'w') as out:
                json.dump(self.data, out, ensure_ascii=False)
            os.chmod(temp, 0o644)
            os.replace(temp, target)
        finally:
            if os.path.exists(temp):
                os.unlink(temp)

    def update(self, **values):
        self.data.update(values, updated_at=time.time(), heartbeat_at=time.time())
        self.write()

    def finish(self, error=None):
        # A subprocess or compute worker may have published more recent frame counters.
        for candidate in (self.job_file, self.scratch_file):
            try:
                saved = json.loads(candidate.read_text())
                if saved['id'] == self.data['id']:
                    self.data = saved
                    break
            except (OSError, ValueError, KeyError):
                pass
        self.update(status='error' if error else 'complete', error=str(error)[:500] if error else None,
                    stage='处理失败' if error else '处理完成', finished_at=time.time())


def stage_progress(task, base, attach=True):
    kind = task['stage']
    return TaskProgress(base, kind, '提取水印' if kind == 'text' else '人脸分析',
                        current=task.get('name'), vid=task['vid'], job_id=task.get('_progress_id') or task.get('run_id'), attach=attach,
                        isolated=bool(task.get('_progress_id') or task.get('run_id')))
