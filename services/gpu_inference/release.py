"""Content-addressed release metadata; no business or model files are copied."""
from __future__ import annotations

import argparse
import hashlib
import json
import pathlib
import re

from .protocol import SCHEMA, canonical, sha, token

PACKAGE = pathlib.Path(__file__).parent


def code_digest(package=PACKAGE):
    # Include the exact executable package and lock; bytecode and generated manifests
    # are excluded. Relative names make the digest independent of private paths.
    files = sorted(p for p in pathlib.Path(package).rglob('*') if p.is_file() and
                   (p.suffix == '.py' or p.name in {'requirements.lock','requirements.in','face-gpu.service','model.env.example'}))
    return hashlib.sha256(canonical([
        [str(p.relative_to(package)), hashlib.sha256(p.read_bytes()).hexdigest()] for p in files
    ])).hexdigest()


def create_manifest(release_id, revision, model_sha256, package=PACKAGE):
    token(release_id); sha(model_sha256)
    if not re.fullmatch(r'[a-f0-9]{40}', revision):
        raise ValueError('full_git_revision_required')
    return {'manifest_schema': 1, 'release_id': release_id, 'revision': revision,
            'protocol_schema': SCHEMA, 'code_sha256': code_digest(package),
            'lock_sha256': hashlib.sha256((pathlib.Path(package)/'deploy/requirements.lock').read_bytes()).hexdigest(),
            'model_sha256': model_sha256, 'stage': 'detection', 'detector_size': 640,
            'preprocess_version': 'scrfd-640-bgr8-det0.5-nms0.4-v1',
            'python_abi': 'cp312', 'platform': 'linux-x86_64',
            'admission': {'min_free_mib': 600, 'max_utilization_pct': 25, 'arena_mib': 128,
                          'concurrency': 1, 'waiting': 1}}


def verify_manifest(path, package=PACKAGE):
    from .protocol import decode_json
    value = decode_json(pathlib.Path(path).read_bytes())
    expected = create_manifest(value.get('release_id'), value.get('revision'), value.get('model_sha256'), package)
    if value != expected:
        raise ValueError('release_manifest_mismatch')
    return {**value, 'manifest_sha256': hashlib.sha256(canonical(value)).hexdigest()}


def health_release(value):
    if value is None:
        return {'managed': False}
    return {'managed': True, **{key: value[key] for key in
            ('release_id', 'revision', 'manifest_sha256', 'code_sha256', 'lock_sha256')}}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--release-id', required=True)
    parser.add_argument('--revision', required=True)
    parser.add_argument('--model-sha256', required=True)
    args = parser.parse_args()
    print(json.dumps(create_manifest(args.release_id,args.revision,args.model_sha256),indent=2,sort_keys=True))


if __name__ == '__main__':
    main()
