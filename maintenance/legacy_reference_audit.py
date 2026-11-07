#!/usr/bin/env python3
"""Bounded, read-only legacy-cache compatibility preflight. No NumPy/model imports.

Only version/path/size/mtime/model-hash scalar NPY members are opened. Feature
vectors, images and media contents are never read. Historical paths alone never
become movement proof. Output contains aggregates and hashed identifiers only.
"""
import argparse
import ast
import collections
import hashlib
import json
import os
from pathlib import Path
import re
import sqlite3
import stat
import time
import urllib.error
import urllib.request
import zipfile

VERSION = 'buffalo_l-det-rec-8frames-v1'
MODELS = {'det_10g.onnx': '5838f7fe053675b1c7a08b633df49e7af5495cee0493c7dcf6697200b85b5b91',
          'w600k_r50.onnx': '4c06341c33c2ca1f86781dab0e829f88ad5b64be9fba56e56bc9ebdefc619e43'}
LIMITS = {'cache_files': 1500, 'catalog_rows': 50000, 'stat_calls': 5000,
          'directory_entries': 5000, 'json_bytes': 32 * 1024**2, 'zip_bytes': 64 * 1024**2,
          'scalar_bytes': 16384, 'proof_calls': 128, 'wall_seconds': 90}
SCALARS = frozenset(('version.npy', 'path.npy', 'size.npy', 'mtime_ns.npy', 'model_hashes.npy'))


def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(',', ':'), ensure_ascii=False).encode()).hexdigest()


def identity(s):
    return {k: str(getattr(s, 'st_' + k)) for k in ('dev', 'ino', 'size', 'mtime_ns')}


def scalar(archive, key):
    if key not in SCALARS:
        raise ValueError('forbidden_member')
    item = archive.getinfo(key)
    if item.file_size > LIMITS['scalar_bytes']:
        raise ValueError('scalar_bound')
    with archive.open(item) as source:
        raw = source.read(LIMITS['scalar_bytes'] + 1)
    if len(raw) != item.file_size or raw[:6] != b'\x93NUMPY':
        raise ValueError('scalar_format')
    offset = 10 if raw[6] == 1 else 12 if raw[6] in (2, 3) else 0
    length = int.from_bytes(raw[8:offset], 'little') if offset else 0
    if not offset or length > 4096 or offset + length > len(raw):
        raise ValueError('scalar_header')
    header = ast.literal_eval(raw[offset:offset + length].decode('utf8'))
    match = re.fullmatch(r'([<>=|])([USiu])(\d+)', header['descr'])
    if not match or header['shape'] not in ((), (1,)):
        raise ValueError('scalar_type')
    endian, kind, width = match[1], match[2], int(match[3])
    body = raw[offset + length:]
    if len(body) != width * (4 if kind == 'U' else 1):
        raise ValueError('scalar_size')
    if kind == 'U':
        return body.decode('utf-32-be' if endian == '>' else 'utf-32-le').rstrip('\0')
    if kind == 'S':
        return body.decode('utf8').rstrip('\0')
    if width not in (1, 2, 4, 8):
        raise ValueError('scalar_integer')
    return int.from_bytes(body, 'big' if endian == '>' else 'little', signed=kind == 'i')


def origin_client(url, token=None):
    def prove(body, timeout):
        headers = {'Content-Type': 'application/json'}
        if token:
            headers['Authorization'] = 'Bearer ' + token
        req = urllib.request.Request(url.rstrip('/') + '/v1/reference-origin',
            data=json.dumps(body).encode(), headers=headers)
        with urllib.request.urlopen(req, timeout=timeout) as response:
            result = response.read(65537)
            if len(result) > 65536:
                raise ValueError('proof_response_bound')
            return json.loads(result)
    return prove


class Audit:
    def __init__(self, base, media, prover):
        self.base, self.media, self.prover = Path(base), Path(media), prover
        self.started = time.monotonic()
        self.stats = self.proofs = 0
        self.inputs, self.files, self.databases = [], {}, []
        self.counts = collections.Counter(assigned_caches=0, direct=0, native_proven=0, unresolved=0, unknown=0)
        self.cases = []

    def deadline(self):
        remaining = LIMITS['wall_seconds'] - (time.monotonic() - self.started)
        if remaining <= 0:
            raise ValueError('wall_budget')
        return remaining

    def observe(self, path, optional=False):
        self.deadline(); self.stats += 1
        if self.stats > LIMITS['stat_calls']:
            raise ValueError('stat_budget')
        try:
            item = Path(path).lstat()
        except FileNotFoundError:
            if not optional:
                raise
            if str(path) in self.files and self.files[str(path)] is not None:
                raise ValueError('inputs_changed')
            self.files[str(path)] = None
            return None
        if stat.S_ISLNK(item.st_mode) or Path(os.path.realpath(path)) != Path(path):
            raise ValueError('symlink')
        if str(path) in self.files and self.files[str(path)] != identity(item):
            raise ValueError('inputs_changed')
        self.files[str(path)] = identity(item)
        return item

    def document(self, name, default):
        path = self.base / name
        before = self.observe(path, optional=True)
        if before is None:
            self.inputs.append([name, None]); return default
        if not stat.S_ISREG(before.st_mode) or before.st_size > LIMITS['json_bytes']:
            raise ValueError('document_bound_or_type')
        fd = os.open(path, os.O_RDONLY | os.O_NONBLOCK | os.O_NOFOLLOW)
        with os.fdopen(fd, 'rb') as source:
            raw = source.read(LIMITS['json_bytes'] + 1)
            if identity(os.fstat(source.fileno())) != identity(before) or len(raw) > LIMITS['json_bytes']:
                raise ValueError('document_changed')
        result = json.loads(raw)
        if type(result) is not type(default):
            raise ValueError('document_shape')
        self.inputs.append([name, hashlib.sha256(raw).hexdigest()])
        return result

    def rows(self, name, table, optional=False, record=True):
        path = self.base / name
        before = self.observe(path, optional=optional)
        if before is None:
            return []
        if not stat.S_ISREG(before.st_mode):
            raise ValueError('database_type')
        with sqlite3.connect(path.as_uri() + '?mode=ro', uri=True, timeout=2) as db:
            db.execute('PRAGMA query_only=ON')
            db.set_progress_handler(lambda: int(time.monotonic() - self.started > LIMITS['wall_seconds']), 1000)
            # Closed table set. No embedding column is selected, even for counts.
            if table not in ('videos', 'tasks'):
                raise ValueError('forbidden_table')
            result = db.execute('SELECT vid,path FROM ' + table + ' LIMIT ?', (LIMITS['catalog_rows'] + 1,)).fetchall()
        if len(result) > LIMITS['catalog_rows'] or any(not isinstance(v, str) or not isinstance(p, str) for v, p in result):
            raise ValueError('catalog_bound_or_shape')
        self.files.pop(str(path),None)  # Progress-only SQLite writes are not source changes.
        if record:
            self.databases.append((name,table,sorted(result)))
            self.inputs.append([name, sorted(result)])
        return result

    def mapped(self, value):
        if not isinstance(value, str):
            return None
        value = value.replace('\\', '/')
        if '..' in value.split('/'):
            return None
        prefix = str(self.media).rstrip('/') + '/'
        if value.startswith(prefix):
            return value
        if '/media/' in value:
            return prefix + value.split('/media/', 1)[1]
        return None

    def run(self):
        queue = self.document('integration-queue.json', [])
        history = self.document('archive-history.json', [])
        aliases = self.document('person-renames.json', {})
        overrides = self.document('video-path-overrides.json', {})
        if max(len(queue), len(history), len(aliases), len(overrides)) > LIMITS['catalog_rows']:
            raise ValueError('assignment_bound')
        assigned = {}
        for row in queue + history:
            if not isinstance(row, dict) or not isinstance(row.get('vid'), str) or not isinstance(row.get('creator'), str):
                raise ValueError('assignment_shape')
            if row.get('file_state') != 'staged':
                assigned[row['vid']] = row['creator']
        sources = dict(self.rows('faces.db', 'videos'))
        if any(not isinstance(k, str) or not isinstance(v, str) for k, v in overrides.items()):
            raise ValueError('override_shape')
        sources.update(overrides)
        for vid, path in self.rows('processing/processing.db', 'tasks', optional=True):
            sources.setdefault(vid, path)
        for vid, path in sources.items():
            current = self.mapped(path) or self.mapped(vid)
            prefix = str(self.media) + '/pikpak/creators/'
            if current and current.startswith(prefix):
                assigned.setdefault(vid, current[len(prefix):].split('/')[0])
        by_hash = {hashlib.sha256(vid.encode()).hexdigest(): vid for vid in assigned}
        cache_dir = self.base / 'processing/faces'
        files = []
        directory = self.observe(cache_dir, optional=True)
        if directory is not None and not stat.S_ISDIR(directory.st_mode):
            raise ValueError('cache_directory_type')
        # Missing cache directory is a valid empty legacy inventory; unreadable is unknown.
        try:
            with os.scandir(cache_dir) as entries:
                for index, entry in enumerate(entries):
                    self.deadline()
                    if index >= LIMITS['directory_entries']:
                        raise ValueError('directory_budget')
                    if entry.name.endswith('.npz'):
                        files.append(Path(entry.path))
                        if len(files) > LIMITS['cache_files']:
                            raise ValueError('cache_inventory_bound')
        except FileNotFoundError:
            pass
        self.inputs.append(['cache_inventory', sorted(p.name for p in files)])
        self.observe(self.base / 'processing/references.npz', optional=True)
        for cache in sorted(files):
            vid = by_hash.get(cache.stem)
            if vid is None:
                continue
            self.counts['assigned_caches'] += 1
            case = {'vid_hash': hashlib.sha256(vid.encode()).hexdigest()[:20], 'person_hash': hashlib.sha256(assigned[vid].encode()).hexdigest()[:20]}
            try:
                before = self.observe(cache)
                if not stat.S_ISREG(before.st_mode) or before.st_size > LIMITS['zip_bytes']:
                    raise ValueError('cache_bound_or_type')
                fd = os.open(cache, os.O_RDONLY | os.O_NONBLOCK | os.O_NOFOLLOW)
                with os.fdopen(fd, 'rb') as source, zipfile.ZipFile(source) as archive:
                    if len(archive.infolist()) > 32:
                        raise ValueError('cache_member_bound')
                    meta = {k: scalar(archive, k + '.npy') for k in ('version', 'path', 'size', 'mtime_ns')}
                    model = scalar(archive, 'model_hashes.npy') if 'model_hashes.npy' in archive.namelist() else None
                    if identity(os.fstat(source.fileno())) != identity(before):
                        raise ValueError('cache_changed')
                self.inputs.append([cache.name, meta, model])
                current = source_stat = None
                for value in (sources.get(vid), vid):
                    candidate = self.mapped(value)
                    if candidate:
                        candidate_stat = self.observe(candidate, optional=True)
                        if candidate_stat is not None and stat.S_ISREG(candidate_stat.st_mode):
                            current, source_stat = candidate, candidate_stat; break
                if current is None:
                    reason = 'source_unresolved'
                elif str(meta['version']) != VERSION or (model is not None and json.loads(model) != MODELS):
                    reason = 'model_metadata'
                elif str(meta['size']) != str(source_stat.st_size) or str(meta['mtime_ns']) != str(source_stat.st_mtime_ns):
                    reason = 'source_fingerprint'
                elif str(meta['path']) == current:
                    self.counts['direct'] += 1; continue
                elif self.prover is None:
                    reason = 'native_origin_unproven'
                else:
                    self.proofs += 1
                    if self.proofs > LIMITS['proof_calls']:
                        raise ValueError('proof_budget')
                    body = {'vid': vid, 'source': current, 'origin': {'path': str(meta['path']), 'size': str(meta['size']), 'mtime_ns': str(meta['mtime_ns'])}, 'current_identity': identity(source_stat)}
                    try:
                        proof = self.prover(body, min(5, self.deadline()))
                    except urllib.error.HTTPError as error:
                        if error.code == 409:
                            proof = None
                        else:
                            raise
                    if isinstance(proof, dict) and proof.get('metadata_only') is True and proof.get('verified') is True and proof.get('current_identity') == identity(source_stat) and re.fullmatch('[a-f0-9]{64}', str(proof.get('version', ''))):
                        self.inputs.append(['origin_proof', case['vid_hash'], proof['version']])
                        self.counts['native_proven'] += 1; continue
                    reason = 'native_origin_unproven'
                case.update(status='unresolved', reason=reason); self.counts['unresolved'] += 1
            except Exception as error:
                case.update(status='unknown', reason=type(error).__name__); self.counts['unknown'] += 1
            self.cases.append(case)
        # Re-read only scalar source rows, never hash the whole SQLite file or
        # invalidate compatibility because unrelated progress updated its mtime.
        for name, table, previous in list(self.databases):
            if sorted(self.rows(name, table, record=False)) != previous:
                raise ValueError('source_rows_changed')
        # No mutation is made if the source set changes while the snapshot is read.
        for path, old in list(self.files.items()):
            item = self.observe(path, optional=True)
            if (identity(item) if item else None) != old:
                raise ValueError('inputs_changed')
        return {'audit_schema': 1, 'complete': True, 'status': 'failed' if self.counts['unresolved'] else 'unknown' if self.counts['unknown'] else 'passed',
                'input_version': digest([self.inputs, sorted(self.files.items())]), 'counts': dict(self.counts), 'cases': self.cases,
                'limits': LIMITS, 'stat_calls': self.stats, 'metadata_only': True, 'numerical_reference_effect': 'not_evaluated'}


def audit(base, media, prover=None):
    try:
        base, media = Path(base), Path(media)
        if not base.is_absolute() or not media.is_absolute():
            raise ValueError('absolute_roots_required')
        return Audit(base, media, prover).run()
    except Exception as error:
        return {'audit_schema': 1, 'complete': False, 'status': 'unknown', 'metadata_only': True,
                'reason': type(error).__name__, 'counts': None, 'numerical_reference_effect': 'not_evaluated'}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--base', default=os.environ.get('FACES_BASE'))
    parser.add_argument('--media-root', default=os.environ.get('MEDIA2_ROOT'))
    args = parser.parse_args()
    if not args.base or not args.media_root:
        parser.error('provide --base/--media-root or FACES_BASE/MEDIA2_ROOT')
    url = os.environ.get('FILE_OPERATIONS_RUST_URL')
    report = audit(args.base, args.media_root, origin_client(url, os.environ.get('FILE_OPERATIONS_RUST_TOKEN')) if url else None)
    print(json.dumps(report, ensure_ascii=False, indent=2))
    raise SystemExit(0 if report['status'] == 'passed' else 1)
