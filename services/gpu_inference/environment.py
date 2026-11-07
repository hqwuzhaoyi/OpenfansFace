"""Validate the locked GPU environment without initializing a model/CUDA context."""
from __future__ import annotations

import hashlib
import importlib.metadata as metadata
import json
import pathlib
import platform
import re
import sys

LOCK = pathlib.Path(__file__).parent / 'deploy/requirements.lock'


def normalize(name):
    return re.sub(r'[-_.]+', '-', name).lower()


def lock_versions(path=LOCK):
    return {normalize(name): version for name, version in
            re.findall(r'^([a-zA-Z0-9_.-]+)==([^\s\\;]+)', pathlib.Path(path).read_text(), re.MULTILINE)}


def check_environment(path=LOCK, require_venv=True):
    if require_venv and sys.prefix == sys.base_prefix:
        raise RuntimeError('isolated_virtualenv_required')
    if sys.version_info[:2] != (3, 12) or platform.system() != 'Linux' or platform.machine() != 'x86_64':
        raise RuntimeError('unsupported_runtime_platform')
    expected = lock_versions(path)
    if not expected or 'onnxruntime' in expected or 'onnxruntime-gpu' not in expected:
        raise RuntimeError('invalid_gpu_lock')
    installed = {normalize(d.metadata['Name']): d for d in metadata.distributions()}
    if 'onnxruntime' in installed:
        raise RuntimeError('cpu_gpu_distribution_overlap')
    for name, version in expected.items():
        if name not in installed or installed[name].version != version:
            raise RuntimeError('locked_distribution_mismatch:' + name)
    unexpected = set(installed) - set(expected) - {'pip', 'setuptools'}
    if unexpected:
        raise RuntimeError('unexpected_distributions:' + ','.join(sorted(unexpected)))
    from packaging.markers import default_environment
    from packaging.requirements import Requirement
    marker_environment = {**default_environment(), 'extra': ''}
    substitutions = []
    for name in expected:
        for spec in installed[name].requires or []:
            requirement = Requirement(spec)
            if requirement.marker and not requirement.marker.evaluate(marker_environment):
                continue
            dependency = normalize(requirement.name)
            if name == 'insightface' and dependency == 'onnxruntime':
                dependency = 'onnxruntime-gpu'
                substitutions.append('insightface:onnxruntime=>onnxruntime-gpu')
            if dependency not in expected or not requirement.specifier.contains(expected[dependency]):
                raise RuntimeError('unsatisfied_dependency:' + name + ':' + dependency)
    import cv2
    import numpy
    import onnxruntime as ort
    from PIL import Image
    from insightface.model_zoo.scrfd import SCRFD
    owner = pathlib.Path(installed['onnxruntime-gpu'].locate_file('onnxruntime/__init__.py')).resolve()
    if pathlib.Path(ort.__file__).resolve() != owner or ort.__version__ != expected['onnxruntime-gpu']:
        raise RuntimeError('runtime_module_owner_mismatch')
    if 'CUDAExecutionProvider' not in ort.get_available_providers():
        raise RuntimeError('cuda_provider_missing')
    # This is provider availability only, NOT actual GPU execution readiness.
    return {'python': platform.python_version(), 'platform': 'linux-x86_64',
            'lock_sha256': hashlib.sha256(pathlib.Path(path).read_bytes()).hexdigest(),
            'packages': expected, 'dependency_substitutions': substitutions,
            'available_providers': ort.get_available_providers(), 'cuda_execution_proven': False}


if __name__ == '__main__':
    print(json.dumps(check_environment(), sort_keys=True))
