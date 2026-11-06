#!/usr/bin/env python3
"""calibrator.py — numpy 校准 sidecar（替代 server.py 的 identity/groups、identity/pick、rebuild 端点）
纯 stdlib http.server：同一主机内 Next 容器经 CALIBRATOR_URL=http://127.0.0.1:8921 反代。
通过 FACES_BASE 配置容器内的人脸数据与管线目录。
"""
import json, os, subprocess, threading, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse, parse_qs

import numpy as np

from tplgroups import mutual_groups

BASE = os.environ.get('FACES_BASE', '/data/base')
OVR = f'{BASE}/template-overrides.json'
PYBIN = os.environ.get('CAL_PYBIN', 'python3')  # sidecar 自带 numpy 的 python

REBUILD = {'running': False, 'done_at': None, 'ok': None, 'msg': ''}


def load_clusters():
    return json.load(open(f'{BASE}/clusters.json'))


def rel(vid):
    w = vid.split('\\', 1)[1].replace('\\', '/')
    if 'media/pikpak/' in w:
        w = w.split('media/pikpak/', 1)[1]
    return w


def identity_of(p):
    if p[0] == 'creators' and len(p) >= 3:
        return p[1]
    if p[0] == 'share':
        if len(p) >= 3 and p[1] == '收藏':
            return p[2]
        if len(p) >= 2:
            return p[1]
    if p[0] == 'My Telegram' and len(p) >= 3 and p[1].startswith('#'):
        return p[1].split()[0]
    return None


def _load_override():
    try:
        return json.load(open(OVR))
    except Exception:
        return {}


def _picked_index(name):
    ov = _load_override().get(name)
    if isinstance(ov, dict):
        return ov.get('groups')
    return None


def identity_groups(name):
    import sqlite3
    con = sqlite3.connect(f'{BASE}/faces.db')
    rows = con.execute("""SELECT f.emb, f.thumb, f.ts, f.vid, v.duration FROM faces f
                          JOIN videos v ON v.vid=f.vid
                          WHERE v.status='done' ORDER BY f.id""").fetchall()
    con.close()
    idxs = [i for i, r in enumerate(rows) if identity_of(rel(r[3]).split('/')) == name]
    if not idxs:
        return {'name': name, 'groups': [], 'picked': None, 'note': '该身份没有文件夹脸（纯认领）'}
    E = np.stack([np.frombuffer(rows[i][0], np.float32) for i in idxs])
    E /= np.linalg.norm(E, axis=1, keepdims=True) + 1e-9
    gs = mutual_groups(E)
    out = []
    for gi, mem in enumerate(gs):
        if len(mem) < 2:
            break
        sub = [idxs[m] for m in mem]
        c = E[mem].mean(0)
        c /= np.linalg.norm(c) + 1e-9
        coh = E[mem] @ c
        rep = sub[int(np.argmax(coh))]
        out.append({'gi': gi, 'n_faces': len(mem),
                    'n_vids': len({rows[i][3] for i in sub}),
                    'coh_min': round(float(coh.min()), 3),
                    'coh_mean': round(float(coh.mean()), 3),
                    'rep_thumb': '/thumb?p=' + rows[rep][1],
                    'rep_vid': rows[rep][3], 'rep_ts': rows[rep][2]})
    return {'name': name, 'groups': out, 'picked': _picked_index(name)}


def identity_pick(payload):
    name = payload.get('name')
    groups = payload.get('groups')
    if not name or groups is None:
        return {'error': 'name/groups required'}
    cur = _load_override()
    cur[name] = {'groups': groups, 'at': time.strftime('%F %T')}
    with open(OVR, 'w') as f:
        json.dump(cur, f, ensure_ascii=False, indent=1)
    if REBUILD['running']:
        return {'queued': True, 'running': True}
    REBUILD.update(running=True, ok=None, msg='starting', done_at=None)

    def run():
        try:
            r = subprocess.run([PYBIN, f'{BASE}/identify_faces.py'],
                               capture_output=True, text=True, timeout=900, cwd=BASE)
            REBUILD['ok'] = r.returncode == 0
            REBUILD['msg'] = ((r.stdout or '') + (r.stderr or ''))[-500:]
        except Exception as e:
            REBUILD['ok'] = False
            REBUILD['msg'] = str(e)
        REBUILD['running'] = False
        REBUILD['done_at'] = time.strftime('%T')

    threading.Thread(target=run, daemon=True).start()
    return {'started': True}


# ==== cluster suggest（给无名簇推荐最像的注册身份，与 identify_faces.py 同源计分）====
_SUG = {'mod': None}


def _suggest_mod():
    if _SUG['mod'] is None:
        import importlib.util
        spec = importlib.util.spec_from_file_location('suggest', f'{BASE}/sidecar-suggest.py')
        m = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(m)
        _SUG['mod'] = m
    return _SUG['mod']


def cluster_suggest(cid, topk=12):
    try:
        return _suggest_mod().suggest(cid, topk)
    except Exception as e:
        return {'error': str(e)}


def cluster_suggest_wall():
    try:
        return _suggest_mod().suggest_wall()
    except Exception as e:
        return {'error': str(e)}


class Handler(BaseHTTPRequestHandler):
    def _send(self, code, obj):
        body = json.dumps(obj, ensure_ascii=False).encode()
        self.send_response(code)
        self.send_header('Content-Type', 'application/json; charset=utf-8')
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        u = urlparse(self.path)
        if u.path == '/api/rebuild':
            self._send(200, REBUILD)
        elif u.path == '/api/cluster/suggest':
            qs = parse_qs(u.query)
            cid = qs.get('cid', [None])[0]
            if not cid:
                self._send(400, {'error': 'cid required'})
                return
            try:
                self._send(200, cluster_suggest(cid))
            except Exception as e:
                self._send(500, {'error': str(e)})
        elif u.path == '/api/cluster/suggest_wall':
            try:
                self._send(200, cluster_suggest_wall())
            except Exception as e:
                self._send(500, {'error': str(e)})
        elif u.path == '/api/identity/groups':
            name = parse_qs(u.query).get('name', [None])[0]
            if not name:
                self._send(400, {'error': 'name required'})
                return
            try:
                self._send(200, identity_groups(name))
            except Exception as e:
                self._send(500, {'error': str(e)})
        elif u.path == '/health':
            self._send(200, {'ok': True})
        else:
            self._send(404, {'error': 'not found'})

    def do_POST(self):
        u = urlparse(self.path)
        if u.path != '/api/identity/pick':
            self._send(404, {'error': 'not found'})
            return
        n = int(self.headers.get('Content-Length', 0))
        try:
            payload = json.loads(self.rfile.read(n) or b'{}')
        except Exception:
            self._send(400, {'error': 'bad json'})
            return
        try:
            self._send(200, dict(identity_pick(payload), _cal_ok=True))
        except Exception as e:
            self._send(500, {'error': str(e)})

    def log_message(self, fmt, *a):
        pass


if __name__ == '__main__':
    port = int(os.environ.get('CAL_PORT', '8921'))
    print(f'calibrator listening :{port}', flush=True)
    ThreadingHTTPServer(('0.0.0.0', port), Handler).serve_forever()
