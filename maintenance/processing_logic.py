"""Authoritative evidence resolution delegated to Rust (services/file-operations/src/processing_result.rs)."""
import json
import os
from pathlib import Path
import subprocess
from processing_publication import native

REPO = Path(__file__).resolve().parents[1]
MANIFEST = REPO / 'services/file-operations/Cargo.toml'
BINARY = Path(os.environ.get('FILE_OPERATIONS_RUST_BIN', REPO / 'services/file-operations/target/debug/face-file-operations'))


def _evaluate(payload):
    if os.environ.get('FILE_OPERATIONS_RUST_URL'):
        return native('processing-evaluate-rules', payload)
    if not BINARY.exists():
        subprocess.run(['cargo', 'build', '--quiet', '--manifest-path', str(MANIFEST)], check=True)
    proc = subprocess.run([str(BINARY), '--evaluate-rules', json.dumps(payload, ensure_ascii=False)],
                          capture_output=True, text=True, check=True)
    return json.loads(proc.stdout)


def folder_hints(file, people):
    return _evaluate({'op': 'folder_hints', 'file': str(file), 'people': list(people)})


def candidates_from_text(watermark, mappings, folder):
    return _evaluate({'op': 'candidates_from_text', 'watermark': watermark, 'mappings': mappings, 'folder': list(folder)})


def can_skip_face(candidates, legacy_names=()):
    return _evaluate({'op': 'can_skip_face', 'candidates': list(candidates), 'legacy_names': list(legacy_names)})


def resolve_candidates(candidates):
    return _evaluate({'op': 'resolve_candidates', 'candidates': list(candidates)})


def evaluate_stage(base, task, data):
    return _evaluate({'op': 'stage', 'base': str(base), 'task': dict(task), 'data': data})

