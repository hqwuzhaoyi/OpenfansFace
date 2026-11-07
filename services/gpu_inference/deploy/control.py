"""Authenticated release health and drain checks. Never changes system services."""
import argparse
import json
import pathlib
import time

from services.gpu_inference.client import Client
from services.gpu_inference.release import health_release, verify_manifest


def verify_release_health(health, manifest):
    if health.get('release') != health_release(manifest):
        raise RuntimeError('remote_release_mismatch')
    execution=health.get('execution')
    if health.get('model_ready'):
        if (not isinstance(execution,dict) or execution.get('model_sha256')!=manifest['model_sha256'] or
                execution.get('preprocess_version')!=manifest['preprocess_version'] or
                execution.get('provider_proof')!='startup_profile' or
                'CUDAExecutionProvider' not in execution.get('providers',[]) or
                execution.get('cuda_kernel_events',0)<=0):
            raise RuntimeError('remote_model_readiness_mismatch')
    return health


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action',choices=('health','drain'))
    parser.add_argument('--origin',default='http://127.0.0.1:8921')
    parser.add_argument('--token-file',type=pathlib.Path,required=True)
    parser.add_argument('--manifest',required=True)
    parser.add_argument('--require-ready',action='store_true')
    parser.add_argument('--timeout',type=float,default=120)
    args=parser.parse_args()
    if not 0<args.timeout<=120:parser.error('timeout must be within 120 seconds')
    if args.token_file.stat().st_mode&0o077:parser.error('token file must be private')
    manifest=verify_manifest(args.manifest)
    client=Client(args.origin,args.token_file.read_text().strip(),timeout=min(args.timeout,10))
    health=verify_release_health(client.ready(),manifest)
    if args.action=='drain':
        instance=health['service_instance'];client.drain();deadline=time.monotonic()+args.timeout
        while True:
            health=verify_release_health(client.ready(),manifest)
            if health['service_instance']!=instance:raise RuntimeError('service_restarted_during_drain')
            if health.get('drained'):break
            if time.monotonic()>=deadline:raise RuntimeError('drain_timeout_execution_not_confirmed_stopped')
            time.sleep(min(.5,max(0,deadline-time.monotonic())))
    print(json.dumps(health,sort_keys=True))
    return 2 if args.require_ready and not health.get('accepting') else 0


if __name__=='__main__':raise SystemExit(main())
