"""Per-video face features and versioned reference matching. Legacy tables remain intact."""
import hashlib
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import tempfile

from watermark_worker import read_json, sample_times, validate_source
from processing_store import connect
from task_progress import stage_progress

FACE_VERSION = 'buffalo_l-det-rec-8frames-v1'


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
    file = cache_path(base, task['vid'])
    if not file.exists():
        return None
    with np.load(file, allow_pickle=False) as data:
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
        os.chmod(temporary, 0o644)
        os.replace(temporary, file)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def build_references(base, root):
    """Use unambiguous existing representative faces and coherent confirmed-video faces."""
    import numpy as np
    base = Path(base)
    report = read_json(base / 'report.json', {})
    renames = read_json(base / 'person-renames.json', {})
    assigned = {v['vid']: canonical_person(v['creator'], renames) for v in read_json(base / 'integration-queue.json', []) + read_json(base / 'archive-history.json', []) if v.get('file_state') != 'staged'}
    con = sqlite3.connect('file:' + str(base / 'faces.db') + '?mode=ro', uri=True)
    try:
        sources = dict(con.execute('SELECT vid,path FROM videos'))
        columns = {row[1] for row in con.execute('PRAGMA table_info(faces)')}
        rows = con.execute('SELECT vid,score,emb,thumb FROM faces ORDER BY score DESC').fetchall() if 'emb' in columns else []
    finally:
        con.close()
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
        file = cache_path(base, vid)
        if file.exists():
            with np.load(file, allow_pickle=False) as cached:
                if str(cached['version']) == FACE_VERSION:
                    by_name.setdefault(name, []).extend(cached['embeddings'][:24])
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
    save_npz(base / 'processing/references.npz', names=np.asarray(names, dtype=str), embeddings=matrix, version=version)
    return dict(version=version, people=len(set(names)), templates=len(names))


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
    engine = FaceAnalysis(name='buffalo_l', root=os.environ.get('INSIGHTFACE_HOME', '/root/.insightface'),
                          allowed_modules=['detection', 'recognition'], providers=['CPUExecutionProvider'], sess_options=options)
    engine.prepare(ctx_id=-1, det_size=(640, 640))
    probe = subprocess.run(['ffprobe', '-v', 'error', '-show_entries', 'format=duration', '-of', 'json', str(file)],
                           capture_output=True, text=True, timeout=30, check=True)
    duration = float(json.loads(probe.stdout)['format']['duration'])
    embeddings, thumbs, timestamps, successful = [], [], [], 0
    thumb_dir = Path(base) / 'processing/thumbs'; thumb_dir.mkdir(parents=True, exist_ok=True)
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
            for face_index, face in enumerate(engine.get(image)):
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
    save_npz(cache_path(base, task['vid']), **result, version=FACE_VERSION, path=task['path'], size=task['size'], mtime_ns=task['mtime_ns'])
    return result, False, successful


def analyze(task, base, root):
    import numpy as np
    progress = stage_progress(task, base)
    progress.update(stage="检查人脸缓存")
    features, reused, sampled = extract_features(task, base, root, progress)
    with np.load(Path(base) / 'processing/references.npz', allow_pickle=False) as refs:
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
                handled = {r['vid'] for r in read_json(Path(base) / 'integration-queue.json', []) + read_json(Path(base) / 'archive-history.json', [])}
                active = {json.loads(r['data']).get('face', {}).get('unknown_group') for r in con.execute("SELECT vid,data FROM tasks WHERE stage='review' AND json_extract(data,'$.kind')='none'") if r['vid'] not in handled}
                best = .60
                for row in con.execute('SELECT * FROM face_groups WHERE version=?', (FACE_VERSION,)):
                    if row['id'] not in active:
                        continue
                    score = float(center @ np.frombuffer(row['embedding'], np.float32))
                    if score >= best:
                        best, unknown_group = score, row['id']
                if unknown_group is None:
                    unknown_group = hashlib.sha256((task['vid'] + task['mtime_ns'] + FACE_VERSION).encode()).hexdigest()[:16]
                    with con:
                        con.execute('INSERT OR IGNORE INTO face_groups VALUES (?,?,?)', (unknown_group, FACE_VERSION, center.astype(np.float32).tobytes()))
            finally:
                con.close()
    return dict(matches=matches[:5], face_count=len(features['embeddings']), cache_reused=reused,
                sampled_frames=sampled, reference_version=version, reference_people=len(set(names)), version=FACE_VERSION,
                unknown_group=unknown_group,
                thumbs=[str(p) for p in features['thumbs'][:12]])
