#!/usr/bin/env python3
"""Versioned native release controller. Defaults to checks; never runs remote SSH.

Run on the deployment host with a private, reviewed manifest. All replacements
are individually journaled, not an atomic multi-service transaction. A failed
step leaves admission closed and records actual completed steps.
"""
import argparse
import contextlib
import fcntl
import hashlib
import json
import os
from pathlib import Path
import shutil
import sqlite3
import subprocess
import time
import urllib.request

ROLES = ('native', 'worker', 'calibrator', 'web')
# GPU runs on a different WSL host under systemd; this controller never pretends it is local Docker.
GATES = ('isolated_contracts', 'full_peach_schema', 'nfs_identity_permissions',
         'real_readonly_parity', 'gpu_full_640_coexistence', 'worker_native_joint',
         'review_continuity', 'accepted_ui_baseline', 'writer_retirement')


def fail(message):
    raise RuntimeError(message)


def private_json(path, value):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, 'w') as out:
        json.dump(value, out, ensure_ascii=False, indent=2)
        out.flush(); os.fsync(out.fileno())


def run(*args):
    # Never log inspect/config output: it contains private env and may contain tokens.
    completed=subprocess.run(args,text=True,capture_output=True)
    if completed.returncode: fail('command failed: '+args[0]+' '+args[1]+' (exit '+str(completed.returncode)+'); inspect private service logs')
    return completed.stdout


def inspect(name):
    return json.loads(run('docker', 'inspect', name))[0]


def api(manifest, path, body=None):
    headers = {'Content-Type': 'application/json'}
    token = os.environ.get(manifest['native_token_env'])
    if not token:
        fail('native token environment variable is missing')
    headers['Authorization'] = 'Bearer ' + token
    request = urllib.request.Request(manifest['native_url'].rstrip('/') + path,
                                    data=None if body is None else json.dumps(body).encode(), headers=headers)
    with urllib.request.urlopen(request, timeout=10) as response:
        return json.load(response)


def validate(manifest, release=False):
    if manifest.get('version') != 1 or set(manifest.get('services', {})) != set(ROLES):
        fail('manifest version or four local service roles are invalid')
    gpu=manifest.get('gpu_peer',{})
    if gpu.get('adapter')!='external-systemd' or not gpu.get('unit') or not gpu.get('url') or not gpu.get('expected_health'):
        fail('GPU requires a frozen external systemd peer contract')
    if manifest.get('native_schema') != 9:
        fail('this controller requires schema 9 admission persistence')
    for role, service in manifest['services'].items():
        if (not service.get('expected_id') or not service.get('expected_image')) and not (role == 'native' and service.get('initially_absent') is True):
            fail(f'{role}: freeze current container identity and image first')
        args = service.get('create_args', [])
        if not args or '--name' not in args or args[args.index('--name') + 1] != service.get('name'):
            fail(f'{role}: candidate create args must name the frozen service')
        digest = service.get('candidate_image', '')
        if not digest.startswith('sha256:') or digest not in args:
            fail(f'{role}: candidate must use a local immutable image ID')
        if any('FILE_OPERATIONS_FIXTURE_ROOT' in a or 'FILE_OPERATIONS_PARITY_ROOT' in a for a in args):
            fail(f'{role}: fixture/parity configuration is forbidden in production')
    if release:
        remote=manifest['gpu_peer'].get('prepared_receipt',{})
        receipt=Path(remote.get('path','/nonexistent'))
        if not receipt.is_file() or hashlib.sha256(receipt.read_bytes()).hexdigest()!=remote.get('sha256'):
            fail('remote GPU prepare receipt missing or changed')
        for gate in GATES:
            evidence = manifest.get('evidence', {}).get(gate, {})
            path = Path(evidence.get('path', '/nonexistent'))
            if evidence.get('passed') is not True or not path.is_file() or hashlib.sha256(path.read_bytes()).hexdigest() != evidence.get('sha256'):
                fail(f'release gate not verified: {gate}')
        if manifest.get('native_max_schema', 0) < manifest['native_schema']:
            fail('candidate would downgrade native state')
        if manifest.get('single_native_writer') is not True:
            fail('release and rollback must retain one native writer')
        if manifest.get('workers_start_paused') is not True or manifest.get('native_starts_closed') is not True:
            fail('candidate must start with workers paused and native admission closed')


def gpu_probe(manifest):
    peer=manifest['gpu_peer']
    headers={}
    if peer.get('token_env'):
        token=os.environ.get(peer['token_env'])
        if not token: fail('GPU token env missing')
        headers['Authorization']='Bearer '+token
    request=urllib.request.Request(peer['url'],headers=headers)
    with urllib.request.urlopen(request,timeout=10) as response:
        health=json.load(response)
    def compare(expected, observed, prefix=''):
        for key,value in expected.items():
            if isinstance(value,dict): compare(value,observed.get(key,{}),prefix+key+'.')
            elif observed.get(key)!=value: fail('GPU version/model/instance health mismatch: '+prefix+key)
    compare(peer['expected_health'],health)
    if health.get('execution',{}).get('cuda_kernel_events',0)<=0: fail('GPU health lacks actual CUDA kernel proof')
    return health


def fence_peers(manifest, timeout):
    validate(manifest,release=True)
    capacity(manifest)
    baseline(manifest)
    # Pause claims through the existing worker settings API before stopping web.
    request=urllib.request.Request(manifest['web_url'].rstrip('/')+'/api/processing',data=b'{"action":"pause"}',headers={'Content-Type':'application/json'})
    with urllib.request.urlopen(request,timeout=10) as response:
        if not json.load(response).get('ok'): fail('worker pause was not acknowledged')
    # Web can otherwise start a new calibrator request between idle checks.
    run('docker','stop','--time','120',manifest['services']['web']['name'])
    end=time.monotonic()+timeout
    while time.monotonic()<end:
        with contextlib.closing(sqlite3.connect((Path(manifest['base'])/'processing/processing.db').resolve().as_uri()+'?mode=ro',uri=True)) as db:
            paused=json.loads(db.execute("SELECT value FROM settings WHERE key='paused'").fetchone()[0])
            active=db.execute("SELECT count(*) FROM tasks WHERE stage IN ('text','face')").fetchone()[0]
        with urllib.request.urlopen(manifest['calibrator_url'].rstrip('/')+'/api/rebuild',timeout=10) as response:
            rebuilding=json.load(response).get('running')
        if paused is True and active==0 and rebuilding is False:
            break
        time.sleep(1)
    else: fail('peer drain timed out; web remains stopped, tasks have not been cancelled')
    # Stop only named application writers; never pause unrelated GPU neighbors.
    for role in ('worker','calibrator'):
        if inspect(manifest['services'][role]['name'])['Id']!=manifest['services'][role]['expected_id']: fail(role+': identity changed')
        run('docker','stop','--time','120',manifest['services'][role]['name'])
    if not manifest['services']['native'].get('initially_absent'):
        drain(manifest,timeout)
    check_peer_drain(manifest)


def permissions(manifest):
    if os.geteuid()!=manifest['native_uid'] or os.getegid()!=manifest['native_gid']:
        fail('permission inventory must run as actual native UID/GID, not root')
    base=Path(manifest['base'])
    parents={base,Path(manifest['ledger']).parent,Path(manifest['authority_dir'])}
    parents.update(base/name for name in ('watermarks','watermarks/evidence','external-profiles','processing','processing/attempts','processing/faces','groups','jobs','file-operation-routing'))
    parents.update(Path(p) for p in manifest.get('permission_media_dirs',[]))
    rows=[]
    for wanted in sorted(parents):
        current=wanted
        while not current.exists(): current=current.parent
        if current.is_symlink(): fail('mutable parent is symlink: '+str(current))
        stat=current.stat();allowed=os.access(current,os.R_OK|os.W_OK|os.X_OK,effective_ids=True)
        rows.append({'requested':str(wanted),'checked_parent':str(current),'uid':stat.st_uid,'gid':stat.st_gid,'mode':oct(stat.st_mode & 0o7777),'access':allowed})
    for path in (base/'faces.db',base/'processing/processing.db',Path(manifest['ledger'])):
        stat=path.stat();rows.append({'database':str(path),'uid':stat.st_uid,'gid':stat.st_gid,'mode':oct(stat.st_mode & 0o7777),'access':os.access(path,os.R_OK|os.W_OK,effective_ids=True)})
    return {'uid':os.geteuid(),'gid':os.getegid(),'groups':os.getgroups(),'read_only_access_inventory':True,'ok':all(r['access'] for r in rows),'paths':rows}


def capacity(manifest):
    destination=Path(manifest['evidence_dir'])
    while not destination.exists(): destination=destination.parent
    source=Path(manifest['base'])
    required=sum(p.stat().st_size for p in source.rglob('*') if p.is_file())+Path(manifest['ledger']).stat().st_size
    margin=manifest.get('minimum_free_after_backup_bytes',2*1024**3)
    free=shutil.disk_usage(destination).free
    if free < required + margin: fail('backup destination lacks space for complete state plus reserve; do not prune unrelated data')
    for role,service in manifest['services'].items():
        image=json.loads(run('docker','image','inspect',service['candidate_image']))[0]
        if image['Id']!=service['candidate_image']: fail(role+': candidate image not present at frozen digest')
    return {'backup_estimate_bytes':required,'destination_free_bytes':free,'reserve_bytes':margin,'candidate_images_present':True}


def authority(manifest):
    lock = Path(manifest['authority_dir']) / 'executor.lock'
    stat = lock.stat()
    result = {'dev': stat.st_dev, 'ino': stat.st_ino}
    if result != manifest['authority_identity']:
        fail('native authority inode changed; never recreate/copy the live authority lock')
    return result


def baseline(manifest):
    observed = {}
    for role, service in manifest['services'].items():
        if service.get('initially_absent'):
            if run('docker','ps','-aq','--filter','name=^/'+service['name']+'$').strip(): fail(f'{role}: expected absent service now exists')
            observed[role] = None
            continue
        state = inspect(service['name'])
        if state['Id'] != service['expected_id'] or state['Image'] != service['expected_image']:
            fail(f'{role}: container changed since inspection')
        observed[role] = state
    peach = inspect(manifest['peach']['name'])
    if peach['Id'] != manifest['peach']['expected_id'] or peach['Image'] != manifest['peach']['expected_image']:
        fail('Peach changed since schema acceptance')
    with sqlite3.connect(Path(manifest['ledger']).resolve().as_uri() + '?mode=ro', uri=True) as db:
        schema = db.execute("SELECT type,name,tbl_name,sql FROM sqlite_master WHERE sql IS NOT NULL ORDER BY type,name").fetchall()
    digest = hashlib.sha256(json.dumps(schema, ensure_ascii=False, separators=(',', ':')).encode()).hexdigest()
    if digest != manifest['peach']['schema_sha256']:
        fail('Peach schema digest differs from accepted fixture')
    return observed


def backup_database(source, target):
    # Backup API includes committed WAL frames. cp of a live SQLite file is forbidden.
    with contextlib.closing(sqlite3.connect(Path(source).resolve().as_uri() + '?mode=ro', uri=True)) as src, contextlib.closing(sqlite3.connect(target)) as dst:
        src.backup(dst)
        dst.execute('PRAGMA wal_checkpoint(TRUNCATE)')
        dst.execute('PRAGMA journal_mode=DELETE')


def snapshot_tree(source, target, exclude=()):
    source, target = Path(source).resolve(), Path(target)
    if target.resolve().is_relative_to(source):
        fail('snapshot destination must be outside source')
    target.mkdir(mode=0o700)
    for item in source.iterdir():
        if item.name in exclude or item.name.endswith(('-wal', '-shm', '-journal')):
            continue
        if item.is_symlink():
            fail('snapshot source contains symlink: ' + item.name)
        dest = target / item.name
        if item.is_dir():
            snapshot_tree(item, dest)
        elif item.is_file():
            with item.open('rb') as stream:
                sqlite = stream.read(16) == b'SQLite format 3\0'
            if sqlite:
                backup_database(item, dest)
            else:
                shutil.copyfile(item, dest)


def parity_snapshot(base, ledger, output):
    output = Path(output).resolve()
    output.mkdir(mode=0o700)  # refuse reuse, including existing native authority
    snapshot_tree(base, output / 'base', ('file-operations-rust', '.review-write.lock'))
    backup_database(ledger, output / 'ledger.db')
    (output / '.fileops-parity').write_text('isolated sqlite backups')
    private_json(output / 'snapshot.json', {'version': 1, 'at': time.time(), 'source_base': str(Path(base).resolve()), 'source_ledger': str(Path(ledger).resolve()), 'recovery': False})


def drain(manifest, timeout):
    api(manifest, '/v1/admission', {'accepting': False})
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        state = api(manifest, '/v1/readiness')
        if state.get('drained') and not state.get('accepting'):
            return state
        time.sleep(1)
    fail('drain timed out; admission remains closed, do not kill/replay in-flight work')


def check_peer_drain(manifest):
    # These service-specific commands are frozen in the private manifest. Pausing
    # claims must not cancel tasks; old calibrator/worker must be stopped before
    # final backup because they do not honor native admission.
    for role in ('worker', 'calibrator', 'web'):
        service = manifest['services'][role]
        state = inspect(service['name'])
        if state['State']['Running']:
            fail(f'{role}: old writer still running; complete the approved drain and stop it first')


def probes(manifest):
    gpu_probe(manifest)
    for probe in manifest['probes']:
        request = urllib.request.Request(probe['url'], headers=probe.get('headers', {}))
        with urllib.request.urlopen(request, timeout=15) as response:
            if response.status != probe['status']:
                fail('probe status mismatch: ' + probe['kind'])
            payload = response.read(1024 * 1024)
            if not payload:
                fail('empty probe: ' + probe['kind'])
            if probe['kind'] == 'playback-range' and not response.headers.get('Content-Range'):
                fail('playback probe lacks Content-Range')
    kinds = {p['kind'] for p in manifest['probes']}
    if not {'web', 'static', 'playback-range', 'legacy-receipt', 'native-receipt', 'gpu', 'worker', 'calibrator'}.issubset(kinds):
        fail('probe coverage incomplete')


def replace(manifest, directory):
    validate(manifest, release=True)
    capacity(manifest)
    original = baseline(manifest)
    gpu_probe(manifest)
    check_peer_drain(manifest)
    if original['native'] is None:
        if manifest.get('authority_identity') is not None or Path(manifest['authority_dir']).exists():
            fail('bootstrap must not adopt an existing native authority')
        state = {'bootstrap': True}
    else:
        state = drain(manifest, manifest.get('drain_timeout', 120))
        authority(manifest)
    private_json(directory / 'original-inspect.json', original)
    private_json(directory / 'drained.json', state)
    snapshot_tree(manifest['base'], directory / 'base')
    backup_database(manifest['ledger'], directory / 'ledger.db')
    # Lock inode remains on its original mount. Backup is evidence, never the live authority.
    for role in ROLES:
        service = manifest['services'][role]
        if original[role] is not None:
            if inspect(service['name'])['Id'] != service['expected_id']:
                fail(f'{role}: container identity changed during cutover')
            run('docker', 'stop', '--time', '120', service['name'])
            anchor = service['name'] + '-before-' + manifest['release_id']
            run('docker', 'rename', service['name'], anchor)
            private_json(directory / (role + '-stopped.json'), {'anchor': anchor, 'id': service['expected_id']})
        candidate = run('docker', 'create', *service['create_args']).strip()
        private_json(directory / (role + '-created.json'), {'id': candidate, 'image': service['candidate_image']})
        created = inspect(candidate)
        env = dict(item.split('=',1) for item in created['Config'].get('Env',[]) if '=' in item)
        if any(key in env for key in ('FILE_OPERATIONS_FIXTURE_ROOT','FILE_OPERATIONS_PARITY_ROOT')): fail('candidate env file includes fixture/parity mode')
        if role == 'native':
            if env.get('FILE_OPERATIONS_ADMISSION') != 'closed': fail('native must start admission closed')
            if created['Config'].get('User') != str(manifest['native_uid'])+':'+str(manifest['native_gid']): fail('native container UID/GID differs from permission acceptance')
        if role in ('worker','calibrator','web') and not env.get('FILE_OPERATIONS_RUST_URL'): fail('peer lacks native coordinator URL')
        run('docker', 'start', candidate)
        private_json(directory / (role + '-started.json'), {'id': candidate})
        if role == 'native' and original['native'] is None:
            deadline=time.monotonic()+30
            lock=Path(manifest['authority_dir'])/'executor.lock'
            while not lock.exists() and time.monotonic()<deadline: time.sleep(.2)
            stat=lock.stat();manifest['authority_identity']={'dev':stat.st_dev,'ino':stat.st_ino}
        authority(manifest)
        service.update(expected_id=candidate,expected_image=service['candidate_image'],initially_absent=False)
    private_json(directory / 'activated-manifest.json',manifest)
    state = api(manifest, '/v1/readiness')
    if state.get('schema') != manifest['native_schema'] or state.get('accepting') or not state.get('drained') or state.get('mode') != 'writer' or state.get('authority') != manifest['authority_identity']:
        fail('candidate is not closed, drained, schema-compatible native authority')
    probes(manifest)
    private_json(directory / 'verified-closed.json', {'at': time.time(), 'native': state})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=('check', 'close', 'verify', 'replace', 'rollback', 'open', 'parity-snapshot', 'fence-peers', 'permissions'))
    parser.add_argument('--manifest', type=Path)
    parser.add_argument('--execute', action='store_true')
    parser.add_argument('--source-base'); parser.add_argument('--source-ledger'); parser.add_argument('--output')
    args = parser.parse_args()
    if args.action == 'parity-snapshot':
        parity_snapshot(args.source_base, args.source_ledger, args.output); return
    manifest = json.loads(args.manifest.read_text()); validate(manifest)
    if args.action == 'permissions':
        report=permissions(manifest);print(json.dumps(report,indent=2));
        if not report['ok']: fail('native access checks failed; review targeted ownership/ACL changes')
        return
    if args.action == 'check':
        baseline(manifest); capacity(manifest); print('frozen container, Peach schema, candidate images and backup capacity match'); return
    if not args.execute:
        print(json.dumps({'action': args.action, 'release_id': manifest['release_id'], 'roles': ROLES, 'executed': False})); return
    # Existing lock is opened without O_CREAT (Linux protected_regular compatible).
    lockpath = Path(manifest['deploy_lock'])
    if lockpath.is_symlink(): fail('deployment lock must not be a symlink')
    lock = lockpath.open('r+') if lockpath.exists() else lockpath.open('x+')
    with lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        if args.action == 'fence-peers':
            fence_peers(manifest,manifest.get('drain_timeout',120));print('web, worker and calibrator drained/stopped; native admission closed');return
        if args.action == 'close':
            print(json.dumps(drain(manifest, manifest.get('drain_timeout', 120)))); return
        if args.action == 'verify':
            authority(manifest); probes(manifest); print('all frozen probes passed'); return
        if args.action == 'open':
            validate(manifest, release=True); authority(manifest); probes(manifest)
            state = api(manifest, '/v1/readiness')
            if not state.get('drained') or state.get('schema') != manifest['native_schema'] or state.get('authority') != manifest['authority_identity']:
                fail('native state not ready to open')
            print(json.dumps(api(manifest, '/v1/admission', {'accepting': True}))); return
        if args.action == 'rollback' and manifest.get('purpose') != 'compatible-native-rollback':
            fail('rollback requires its own compatible native manifest; legacy writer toggle is forbidden')
        directory = Path(manifest['evidence_dir']) / (manifest['release_id'] + '-' + args.action)
        directory.mkdir(parents=True, mode=0o700)  # repeated/partial execution must be reviewed
        try:
            replace(manifest, directory)
        except Exception as error:
            private_json(directory / 'failure.json', {'at': time.time(), 'error_type': type(error).__name__, 'message': str(error)})
            raise


if __name__ == '__main__':
    main()
