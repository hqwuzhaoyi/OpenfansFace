"""Build a new immutable-style release directory; never change an existing venv.

Run with the target Linux x86_64 CPython 3.12 interpreter. This does not install,
start, enable, or replace a system service or the current-release symlink.
"""
import argparse
import json
import pathlib
import platform
import shutil
import subprocess
import sys
import venv


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--destination', type=pathlib.Path, required=True)
    parser.add_argument('--release-id', required=True)
    parser.add_argument('--revision', required=True)
    parser.add_argument('--model-sha256', required=True)
    args = parser.parse_args()
    destination = args.destination.absolute()
    if destination.exists() or destination.is_symlink():
        parser.error('destination must be a new path')
    if sys.version_info[:2] != (3,12) or not sys.platform.startswith('linux') or platform.machine() != 'x86_64':
        parser.error('build with target Linux CPython 3.12')
    destination.mkdir(parents=True, exist_ok=False)
    try:
        package = destination/'services/gpu_inference'
        shutil.copytree(pathlib.Path(__file__).resolve().parents[1], package,
                        ignore=shutil.ignore_patterns('__pycache__','*.pyc'))
        venv.EnvBuilder(with_pip=True).create(destination/'venv')
        python = str(destination/'venv/bin/python')
        # --isolated ignores pip user/environment indexes; hashes and --no-deps
        # enforce the complete lock and the documented ORT provider substitution.
        subprocess.run([python,'-m','pip','--isolated','install','--no-deps','--require-hashes',
                        '--only-binary=:all:','--no-cache-dir','--no-compile','-r',str(package/'deploy/requirements.lock')],check=True)
        report = subprocess.check_output([python,'-m','services.gpu_inference.environment'],cwd=destination,text=True)
        (destination/'environment.json').write_text(report)
        manifest = subprocess.check_output([python,'-m','services.gpu_inference.release','--release-id',args.release_id,
                    '--revision',args.revision,'--model-sha256',args.model_sha256],cwd=destination,text=True)
        (destination/'release.json').write_text(manifest)
        print(json.dumps({'built':True,'release_id':args.release_id,'manifest':json.loads(manifest)},sort_keys=True))
    except BaseException:
        # Only the new directory created by this invocation is removed on failure.
        shutil.rmtree(destination)
        raise


if __name__=='__main__':
    main()
