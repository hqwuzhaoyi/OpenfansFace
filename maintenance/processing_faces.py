"""Per-video face features and versioned reference matching. Legacy tables remain intact."""
import hashlib
import io
import json
import os
from pathlib import Path
import sqlite3
import shutil
import subprocess
import tempfile

from watermark_worker import read_json, sample_times, validate_source
from processing_store import connect
from task_progress import stage_progress
from processing_publication import attempt_directory, native, PublicationError
from processing_gpu import backend, Detector, DETECTOR_SHA

FACE_VERSION = 'buffalo_l-det-rec-8frames-v1'
MODEL_HASHES = {'det_10g.onnx': DETECTOR_SHA, 'w600k_r50.onnx': '4c06341c33c2ca1f86781dab0e829f88ad5b64be9fba56e56bc9ebdefc619e43'}


def canonical_person(name, renames):
    seen = set()
    while isinstance(renames.get(name), str) and name not in seen:
        seen.add(name)
        name = renames[name]
    return name


def mapped(value, root):
    value = (value or '').replace('\\', '/')
    if '..' in value.split('/'):
        return None
    prefix = str(root).rstrip('/') + '/'
    if value.startswith(prefix):
        return value
    if '/media/' in value:
        return prefix + value.split('/media/', 1)[1]
    return None


def cache_path(base, vid):
    return Path(base) / 'processing/faces' / (hashlib.sha256(vid.encode()).hexdigest() + '.npz')


def cached_features(base, task):
    import numpy as np
    previous = json.loads(task.get('data', '{}')).get('face', {}).get('feature_cache')
    if not previous:
        return None
    file = Path(previous)
    if previous and not file.resolve().is_relative_to((Path(base) / 'processing/attempts').resolve()):
        return None
    if not file.exists():
        return None
    payload = file.read_bytes()
    expected = json.loads(task.get('data', '{}')).get('face', {}).get('feature_cache_sha256')
    if not expected or hashlib.sha256(payload).hexdigest() != expected:
        return None
    with np.load(io.BytesIO(payload), allow_pickle=False) as data:
        if 'model_hashes' in data and json.loads(str(data['model_hashes'])) != MODEL_HASHES:
            return None
        if str(data['version']) != FACE_VERSION or str(data['path']) != task['path'] or int(data['size']) != task['size'] or str(data['mtime_ns']) != task['mtime_ns']:
            return None
        return {key: data[key].copy() for key in ('embeddings', 'thumbs', 'timestamps')}


def save_npz(file, **data):
    import numpy as np
    file.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(prefix='.' + file.name, dir=file.parent)
    try:
        with os.fdopen(fd, 'wb') as out:
            np.savez_compressed(out, **data)
            out.flush(); os.fsync(out.fileno())
        os.chmod(temporary, 0o644)
        os.replace(temporary, file)
        directory = os.open(file.parent, os.O_RDONLY)
        try: os.fsync(directory)
        finally: os.close(directory)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def reference_features(base, vid, source):
    """Only current legacy bytes or immutable, accepted same-file attempt features."""
    import numpy as np
    source = Path(source)
    try:
        stat = source.stat()
        if source.is_symlink() or not source.is_file():
            return []
    except OSError:
        return []
    candidates = []
    database = Path(base) / 'processing/processing.db'
    if database.exists():
        con = sqlite3.connect('file:' + str(database) + '?mode=ro', uri=True)
        try:
            if con.execute("SELECT 1 FROM sqlite_master WHERE name='_face_admin_processing_results'").fetchone():
                for (raw,) in con.execute('SELECT result FROM _face_admin_processing_results ORDER BY rowid DESC'):
                    receipt = json.loads(raw)
                    if receipt.get('ok') is not True or receipt.get('accepted') is not True or receipt.get('vid') != vid:
                        continue
                    face = receipt.get('data', {}).get('face', {})
                    cache = face.get('feature_cache')
                    if not cache or not face.get('feature_cache_sha256'):
                        continue
                    file = Path(cache)
                    if file.name != 'features.npz' or not file.resolve().is_relative_to((Path(base) / 'processing/attempts').resolve()):
                        continue
                    candidates.append((file, face['feature_cache_sha256'], face.get('source_identity') or receipt.get('source_identity') or {}))
        finally:
            con.close()
    candidates.append((cache_path(base, vid), None, None))
    for file, expected, receipt_identity in candidates:
        accepted = receipt_identity is not None
        try:
            if not file.is_file() or file.is_symlink():
                continue
            payload = file.read_bytes()
            if expected and hashlib.sha256(payload).hexdigest() != expected:
                continue
            with np.load(io.BytesIO(payload), allow_pickle=False) as cached:
                if 'model_hashes' in cached and json.loads(str(cached['model_hashes'])) != MODEL_HASHES:
                    continue
                if str(cached['version']) != FACE_VERSION or int(cached['size']) != stat.st_size or str(cached['mtime_ns']) != str(stat.st_mtime_ns):
                    continue
                if accepted and (int(cached['source_dev']) != int(receipt_identity.get('dev', -1)) or int(cached['source_ino']) != int(receipt_identity.get('ino', -1))):
                    continue
                same_origin = str(cached['path']) == str(source)
                if accepted:
                    same_origin = same_origin and int(cached['source_dev']) == stat.st_dev and int(cached['source_ino']) == stat.st_ino
                if not same_origin:
                    origin = {'path': str(cached['path']), 'size': str(int(cached['size'])), 'mtime_ns': str(cached['mtime_ns'])}
                    if accepted:
                        origin.update(dev=str(int(cached['source_dev'])), ino=str(int(cached['source_ino'])))
                    current_identity = {key: str(getattr(stat, 'st_' + key)) for key in ('dev', 'ino', 'size', 'mtime_ns')}
                    try:
                        proof = native('reference-relocation', {'vid': vid, 'source': str(source), 'cache': str(file),
                            'cache_sha256': hashlib.sha256(payload).hexdigest(), 'origin': origin, 'current_identity': current_identity})
                    except PublicationError:
                        continue
                    if proof.get('verified') is not True or proof.get('current_identity') != current_identity:
                        continue
                    after = source.stat()
                    if any(getattr(after, 'st_' + key) != getattr(stat, 'st_' + key) for key in ('dev', 'ino', 'size', 'mtime_ns')):
                        continue
                vectors = cached['embeddings'][:24].copy()
                if vectors.ndim != 2 or vectors.shape[1] != 512 or not np.isfinite(vectors).all():
                    continue
                norms = np.linalg.norm(vectors, axis=1)
                if np.any(norms < .99) or np.any(norms > 1.01):
                    continue
                return list(vectors)
        except (OSError, ValueError, KeyError):
            continue
    return []


def build_references(base, root):
    """Use unambiguous existing representative faces and coherent confirmed-video faces."""
    import numpy as np
    base = Path(base)
    input_version = native('analysis-state', {})['version']
    report = read_json(base / 'report.json', {})
    renames = read_json(base / 'person-renames.json', {})
    assigned = {v['vid']: canonical_person(v['creator'], renames) for v in read_json(base / 'integration-queue.json', []) + read_json(base / 'archive-history.json', []) if v.get('file_state') != 'staged'}
    con = sqlite3.connect('file:' + str(base / 'faces.db') + '?mode=ro', uri=True)
    try:
        sources = dict(con.execute('SELECT vid,path FROM videos'))
        sources.update(read_json(base / 'video-path-overrides.json', {}))
        columns = {row[1] for row in con.execute('PRAGMA table_info(faces)')}
        rows = con.execute('SELECT vid,score,emb,thumb FROM faces ORDER BY score DESC').fetchall() if 'emb' in columns else []
    finally:
        con.close()
    processing_path = base / 'processing/processing.db'
    if processing_path.exists():
        queue = sqlite3.connect('file:' + str(processing_path) + '?mode=ro', uri=True)
        try:
            for vid, path in queue.execute('SELECT vid,path FROM tasks'):
                sources.setdefault(vid, path)
        finally:
            queue.close()
    for vid, path in sources.items():
        file = mapped(path, root) or mapped(vid, root)
        prefix = str(root) + '/pikpak/creators/'
        if file and file.startswith(prefix):
            assigned.setdefault(vid, file[len(prefix):].split('/')[0])
    by_thumb, by_name = {}, {}
    for vid, score, blob, thumb in rows:
        if not blob or len(blob) != 2048:
            continue
        vector = np.frombuffer(blob, np.float32)
        if not np.isfinite(vector).all():
            continue
        vector = vector / (np.linalg.norm(vector) + 1e-9)
        if thumb:
            by_thumb.setdefault(thumb, []).append(vector)
        name = assigned.get(vid)
        if name and score >= .7 and len(by_name.get(name, [])) < 200:
            by_name.setdefault(name, []).append(vector)
    for vid, name in assigned.items():
        source = next((p for value in (sources.get(vid), vid) if (p := mapped(value, root)) and Path(p).is_file()), None)
        if source:
            by_name.setdefault(name, []).extend(reference_features(base, vid, source))
    anchors = {}
    for person in report.get('identities', []):
        vectors = by_thumb.get(person.get('rep_thumb'), [])
        if vectors and all(float(vectors[0] @ v) > .995 for v in vectors[1:]):
            anchors[canonical_person(person['name'], renames)] = vectors[0]
    names, vectors = [], []
    for name in sorted(set(by_name) | set(anchors)):
        candidates = by_name.get(name, [])[:240]
        anchor = anchors.get(name)
        if anchor is not None:
            names.append(name); vectors.append(anchor)
        if len(candidates) < 5:
            continue
        matrix = np.stack(candidates)
        if anchor is None:
            similarity = matrix @ matrix.T
            seed = int((similarity >= .50).sum(axis=1).argmax())
            selected = matrix[similarity[seed] >= .50]
        else:
            selected = matrix[matrix @ anchor >= .45]
        if len(selected) >= 5:
            center = selected.mean(axis=0); center /= np.linalg.norm(center) + 1e-9
            names.append(name); vectors.append(center)
    matrix = np.stack(vectors).astype(np.float32) if vectors else np.zeros((0, 512), np.float32)
    version = hashlib.sha256(json.dumps(names, ensure_ascii=False).encode() + matrix.tobytes()).hexdigest()
    if native('analysis-state', {})['version'] != input_version:
        raise PublicationError('人物参考资料已变化，等待重新建立参考', 409)
    save_npz(base / 'processing/references.npz', names=np.asarray(names, dtype=str), embeddings=matrix, version=version, input_version=input_version)
    return dict(version=version, input_version=input_version, people=len(set(names)), templates=len(names))



def capture_references(task, base, root):
    """Refresh stale reference inputs before spending remote/model work."""
    import numpy as np
    source = Path(base) / 'processing/references.npz'
    current = native('analysis-state', {})['version']
    try:
        with np.load(source, allow_pickle=False) as cached:
            valid = str(cached['input_version']) == current
    except (OSError, ValueError, KeyError):
        valid = False
    if not valid:
        build_references(base, root)
    captured = attempt_directory(base, task) / 'references.npz'
    shutil.copyfile(source, captured)
    with np.load(captured, allow_pickle=False) as reference:
        task['_reference_input_version'] = str(reference['input_version'])
    task['_references_path'] = str(captured)


def extract_features(task, base, root, progress=None):
    import cv2
    import numpy as np
    import onnxruntime as ort
    from insightface.app import FaceAnalysis
    file = validate_source(task, root)
    existing = cached_features(base, task)
    if existing is not None:
        return existing, True, None
    if progress:
        progress.update(stage='加载人脸模型', done=None, total=None)
    options = ort.SessionOptions(); options.intra_op_num_threads = 1; options.inter_op_num_threads = 1
    model_home = Path(os.environ.get('INSIGHTFACE_HOME', '/root/.insightface')) / 'models/buffalo_l'
    model_hashes = {name: hashlib.sha256((model_home / name).read_bytes()).hexdigest() for name in ('det_10g.onnx', 'w600k_r50.onnx')}
    if model_hashes != MODEL_HASHES:
        raise ValueError('人脸模型内容与已登记版本不符，暂停该次分析')
    remote = Detector(task, progress=progress) if backend() == 'gpu-detection' else None
    if remote:
        from insightface.model_zoo import get_model
        recognition_path = Path(os.environ.get('INSIGHTFACE_HOME', '/root/.insightface')) / 'models/buffalo_l/w600k_r50.onnx'
        recognizer = get_model(str(recognition_path), providers=['CPUExecutionProvider'], sess_options=options)
        recognizer.prepare(ctx_id=-1)
    else:
        engine = FaceAnalysis(name='buffalo_l', root=os.environ.get('INSIGHTFACE_HOME', '/root/.insightface'),
                              allowed_modules=['detection', 'recognition'], providers=['CPUExecutionProvider'], sess_options=options)
        engine.prepare(ctx_id=-1, det_size=(640, 640))
    probe = subprocess.run(['ffprobe', '-v', 'error', '-show_entries', 'format=duration', '-of', 'json', str(file)],
                           capture_output=True, text=True, timeout=30, check=True)
    duration = float(json.loads(probe.stdout)['format']['duration'])
    embeddings, thumbs, timestamps, successful = [], [], [], 0
    thumb_dir = attempt_directory(base, task) / 'thumbs'; thumb_dir.mkdir(parents=True, exist_ok=True)
    prefix = hashlib.sha256((task['vid'] + task['mtime_ns'] + str(task['size'])).encode()).hexdigest()
    with tempfile.TemporaryDirectory(prefix='face-analysis-') as temp:
        times = sample_times(duration)
        for index, stamp in enumerate(times):
            if progress:
                progress.update(stage="分析视频帧", done=index, total=len(times), unit="帧")
            frame = Path(temp) / f'{index}.jpg'
            try:
                subprocess.run(['ffmpeg', '-nostdin', '-v', 'error', '-y', '-threads', '1', '-ss', str(stamp), '-i', str(file),
                                '-frames:v', '1', '-vf', r'scale=w=min(1280\,iw):h=-2', '-threads', '1', str(frame)],
                               capture_output=True, timeout=40, check=True)
                image = cv2.imread(str(frame))
                if image is None:
                    continue
            except subprocess.SubprocessError:
                continue
            successful += 1
            if remote:
                from insightface.app.common import Face
                faces = []
                for detected in remote.detect(image, frame.read_bytes(), index, stamp):
                    face = Face(bbox=np.asarray(detected['bbox'], dtype=np.float32),
                                kps=np.asarray(detected['keypoints'], dtype=np.float32), det_score=detected['score'])
                    if face.det_score >= .7:
                        recognizer.get(image, face)
                    faces.append(face)
            else:
                faces = engine.get(image)
            for face_index, face in enumerate(faces):
                if face.det_score < .7:
                    continue
                embedding = face.normed_embedding.astype(np.float32)
                if embedding.shape != (512,) or not np.isfinite(embedding).all():
                    continue
                x1, y1, x2, y2 = face.bbox.astype(int)
                crop = image[max(0, y1):min(image.shape[0], y2), max(0, x1):min(image.shape[1], x2)]
                if not crop.size:
                    continue
                thumb = thumb_dir / f'{prefix}-{index}-{face_index}.jpg'
                if not cv2.imwrite(str(thumb), crop):
                    raise RuntimeError('无法保存人脸证据')
                embeddings.append(embedding); thumbs.append(str(thumb)); timestamps.append(stamp)
    if progress:
        progress.update(stage="匹配人物参考", done=len(times), total=len(times), unit="帧")
    if successful < 2:
        raise ValueError('可解码采样帧不足，不能判定为未检出人脸')
    validate_source(task, root)
    result = dict(embeddings=np.stack(embeddings) if embeddings else np.zeros((0, 512), np.float32),
                  thumbs=np.asarray(thumbs, dtype=str), timestamps=np.asarray(timestamps))
    feature_cache = attempt_directory(base, task) / 'features.npz'
    identity = file.stat()
    save_npz(feature_cache, **result, version=FACE_VERSION, path=task['path'], size=task['size'], mtime_ns=task['mtime_ns'], source_dev=identity.st_dev, source_ino=identity.st_ino, model_hashes=json.dumps(model_hashes, sort_keys=True))
    task['_feature_cache_sha256'] = hashlib.sha256(feature_cache.read_bytes()).hexdigest()
    task['_source_identity'] = {'dev': identity.st_dev, 'ino': identity.st_ino}
    task['_feature_cache'] = str(feature_cache)
    task['_execution'] = remote.execution if remote else {'backend': 'cpu', 'detection_provider': 'CPUExecutionProvider', 'recognition_provider': 'CPUExecutionProvider'}
    task['_execution']['model_hashes'] = model_hashes
    if remote:
        task['_execution']['frames'] = remote.timings
    return result, False, successful


def analyze(task, base, root):
    import numpy as np
    progress = stage_progress(task, base)
    progress.update(stage="检查人脸缓存")
    features, reused, sampled = extract_features(task, base, root, progress)
    with np.load(task.get('_references_path', Path(base) / 'processing/references.npz'), allow_pickle=False) as refs:
        renames = read_json(Path(base) / 'person-renames.json', {})
        names, matrix, version = np.asarray([canonical_person(str(name), renames) for name in refs['names']]), refs['embeddings'], str(refs['version'])
    matches = []
    if len(features['embeddings']) and len(matrix):
        scores = features['embeddings'] @ matrix.T
        for name in sorted(set(names)):
            per_face = scores[:, names == name].max(axis=1)
            best = int(per_face.argmax()); score = float(per_face[best])
            if score >= .50:
                matches.append(dict(name=str(name), source='face', score=round(score, 4),
                                    support=int((per_face >= .50).sum()), thumb=str(features['thumbs'][best]),
                                    timestamp=float(features['timestamps'][best]), detail='人脸相似度候选，需人工核对'))
    matches.sort(key=lambda m: -m['score'])
    unknown_group = None
    if not matches and len(features['embeddings']) >= 2:
        vectors = features['embeddings']; similarities = vectors @ vectors.T
        seed = int((similarities >= .55).sum(axis=1).argmax())
        coherent = vectors[similarities[seed] >= .55]
        if len(coherent) >= 2:
            center = coherent.mean(axis=0); center /= np.linalg.norm(center) + 1e-9
            con = connect(base)
            try:
                con.execute('BEGIN IMMEDIATE')
                handled = {r['vid'] for r in read_json(Path(base) / 'integration-queue.json', []) + read_json(Path(base) / 'archive-history.json', [])}
                active = {json.loads(r['data']).get('face', {}).get('unknown_group') for r in con.execute("SELECT vid,data FROM tasks WHERE stage IN ('review','face') AND json_extract(data,'$.kind')='none'") if r['vid'] not in handled}
                best = .60
                for row in con.execute('SELECT * FROM face_groups WHERE version=?', (FACE_VERSION,)):
                    if row['id'] not in active:
                        continue
                    score = float(center @ np.frombuffer(row['embedding'], np.float32))
                    if score >= best:
                        best, unknown_group = score, row['id']
                if unknown_group is None:
                    unknown_group = hashlib.sha256((task['vid'] + task['mtime_ns'] + FACE_VERSION).encode()).hexdigest()[:16]
                # Group insertion and evidence visibility belong to native acceptance.
                task['_unknown_group'] = {'id': unknown_group, 'version': FACE_VERSION, 'embedding': center.astype(np.float32).tolist()}
                con.commit()
            finally:
                con.close()
    return dict(matches=matches[:5], face_count=len(features['embeddings']), cache_reused=reused,
                sampled_frames=sampled, reference_version=version, reference_people=len(set(names)), version=FACE_VERSION,
                unknown_group=unknown_group, unknown_group_input=task.get('_unknown_group'),
                feature_cache=task.get('_feature_cache') or json.loads(task.get('data', '{}')).get('face', {}).get('feature_cache'),
                feature_cache_sha256=task.get('_feature_cache_sha256') or json.loads(task.get('data', '{}')).get('face', {}).get('feature_cache_sha256'),
                source_identity=task.get('_source_identity') or json.loads(task.get('data', '{}')).get('face', {}).get('source_identity'),
                execution=task.get('_execution') or json.loads(task.get('data', '{}')).get('face', {}).get('execution') or {'backend': 'unknown', 'cache_reused': True},
                thumbs=[str(p) for p in features['thumbs'][:12]])
