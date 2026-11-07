#!/usr/bin/env python3
"""suggest(cid) — 给无名簇推荐最像的注册身份（单质心口径，与生产同源计分）。
被 calibrator.py 以 importlib 动态加载；部署在管线目录（BASE）。
全部注册身份平等参与推荐。注册表缓存 600s。
"""
import json, os, sqlite3, threading, time
import numpy as np

BASE = os.environ.get('FACES_BASE', '/data/base')
MIN_TPL = 5
_cache = {'C': None, 'names': [], 'reps': [], 'tpln': [], 'at': 0.0, 'report_stamp': None}
_lock = threading.Lock()


def _sq(s):
    return ''.join(ch for ch in s if ch.isalnum()).lower()


def _build():
    t0 = time.time()
    report_stamp = os.stat(f'{BASE}/report.json').st_mtime_ns
    rep = json.load(open(f'{BASE}/report.json'))
    db = sqlite3.connect(f'{BASE}/faces.db')
    cents, names, reps, tpln = [], [], [], []
    for i in rep.get('identities', []):
        name = i.get('name') or ''
        if not name or i.get('tpl_faces', 0) < MIN_TPL:
            continue
        vids = [v['vid'] for v in (i.get('vids') or [])[:12]]
        if not vids:
            continue
        qs = ','.join('?' * len(vids))
        rows = db.execute(
            f"SELECT emb, thumb, score FROM faces f JOIN videos v ON v.vid=f.vid "
            f"WHERE f.vid IN ({qs}) AND v.status='done' ORDER BY f.score DESC LIMIT 60",
            vids).fetchall()
        if len(rows) < 3:
            continue
        E = np.stack([np.frombuffer(r[0], np.float32) for r in rows])
        E /= np.linalg.norm(E, axis=1, keepdims=True) + 1e-9
        c = E.mean(0)
        c /= np.linalg.norm(c) + 1e-9
        cents.append(c)
        names.append(name)
        reps.append('/thumb?p=' + rows[0][1])
        tpln.append(i.get('tpl_faces', 0))
    db.close()
    _cache['C'] = np.stack(cents) if cents else np.zeros((0, 512), np.float32)
    _cache['names'] = names
    _cache['reps'] = reps
    _cache['tpln'] = tpln
    _cache['at'] = time.time()
    _cache['report_stamp'] = report_stamp
    print(f'[suggest] registry: {len(names)} identities in {time.time()-t0:.1f}s', flush=True)


def _registry():
    with _lock:
        if _cache['C'] is None or time.time() - _cache['at'] > 600 or _cache['report_stamp'] != os.stat(f'{BASE}/report.json').st_mtime_ns:
            _build()
        try:
            with open(f'{BASE}/person-renames.json') as source:
                renames = json.load(source)
        except FileNotFoundError:
            renames = {}
        names = []
        for name in _cache['names']:
            seen = set()
            while isinstance(renames.get(name), str) and name not in seen:
                seen.add(name); name = renames[name]
            names.append(name)
        return _cache['C'], names, _cache['reps'], _cache['tpln']


def suggest(cid, topk=12):
    C, names, reps, tpln = _registry()
    cj = json.load(open(f'{BASE}/clusters.json'))
    cl = next((c for c in cj if c.get('cid') == cid), None)
    if cl is None:
        return {'error': 'cid not found'}
    vids = [v['vid'] for v in (cl.get('vids') or [])[:10]]
    if not vids or not len(C):
        return {'cid': cid, 'n_vids': cl.get('n_videos'), 'suggestions': []}
    db = sqlite3.connect(f'{BASE}/faces.db')
    qs = ','.join('?' * len(vids))
    rows = db.execute(
        f"SELECT emb FROM faces WHERE vid IN ({qs}) ORDER BY score DESC LIMIT 80",
        vids).fetchall()
    db.close()
    if not rows:
        return {'cid': cid, 'n_vids': cl.get('n_videos'), 'suggestions': []}
    E = np.stack([np.frombuffer(r[0], np.float32) for r in rows])
    E /= np.linalg.norm(E, axis=1, keepdims=True) + 1e-9
    S = E @ C.T                                  # (n_face, n_id)
    best_per_id = S.max(axis=0)                  # 每身份最高单脸命中
    top_faces = np.argsort(-S.max(axis=1))[:10]  # 簇内最有信息的 10 张脸
    score = S[top_faces].mean(axis=0) * 0.5 + best_per_id * 0.5
    order = np.argsort(-score)[:topk]
    return {'cid': cid, 'n_vids': cl.get('n_videos'),
            'suggestions': [{'name': names[i], 'sim': round(float(score[i]), 3),
                             'rep_thumb': reps[i], 'tpl_faces': tpln[i]} for i in order]}


def suggest_wall(min_sim=0.58, min_gap=0.03):
    """批量版：只返回高置信簇的 top1（sim≥min_sim 且 top1−top2≥min_gap），供卡片墙一键直选。"""
    C, names, reps, tpln = _registry()
    if not len(C):
        return {'suggest_wall': {}}
    cj = json.load(open(f'{BASE}/clusters.json'))
    db = sqlite3.connect(f'{BASE}/faces.db')
    out = {}
    for cl in cj:
        vids = [v['vid'] for v in (cl.get('vids') or [])[:10]]
        if not vids:
            continue
        qs = ','.join('?' * len(vids))
        try:
            rows = db.execute(
                f"SELECT emb FROM faces WHERE vid IN ({qs}) ORDER BY score DESC LIMIT 80",
                vids).fetchall()
        except Exception:
            continue
        if len(rows) < 3:
            continue
        E = np.stack([np.frombuffer(r[0], np.float32) for r in rows])
        E /= np.linalg.norm(E, axis=1, keepdims=True) + 1e-9
        S = E @ C.T
        top_faces = np.argsort(-S.max(axis=1))[:10]
        score = S[top_faces].mean(axis=0) * 0.5 + S.max(axis=0) * 0.5
        order = np.argsort(-score)
        i1 = order[0]
        gap = float(score[i1] - score[order[1]]) if len(order) > 1 else 1.0
        if float(score[i1]) >= min_sim and gap >= min_gap:
            out[cl.get('cid')] = {'name': names[i1], 'sim': round(float(score[i1]), 3),
                                  'rep_thumb': reps[i1]}
    db.close()
    return {'suggest_wall': out}
