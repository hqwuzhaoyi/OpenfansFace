#!/usr/bin/env python3
"""calibrator.py — numpy 校准 sidecar（替代 server.py 的 identity/groups、identity/pick、rebuild 端点）
纯 stdlib http.server：同一主机内 Next 容器经 CALIBRATOR_URL=http://127.0.0.1:8921 反代。
通过 FACES_BASE 配置容器内的人脸数据与管线目录。
"""
import json, os
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse, parse_qs

import sys

import numpy as np

BASE = os.environ.get('FACES_BASE', '/data/base')
if BASE not in sys.path:
    sys.path.insert(0, BASE)  # 容器内同路径挂载管线目录，tplgroups/identify_faces 同源可用
from tplgroups import mutual_groups

OVR = f'{BASE}/template-overrides.json'
PYBIN = os.environ.get('CAL_PYBIN', 'python3')  # sidecar 自带 numpy 的 python

REBUILD = {'running': False, 'done_at': None, 'ok': None, 'msg': ''}
from sidecar.native_metadata import Rebuilder, MetadataError
_NATIVE_REBUILD = Rebuilder(BASE, PYBIN, REBUILD)


def load_clusters():
    return json.load(open(f'{BASE}/clusters.json'))


def rel(vid):
    value = (vid or '').replace('\\', '/')
    return value.split('/pikpak/', 1)[1] if '/pikpak/' in value else value.lstrip('/')


def canonical_name(name):
    try:
        with open(f'{BASE}/person-renames.json') as source:
            renames = json.load(source)
    except FileNotFoundError:
        return name
    seen = set()
    while isinstance(renames.get(name), str) and name not in seen:
        seen.add(name)
        name = renames[name]
    return name


def identity_of(p):
    if p[0] == 'creators' and len(p) >= 3:
        return canonical_name(p[1])
    if p[0] == 'share':
        if len(p) >= 3 and p[1] == '收藏':
            return canonical_name(p[2])
        if len(p) >= 2:
            return canonical_name(p[1])
    if p[0] == 'My Telegram' and len(p) >= 3 and p[1].startswith('#'):
        return canonical_name(p[1].split()[0])
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
    name = canonical_name(name)
    import sqlite3
    con = sqlite3.connect(f'file:{BASE}/faces.db?mode=ro', uri=True)
    rows = con.execute("""SELECT f.emb, f.thumb, f.ts, f.vid, v.duration, v.path FROM faces f
                          JOIN videos v ON v.vid=f.vid
                          WHERE v.status='done' ORDER BY f.id""").fetchall()
    con.close()
    idxs = [i for i, r in enumerate(rows) if identity_of(rel(r[5] or r[3]).split('/')) == canonical_name(name)]
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
    return _NATIVE_REBUILD.pick(payload)


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
        except MetadataError as error:
            self._send(error.status, {'error': str(error), 'operation_id': error.operation_id})
        except Exception as e:
            self._send(500, {'error': str(e)})

    def log_message(self, fmt, *a):
        pass


if __name__ == '__main__':
    port = int(os.environ.get('CAL_PORT', '8921'))
    print(f'calibrator listening :{port}', flush=True)
    ThreadingHTTPServer(('0.0.0.0', port), Handler).serve_forever()
