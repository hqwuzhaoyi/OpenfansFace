"""Atomic, restart-visible progress records; separate from evidence and task decisions."""
import json
import os
from pathlib import Path
import tempfile
import time
import uuid


class TaskProgress:
    def __init__(self, base, kind, title, current=None, vid=None, job_id=None, attach=False):
        self.file = Path(base) / 'jobs' / (kind + '.json')
        self.file.parent.mkdir(parents=True, exist_ok=True)
        now = time.time()
        self.data = dict(id=job_id or str(uuid.uuid4()), kind=kind, title=title, status='running',
                         stage='准备中', current=current, vid=vid, done=None, total=None, unit='', failed=0,
                         started_at=now, updated_at=now, heartbeat_at=now, href='/')
        if attach:
            try:
                saved = json.loads(self.file.read_text())
                if saved['id'] == job_id:
                    self.data = saved
            except (OSError, ValueError, KeyError):
                pass
        self.write()

    def write(self):
        fd, temp = tempfile.mkstemp(dir=self.file.parent, prefix='.' + self.file.name)
        try:
            with os.fdopen(fd, 'w') as out:
                json.dump(self.data, out, ensure_ascii=False)
            os.chmod(temp, 0o644)
            os.replace(temp, self.file)
        finally:
            if os.path.exists(temp):
                os.unlink(temp)

    def update(self, **values):
        self.data.update(values, updated_at=time.time(), heartbeat_at=time.time())
        self.write()

    def finish(self, error=None):
        # A subprocess may have published more recent frame counters.
        try:
            saved = json.loads(self.file.read_text())
            if saved['id'] == self.data['id']:
                self.data = saved
        except (OSError, ValueError, KeyError):
            pass
        self.update(status='error' if error else 'complete', error=str(error)[:500] if error else None,
                    stage='处理失败' if error else '处理完成', finished_at=time.time())


def stage_progress(task, base, attach=True):
    kind = task['stage']
    return TaskProgress(base, kind, '提取水印' if kind == 'text' else '人脸分析',
                        current=task.get('name'), vid=task['vid'], job_id=task.get('_progress_id'), attach=attach)
