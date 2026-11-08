"""Automatic media discovery, text evidence, conditional face analysis and review readiness."""
import argparse
from concurrent.futures import ThreadPoolExecutor
import signal
import datetime as dt
import fcntl
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request
from task_progress import TaskProgress, stage_progress

from processing_store import connect, setting, set_setting, event, reconcile, claim, fail, recover, release_task, effective_concurrency
from processing_publication import prepare, publish_result, recover_publications, attempt_directory, PublicationError, native
from processing_gpu import inference_status
from processing_faces import mapped, build_references, analyze, canonical_person, FACE_VERSION, MODEL_HASHES, capture_references
from processing_compute import RUNTIME, compute_write_guard, get_resident_client, close_resident_client
from watermark_worker import PIPELINE, PaddleEngine, read_json, publish, scan_video, same_file, validate_source, now


_ACTIVE_RUN_IDS = set()
_ACTIVE_RUN_LOCK = threading.Lock()


def media_available(con, root):
    """A shared mount outage must not spend each video's retry budget."""
    ok = True
    detail = None
    try:
        root.stat()
        with os.scandir(root) as entries:
            next(entries, None)
    except OSError as error:
        ok = False
        detail = str(error)[:350]
    if os.environ.get('FILE_OPERATIONS_RUST_URL'):
        try:
            resp = native('processing-media-status', {
                'available': bool(ok),
                'root': str(root),
                'error': detail,
            }, timeout=5)
            if isinstance(resp, dict) and 'available' in resp:
                return bool(resp['available'])
        except Exception:
            pass
        return ok
    if not ok:
        with con:
            if not setting(con, 'media_unavailable', False):
                event(con, None, 'media_unavailable', '媒体挂载不可读，暂停领取任务：' + detail)
                set_setting(con, 'media_unavailable', dict(at=time.time(), error=detail))
        return False
    if setting(con, 'media_unavailable', False):
        with con:
            set_setting(con, 'media_unavailable', False)
            event(con, None, 'media_recovered', '媒体挂载恢复，继续自动分析')
    return True


def catalog(base):
    report = read_json(base / 'report.json', {})
    names = {p['name'] for p in report.get('identities', [])}
    names.update(p['name'] for p in read_json(base / 'people-manual.json', []))
    names.update(v['creator'] for v in read_json(base / 'integration-queue.json', []) + read_json(base / 'archive-history.json', []))
    renames = read_json(base / 'person-renames.json', {})
    return sorted({canonical_person(name, renames) for name in names})


def handled(base, root, sources):
    queue = read_json(base / 'integration-queue.json', [])
    records = queue + read_json(base / 'archive-history.json', [])
    ids = {r['vid'] for r in records}
    paths = {mapped(sources.get(r['vid']), root) or mapped(r['vid'], root) for r in queue}
    return ids, paths


def source_item(vid, stored, root):
    for value in dict.fromkeys([stored, vid]):
        path = mapped(value, root)
        if not path:
            continue
        try:
            file = Path(path); stat = file.stat()
            real = file.resolve()
            if not file.is_file() or not stat.st_size or not real.is_relative_to(root.resolve()):
                continue
            if real.is_relative_to((root / 'pikpak/creators').resolve()) or '.face-cleanup-staging' in real.parts:
                continue
            return dict(vid=vid, path=path, name=file.name, size=stat.st_size, mtime_ns=str(stat.st_mtime_ns))
        except OSError:
            continue
    return None


def discover(base, root, api_url, assets=None):
    if os.environ.get('FILE_OPERATIONS_RUST_URL'):
        references = build_references(base, root)
        payload = {'pipeline': PIPELINE, 'face_version': FACE_VERSION, 'references': references}
        if assets is not None:
            payload['assets'] = list(assets)
        return native('processing-discover', payload, timeout=120)['eligible']
    progress = TaskProgress(base, "discovery", "媒体发现")
    progress.update(stage="读取 Peach 账本")
    con = connect(base)
    try:
        if not media_available(con, root):
            raise RuntimeError('媒体挂载不可读，保留原任务')
        if assets is None:
            with urllib.request.urlopen(api_url.rstrip('/') + '/api/processing/inventory', timeout=45) as response:
                assets = json.load(response)['assets']
        progress.update(stage='登记新文件', done=0, total=len(assets), unit='条记录')
        faces = sqlite3.connect('file:' + str(base / 'faces.db') + '?mode=ro', uri=True, timeout=15)
        try:
            sources = dict(faces.execute('SELECT vid,path FROM videos'))
            seen_paths = {mapped(p, root) or mapped(vid, root) for vid, p in sources.items()}
            registered = 0
            for asset_index, asset in enumerate(assets):
                if asset_index % 100 == 0:
                    progress.update(done=asset_index, current=asset.get("name") or Path(asset.get("path", "")).name)
                if asset.get('disposal') is not None:
                    continue
                path = mapped(asset.get('path'), root)
                if not path or path in seen_paths or asset['path'] in sources:
                    continue
                item = source_item(asset['path'], path, root)
                if not item:
                    continue
                # Discovery owns only the analysis queue. Native business writers own videos.
                sources[asset['path']] = path; seen_paths.add(path); registered += 1
        finally:
            faces.close()
        resolved, resolved_paths = handled(base, root, sources)
        disposed = {mapped(a.get('path'), root) for a in assets if a.get('disposal') is not None}
        active_paths = {mapped(a.get('path'), root) for a in assets if a.get('disposal') is None}
        disposed -= active_paths
        health = read_json(base / 'video-health.json', {})
        try:
            fresh_health = time.time() - dt.datetime.fromisoformat(health.get('last_success_at', '')).timestamp() < 7 * 86400
        except ValueError:
            fresh_health = False
        items, paths = [], set()
        progress.update(stage="核对媒体文件", done=0, total=len(sources), unit="个文件")
        for file_index, (vid, stored) in enumerate(sorted(sources.items())):
            if file_index % 50 == 0:
                progress.update(done=file_index, current=Path(str(stored or vid)).name)
            if vid in resolved:
                continue
            item = source_item(vid, stored, root)
            if not item or item['path'] in paths | resolved_paths | disposed:
                continue
            checked = health.get('records', {}).get(vid, {})
            if fresh_health and checked.get('hidden') and same_file(checked, item):
                continue
            paths.add(item['path']); items.append(item)
        bootstrapped = setting(con, 'bootstrapped', False)
        expected = sum(1 for row in con.execute("SELECT vid,path FROM tasks WHERE stage!='excluded'")
                       if row['vid'] not in resolved and row['path'] not in resolved_paths | disposed)
        if expected >= 20 and len(items) < expected * .2:
            raise RuntimeError('媒体可读数量突然下降，本轮保留原任务，请检查媒体挂载')
        batch = dt.datetime.now(dt.timezone.utc).strftime('%Y%m%dT%H%M%SZ')
        progress.update(stage="更新人物参考", done=None, total=None, unit="", current=None)
        references = build_references(base, root)
        with con:
            count = reconcile(con, items, batch, bootstrapped)
            con.execute("""UPDATE tasks SET stage='queued',retry_stage='text',text_attempts=0,face_attempts=0,next_retry=0,
                updated_at=? WHERE stage='review' AND manual=0 AND json_extract(data,'$.watermark.pipeline')<>?""", (time.time(), PIPELINE))
            con.execute("""UPDATE tasks SET stage='face_queued',retry_stage='face',face_attempts=0,next_retry=0,
                updated_at=? WHERE stage='review' AND manual=0 AND (json_extract(data,'$.face.reference_version')<>? OR json_extract(data,'$.face.version')<>?)""", (time.time(), references['version'], FACE_VERSION))
            set_setting(con, 'bootstrapped', True)
            set_setting(con, 'last_discovery', dict(at=time.time(), registered=registered, eligible=count, source_count=len(assets), error=None))
            set_setting(con, 'references', references)
        progress.update(stage="发现完成", done=count, total=count, unit="个可处理视频", current=None)
        progress.finish()
        return count
    except Exception as error:
        progress.finish(error)
        with con:
            previous = setting(con, 'last_discovery', {})
            set_setting(con, 'last_discovery', {**previous, 'error': str(error)[:500], 'attempted_at': time.time()})
            event(con, None, 'discovery_error', str(error)[:500])
        raise
    finally:
        con.close()


def max_concurrency():
    try:
        return max(1, min(2, int(os.environ.get('PROCESSING_MAX_CONCURRENCY', '1'))))
    except ValueError:
        return 1


def was_handled(base, vid):
    return any(r['vid'] == vid for r in read_json(base / 'integration-queue.json', []) + read_json(base / 'archive-history.json', []))


def text_stage(task, base, root):
    progress = stage_progress(task, base)
    progress.update(stage="检查文字缓存")
    validate_source(task, root)
    directory = attempt_directory(base, task) / 'watermarks'; directory.mkdir(exist_ok=True)
    cache_check = task.get('_cache_check')
    if isinstance(cache_check, dict):
        if cache_check.get('text_valid') is True and isinstance(cache_check.get('text_cached'), dict):
            return cache_check['text_cached']
    else:
        records = read_json(base / 'watermarks/results.json', {})
        existing = records.get(task['vid'], {})
        if existing.get('status') == 'done' and existing.get('pipeline') == PIPELINE and same_file(existing, task):
            return existing
    progress.update(stage='加载文字模型', done=None, total=None)
    engine = RUNTIME.get_ocr_engine(PaddleEngine)
    result = scan_video(task, root, directory, engine, task['run_id'], progress=progress)
    record = {key: task[key] for key in ('vid', 'path', 'size', 'mtime_ns')}
    record.update(
        result,
        status='done',
        pipeline=PIPELINE,
        job_id=task['batch_id'],
        checked_at=now(),
        artifact_attempt=task['run_id'],
        execution={
            'backend': 'cpu',
            'stage': 'ocr',
            'pipeline': PIPELINE,
            'resident_load_count': RUNTIME.ocr_load_count,
            'resident_tasks_run': RUNTIME.ocr_tasks_run,
        },
    )
    return record


def stage_subprocess(task, base, root):
    if os.environ.get('PROCESSING_COMPUTE_MODE') != 'subprocess':
        return get_resident_client().run_stage(task, base, root)
    folder = base / 'processing/tmp'; folder.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(dir=folder, prefix='stage-') as tmp:
        manifest, result = Path(tmp) / 'task.json', Path(tmp) / 'result.json'
        manifest.write_text(json.dumps({**task, '_stage_deadline_ms': int((time.time() + 600) * 1000)}, ensure_ascii=False))
        command = [sys.executable, str(Path(__file__).resolve()), '--base', str(base), '--root', str(root),
                   '--stage', task['stage'], '--task', str(manifest), '--result', str(result)]
        process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, start_new_session=True)
        try:
            stdout, stderr = process.communicate(timeout=600)
        except BaseException:
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGKILL)
                process.communicate()
            raise
        if process.returncode or not result.exists():
            raise RuntimeError(('文字提取失败：' if task['stage'] == 'text' else '人脸分析失败：') + (stderr or stdout)[-450:])
        return json.loads(result.read_text())


def process_one(base, root, runner=stage_subprocess, capacity=1, publisher=publish_result, preparer=prepare, reference_loader=capture_references):
    con = None if os.environ.get('FILE_OPERATIONS_RUST_URL') else connect(base)
    try:
        if not media_available(con, root):
            return False
        task = claim(con, capacity=capacity)
        if not task:
            return False
        run_id = task.get('run_id')
        if run_id:
            with _ACTIVE_RUN_LOCK:
                _ACTIVE_RUN_IDS.add(run_id)
        task["_progress_id"] = task["run_id"]
        progress = None
        try:
            progress = stage_progress(task, base, attach=False)
            if was_handled(base, task['vid']):
                release_task(con, task, '已由人工处理', excluded=True)
                progress.finish()
                return True
            validate_source(task, root)
            if os.environ.get('FILE_OPERATIONS_RUST_URL'):
                try:
                    cache_info = native('processing-cache-check', {
                        'stage': task['stage'],
                        'vid': task['vid'],
                        'path': task['path'],
                        'size': task['size'],
                        'mtime_ns': str(task['mtime_ns']),
                        'pipeline': PIPELINE,
                        'face_version': FACE_VERSION,
                        'model_hashes': MODEL_HASHES,
                    })
                    if isinstance(cache_info, dict) and 'text_valid' in cache_info:
                        task['_cache_check'] = cache_info
                        if isinstance(cache_info.get('active_face_groups'), list):
                            task['_active_face_groups'] = cache_info['active_face_groups']
                except Exception:
                    pass
            data = json.loads(task['data'])
            people = catalog(base)
            mapping = read_json(base / 'watermarks/mappings.json', {})
            keys = set(mapping) | {c['key'] for c in data.get('watermark', {}).get('candidates', [])}
            preparer(task, base, people, keys)
            if task['stage'] == 'face':
                reference_loader(task, base, root)
            if task['stage'] == 'text':
                watermark = runner(task, base, root)
                raw = {'watermark': watermark}
            else:
                face = runner(task, base, root)
                raw = {'face': face}
            if was_handled(base, task['vid']):
                release_task(con, task, '分析期间已由人工处理', excluded=True)
                progress.finish()
                return True
            validate_source(task, root)
            publisher(task, base, raw, None)
            if os.environ.get('FILE_OPERATIONS_RUST_URL'):
                try:
                    native('processing-evict-artifacts', {'tmp_max_age_sec': 3600})
                except Exception:
                    pass
            progress.finish()
        except Exception as error:
            if isinstance(error, PublicationError) and error.uncertain:
                # Keep active attempt fenced until exact receipt replay resolves it.
                if os.environ.get('FILE_OPERATIONS_RUST_URL'):
                    try:
                        native('processing-event', {
                            'vid': task['vid'],
                            'kind': 'publication_unknown',
                            'message': str(error),
                            'detail': str(error),
                        })
                    except Exception:
                        pass
                else:
                    with con:
                        event(con, task['vid'], 'publication_unknown', str(error))
            elif isinstance(error, PublicationError) and error.status >= 500 and not error.operation_id:
                release_task(con, task, '原生协调器不可用，等待恢复', retry_delay=30)
            elif was_handled(base, task['vid']):
                release_task(con, task, '分析期间已由人工处理', excluded=True)
            elif media_available(con, root):
                fail(con, task, error)
            else:
                release_task(con, task, '媒体挂载不可读，等待恢复')
            if progress:
                progress.finish(error)
        finally:
            if run_id:
                with _ACTIVE_RUN_LOCK:
                    _ACTIVE_RUN_IDS.discard(run_id)
        return True
    finally:
        if con is not None:
            con.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--base', type=Path, default=Path(os.environ.get('FACES_BASE', '/data/base')))
    parser.add_argument('--root', type=Path, default=Path(os.environ.get('MEDIA2_ROOT', '/mnt/media2')))
    parser.add_argument('--api-url', default=os.environ.get('FACE_ADMIN_URL', 'http://127.0.0.1:8919'))
    parser.add_argument('--stage', choices=['text', 'face'])
    parser.add_argument('--task', type=Path); parser.add_argument('--result', type=Path)
    parser.add_argument('--once', action='store_true')
    args = parser.parse_args(); base, root = args.base, args.root
    if args.stage:
        task = json.loads(args.task.read_text())
        with compute_write_guard(base, root, run_id=task.get('run_id')):
            result = text_stage(task, base, root) if args.stage == 'text' else analyze(task, base, root)
        publish(args.result, result)
        return
    if not os.environ.get('FILE_OPERATIONS_RUST_URL'):
        raise SystemExit('原生结果协调器未配置，尚未领取任务')
    folder = base / 'processing'; folder.mkdir(exist_ok=True)
    with (folder / '.worker.lock').open('w') as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise SystemExit('Processing coordinator already running')
        stop = threading.Event()
        native_ready = threading.Event()
        shared_state = {'desired': max_concurrency(), 'paused': False, 'active': 0, 'discovery_requested': 0.0}
        def heartbeats():
            while not stop.is_set():
                if not native_ready.is_set():
                    # Closed-admission bootstrap must be observable without opening
                    # SQLite, recovering attempts, discovering or reading references.
                    publish(folder / 'heartbeat.json', dict(at=time.time(), pid=os.getpid(),
                            text_pipeline=PIPELINE, protocol=3, max_concurrency=max_concurrency(),
                            concurrency=0, active=0, waiting_native_admission=True, native_ready=False))
                else:
                    try:
                        with _ACTIVE_RUN_LOCK:
                            active_run_ids = sorted(_ACTIVE_RUN_IDS)
                        hb_resp = native('processing-heartbeat', {
                            'run_ids': active_run_ids,
                            'capacity': max_concurrency(),
                            'heartbeat': dict(
                                pid=os.getpid(),
                                text_pipeline=PIPELINE,
                                protocol=3,
                                max_concurrency=max_concurrency(),
                                waiting_native_admission=False,
                                native_ready=True,
                                inference=inference_status(),
                            ),
                        }, timeout=10)
                        if isinstance(hb_resp, dict):
                            shared_state['desired'] = int(hb_resp.get('concurrency', max_concurrency()))
                            shared_state['paused'] = bool(hb_resp.get('paused', False))
                            shared_state['active'] = int(hb_resp.get('active', 0))
                            shared_state['discovery_requested'] = float(hb_resp.get('discovery_requested', 0.0))
                    except Exception:
                        publish(folder / 'heartbeat.json', dict(at=time.time(), pid=os.getpid(), text_pipeline=PIPELINE,
                                protocol=3, max_concurrency=max_concurrency(), concurrency=shared_state['desired'], active=shared_state['active'],
                                waiting_native_admission=False, native_ready=True, inference=inference_status()))
                stop.wait(10 if native_ready.is_set() else 1)
        def discovery():
            due = 0
            while not stop.is_set():
                requested = shared_state.get('discovery_requested', 0.0)
                if time.time() >= due or requested > due - 300:
                    try:
                        discover(base, root, args.api_url)
                        native('processing-evict-artifacts', {'tmp_max_age_sec': 3600})
                    except Exception as error:
                        print('Discovery:', str(error), flush=True)
                    due = time.time() + 300
                stop.wait(5)
        signal.signal(signal.SIGTERM, lambda *_: stop.set())
        signal.signal(signal.SIGINT, lambda *_: stop.set())
        heartbeat = threading.Thread(target=heartbeats, daemon=True); heartbeat.start()
        try:
            while not stop.is_set():
                try:
                    native('state', {'scope': {}}, timeout=5)
                    break
                except PublicationError as error:
                    if error.status != 503:
                        raise
                    # Admission closure/unavailability is a wait, not readiness.
                    # Authentication and other contract errors still fail closed.
                    stop.wait(2)
            if stop.is_set():
                return
            recover_publications(base)
            recover(None)
            native_ready.set()
            if args.once:
                discover(base, root, args.api_url)
                process_one(base, root)
                return
            discover_thread = threading.Thread(target=discovery, daemon=True); discover_thread.start()
            with ThreadPoolExecutor(max_workers=max_concurrency(), thread_name_prefix='processing') as executor:
                inflight = set()
                while not stop.is_set():
                    for future in list(inflight):
                        if future.done():
                            inflight.remove(future)
                            try:
                                future.result()
                            except Exception as error:
                                print('Coordinator stage:', str(error), flush=True)
                    if not inflight and shared_state.get('active', 0) > 0:
                        recover_publications(base)
                        recover(None)
                    desired = shared_state.get('desired', max_concurrency())
                    paused = shared_state.get('paused', False)
                    if not paused:
                        while len(inflight) < desired:
                            inflight.add(executor.submit(process_one, base, root, capacity=max_concurrency()))
                    stop.wait(1)
        finally:
            stop.set(); heartbeat.join(timeout=12)
            close_resident_client()
            publish(folder / 'heartbeat.json', dict(at=0, protocol=3, max_concurrency=max_concurrency(),
                    waiting_native_admission=False, native_ready=False, active=0))



if __name__ == '__main__':
    main()
