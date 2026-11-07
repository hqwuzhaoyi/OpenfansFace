"""Automatic media discovery, text evidence, conditional face analysis and review readiness."""
import argparse
import datetime as dt
import fcntl
import hashlib
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
import uuid

from processing_store import connect, setting, set_setting, event, reconcile, claim, finish, fail, recover
from processing_logic import folder_hints, candidates_from_text, can_skip_face, resolve_candidates
from processing_faces import mapped, build_references, analyze, canonical_person, FACE_VERSION
from watermark_worker import PIPELINE, PaddleEngine, read_json, publish, scan_video, same_file, validate_source, now


def media_available(con, root):
    """A shared mount outage must not spend each video's retry budget."""
    try:
        root.stat()
        with os.scandir(root) as entries:
            next(entries, None)
    except OSError as error:
        with con:
            if not setting(con, 'media_unavailable', False):
                event(con, None, 'media_unavailable', '媒体挂载不可读，暂停领取任务：' + str(error)[:350])
                set_setting(con, 'media_unavailable', dict(at=time.time(), error=str(error)[:350]))
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
        faces = sqlite3.connect(base / 'faces.db', timeout=15)
        try:
            sources = dict(faces.execute('SELECT vid,path FROM videos'))
            seen_paths = {mapped(p, root) or mapped(vid, root) for vid, p in sources.items()}
            columns = {row[1] for row in faces.execute('PRAGMA table_info(videos)')}
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
                record = dict(vid=asset['path'], path=path, name=item['name'], size=item['size'], duration=asset.get('duration'),
                              zone='discovered', collection=str(Path(path).parent), status='pipeline')
                keys = [key for key in record if key in columns]
                cursor = faces.execute(f"INSERT INTO videos({','.join(keys)}) VALUES ({','.join('?' for _ in keys)}) ON CONFLICT(vid) DO NOTHING", [record[key] for key in keys])
                sources[record['vid']] = path; seen_paths.add(path); registered += cursor.rowcount
            faces.commit()
            sources = dict(faces.execute('SELECT vid,path FROM videos'))
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


def text_stage(task, base, root):
    progress = stage_progress(task, base)
    progress.update(stage="检查文字缓存")
    validate_source(task, root)
    directory = base / 'watermarks'; directory.mkdir(exist_ok=True)
    records = read_json(directory / 'results.json', {})
    existing = records.get(task['vid'], {})
    if existing.get('status') == 'done' and existing.get('pipeline') == PIPELINE and same_file(existing, task):
        return existing
    progress.update(stage='加载文字模型', done=None, total=None)
    result = scan_video(task, root, directory, PaddleEngine(), 'automatic-' + hashlib.sha256((task['vid'] + task['mtime_ns']).encode()).hexdigest()[:16], progress=progress)
    record = {key: task[key] for key in ('vid', 'path', 'size', 'mtime_ns')}
    record.update(result, status='done', pipeline=PIPELINE, job_id=task['batch_id'], checked_at=now())
    records[task['vid']] = record
    publish(directory / 'results.json', records)
    return record


def stage_subprocess(task, base, root):
    folder = base / 'processing/tmp'; folder.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(dir=folder, prefix='stage-') as tmp:
        manifest, result = Path(tmp) / 'task.json', Path(tmp) / 'result.json'
        manifest.write_text(json.dumps(task, ensure_ascii=False))
        command = [sys.executable, str(Path(__file__).resolve()), '--base', str(base), '--root', str(root),
                   '--stage', task['stage'], '--task', str(manifest), '--result', str(result)]
        process = subprocess.run(command, capture_output=True, text=True, timeout=600)
        if process.returncode or not result.exists():
            raise RuntimeError(('文字提取失败：' if task['stage'] == 'text' else '人脸分析失败：') + (process.stderr or process.stdout)[-450:])
        return json.loads(result.read_text())


def process_one(base, root, runner=stage_subprocess):
    con = connect(base)
    try:
        if not media_available(con, root):
            return False
        task = claim(con)
        if not task:
            return False
        task["_progress_id"] = str(uuid.uuid4())
        progress = stage_progress(task, base, attach=False)
        try:
            resolved = {r['vid'] for r in read_json(base / 'integration-queue.json', []) + read_json(base / 'archive-history.json', [])}
            if task['vid'] in resolved:
                with con:
                    con.execute("UPDATE tasks SET stage='excluded',updated_at=? WHERE vid=?", (time.time(), task['vid']))
                progress.finish()
                return True
            validate_source(task, root)
            data = json.loads(task['data'])
            if task['stage'] == 'text':
                watermark = runner(task, base, root)
                folder = folder_hints(task['path'], catalog(base))
                mapping = read_json(base / 'watermarks/mappings.json', {})
                text_candidates = candidates_from_text(watermark, mapping, folder)
                legacy = [p['name'] for p in read_json(base / 'report.json', {}).get('identities', []) if any(v['vid'] == task['vid'] for v in p.get('vids', []))]
                skip = can_skip_face(text_candidates, legacy)
                data.update(watermark=watermark, folder=folder, text_candidates=text_candidates, text_complete=True,
                            face_skipped=skip, ready=skip, **resolve_candidates(text_candidates))
                next_stage = 'review' if skip else 'face_queued'
            else:
                face = runner(task, base, root)
                data.update(face=face, face_complete=True, ready=True, **resolve_candidates(data.get('text_candidates', []) + face['matches']))
                next_stage = 'review'
            validate_source(task, root)
            finish(con, task, data, next_stage)
            progress.finish()
        except Exception as error:
            if media_available(con, root):
                fail(con, task, error)
            else:
                field = 'text_attempts' if task['stage'] == 'text' else 'face_attempts'
                with con:
                    con.execute(f"""UPDATE tasks SET stage='retry',next_retry=0,{field}=MAX(0,{field}-1),
                        error='媒体挂载不可读，等待恢复',updated_at=?
                        WHERE vid=? AND stage=? AND size=? AND mtime_ns=?""",
                        (time.time(), task['vid'], task['stage'], task['size'], task['mtime_ns']))
            progress.finish(error)
        return True
    finally:
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
        result = text_stage(task, base, root) if args.stage == 'text' else analyze(task, base, root)
        publish(args.result, result)
        return
    folder = base / 'processing'; folder.mkdir(exist_ok=True)
    with (folder / '.worker.lock').open('w') as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise SystemExit('Processing coordinator already running')
        con = connect(base); recover(con); con.close()
        stop = threading.Event()
        def heartbeats():
            while not stop.is_set():
                publish(folder / 'heartbeat.json', dict(at=time.time(), pid=os.getpid(), text_pipeline=PIPELINE))
                stop.wait(10)
        def discovery():
            due = 0
            while not stop.is_set():
                state = connect(base)
                requested = setting(state, 'discovery_requested', 0)
                state.close()
                if time.time() >= due or requested > due - 300:
                    try:
                        discover(base, root, args.api_url)
                    except Exception as error:
                        print('Discovery:', str(error), flush=True)
                    due = time.time() + 300
                stop.wait(5)
        heartbeat = threading.Thread(target=heartbeats, daemon=True); heartbeat.start()
        if args.once:
            discover(base, root, args.api_url)
            process_one(base, root)
            stop.set(); heartbeat.join(timeout=12)
            return
        discover_thread = threading.Thread(target=discovery, daemon=True); discover_thread.start()
        try:
            while True:
                if not process_one(base, root):
                    time.sleep(2)
        finally:
            stop.set(); heartbeat.join(timeout=12)
            publish(folder / 'heartbeat.json', dict(at=0))


if __name__ == '__main__':
    main()
