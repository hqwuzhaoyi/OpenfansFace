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
import stat
import uuid
import urllib.request
import urllib.error

TRANSIENT_SNAPSHOT_EXCLUSIONS = {'processing/tmp'}  # worker scratch frames, never accepted attempts/receipts
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


def rehearsal_guard(manifest):
    rehearsal=manifest.get('rehearsal')
    if not rehearsal: return False
    root=Path(rehearsal['root']).resolve()
    if os.environ.get('FILE_OPERATIONS_RELEASE_REHEARSAL_ROOT')!=str(root) or root.parent!=Path('/tmp').resolve() or not root.name.startswith('face-admin-release-rehearsal-') or (root/'.release-rehearsal').read_text()!='isolated controller rehearsal':
        fail('rehearsal requires an explicitly enabled marked isolated /tmp root')
    for key in ('base','ledger','authority_dir','evidence_dir','deploy_lock'):
        if not Path(manifest[key]).resolve().is_relative_to(root): fail('rehearsal path escapes isolated root: '+key)
    prefix='face-release-rehearsal-'+rehearsal['id']+'-'
    for role,service in manifest['services'].items():
        if not service['name'].startswith(prefix): fail('rehearsal container name outside owned prefix')
        if 'face-admin.release-rehearsal='+rehearsal['id'] not in service['create_args']: fail('rehearsal candidate lacks ownership label')
        args=service['create_args']
        if any(arg in ('--privileged','--pid=host','--network=host','--volumes-from','--device','--mount','-p','--publish','-P','--publish-all') for arg in args): fail('rehearsal forbids host/device/published-port access')
        if '--network' not in args or args[args.index('--network')+1]!=prefix.rstrip('-'): fail('rehearsal requires its isolated owned Docker network')
        for index,arg in enumerate(args[:-1]):
            if arg in ('-v','--volume'):
                mount=args[index+1].split(':')
                if len(mount)<2 or not Path(mount[0]).resolve().is_relative_to(root) or not Path(mount[1]).is_relative_to(root): fail('rehearsal mount escapes isolated root')
            if arg=='--env-file' and not Path(args[index+1]).resolve().is_relative_to(root): fail('rehearsal env file escapes isolated root')

        if not service.get('initially_absent'):
            current=inspect(service['name'])
            if current['Config'].get('Labels',{}).get('face-admin.release-rehearsal')!=rehearsal['id']: fail('rehearsal refuses an unmarked existing container')
    if not manifest['peach']['name'].startswith(prefix): fail('rehearsal Peach peer outside owned prefix')
    return True


def validate(manifest, release=False):
    rehearsal=rehearsal_guard(manifest)
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
            try: proof=json.loads(path.read_text())
            except (ValueError,UnicodeError): proof={}
            if isinstance(proof,dict) and proof.get('simulated') is True and not rehearsal: fail('simulated gate evidence is forbidden outside marked rehearsal')
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


def legacy_health_gate(manifest):
    config=manifest.get('legacy_health',{})
    if config.get('adapter')=='rehearsal':
        if not rehearsal_guard(manifest): fail('simulated timer state is only allowed in a marked rehearsal')
        states=config['states']
    elif config.get('adapter')=='systemd':
        states={}
        for role in ('timer','service'):
            unit=config[role]
            output=run('systemctl','show',unit,'--property=LoadState','--property=ActiveState','--property=UnitFileState')
            states[role]=dict(line.split('=',1) for line in output.splitlines() if '=' in line)
    else: fail('explicit legacy video-health timer/service inspection is required')
    for role,state in states.items():
        absent=state.get('LoadState')=='not-found'
        allowed=('disabled','masked') if role=='timer' else ('disabled','masked','static')
        if state.get('ActiveState')!='inactive' or (not absent and state.get('UnitFileState') not in allowed):
            fail('legacy video-health '+role+' is not fenced: require inactive and disabled timer (inactive static service is valid)')
    if set(states)!= {'timer','service'}: fail('both legacy health timer and service must be inspected')
    return states


def legacy_journals(manifest):
    base=Path(manifest['base']);rows=[]
    paths=set()
    for folder,pattern in (('person-operations','**/operation.json'),('person-operations/batches','*.json'),('staging-operations','**/operation.json'),('archive-runs','*.json'),('manifests','*.json')):
        directory=base/folder
        if directory.exists(): paths.update(directory.glob(pattern))
    if (base/'jobs/archive.json').exists(): paths.add(base/'jobs/archive.json')
    for path in sorted(paths):
        if path.is_symlink(): fail('legacy journal is a symlink')
        raw=path.read_bytes();record=json.loads(raw);relative=str(path.relative_to(base));digest=hashlib.sha256(raw).hexdigest()
        state=record.get('status');result=record.get('result') or {};job=record.get('job') or record
        terminal=state in ('complete','partial','rolled_back','cancelled','preview','not_started')
        if state=='failed' and not record.get('moves'): terminal=True
        if relative.startswith('archive-runs/') or relative=='jobs/archive.json':
            terminal=job.get('status') in ('complete','error') and job.get('finished_at') is not None
        if record.get('recovery_needed') or result.get('recovery_needed') or record.get('recovery_errors'): terminal=False
        acknowledged=manifest.get('legacy_terminal_acknowledgements',{}).get(relative)==digest
        # Explicitly active/uncertain states are never overridden by an acknowledgement.
        if state in ('preparing','checking','prepared','accepted','running','moving','updating_ledger','committing','recovery_needed','rolling_back') or job.get('status')=='running' or record.get('recovery_needed') or result.get('recovery_needed') or record.get('recovery_errors'): acknowledged=False
        if not terminal and not acknowledged: fail('legacy journal remains unresolved or unclassified: '+relative)
        rows.append({'path':relative,'sha256':digest,'state':state or job.get('status'),'terminal_acknowledgement':acknowledged})
    return rows


def sync_directory(path):
    descriptor=os.open(path,os.O_RDONLY|os.O_DIRECTORY)
    try: os.fsync(descriptor)
    finally: os.close(descriptor)


def acquire_deploy_fence(manifest,directory,timeout):
    path=Path(manifest['base'])/'.review-write.lock'
    marker={'backend':'deployment','release_id':manifest['release_id'],'token':str(uuid.uuid4()),'at':time.time()}
    body=json.dumps(marker,sort_keys=True,separators=(',',':')).encode()
    private_json(directory/'fence-intent.json',{'path':str(path),'marker':marker,'sha256':hashlib.sha256(body).hexdigest()})
    deadline=time.monotonic()+timeout
    while True:
        try: descriptor=os.open(path,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o600);break
        except FileExistsError:
            if time.monotonic()>=deadline: fail('existing shared write lock did not drain; no web stop and no foreign lock removal')
            time.sleep(.2)
    try:
        os.write(descriptor,body);os.fsync(descriptor);held=os.fstat(descriptor)
    finally: os.close(descriptor)
    sync_directory(path.parent)
    evidence={'path':str(path),'marker':marker,'identity':{'dev':str(held.st_dev),'ino':str(held.st_ino)},'sha256':hashlib.sha256(body).hexdigest()}
    private_json(directory/'fence-owned.json',evidence)
    return evidence


def release_deploy_fence(manifest,directory):
    check_peer_drain(manifest)
    evidence=json.loads((directory/'fence-owned.json').read_text());path=Path(evidence['path'])
    if path!=Path(manifest['base'])/'.review-write.lock' or evidence['marker']['release_id']!=manifest['release_id']: fail('deployment fence evidence does not match release')
    descriptor=os.open(path,os.O_RDONLY|os.O_NOFOLLOW)
    try:
        held=os.fstat(descriptor);raw=os.read(descriptor,1024*1024);current=path.lstat()
        if not stat.S_ISREG(held.st_mode) or {'dev':str(held.st_dev),'ino':str(held.st_ino)}!=evidence['identity'] or (held.st_dev,held.st_ino)!=(current.st_dev,current.st_ino) or hashlib.sha256(raw).hexdigest()!=evidence['sha256']: fail('deployment fence changed; never remove a foreign marker')
        path.unlink();sync_directory(path.parent)
    finally: os.close(descriptor)
    private_json(directory/'fence-released.json',{'at':time.time(),'identity':evidence['identity']})


def peer_idle(manifest):
    with contextlib.closing(sqlite3.connect((Path(manifest['base'])/'processing/processing.db').resolve().as_uri()+'?mode=ro',uri=True)) as db:
        paused=json.loads(db.execute("SELECT value FROM settings WHERE key='paused'").fetchone()[0])
        active=db.execute("SELECT count(*) FROM tasks WHERE stage IN ('text','face')").fetchone()[0]
    with urllib.request.urlopen(manifest['calibrator_url'].rstrip('/')+'/api/rebuild',timeout=10) as response:
        rebuilding=json.load(response).get('running')
    return paused is True and active==0 and rebuilding is False


def wait_peer_idle(manifest,timeout):
    end=time.monotonic()+timeout
    while not peer_idle(manifest):
        if time.monotonic()>=end: fail('peer drain timed out; no busy worker/calibrator is force-stopped')
        time.sleep(.2)


def fence_peers(manifest, timeout):
    validate(manifest,release=True);capacity(manifest);original=baseline(manifest)
    health=legacy_health_gate(manifest)
    directory=Path(manifest['evidence_dir'])/(manifest['release_id']+'-fence');directory.mkdir(parents=True,mode=0o700)
    private_json(directory/'original-inspect.json',original);private_json(directory/'legacy-health.json',health)
    try:
        request=urllib.request.Request(manifest['web_url'].rstrip('/')+'/api/processing',data=b'{"action":"pause"}',headers={'Content-Type':'application/json'})
        with urllib.request.urlopen(request,timeout=10) as response:
            if not json.load(response).get('ok'): fail('worker pause was not acknowledged')
        wait_peer_idle(manifest,timeout)
        # Finish native's admitted health publisher before taking the external wx
        # fence; otherwise its late publication would be rejected by our own fence.
        if not manifest['services']['native'].get('initially_absent'):
            private_json(directory/'native-drained.json',drain(manifest,timeout))
        acquire_deploy_fence(manifest,directory,timeout)
        private_json(directory/'legacy-journals.json',legacy_journals(manifest))
        # The shared wx marker now blocks old file writers. Existing holders were
        # allowed to finish; no long copy is killed to acquire the deployment fence.
        if inspect(manifest['services']['web']['name'])['Id']!=manifest['services']['web']['expected_id']: fail('web identity changed before fenced stop')
        run('docker','stop','--time','120',manifest['services']['web']['name'])
        # A late calibrator request may have raced the initial idle check. Wait;
        # on failure retain the exact fence and receipt, never kill known busy work.
        wait_peer_idle(manifest,timeout)
        for role in ('worker','calibrator'):
            if inspect(manifest['services'][role]['name'])['Id']!=manifest['services'][role]['expected_id']: fail(role+': identity changed')
            run('docker','stop','--time','120',manifest['services'][role]['name'])
        check_peer_drain(manifest);legacy_journals(manifest);legacy_health_gate(manifest)
        release_deploy_fence(manifest,directory)
        if not manifest['services']['native'].get('initially_absent'): drain(manifest,timeout)
        private_json(directory/'complete.json',{'at':time.time(),'peers_stopped':True,'fence_released':True})
    except Exception as error:
        private_json(directory/'failure.json',{'at':time.time(),'error_type':type(error).__name__,'message':str(error)})
        raise


def permissions(manifest):
    if os.geteuid()!=manifest['native_uid'] or os.getegid()!=manifest['native_gid']:
        fail('permission inventory must run as actual native UID/GID, not root')
    base=Path(manifest['base'])
    # Rust replaces these JSON documents atomically: existing file needs READ;
    # its containing directory needs RWX. Files are not opened for in-place writes.
    documents=('people-deleted.json','report.json','claims-detail.json','promoted.json','clusters.json',
        'integration-queue.json','review-decisions.json','archive-history.json','people-manual.json',
        'template-overrides.json','person-rejections.json','video-path-overrides.json','person-renames.json',
        'watermarks/mappings.json','watermarks/results.json','external-profiles/manual-links.json',
        'external-profiles/reviewed-links.json','video-health.json','video-health-reports.json')
    writable={base,Path(manifest['ledger']).parent,Path(manifest['authority_dir']),base/'processing',base/'file-operation-routing'}
    writable.update((base/name).parent for name in documents)
    writable.update(Path(p) for p in manifest.get('permission_media_dirs',[]))
    # These are worker-owned artifacts or historical receipts, only read by native.
    readable={base/name for name in ('processing/attempts','processing/faces','watermarks/evidence','groups','jobs','archive-runs','person-operations','staging-operations','manifests')}
    rows=[]
    for wanted in sorted(writable|readable):
        current=wanted
        while not current.exists(): current=current.parent
        if current.is_symlink(): fail('required directory is symlink: '+str(current))
        stat=current.stat();mode=os.R_OK|os.X_OK|(os.W_OK if wanted in writable else 0)
        rows.append({'requested':str(wanted),'checked_parent':str(current),'uid':stat.st_uid,'gid':stat.st_gid,'mode':oct(stat.st_mode & 0o7777),'required':'rwx' if wanted in writable else 'rx','ownership':'native-mutable-parent' if wanted in writable else 'read-only-input','access':os.access(current,mode,effective_ids=True)})
    for path in (base/'faces.db',base/'processing/processing.db',Path(manifest['ledger'])):
        stat=path.stat();rows.append({'database':str(path),'uid':stat.st_uid,'gid':stat.st_gid,'mode':oct(stat.st_mode & 0o7777),'required':'rw','ownership':'native-database','access':os.access(path,os.R_OK|os.W_OK,effective_ids=True)})
    for path in [*(base/name for name in documents),*(Path(p) for p in manifest.get('permission_read_files',[]))]:
        if not path.exists(): continue
        if path.is_symlink(): fail('required input is symlink: '+str(path))
        stat=path.stat();rows.append({'input':str(path),'uid':stat.st_uid,'gid':stat.st_gid,'mode':oct(stat.st_mode & 0o7777),'required':'r','ownership':'native-read-input','access':os.access(path,os.R_OK,effective_ids=True)})
    return {'uid':os.geteuid(),'gid':os.getegid(),'groups':os.getgroups(),'read_only_access_inventory':True,'ok':all(r['access'] for r in rows),'paths':rows}


def snapshot_size(source, relative=Path()):
    total=0
    # Explicit traversal fails on unreadable durable directories instead of silently
    # underestimating them as Path.rglob can do. Only known disposable scratch is skipped.
    for item in Path(source).iterdir():
        child=relative/item.name
        if child.as_posix() in TRANSIENT_SNAPSHOT_EXCLUSIONS: continue
        if item.is_symlink(): fail('snapshot source contains symlink: '+str(child))
        if item.is_dir(): total+=snapshot_size(item,child)
        elif item.is_file(): total+=item.stat().st_size
    return total


def capacity(manifest):
    destination=Path(manifest['evidence_dir'])
    while not destination.exists(): destination=destination.parent
    source=Path(manifest['base'])
    required=snapshot_size(source)+Path(manifest['ledger']).stat().st_size
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
    result = {'dev': str(stat.st_dev), 'ino': str(stat.st_ino)}
    if result != {key:str(manifest['authority_identity'][key]) for key in ('dev','ino')}:
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


def snapshot_tree(source, target, exclude=(), relative=Path()):
    source, target = Path(source).resolve(), Path(target)
    if target.resolve().is_relative_to(source):
        fail('snapshot destination must be outside source')
    target.mkdir(mode=0o700)
    for item in source.iterdir():
        child=relative/item.name
        if child.as_posix() in TRANSIENT_SNAPSHOT_EXCLUSIONS or item.name in exclude or item.name.endswith(('-wal', '-shm', '-journal')):
            continue
        if item.is_symlink():
            fail('snapshot source contains symlink: ' + item.name)
        dest = target / item.name
        if item.is_dir():
            snapshot_tree(item, dest, relative=child)
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
    private_json(output / 'snapshot.json', {'version': 1, 'at': time.time(), 'source_base': str(Path(base).resolve()), 'source_ledger': str(Path(ledger).resolve()), 'recovery': False, 'transient_exclusions':sorted(TRANSIENT_SNAPSHOT_EXCLUSIONS)})


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


def await_available(action, seconds):
    deadline=time.monotonic()+max(0,min(float(seconds),120))
    while True:
        try: return action()
        except urllib.error.HTTPError as error:
            if error.code not in (502,503): raise
        except (urllib.error.URLError,TimeoutError,ConnectionError):
            pass
        if time.monotonic()>=deadline: fail('candidate readiness deadline exceeded; admission remains closed')
        time.sleep(.2)


def probes(manifest):
    gpu_probe(manifest)
    for probe in manifest['probes']:
        def read_probe():
            request = urllib.request.Request(probe['url'], headers=probe.get('headers', {}))
            with urllib.request.urlopen(request, timeout=5) as response:
                if response.status != probe['status']:
                    fail('probe status mismatch: ' + probe['kind'])
                payload = response.read(1024 * 1024)
                if not payload:
                    fail('empty probe: ' + probe['kind'])
                if probe['kind'] == 'playback-range' and not response.headers.get('Content-Range'):
                    fail('playback probe lacks Content-Range')
        await_available(read_probe,manifest.get('startup_timeout',30))
    kinds = {p['kind'] for p in manifest['probes']}
    if not {'web', 'static', 'playback-range', 'legacy-receipt', 'native-receipt', 'gpu', 'worker', 'calibrator'}.issubset(kinds):
        fail('probe coverage incomplete')


def replace(manifest, directory):
    validate(manifest, release=True)
    capacity(manifest)
    original = baseline(manifest)
    gpu_probe(manifest)
    legacy_health_gate(manifest)
    legacy_journals(manifest)
    check_peer_drain(manifest)
    if os.path.lexists(Path(manifest['base'])/'.review-write.lock'): fail('shared write fence remains; release only the exact owned deployment fence after peer drain')
    if original['native'] is None:
        if manifest.get('authority_identity') is not None or Path(manifest['authority_dir']).exists():
            fail('bootstrap must not adopt an existing native authority')
        state = {'bootstrap': True}
    else:
        state = drain(manifest, manifest.get('drain_timeout', 120))
        if state.get('schema',0)>manifest['native_schema'] or state.get('schema',0)>manifest['native_max_schema']:
            fail('actual native schema is newer than candidate; rollback cannot downgrade state')
        authority(manifest)
    private_json(directory / 'original-inspect.json', original)
    private_json(directory / 'drained.json', state)
    private_json(directory / 'backup-policy.json', {'transient_exclusions':sorted(TRANSIENT_SNAPSHOT_EXCLUSIONS),'accepted_attempts_and_receipts':'included'})
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
            stat=lock.stat();manifest['authority_identity']={'dev':str(stat.st_dev),'ino':str(stat.st_ino)}
        authority(manifest)
        service.update(expected_id=candidate,expected_image=service['candidate_image'],initially_absent=False)
    private_json(directory / 'activated-manifest.json',manifest)
    state = await_available(lambda:api(manifest, '/v1/readiness'),manifest.get('startup_timeout',30))
    if state.get('schema') != manifest['native_schema'] or state.get('accepting') or not state.get('drained') or state.get('mode') != 'writer' or state.get('authority') != {key:str(manifest['authority_identity'][key]) for key in ('dev','ino')}:
        fail('candidate is not closed, drained, schema-compatible native authority')
    probes(manifest)
    private_json(directory / 'verified-closed.json', {'at': time.time(), 'native': state})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=('check', 'close', 'verify', 'replace', 'rollback', 'open', 'parity-snapshot', 'fence-peers', 'release-fence', 'permissions'))
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
        baseline(manifest); capacity(manifest); legacy_health_gate(manifest); print('frozen container, Peach schema, candidate images and backup capacity match'); return
    if not args.execute:
        print(json.dumps({'action': args.action, 'release_id': manifest['release_id'], 'roles': ROLES, 'executed': False})); return
    # Existing lock is opened without O_CREAT (Linux protected_regular compatible).
    lockpath = Path(manifest['deploy_lock'])
    if lockpath.is_symlink(): fail('deployment lock must not be a symlink')
    lock = lockpath.open('r+') if lockpath.exists() else lockpath.open('x+')
    with lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        if args.action == 'release-fence':
            baseline(manifest);legacy_journals(manifest);legacy_health_gate(manifest);release_deploy_fence(manifest,Path(manifest['evidence_dir'])/(manifest['release_id']+'-fence'));print('exact owned deployment fence released after all peers stopped');return
        if args.action == 'fence-peers':
            fence_peers(manifest,manifest.get('drain_timeout',120));print('web, worker and calibrator drained/stopped; native admission closed');return
        if args.action == 'close':
            print(json.dumps(drain(manifest, manifest.get('drain_timeout', 120)))); return
        if args.action == 'verify':
            authority(manifest); probes(manifest); print('all frozen probes passed'); return
        if args.action == 'open':
            validate(manifest, release=True); baseline(manifest); legacy_health_gate(manifest); authority(manifest); probes(manifest)
            state = api(manifest, '/v1/readiness')
            if not state.get('drained') or state.get('schema') != manifest['native_schema'] or state.get('authority') != {key:str(manifest['authority_identity'][key]) for key in ('dev','ino')}:
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
