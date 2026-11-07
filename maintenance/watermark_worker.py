"""Read-only video OCR worker. Shares FACES_BASE and MEDIA2_ROOT with the web app."""
import argparse
import datetime as dt
import fcntl
import hashlib
import json
import math
import os
from pathlib import Path
import re
import subprocess
import tempfile
import threading
import time
import unicodedata

PIPELINE = 'paddleocr-3.3.2-ppocrv5-mobile-watermark-v2'


def now():
    return dt.datetime.now(dt.timezone.utc).isoformat()


def read_json(path, fallback):
    try:
        return json.loads(path.read_text())
    except FileNotFoundError:
        return fallback


def native_results_owned():
    return os.environ.get('FILE_OPERATIONS_BACKEND') == 'rust' or bool(os.environ.get('FILE_OPERATIONS_RUST_URL'))


def publish(path, data):
    if path.name == 'results.json' and path.parent.name == 'watermarks' and native_results_owned():
        raise RuntimeError('共享水印结果由原生协调器管理；请使用自动处理队列')
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(prefix='.' + path.name, dir=path.parent)
    try:
        with os.fdopen(fd, 'w') as out:
            json.dump(data, out, ensure_ascii=False, allow_nan=False)
            out.flush()
            os.fsync(out.fileno())
        os.chmod(tmp, 0o644)
        os.replace(tmp, path)
    finally:
        if os.path.exists(tmp):
            os.unlink(tmp)


def fingerprint(file):
    stat = file.stat()
    return dict(path=str(file), size=stat.st_size, mtime_ns=str(stat.st_mtime_ns))


def same_file(record, source):
    return all(record.get(k) == source.get(k) for k in ('path', 'size', 'mtime_ns'))


def validate_source(item, root):
    file = Path(item['path'])
    resolved = file.resolve(strict=True)
    if not resolved.is_relative_to(root.resolve()) or resolved.is_relative_to((root / 'pikpak/creators').resolve()):
        raise ValueError('视频已归档或不在媒体目录中')
    if not file.is_file() or not same_file(item, fingerprint(file)) or not item['size']:
        raise ValueError('视频文件已变化，请刷新列表后重试')
    return file


def sample_times(duration, count=8):
    if not math.isfinite(duration) or duration <= 0:
        raise ValueError('无法取得有效视频时长')
    end = max(0, duration - min(.1, duration / 10))
    return sorted(set(round(min(end, duration * (.02 + .94 * i / (count - 1))), 3) for i in range(count)))


def text_identity(text):
    """Preserve punctuation and unknown-platform case; never fuzzy-merge accounts."""
    text = unicodedata.normalize('NFKC', text).strip()
    text = re.sub(r'\s+', ' ', text)
    if len(text) < 3 or len(text) > 100 or not re.search(r'[\w\u4e00-\u9fff]', text):
        return None
    if re.fullmatch(r'[\d\s:./%-]+', text):
        return None
    platform, token, kind = 'unknown', text, 'text'
    # Platform URLs and TG-prefixed handles are common in real video watermarks.
    # Recognizing an account never establishes that it is the person in the video.
    link = re.search(r'(?<![\w.])(onlyfans\.com|fansly\.com|twitter\.com|x\.com|t\.me|patreon\.com|instagram\.com)/([A-Za-z0-9_][A-Za-z0-9_.-]{2,63})(?![\w.-])', text, re.I)
    telegram = re.search(r'(?<!\w)(?:TG|Telegram|电报)\s*[:：]?\s*@([A-Za-z0-9_]{3,64})', text, re.I)
    prefixed = re.search(r'(?<!\w)(OnlyFans|Fansly|Twitter|X|Instagram|IG)\s*[:：]?\s*@([A-Za-z0-9_][A-Za-z0-9_.-]{2,63})', text, re.I)
    match = re.search(r'(抖音号|快手号|小红书号|小红书ID|TikTok)\s*[:：]?\s*@?([\w.-]{3,64})', text, re.I)
    if link:
        platform = {'onlyfans.com': 'OnlyFans', 'fansly.com': 'Fansly', 'twitter.com': 'X', 'x.com': 'X', 't.me': 'Telegram', 'patreon.com': 'Patreon', 'instagram.com': 'Instagram'}[link[1].lower()]
        token, kind = link[2], 'account'
    elif telegram:
        platform, token, kind = 'Telegram', telegram[1], 'account'
    elif prefixed:
        platform = {'onlyfans': 'OnlyFans', 'fansly': 'Fansly', 'twitter': 'X', 'x': 'X', 'instagram': 'Instagram', 'ig': 'Instagram'}[prefixed[1].lower()]
        token, kind = prefixed[2], 'account'
    elif match:
        platform = {'抖音号': '抖音', '快手号': '快手', '小红书号': '小红书', '小红书id': '小红书', 'tiktok': 'TikTok'}[match[1].lower()]
        token, kind = match[2], 'account'
    else:
        match = re.fullmatch(r'@\s*([\w.-]{3,64})', text)
        if match:
            token, kind = '@' + match[1], 'account'
    key = hashlib.sha256((platform + '\0' + token).encode()).hexdigest()
    return dict(key=key, text=text, token=token, platform=platform, kind=kind)


def aggregate(observations):
    groups = {}
    for observation in observations:
        identity = text_identity(observation['text'])
        score = float(observation['score'])
        if not identity or not math.isfinite(score) or score < .60:
            continue
        key = identity['key']
        entry = groups.setdefault(key, dict(identity, seen=set(), times=set(), best=observation))
        entry['seen'].add(observation['frame'])
        entry['times'].add(observation['timestamp'])
        if score > entry['best']['score']:
            entry['best'] = observation
    # Multiple crops from one frame are one vote, not independent evidence.
    candidates = []
    for entry in groups.values():
        if len(entry['seen']) < 2:
            continue
        best = entry.pop('best')
        frames = len(entry.pop('seen'))
        timestamps = sorted(entry.pop('times'))
        candidates.append(dict(entry, frames=frames, score=round(float(best['score']), 4),
                               timestamp=best['timestamp'], evidence=best['evidence'], timestamps=timestamps,
                               box=best.get('source_box', []), frame_size=best.get('frame_size', [])))
    return sorted(candidates, key=lambda c: (c['kind'] != 'account', -c['frames'], -c['score']))[:24]


class PaddleEngine:
    def __init__(self):
        from paddleocr import PaddleOCR
        self.ocr = PaddleOCR(
            text_detection_model_name='PP-OCRv5_mobile_det',
            text_recognition_model_name='PP-OCRv5_mobile_rec',
            use_doc_orientation_classify=False, use_doc_unwarping=False,
            use_textline_orientation=False, device='cpu', enable_mkldnn=False,
            cpu_threads=max(1, int(os.environ.get('WATERMARK_CPU_THREADS', '2'))),
        )

    def recognize(self, image):
        import numpy as np
        for result in self.ocr.predict(np.asarray(image)[:, :, ::-1].copy()):
            data = result.json['res']
            for text, score, box in zip(data['rec_texts'], data['rec_scores'], data['rec_boxes']):
                yield dict(text=text, score=float(score), box=[int(v) for v in box])


def scan_video(item, root, directory, engine, job_id, progress=None):
    from PIL import Image
    file = validate_source(item, root)
    probe = subprocess.run(['ffprobe', '-v', 'error', '-show_entries', 'format=duration', '-of', 'json', str(file)],
                           capture_output=True, text=True, timeout=30, check=True)
    duration = float(json.loads(probe.stdout)['format']['duration'])
    observations, successes, best_scores = [], 0, {}
    evidence_dir = directory / 'evidence'
    evidence_dir.mkdir(exist_ok=True)
    prefix = hashlib.sha256((item['vid'] + job_id).encode()).hexdigest()
    with tempfile.TemporaryDirectory(prefix='face-watermark-') as temp:
        times = sample_times(duration)
        for index, timestamp in enumerate(times):
            if progress:
                progress.update(stage="识别视频帧", done=index, total=len(times), unit="帧")
            frame = Path(temp) / 'frame.png'
            # ffmpeg can exit successfully without producing a frame near EOF.
            # Never count the previous timestamp's image as fresh evidence.
            frame.unlink(missing_ok=True)
            try:
                subprocess.run(['ffmpeg', '-nostdin', '-v', 'error', '-y', '-ss', str(timestamp), '-i', str(file),
                                '-frames:v', '1', '-vf', r'scale=w=min(1920\,iw):h=-2', str(frame)],
                               capture_output=True, timeout=40, check=True)
                with Image.open(frame) as opened:
                    original = opened.convert('RGB')
            except (subprocess.SubprocessError, OSError):
                continue
            successes += 1
            width, height = original.size
            regions = [(0, 0, width, height)]
            # Full-frame detection covers central/moving marks; enlarged corners recover small text.
            regions += [(x, y, x + width // 2, y + height // 3)
                        for x in (0, width - width // 2) for y in (0, height - height // 3)]
            for region in regions:
                crop = original.crop(region)
                scale = min(2 if region != regions[0] else 1, 1600 / max(crop.size))
                crop = crop.resize((max(1, round(crop.width * scale)), max(1, round(crop.height * scale))))
                for result in engine.recognize(crop):
                    identity = text_identity(result['text'])
                    if not identity or result['score'] < .60:
                        continue
                    key = identity['key']
                    filename = f'{prefix}-{key}.jpg'
                    if result['score'] > best_scores.get(key, 0):
                        x1, y1, x2, y2 = result['box']
                        box = (max(0, x1 - 18), max(0, y1 - 18), min(crop.width, x2 + 18), min(crop.height, y2 + 18))
                        if box[2] <= box[0] or box[3] <= box[1]:
                            continue
                        evidence = crop.crop(box)
                        evidence.thumbnail((800, 400))
                        evidence.save(evidence_dir / filename, quality=90)
                        best_scores[key] = result['score']
                    x1, y1, x2, y2 = result['box']
                    source_box = [round(region[0] + x1 / scale), round(region[1] + y1 / scale),
                                  round(region[0] + x2 / scale), round(region[1] + y2 / scale)]
                    observations.append(dict(result, frame=index, timestamp=timestamp, evidence=filename,
                                             source_box=source_box, frame_size=[width, height]))
    if progress:
        progress.update(stage="汇总文字线索", done=len(times), total=len(times), unit="帧")
    if successes < 2:
        raise ValueError('有效视频帧不足，无法核对重复水印')
    validate_source(item, root)
    candidates = aggregate(observations)
    retained = {c['evidence'] for c in candidates}
    for evidence in evidence_dir.glob(prefix + '-*.jpg'):
        if evidence.name not in retained:
            evidence.unlink()
    return dict(candidates=candidates, sampled_frames=successes)


def process_job(job, directory, root, engine_factory=PaddleEngine, scanner=scan_video):
    records = read_json(directory / 'results.json', {})
    progress = dict(job_id=job['id'], status='running', processed=0, total=len(job['videos']), failed=0, stage='准备识别')
    publish(directory / 'progress.json', progress)
    engine = None
    try:
        for item in job['videos']:
            before = records.get(item['vid'], {})
            cached = same_file(before, item) and before.get('pipeline') == PIPELINE
            try:
                validate_source(item, root)
            except (OSError, ValueError):
                cached = False
            # A restart resumes the same job, including already-finished failures.
            reuse = cached and (before.get('job_id') == job['id'] or (before.get('status') == 'done' and not job.get('force')))
            if reuse:
                record = dict(before, job_id=job['id'])
            else:
                record = dict(item, pipeline=PIPELINE, job_id=job['id'], checked_at=now(), sampled_frames=0, candidates=[])
                try:
                    validate_source(item, root)
                    if engine is None:
                        progress['stage'] = '加载文字识别模型（首次需要下载）'
                        publish(directory / 'progress.json', progress)
                        # Model setup failure is a job failure, not "no watermark" for every video.
                        engine = engine_factory()
                    progress['stage'] = '识别 ' + Path(item['path']).name
                    publish(directory / 'progress.json', progress)
                    record.update(scanner(item, root, directory, engine, job['id']), status='done')
                except Exception as error:
                    if engine is None and progress['stage'].startswith('加载'):
                        raise
                    record.update(status='error', error=str(error)[:400])
            records[item['vid']] = record
            publish(directory / 'results.json', records)
            retained = {c['evidence'] for c in record['candidates']}
            for candidate in before.get('candidates', []):
                filename = candidate.get('evidence', '')
                if filename not in retained and re.fullmatch(r'[a-f0-9-]+\.jpg', filename):
                    try:
                        (directory / 'evidence' / filename).unlink(missing_ok=True)
                    except OSError:
                        pass  # Evidence cleanup must not fail a successfully persisted scan.
            progress['processed'] += 1
            progress['failed'] += record['status'] == 'error'
            publish(directory / 'progress.json', progress)
        progress.update(status='complete', stage='识别完成')
    except Exception as error:
        progress.update(status='error', error=str(error)[:500])
    publish(directory / 'progress.json', progress)
    return progress


def main():
    if native_results_owned():
        raise SystemExit('Rust 模式下已停用独立水印写入 worker；使用 processing_worker.py')
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--base', type=Path, default=Path(os.environ.get('FACES_BASE', '/data/base')))
    parser.add_argument('--root', type=Path, default=Path(os.environ.get('MEDIA2_ROOT', '/mnt/media2')))
    parser.add_argument('--once', action='store_true', help='Process the current request then exit')
    args = parser.parse_args()
    directory = args.base / 'watermarks'
    directory.mkdir(parents=True, exist_ok=True)
    with (directory / '.worker.lock').open('w') as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise SystemExit('A watermark worker is already running')
        stopped = threading.Event()
        def heartbeat():
            while not stopped.is_set():
                publish(directory / 'worker.json', dict(at=now(), pipeline=PIPELINE))
                stopped.wait(10)
        thread = threading.Thread(target=heartbeat, daemon=True)
        thread.start()
        try:
            while True:
                job = read_json(directory / 'job.json', None)
                progress = read_json(directory / 'progress.json', {})
                if job and (progress.get('job_id') != job['id'] or progress.get('status') == 'running'):
                    result = process_job(job, directory, args.root)
                    print(json.dumps(result, ensure_ascii=False), flush=True)
                    if args.once and result['status'] == 'error':
                        return 1
                if args.once:
                    return 0
                time.sleep(3)
        finally:
            stopped.set()
            thread.join(timeout=12)
            publish(directory / 'worker.json', dict(at='1970-01-01T00:00:00+00:00', pipeline=PIPELINE))


if __name__ == '__main__':
    raise SystemExit(main())
