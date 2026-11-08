"""VM attempt fencing and native-only publication of accepted analysis results."""
import json
import os
from pathlib import Path
import urllib.error
import urllib.request
import uuid
import tempfile


class PublicationError(RuntimeError):
    def __init__(self, message, status=503, operation_id=None, uncertain=False):
        super().__init__(message)
        self.status, self.operation_id, self.uncertain = status, operation_id, uncertain


def native(command, body, *, timeout=120):
    origin = os.environ.get('FILE_OPERATIONS_RUST_URL')
    if not origin:
        raise PublicationError('原生结果协调器未配置，保留任务等待恢复')
    headers = {'Content-Type': 'application/json'}
    token = os.environ.get('FILE_OPERATIONS_RUST_TOKEN')
    if token:
        headers['Authorization'] = 'Bearer ' + token
    request = urllib.request.Request(origin.rstrip('/') + '/v1/metadata',
        data=json.dumps({'command': command, 'body': body}, allow_nan=False).encode(), headers=headers)
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            return json.load(response)
    except urllib.error.HTTPError as error:
        try:
            message = json.load(error).get('error', str(error))
        except (ValueError, OSError):
            message = str(error)
        raise PublicationError(message, error.code, body.get('operation_id'), error.code >= 500 and bool(body.get('operation_id'))) from error
    except (OSError, ValueError) as error:
        raise PublicationError('结果提交状态未知，等待原操作回执', operation_id=body.get('operation_id'), uncertain=bool(body.get('operation_id'))) from error


def attempt_directory(base, task):
    # Validate UUID, never interpolate arbitrary task/video identifiers into paths.
    attempt = str(uuid.UUID(task['run_id']))
    directory = Path(base) / 'processing/attempts' / attempt
    directory.mkdir(parents=True, exist_ok=True)
    return directory


def prepare(task, base, people, keys):
    scope = {'vids': [task['vid']], 'people': sorted(set(people)), 'watermark_keys': sorted(set(keys)), 'all_watermark_rules': task['stage'] == 'text'}
    state = native('state', {'scope': scope})
    task['_material'] = {'scope': scope, 'version': state['version']}
    return task


def publish_result(task, base, data, next_stage=None):
    material = task['_material']
    body = {'operation_id': str(uuid.UUID(task['run_id'])),
            **material,
            'task': {key: task[key] for key in ('vid', 'path', 'size', 'mtime_ns', 'stage', 'run_id', 'batch_id', 'lease_expires_at', 'coordinator_generation')},
            'data': data}
    if next_stage is not None:
        body['next_stage'] = next_stage
    if task['stage'] == 'face':
        body['reference_input_version'] = task['_reference_input_version']
    # Durable exact body allows recovery to replay the SAME operation after a lost response.
    directory = attempt_directory(base, task)
    seal_artifacts(directory)
    pending = directory / 'publication.json'
    encoded = json.dumps(body, ensure_ascii=False, allow_nan=False, sort_keys=True)
    if pending.exists() and pending.read_text() != encoded:
        raise PublicationError('同一次执行的发布内容已变化', 409, body['operation_id'])
    if not pending.exists():
        atomic_json(pending, encoded)
    result = native('processing-result', body)
    if result.get('ok') is not True:
        raise PublicationError('原生结果未确认成功，等待原操作回执', operation_id=body['operation_id'], uncertain=True)
    atomic_json(directory / 'receipt.json', json.dumps(result))
    return result


def recover_publications(base):
    """Resolve uncertain writes before permitting coordinator recovery/new attempts."""
    for pending in sorted((Path(base) / 'processing/attempts').glob('*/publication.json')):
        receipt = pending.with_name('receipt.json')
        if receipt.exists():
            continue
        body = json.loads(pending.read_text())
        try:
            result = native('processing-result', body)
        except PublicationError as error:
            if error.status != 409:
                raise
            result = {'ok': False, 'id': body['operation_id'], 'rejected': str(error)}
        atomic_json(receipt, json.dumps(result))


def atomic_json(target, encoded):
    fd, temporary = tempfile.mkstemp(prefix='.receipt-', dir=target.parent)
    try:
        with os.fdopen(fd, 'w') as out:
            out.write(encoded); out.flush(); os.fsync(out.fileno())
        os.replace(temporary, target)
        directory = os.open(target.parent, os.O_RDONLY)
        try: os.fsync(directory)
        finally: os.close(directory)
    finally:
        if os.path.exists(temporary): os.unlink(temporary)


def seal_artifacts(directory):
    """A durable result must not point at image/cache bytes still only in page cache."""
    for item in directory.rglob('*'):
        if item.is_symlink():
            raise PublicationError('执行素材目录包含无效链接', 409)
        if item.is_file():
            with item.open('rb') as source: os.fsync(source.fileno())
    for item in [p for p in directory.rglob('*') if p.is_dir()] + [directory]:
        handle = os.open(item, os.O_RDONLY)
        try: os.fsync(handle)
        finally: os.close(handle)
