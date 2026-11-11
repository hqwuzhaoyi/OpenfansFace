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
import uuid

from watermark_worker import read_json, sample_times, validate_source
from processing_store import connect
from task_progress import stage_progress
from processing_publication import attempt_directory, native, PublicationError
from processing_gpu import backend, Detector, DETECTOR_SHA
from processing_compute import RUNTIME, compute_write_guard, get_resident_client, in_compute_write_guard

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
    cache_check = task.get('_cache_check')
    if isinstance(cache_check, dict) and cache_check.get('face_feature_valid') is False:
        return None
    raw_data = task.get('data', '{}')
    parsed_data = json.loads(raw_data) if isinstance(raw_data, str) else (raw_data or {})
    face_data = parsed_data.get('face', {}) if isinstance(parsed_data, dict) else {}
    previous = None
    expected = None
    source_identity = None
    if isinstance(cache_check, dict) and cache_check.get('face_feature_valid') is True:
        previous = cache_check.get('feature_cache')
        expected = cache_check.get('feature_cache_sha256')
        source_identity = cache_check.get('source_identity')
    if not previous:
        previous = face_data.get('feature_cache')
    if not expected:
        expected = face_data.get('feature_cache_sha256')
    if not source_identity:
        source_identity = face_data.get('source_identity')
    if not previous:
        return None
    file = Path(previous)
    if not file.resolve().is_relative_to((Path(base) / 'processing/attempts').resolve()):
        return None
    if not file.exists():
        return None
    payload = file.read_bytes()
    if not expected or hashlib.sha256(payload).hexdigest() != expected:
        return None
    with np.load(io.BytesIO(payload), allow_pickle=False) as data:
        if 'model_hashes' in data and json.loads(str(data['model_hashes'])) != MODEL_HASHES:
            return None
        if str(data['version']) != FACE_VERSION or str(data['path']) != task['path'] or int(data['size']) != task['size'] or str(data['mtime_ns']) != str(task['mtime_ns']):
            return None
        if source_identity is None and 'source_dev' in data and 'source_ino' in data:
            source_identity = {'dev': int(data['source_dev']), 'ino': int(data['source_ino'])}
        task['_feature_cache'] = str(file)
        task['_feature_cache_sha256'] = expected
        if source_identity is not None:
            task['_source_identity'] = source_identity
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


def reference_features(base, vid, source, accepted_receipts=None):
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
    if isinstance(accepted_receipts, dict):
        for item in accepted_receipts.get(vid, []):
            cache = item.get('feature_cache')
            expected = item.get('feature_cache_sha256')
            if not cache or not expected:
                continue
            file = Path(cache)
            if file.name != 'features.npz' or not file.resolve().is_relative_to((Path(base) / 'processing/attempts').resolve()):
                continue
            candidates.append((file, expected, item.get('source_identity') or {}))
    else:
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
                vectors = cached['embeddings'][:24].copy()
                if len(vectors) == 0 or vectors.ndim != 2 or vectors.shape[1] != 512 or not np.isfinite(vectors).all():
                    continue
                norms = np.linalg.norm(vectors, axis=1)
                if np.any(norms < .99) or np.any(norms > 1.01):
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
                return list(vectors)
        except (OSError, ValueError, KeyError):
            continue
    return []


NON_PERSON_NAMES = {'_unsorted', 'Unknown', '未知', '迷奸系列', '九九 圣水精液食物合集'}


def _build_video_track(face_items):
    """Pool intra-video face detections into a dominant coherent unit track vector."""
    import numpy as np
    if not face_items:
        return None
    ordered = sorted(face_items, key=lambda item: -item[0])[:24]
    scores = np.asarray([item[0] for item in ordered], dtype=np.float32)
    embs = np.stack([item[1] for item in ordered]).astype(np.float32)
    if len(embs) == 1:
        vec = embs[0] / (np.linalg.norm(embs[0]) + 1e-9)
        return vec.astype(np.float32), 1, float(scores[0]), 1.0
    sim = embs @ embs.T
    weighted_deg = (sim >= .50).astype(np.float32) @ scores
    seed = int(weighted_deg.argmax())
    inl_mask = sim[seed] >= .50
    inl_embs = embs[inl_mask]
    inl_scores = scores[inl_mask]
    weights = inl_scores[:, None]
    center = (inl_embs * weights).sum(axis=0)
    center = (center / (np.linalg.norm(center) + 1e-9)).astype(np.float32)
    coherence = float((inl_embs @ center).mean()) if len(inl_embs) > 1 else 1.0
    return center, int(inl_mask.sum()), float(inl_scores.max()), coherence


def _extract_query_tracks(embs):
    """Extract up to 2 coherent face tracks from a query video's frame embeddings."""
    import numpy as np
    if len(embs) == 0:
        return []
    if len(embs) == 1:
        v = embs[0] / (np.linalg.norm(embs[0]) + 1e-9)
        return [(v.astype(np.float32), 1)]
    tracks = []
    remaining = np.arange(len(embs))
    for _ in range(2):
        if len(remaining) == 0:
            break
        sub = embs[remaining]
        sim = sub @ sub.T
        counts = (sim >= .50).sum(axis=1)
        seed = int(counts.argmax())
        inl_local = sim[seed] >= .50
        if tracks and int(inl_local.sum()) < 2:
            break
        inl_idx = remaining[inl_local]
        vec = embs[inl_idx].mean(axis=0)
        vec = (vec / (np.linalg.norm(vec) + 1e-9)).astype(np.float32)
        tracks.append((vec, int(len(inl_idx))))
        remaining = remaining[~inl_local]
    return tracks


def compute_staged_references(base, root, state=None):
    """Pure computation of multi-prototype video-track reference templates staged into `processing/tmp/references-<uuid>.npz`."""
    import numpy as np
    base = Path(base)
    root = Path(root)
    state = state or {}
    input_version = state.get('input_version')
    report = state.get('report') if isinstance(state.get('report'), dict) else read_json(base / 'report.json', {})
    renames = state.get('renames') if isinstance(state.get('renames'), dict) else read_json(base / 'person-renames.json', {})
    if isinstance(state.get('assigned'), dict):
        assigned = {
            vid: canonical_person(creator, renames)
            for vid, creator in state['assigned'].items()
            if isinstance(creator, str) and creator.strip()
        }
    else:
        assigned = {
            v['vid']: canonical_person(v['creator'], renames)
            for v in read_json(base / 'integration-queue.json', []) + read_json(base / 'archive-history.json', [])
            if v.get('file_state') != 'staged' and v.get('creator')
        }
    con = sqlite3.connect('file:' + str(base / 'faces.db') + '?mode=ro', uri=True)
    try:
        video_cols = {row[1] for row in con.execute('PRAGMA table_info(videos)')}
        if isinstance(state.get('sources'), dict):
            sources = dict(state['sources'])
        else:
            sources = dict(con.execute('SELECT vid,path FROM videos'))
            sources.update(read_json(base / 'video-path-overrides.json', {}))
        video_meta = (
            dict(con.execute('SELECT vid,size FROM videos').fetchall())
            if 'size' in video_cols else {}
        )
        columns = {row[1] for row in con.execute('PRAGMA table_info(faces)')}
        rows = con.execute('SELECT vid,score,emb,thumb FROM faces ORDER BY score DESC').fetchall() if 'emb' in columns else []
    finally:
        con.close()
    if not isinstance(state.get('sources'), dict):
        processing_path = base / 'processing/processing.db'
        if processing_path.exists():
            queue = sqlite3.connect('file:' + str(processing_path) + '?mode=ro', uri=True)
            try:
                for vid, path in queue.execute('SELECT vid,path FROM tasks'):
                    sources.setdefault(vid, path)
            finally:
                queue.close()
    prefix = str(root) + '/pikpak/creators/'
    share_prefix = str(root) + '/pikpak/share/收藏/'
    for vid, path in sources.items():
        file = mapped(path, root) or mapped(vid, root)
        if not file:
            continue
        if file.startswith(prefix):
            person = canonical_person(file[len(prefix):].split('/')[0], renames)
            if person and person not in NON_PERSON_NAMES:
                assigned.setdefault(vid, person)
        elif file.startswith(share_prefix):
            parts = file[len(share_prefix):].split('/')
            if len(parts) >= 2:
                person = canonical_person(parts[0], renames)
                if person and person not in NON_PERSON_NAMES:
                    assigned.setdefault(vid, person)

    by_thumb = {}
    by_name_vids = {}
    for vid, score, blob, thumb in rows:
        if not blob or len(blob) != 2048:
            continue
        vector = np.frombuffer(blob, np.float32)
        if not np.isfinite(vector).all():
            continue
        vector = vector / (np.linalg.norm(vector) + 1e-9)
        if thumb:
            by_thumb.setdefault(thumb, []).append(vector)
        name = canonical_person(assigned.get(vid, ''), renames) if assigned.get(vid) else None
        if name and name not in NON_PERSON_NAMES and score >= .68:
            vid_map = by_name_vids.setdefault(name, {})
            if len(vid_map.get(vid, [])) < 24:
                vid_map.setdefault(vid, []).append((float(score), vector))

    accepted_receipts = state.get('accepted_receipts') if isinstance(state.get('accepted_receipts'), dict) else None
    faces_cache_dir = base / 'processing/faces'
    existing_legacy_caches = {p.name for p in faces_cache_dir.iterdir()} if faces_cache_dir.is_dir() else set()
    for vid, raw_name in assigned.items():
        name = canonical_person(raw_name, renames)
        if not name or name in NON_PERSON_NAMES:
            continue
        if vid in by_name_vids.get(name, {}):
            continue
        has_receipt = isinstance(accepted_receipts, dict) and bool(accepted_receipts.get(vid))
        has_legacy = (hashlib.sha256(vid.encode()).hexdigest() + '.npz') in existing_legacy_caches
        if accepted_receipts is not None and not has_receipt and not has_legacy:
            continue
        source = next((p for value in (sources.get(vid), vid) if (p := mapped(value, root)) and Path(p).is_file()), None)
        if source:
            feats = reference_features(base, vid, source, accepted_receipts=accepted_receipts)
            if feats:
                vid_map = by_name_vids.setdefault(name, {})
                vid_map[vid] = [(0.80, (v / (np.linalg.norm(v) + 1e-9)).astype(np.float32)) for v in feats[:24]]

    anchors = {}
    for person in report.get('identities', []):
        p_name = canonical_person(person.get('name', ''), renames)
        if not p_name or p_name in NON_PERSON_NAMES:
            continue
        vectors = by_thumb.get(person.get('rep_thumb'), [])
        if vectors and all(float(vectors[0] @ v) > .995 for v in vectors[1:]):
            anchors[p_name] = vectors[0]

    names, vectors = [], []
    for name in sorted(set(by_name_vids) | set(anchors)):
        if not name or name in NON_PERSON_NAMES:
            continue
        vid_map = by_name_vids.get(name, {})
        tracks = []
        for vid, face_items in vid_map.items():
            built = _build_video_track(face_items)
            if built is not None:
                tracks.append(built)

        if not tracks:
            anchor = anchors.get(name)
            if anchor is not None:
                names.append(name)
                vectors.append((anchor / (np.linalg.norm(anchor) + 1e-9)).astype(np.float32))
            continue

        total_inlier_faces = sum(t[1] for t in tracks)
        if len(tracks) == 1:
            vt, n_inl, max_sc, coh = tracks[0]
            if n_inl >= 5 or (n_inl >= 2 and max_sc >= .72 and coh >= .55):
                names.append(name)
                vectors.append(vt)
            continue

        T = np.stack([t[0] for t in tracks]).astype(np.float32)
        sim_t = T @ T.T
        deg = (sim_t >= .45).sum(axis=1)
        seed_t = int(deg.argmax())
        inl_mask = sim_t[seed_t] >= .44
        n_inl_vids = int(inl_mask.sum())

        # Multi-model / male-producer studio folder purity gate
        if len(T) >= 8 and (n_inl_vids / len(T)) < .22:
            continue
        if n_inl_vids < 2 and total_inlier_faces < 5:
            continue

        inl_T = T[inl_mask]
        c1 = inl_T.mean(axis=0)
        c1 = (c1 / (np.linalg.norm(c1) + 1e-9)).astype(np.float32)
        person_protos = [c1]
        names.append(name)
        vectors.append(c1)

        # Extract up to 3 diverse pose/appearance sub-prototypes from remaining inlier video tracks
        rem = inl_T[(inl_T @ c1) < .85]
        for _ in range(3):
            if len(rem) < 2:
                break
            s_rem = rem @ rem.T
            sd = int((s_rem >= .55).sum(axis=1).argmax())
            cluster_mask = s_rem[sd] >= .55
            if int(cluster_mask.sum()) >= 2:
                ck = rem[cluster_mask].mean(axis=0)
                ck = (ck / (np.linalg.norm(ck) + 1e-9)).astype(np.float32)
                if max(float(ck @ p) for p in person_protos) < .88:
                    person_protos.append(ck)
                    names.append(name)
                    vectors.append(ck)
            rem = rem[~cluster_mask]

    matrix = np.stack(vectors).astype(np.float32) if vectors else np.zeros((0, 512), np.float32)
    version = hashlib.sha256(json.dumps(names, ensure_ascii=False).encode() + matrix.tobytes()).hexdigest()
    staged_file = base / 'processing/tmp' / f'references-{uuid.uuid4()}.npz'
    save_npz(staged_file, names=np.asarray(names, dtype=str), embeddings=matrix, version=version, input_version=input_version)
    staged_sha256 = hashlib.sha256(staged_file.read_bytes()).hexdigest()
    return dict(
        staged_path=str(staged_file),
        sha256=staged_sha256,
        version=version,
        input_version=input_version,
        people=len(set(names)),
        templates=len(names),
    )


def _stage_references(base: Path, root: Path, state: dict) -> dict:
    use_daemon = (
        os.environ.get('PROCESSING_COMPUTE_MODE') != 'subprocess'
        and not in_compute_write_guard()
        and not hasattr(native, 'assert_called')
    )
    if use_daemon:
        return get_resident_client().compute_references(base, root, state=state)
    with compute_write_guard(base, root):
        return compute_staged_references(base, root, state=state)


def build_references(base, root):
    """Use unambiguous existing representative faces and coherent confirmed-video faces."""
    base = Path(base)
    root = Path(root)
    if os.environ.get('FILE_OPERATIONS_RUST_URL'):
        state_resp = native('processing-reference-state', {})
    else:
        state_resp = native('analysis-state', {})

    if isinstance(state_resp, dict) and 'assigned' in state_resp and 'input_version' in state_resp:
        staged = _stage_references(base, root, state_resp)
        published = native('processing-publish-references', staged)
        return dict(
            version=published['version'],
            input_version=published['input_version'],
            people=published['people'],
            templates=published['templates'],
        )

    input_version = state_resp['version']
    staged = _stage_references(base, root, {'input_version': input_version})
    staged_file = Path(staged['staged_path'])
    try:
        if native('analysis-state', {})['version'] != input_version:
            raise PublicationError('人物参考资料已变化，等待重新建立参考', 409)
        target = base / 'processing/references.npz'
        target.parent.mkdir(parents=True, exist_ok=True)
        os.replace(staged_file, target)
    finally:
        if staged_file.exists():
            staged_file.unlink()
    return dict(
        version=staged['version'],
        input_version=input_version,
        people=staged['people'],
        templates=staged['templates'],
    )



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
    file = validate_source(task, root)
    existing = cached_features(base, task)
    if existing is not None:
        return existing, True, None
    if progress:
        progress.update(stage='加载人脸模型', done=None, total=None)
    model_home = Path(os.environ.get('INSIGHTFACE_HOME', '/root/.insightface')) / 'models/buffalo_l'
    mode = backend()
    runtime = RUNTIME.get_face_runtime(mode, model_home, MODEL_HASHES)
    model_hashes = runtime['model_hashes']
    remote = Detector(task, progress=progress) if mode == 'gpu-detection' else None
    recognizer = runtime['recognizer']
    engine = runtime['engine']
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
    task['_execution']['resident_load_count'] = RUNTIME.face_load_count
    task['_execution']['resident_tasks_run'] = RUNTIME.face_tasks_run
    if remote:
        task['_execution']['frames'] = remote.timings
    return result, False, successful


def score_face_candidates(embs, thumbs, timestamps, ref_names, ref_matrix):
    """Score query video face embeddings against multi-prototype reference gallery using track-frame hybrid similarity."""
    import numpy as np
    if len(embs) == 0 or len(ref_matrix) == 0:
        return []
    q_tracks = _extract_query_tracks(embs)
    scores = embs @ ref_matrix.T
    raw_candidates = []
    unique_names = [n for n in sorted(set(ref_names)) if n and n not in NON_PERSON_NAMES]
    for name in unique_names:
        mask = ref_names == name
        protos = ref_matrix[mask]
        per_face = scores[:, mask].max(axis=1)
        best = int(per_face.argmax())
        max_frame = float(per_face[best])
        top2_frame = float(np.sort(per_face)[-2:].mean()) if len(per_face) >= 2 else max_frame
        track_sim = max((float((t_vec @ protos.T).max()) for t_vec, _ in q_tracks), default=max_frame)
        best_track_support = max((t_sup for t_vec, t_sup in q_tracks if float((t_vec @ protos.T).max()) >= .48), default=0)
        if len(per_face) >= 2:
            hybrid = max(top2_frame, .55 * track_sim + .45 * max_frame)
        else:
            hybrid = max_frame
        support = int(max((per_face >= .48).sum(), best_track_support))
        raw_candidates.append(dict(
            name=str(name),
            hybrid=hybrid,
            max_frame=max_frame,
            track_sim=track_sim,
            support=max(1, support) if hybrid >= .48 else 0,
            best=best,
        ))
    if not raw_candidates:
        return []
    raw_candidates.sort(key=lambda c: -c['hybrid'])
    if len(raw_candidates) >= 15:
        cohort_slice = [c['hybrid'] for c in raw_candidates[5:min(35, len(raw_candidates))]]
        bg_mean = float(np.mean(cohort_slice))
    else:
        bg_mean = 0.35

    matches = []
    for cand in raw_candidates:
        hybrid = cand['hybrid']
        track_sim = cand['track_sim']
        support = cand['support']
        admitted = False
        if hybrid >= .50:
            # Suppress single-frame hubness noise when gallery is large
            if len(raw_candidates) >= 20 and hybrid < .525 and support < 2 and (hybrid - bg_mean) < .09:
                admitted = False
            else:
                admitted = True
        elif len(embs) >= 2 and track_sim >= .485 and support >= 2 and hybrid >= .485 and (hybrid - bg_mean) >= .10:
            admitted = True
        if admitted:
            best = cand['best']
            matches.append(dict(
                name=cand['name'],
                source='face',
                score=round(float(max(hybrid, .50 if track_sim >= .485 else hybrid)), 4),
                support=int(support),
                thumb=str(thumbs[best]) if len(thumbs) > best else '',
                timestamp=float(timestamps[best]) if len(timestamps) > best else 0.0,
                detail='人脸相似度候选，需人工核对',
            ))
    matches.sort(key=lambda m: (-m['score'], -m['support'], m['name']))
    return matches


def analyze(task, base, root):
    import numpy as np
    progress = stage_progress(task, base)
    progress.update(stage="检查人脸缓存")
    features, reused, sampled = extract_features(task, base, root, progress)
    with np.load(task.get('_references_path', Path(base) / 'processing/references.npz'), allow_pickle=False) as refs:
        renames = read_json(Path(base) / 'person-renames.json', {})
        names, matrix, version = np.asarray([canonical_person(str(name), renames) for name in refs['names']]), refs['embeddings'], str(refs['version'])
    matches = score_face_candidates(features['embeddings'], features['thumbs'], features['timestamps'], names, matrix)
    unknown_group = None
    if not matches and len(features['embeddings']) >= 2:
        vectors = features['embeddings']; similarities = vectors @ vectors.T
        seed = int((similarities >= .55).sum(axis=1).argmax())
        coherent = vectors[similarities[seed] >= .55]
        if len(coherent) >= 2:
            center = coherent.mean(axis=0); center /= np.linalg.norm(center) + 1e-9
            best = .60
            active_groups = task.get('_active_face_groups')
            if isinstance(active_groups, list):
                for row in active_groups:
                    emb = np.asarray(row.get('embedding', []), dtype=np.float32)
                    if emb.shape != (512,):
                        continue
                    score = float(center @ emb)
                    if score >= best:
                        best, unknown_group = score, row['id']
            else:
                db_path = Path(base) / 'processing/processing.db'
                if db_path.exists():
                    con = sqlite3.connect('file:' + str(db_path) + '?mode=ro', uri=True)
                    con.row_factory = sqlite3.Row
                    try:
                        handled = {r['vid'] for r in read_json(Path(base) / 'integration-queue.json', []) + read_json(Path(base) / 'archive-history.json', [])}
                        active = {json.loads(r['data']).get('face', {}).get('unknown_group') for r in con.execute("SELECT vid,data FROM tasks WHERE stage IN ('review','face') AND json_extract(data,'$.kind')='none'") if r['vid'] not in handled}
                        for row in con.execute('SELECT * FROM face_groups WHERE version=?', (FACE_VERSION,)):
                            if row['id'] not in active:
                                continue
                            score = float(center @ np.frombuffer(row['embedding'], np.float32))
                            if score >= best:
                                best, unknown_group = score, row['id']
                    finally:
                        con.close()
            if unknown_group is None:
                unknown_group = hashlib.sha256((task['vid'] + task['mtime_ns'] + FACE_VERSION).encode()).hexdigest()[:16]
            # Group insertion and evidence visibility belong to native acceptance.
            task['_unknown_group'] = {'id': unknown_group, 'version': FACE_VERSION, 'embedding': center.astype(np.float32).tolist()}
    return dict(matches=matches[:5], face_count=len(features['embeddings']), cache_reused=reused,
                sampled_frames=sampled, reference_version=version, reference_people=len(set(names)), version=FACE_VERSION,
                unknown_group=unknown_group, unknown_group_input=task.get('_unknown_group'),
                feature_cache=task.get('_feature_cache') or json.loads(task.get('data', '{}')).get('face', {}).get('feature_cache'),
                feature_cache_sha256=task.get('_feature_cache_sha256') or json.loads(task.get('data', '{}')).get('face', {}).get('feature_cache_sha256'),
                source_identity=task.get('_source_identity') or json.loads(task.get('data', '{}')).get('face', {}).get('source_identity'),
                execution=task.get('_execution') or json.loads(task.get('data', '{}')).get('face', {}).get('execution') or {'backend': 'unknown', 'cache_reused': True},
                thumbs=[str(p) for p in features['thumbs'][:12]])
