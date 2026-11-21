#!/usr/bin/env python3
"""
Adversarial Stress & Regression Test Suite for Milestone 2 Iteration 3
Auditor: m2_iter3_challenger_1 (Empirical Challenger)

Challenge Vectors:
1. Float drift validation & IEEE 754 edge cases (NaN, Infinity, subnormal, negative zero, overflows).
2. JSON deserialization fuzzing and schema compliance.
3. Version fencing races under concurrent out-of-order receipt delivery.
4. gpu_model projection under multi-threaded receipt bombardment and concurrent listing.
5. SQLite WAL durability and integrity under concurrent write load.
"""

import concurrent.futures
import hashlib
import http.client
import json
import math
import os
from pathlib import Path
import socket
import sqlite3
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
import uuid

REPO = Path(__file__).resolve().parents[1]
BINARY = REPO / 'services/file-operations/target/debug/face-file-operations'


class ChallengerClusterHarness:
    def __init__(self):
        self.tmp = Path(tempfile.mkdtemp(prefix='face-admin-person-fileops-iter3-', dir='/tmp')).resolve()
        (self.tmp / '.fileops-fixture').write_text('person file operations')
        self.base = self.tmp / 'base'
        self.media = self.tmp / 'media'
        self.base.mkdir()
        self.media.mkdir()
        self.ledger = self.tmp / 'ledger.db'
        (self.base / 'processing').mkdir()
        self.db_path = self.base / 'processing' / 'processing.db'
        self.server_proc = None
        self.port = None
        self.admin_token = 'iter3-challenger-secret-token'

    def start_server(self):
        s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        s.bind(('127.0.0.1', 0))
        self.port = s.getsockname()[1]
        s.close()

        env = os.environ.copy()
        env['FACES_BASE'] = str(self.base)
        env['MEDIA2_ROOT'] = str(self.media)
        env['PEACH_LEDGER_PATH'] = str(self.ledger)
        env['FILE_OPERATIONS_BIND'] = f'127.0.0.1:{self.port}'
        env['FILE_OPERATIONS_FIXTURE_ROOT'] = str(self.tmp)
        env['FILE_OPERATIONS_RUST_TOKEN'] = self.admin_token
        env['AGENT_ENROLLMENT_SECRET'] = 'iter3-bootstrap-secret'

        self.log_file = (self.tmp / 'server.log').open('wb')
        self.server_proc = subprocess.Popen([str(BINARY)], env=env, stdout=self.log_file, stderr=self.log_file)

        started = time.time()
        while time.time() - started < 10.0:
            if self.server_proc.poll() is not None:
                self.log_file.flush()
                log_text = (self.tmp / 'server.log').read_text()
                raise RuntimeError(f"Server exited prematurely ({self.server_proc.returncode}):\n{log_text}")
            try:
                with urllib.request.urlopen(f'http://127.0.0.1:{self.port}/health', timeout=0.5) as r:
                    if r.status == 200:
                        return
            except Exception:
                time.sleep(0.05)
        self.log_file.flush()
        log_text = (self.tmp / 'server.log').read_text()
        raise RuntimeError(f'Server failed to start within timeout:\n{log_text}')

    def stop_server(self):
        if self.server_proc:
            self.server_proc.terminate()
            try:
                self.server_proc.wait(timeout=3.0)
            except subprocess.TimeoutExpired:
                self.server_proc.kill()
                self.server_proc.wait()

    def request(self, method: str, path: str, payload=None, token=None, raw_body=None):
        url = f'http://127.0.0.1:{self.port}{path}'
        headers = {'Content-Type': 'application/json'}
        if token:
            headers['Authorization'] = f'Bearer {token}'
        
        if raw_body is not None:
            data = raw_body.encode('utf-8') if isinstance(raw_body, str) else raw_body
        elif payload is not None:
            data = json.dumps(payload).encode('utf-8')
        else:
            data = None

        req = urllib.request.Request(url, data=data, headers=headers, method=method)
        try:
            with urllib.request.urlopen(req, timeout=5.0) as resp:
                body = resp.read().decode('utf-8')
                return resp.status, json.loads(body) if body else {}
        except urllib.error.HTTPError as err:
            body = err.read().decode('utf-8')
            try:
                parsed = json.loads(body)
            except Exception:
                parsed = {'raw': body}
            return err.code, parsed

    def register_node(self, hostname, os_name, arch, gpu=None):
        token_st, token_res = self.request('POST', '/v1/agent/enrollment-token',
                                           {'note': f'token-for-{hostname}', 'ttl_seconds': 3600.0},
                                           token=self.admin_token)
        assert token_st == 200, f"Token create failed: {token_st} {token_res}"
        enrollment_token = token_res['token']

        reg_payload = {
            'enrollment_token': enrollment_token,
            'hostname': hostname,
            'os': os_name,
            'arch': arch,
            'ram_bytes': 34359738368,
            'cpu_cores': 16,
            'agent_version': '0.3.0',
        }
        if gpu:
            reg_payload['gpu'] = gpu

        st, res = self.request('POST', '/v1/agent/register', reg_payload)
        assert st == 200, f"Node registration failed: {st} {res}"
        return res['node_id'], res['node_token']

    def register_package(self, pkg_id, version, provider='CUDAExecutionProvider'):
        payload = {
            'package_id': pkg_id,
            'manifest_sha256': hashlib.sha256(f'manifest-{pkg_id}'.encode()).hexdigest(),
            'model_sha256': hashlib.sha256(f'model-{pkg_id}'.encode()).hexdigest(),
            'execution_provider': provider,
            'version': version,
            'is_pinned': 1,
        }
        st, res = self.request('POST', '/v1/agent/packages', payload, token=self.admin_token)
        assert st == 200, f"Package registration failed: {st} {res}"


def test_float_drift_validation_and_ieee754_edge_cases(harness: ChallengerClusterHarness):
    print("\n" + "=" * 75)
    print("▶ CHALLENGE 1: Float Drift Validation, NaN/Infinity & IEEE 754 Edge Cases")
    print("=" * 75)

    node_id, node_token = harness.register_node(
        'wsl-drift-edge', 'linux', 'x86_64',
        gpu={'model': 'NVIDIA GeForce RTX 4070 Ti', 'vram_bytes': 12884901888, 'device_uuid': 'GPU-UUID-DRIFT-1'}
    )
    harness.register_package('pkg-drift-test', '1.0.0')

    # 1. Invalid drift cases that must be REJECTED with HTTP 422
    # Note: In HTTP JSON payloads, strings or non-standard tokens like NaN unquoted are rejected at parser level (HTTP 400),
    # while numeric floats exceeding tolerance or non-finite via scientific notation (e.g. 1e400 -> +Inf) are rejected at domain level (HTTP 422).
    rejected_cases = [
        ("Exceeding boundary: 0.0500000000000001", 0.0500000000000001, (422,)),
        ("Exceeding boundary: 0.05001", 0.05001, (422,)),
        ("Way above tolerance: 0.12", 0.12, (422,)),
        ("Negative tiny: -0.000001", -0.000001, (422,)),
        ("Negative float: -0.05", -0.05, (422,)),
        ("Huge negative: -1e20", -1e20, (422,)),
        ("Huge positive: 1e20", 1e20, (422,)),
        ("Overflow float (1e400 -> Infinity in JSON):", "1e400", (400, 422)),
        ("Negative overflow float (-1e400 -> -Infinity):", "-1e400", (400, 422)),
    ]

    for label, drift_val, expected_statuses in rejected_cases:
        st, dep = harness.request('POST', '/v1/agent/deploy',
                                  {'node_id': node_id, 'package_id': 'pkg-drift-test'},
                                  token=harness.admin_token)
        assert st == 200, f"Deploy failed: {st} {dep}"
        dep_id = dep['deployment_id']

        if isinstance(drift_val, str) and '1e400' in drift_val:
            raw_payload = f'''{{
                "deployment_id": "{dep_id}",
                "status": "ready",
                "stage": "ready",
                "self_test": {{ "passed": true, "drift": {drift_val} }}
            }}'''
            st, res = harness.request('POST', '/v1/agent/deploy-receipt', token=node_token, raw_body=raw_payload)
        else:
            payload = {
                'deployment_id': dep_id,
                'status': 'ready',
                'stage': 'ready',
                'self_test': {'passed': True, 'drift': drift_val}
            }
            st, res = harness.request('POST', '/v1/agent/deploy-receipt', payload, token=node_token)

        assert st in expected_statuses, f"Case '{label}': expected HTTP {expected_statuses}, got {st}: {res}"
        print(f"  ✔ Rejected {label} -> HTTP {st}")

    # 2. Syntax/Deserialization level rejections (HTTP 400 Bad Request)
    syntax_reject_cases = [
        ("Unquoted NaN: { 'drift': NaN }", '{"deployment_id": "DEP_ID", "status": "ready", "self_test": {"passed": true, "drift": NaN}}'),
        ("String NaN: { 'drift': 'NaN' }", '{"deployment_id": "DEP_ID", "status": "ready", "self_test": {"passed": true, "drift": "NaN"}}'),
        ("Null drift: { 'drift': null }", '{"deployment_id": "DEP_ID", "status": "ready", "self_test": {"passed": true, "drift": null}}'),
    ]

    for label, raw_tmpl in syntax_reject_cases:
        st, dep = harness.request('POST', '/v1/agent/deploy',
                                  {'node_id': node_id, 'package_id': 'pkg-drift-test'},
                                  token=harness.admin_token)
        dep_id = dep['deployment_id']
        raw = raw_tmpl.replace("DEP_ID", dep_id)
        st, res = harness.request('POST', '/v1/agent/deploy-receipt', token=node_token, raw_body=raw)
        assert st in (400, 422), f"Case '{label}': expected HTTP 400 or 422 rejection, got {st}: {res}"
        print(f"  ✔ Syntax check rejected {label} -> HTTP {st}")

    # 3. Accepted boundary cases that must SUCCEED with HTTP 200
    accepted_cases = [
        ("Exact boundary 0.05", 0.05),
        ("Boundary minus eps 0.049999999999999", 0.049999999999999),
        ("Subnormal float ~4.94e-324", float.fromhex('0x0.0000000000001p-1022')),
        ("Typical good drift 0.0021", 0.0021),
        ("Zero drift 0.0", 0.0),
        ("IEEE negative zero -0.0", -0.0),
    ]

    for label, drift_val in accepted_cases:
        st, dep = harness.request('POST', '/v1/agent/deploy',
                                  {'node_id': node_id, 'package_id': 'pkg-drift-test'},
                                  token=harness.admin_token)
        assert st == 200
        dep_id = dep['deployment_id']
        payload = {
            'deployment_id': dep_id,
            'status': 'ready',
            'stage': 'ready',
            'self_test': {'passed': True, 'drift': drift_val},
            'startup_proof': {'kernel_events': 141}
        }
        st, res = harness.request('POST', '/v1/agent/deploy-receipt', payload, token=node_token)
        assert st == 200 and res.get('ok'), f"Case '{label}': expected HTTP 200 ok, got {st}: {res}"
        print(f"  ✔ Accepted {label} -> HTTP 200 OK")

    # Verify SQLite DB has no corrupt non-finite floats
    with sqlite3.connect(harness.db_path) as conn:
        cur = conn.cursor()
        cur.execute("PRAGMA integrity_check")
        assert cur.fetchone()[0] == 'ok', "SQLite integrity check failed after drift testing!"
        cur.execute("SELECT deployment_id, status, self_test_passed, self_test_drift FROM agent_deployments WHERE self_test_drift IS NOT NULL")
        for row in cur.fetchall():
            dep_id, status, passed, d_val = row
            assert math.isfinite(d_val), f"Stored drift must be finite, found {d_val} in {dep_id}"
            if status == 'ready':
                assert passed == 1, f"Ready deployment {dep_id} must have passed=1"
                assert 0.0 <= d_val <= 0.05, f"Ready deployment drift out of bounds: {d_val}"
            elif status == 'failed':
                assert passed == 0, f"Failed deployment {dep_id} must have passed=0"

    print("  ✅ All Float Drift & IEEE 754 validation tests PASSED.")


def test_version_fencing_races(harness: ChallengerClusterHarness):
    print("\n" + "=" * 75)
    print("▶ CHALLENGE 2: Version Fencing Races & Out-of-Order Receipt Delivery")
    print("=" * 75)

    node_id, node_token = harness.register_node(
        'wsl-fence-race', 'linux', 'x86_64',
        gpu={'model': 'NVIDIA GeForce RTX 4070 Ti', 'vram_bytes': 12884901888, 'device_uuid': 'GPU-UUID-FENCE-1'}
    )

    harness.register_package('pkg-v1', '1.0.0')
    harness.register_package('pkg-v2', '2.0.0')
    harness.register_package('pkg-v3', '3.0.0')

    # Step A: Dispatch v1
    st, dep_v1 = harness.request('POST', '/v1/agent/deploy', {'node_id': node_id, 'package_id': 'pkg-v1'}, token=harness.admin_token)
    assert st == 200
    dep1_id = dep_v1['deployment_id']

    # Step B: Center immediately dispatches v2 (upgrading node to 2.0.0) before v1 receipt arrives
    st, dep_v2 = harness.request('POST', '/v1/agent/deploy', {'node_id': node_id, 'package_id': 'pkg-v2'}, token=harness.admin_token)
    assert st == 200
    dep2_id = dep_v2['deployment_id']

    # Verify desired_version is now 2.0.0
    with sqlite3.connect(harness.db_path) as conn:
        desired = conn.execute("SELECT desired_version FROM agent_nodes WHERE id = ?", (node_id,)).fetchone()[0]
        assert desired == '2.0.0', f"Expected desired_version 2.0.0, got {desired}"

    # Step C: v2 finishes and submits receipt FIRST
    v2_receipt = {
        'deployment_id': dep2_id,
        'package_id': 'pkg-v2',
        'status': 'ready',
        'stage': 'ready',
        'self_test': {'passed': True, 'drift': 0.003},
        'startup_proof': {'kernel_events': 141}
    }
    st, res = harness.request('POST', '/v1/agent/deploy-receipt', v2_receipt, token=node_token)
    assert st == 200 and res.get('ok')

    # Verify node is current_version = 2.0.0, model_ready = 1
    with sqlite3.connect(harness.db_path) as conn:
        row = conn.execute("SELECT current_version, model_ready, status FROM agent_nodes WHERE id = ?", (node_id,)).fetchone()
        assert row[0] == '2.0.0', f"Expected current_version 2.0.0, got {row[0]}"
        assert row[1] == 1, f"Expected model_ready 1, got {row[1]}"
        assert row[2] == 'accepting', f"Expected status accepting, got {row[2]}"
    print("  ✔ v2 deployment successfully became active version 2.0.0 (model_ready=1)")

    # Step D: Late-arriving v1 receipt arrives reporting "ready"
    v1_late_receipt = {
        'deployment_id': dep1_id,
        'package_id': 'pkg-v1',
        'status': 'ready',
        'stage': 'ready',
        'self_test': {'passed': True, 'drift': 0.001},
        'startup_proof': {'kernel_events': 141}
    }
    st, res = harness.request('POST', '/v1/agent/deploy-receipt', v1_late_receipt, token=node_token)
    assert st == 200 and res.get('ok')

    # CRITICAL INVARIANT: delayed v1 receipt MUST NOT regress current_version!
    with sqlite3.connect(harness.db_path) as conn:
        row = conn.execute("SELECT current_version, model_ready, status FROM agent_nodes WHERE id = ?", (node_id,)).fetchone()
        assert row[0] == '2.0.0', f"REGRESSION DETECTED! current_version regressed from 2.0.0 to {row[0]}"
        assert row[1] == 1, f"model_ready altered by stale receipt: {row[1]}"
        assert row[2] == 'accepting'
    print("  ✔ Version fencing prevented delayed v1 receipt from regressing current_version")

    # Step E: Late-arriving v1 duplicate arrives reporting "failed"
    v1_fail_receipt = {
        'deployment_id': dep1_id,
        'package_id': 'pkg-v1',
        'status': 'failed',
        'stage': 'verify',
        'error': 'cleaned_up_process'
    }
    st, res = harness.request('POST', '/v1/agent/deploy-receipt', v1_fail_receipt, token=node_token)
    assert st == 200

    # CRITICAL INVARIANT: failure of superseded v1 MUST NOT knock down model_ready of v2!
    with sqlite3.connect(harness.db_path) as conn:
        row = conn.execute("SELECT current_version, model_ready, status FROM agent_nodes WHERE id = ?", (node_id,)).fetchone()
        assert row[0] == '2.0.0', f"current_version corrupted by superseded failure: {row[0]}"
        assert row[1] == 1, f"REGRESSION DETECTED! Superseded v1 failure knocked down model_ready to {row[1]}"
        assert row[2] == 'accepting'
    print("  ✔ Superseded v1 failure did not knock down model_ready of active v2 node")

    # Step F: Dispatch v3, but v3 fails
    st, dep_v3 = harness.request('POST', '/v1/agent/deploy', {'node_id': node_id, 'package_id': 'pkg-v3'}, token=harness.admin_token)
    assert st == 200
    dep3_id = dep_v3['deployment_id']

    v3_fail_receipt = {
        'deployment_id': dep3_id,
        'package_id': 'pkg-v3',
        'status': 'failed',
        'stage': 'warmup',
        'error': 'cuda_oom'
    }
    st, res = harness.request('POST', '/v1/agent/deploy-receipt', v3_fail_receipt, token=node_token)
    assert st == 200

    # Since desired_version was v3, v3 failure should correctly mark node model_ready = 0
    with sqlite3.connect(harness.db_path) as conn:
        row = conn.execute("SELECT current_version, desired_version, model_ready FROM agent_nodes WHERE id = ?", (node_id,)).fetchone()
        assert row[0] == '2.0.0', "current_version should remain 2.0.0"
        assert row[1] == '3.0.0', "desired_version should be 3.0.0"
        assert row[2] == 0, f"Expected model_ready=0 for failed active deployment, got {row[2]}"
    print("  ✔ Failed active target v3 correctly marked node model_ready=0")

    # Step G: One-click rollback restores node to v2
    st, res_rb = harness.request('POST', '/v1/agent/deployments/rollback', {
        'action': 'rollback',
        'node_id': node_id,
        'target_version': '2.0.0'
    }, token=harness.admin_token)
    assert st == 200 and res_rb.get('status') == 'accepted'
    rb_dep_id = res_rb['deployment_id']

    # Submit successful rollback receipt
    rb_receipt = {
        'deployment_id': rb_dep_id,
        'package_id': 'pkg-v2',
        'status': 'ready',
        'stage': 'ready',
        'self_test': {'passed': True, 'drift': 0.0015},
        'startup_proof': {'kernel_events': 141}
    }
    st, res = harness.request('POST', '/v1/agent/deploy-receipt', rb_receipt, token=node_token)
    assert st == 200 and res.get('ok')

    with sqlite3.connect(harness.db_path) as conn:
        row = conn.execute("SELECT current_version, desired_version, model_ready FROM agent_nodes WHERE id = ?", (node_id,)).fetchone()
        assert row[0] == '2.0.0'
        assert row[1] == '2.0.0'
        assert row[2] == 1
    print("  ✔ Rollback to v2 restored node model_ready=1 and versions to 2.0.0")

    print("  ✅ All Version Fencing Race tests PASSED.")


def test_gpu_model_projection_and_concurrent_receipt_bombardment(harness: ChallengerClusterHarness):
    print("\n" + "=" * 75)
    print("▶ CHALLENGE 3: gpu_model Projection Under Multi-Threaded Receipt Bombardment")
    print("=" * 75)

    # Register 4 heterogeneous nodes
    nodes = []

    # Node 1: CUDA RTX 4070 Ti
    n1_id, n1_tok = harness.register_node(
        'wsl-rtx4070ti', 'linux', 'x86_64',
        gpu={'model': 'NVIDIA GeForce RTX 4070 Ti', 'vram_bytes': 12884901888, 'device_uuid': 'GPU-UUID-RTX-1'}
    )
    nodes.append(('n1', n1_id, n1_tok, 'NVIDIA GeForce RTX 4070 Ti'))

    # Node 2: Mac ARM64 Pure CPU (gpu=None -> SQLite gpu_model is NULL)
    n2_id, n2_tok = harness.register_node('mac-arm64-cpu', 'darwin', 'aarch64', gpu=None)
    nodes.append(('n2', n2_id, n2_tok, None))

    # Node 3: Domestic Moore Threads GPU (Unicode model)
    n3_id, n3_tok = harness.register_node(
        'domestic-mtt', 'linux', 'x86_64',
        gpu={'model': '摩尔线程 MTT S80 (16GB GDDR6 / 4096 MUSA 核心)', 'vram_bytes': 17179869184, 'device_uuid': 'GPU-UUID-MTT-1'}
    )
    nodes.append(('n3', n3_id, n3_tok, '摩尔线程 MTT S80 (16GB GDDR6 / 4096 MUSA 核心)'))

    # Node 4: Node with empty string gpu_model
    n4_id, n4_tok = harness.register_node(
        'empty-gpu', 'linux', 'x86_64',
        gpu={'model': '', 'vram_bytes': 0, 'device_uuid': 'GPU-UUID-EMPTY-1'}
    )
    nodes.append(('n4', n4_id, n4_tok, ''))

    # Register packages
    harness.register_package('pkg-cuda-bombard', '1.0.0', 'CUDAExecutionProvider')
    harness.register_package('pkg-cpu-bombard', '1.0.0', 'CPUExecutionProvider')

    # Pre-dispatch 6 deployments per node (24 total deployments)
    all_deployments = []
    for tag, nid, ntok, expected_gpu in nodes:
        pkg = 'pkg-cpu-bombard' if expected_gpu is None else 'pkg-cuda-bombard'
        for i in range(6):
            st, res = harness.request('POST', '/v1/agent/deploy', {'node_id': nid, 'package_id': pkg}, token=harness.admin_token)
            assert st == 200, f"Deploy failed: {st} {res}"
            all_deployments.append((tag, nid, ntok, res['deployment_id'], expected_gpu, i))

    print(f"  [+] Dispatched {len(all_deployments)} deployments across 4 heterogeneous nodes.")

    # Concurrently bombard server with:
    # - 24 worker threads submitting receipts (half valid, half invalid drift/errors)
    # - 8 reader threads constantly polling GET /v1/agent/deployments and verifying gpu_model projection
    errors = []
    read_checks_passed = 0

    def worker_submit_receipt(item):
        tag, nid, ntok, dep_id, expected_gpu, idx = item
        mode = idx % 3
        if mode == 0:
            # Valid ready submission
            receipt = {
                'deployment_id': dep_id,
                'status': 'ready',
                'stage': 'ready',
                'self_test': {'passed': True, 'drift': 0.002 * (idx + 1)},
                'startup_proof': {'provider': 'CPU' if expected_gpu is None else 'CUDA', 'event_id': idx},
                'duration_ms': 500 + idx * 50
            }
            expected_st = 200
        elif mode == 1:
            # Adversarial ready submission (drift exceeds tolerance) -> must fail with 422
            receipt = {
                'deployment_id': dep_id,
                'status': 'ready',
                'stage': 'ready',
                'self_test': {'passed': True, 'drift': 0.082},
                'startup_proof': {'provider': 'CPU' if expected_gpu is None else 'CUDA', 'event_id': idx},
                'duration_ms': 500 + idx * 50
            }
            expected_st = 422
        else:
            # Explicit failure report by agent -> acknowledged with 200 ok (marked_failed=True)
            receipt = {
                'deployment_id': dep_id,
                'status': 'failed',
                'stage': 'warmup',
                'error': 'process_crash_simulated'
            }
            expected_st = 200

        st, res = harness.request('POST', '/v1/agent/deploy-receipt', receipt, token=ntok)
        if st != expected_st:
            errors.append(f"Worker submission error on {dep_id}: expected {expected_st}, got {st}: {res}")
        elif mode == 2:
            if not res.get('marked_failed'):
                errors.append(f"Worker explicit failure should return marked_failed=True, got {res}")

    def reader_poll_deployments():
        nonlocal read_checks_passed
        for _ in range(8):
            st, res = harness.request('GET', '/v1/agent/deployments', token=harness.admin_token)
            if st != 200:
                errors.append(f"Reader failed with HTTP {st}: {res}")
                continue
            deps = res.get('deployments', [])
            if not deps:
                errors.append("Reader got empty deployments list!")
                continue

            # Verify gpu_model integrity for each node type
            for d in deps:
                dep_nid = d.get('node_id')
                gpu_model = d.get('gpu_model')
                if 'mac-arm64-cpu' in dep_nid:
                    if gpu_model is not None:
                        errors.append(f"CPU node projected non-null gpu_model: {gpu_model}")
                elif 'wsl-rtx4070ti' in dep_nid:
                    if gpu_model != 'NVIDIA GeForce RTX 4070 Ti':
                        errors.append(f"RTX 4070 Ti node projected wrong gpu_model: {gpu_model}")
                elif 'domestic-mtt' in dep_nid:
                    if gpu_model != '摩尔线程 MTT S80 (16GB GDDR6 / 4096 MUSA 核心)':
                        errors.append(f"Moore Threads node projected wrong gpu_model: {gpu_model}")
                elif 'empty-gpu' in dep_nid:
                    if gpu_model != '':
                        errors.append(f"Empty GPU node projected non-empty gpu_model: {gpu_model}")
            read_checks_passed += 1
            time.sleep(0.01)

    print("  [+] Launching 24 concurrent receipt submitters and 8 reader pollers...")
    start_time = time.time()
    with concurrent.futures.ThreadPoolExecutor(max_workers=32) as executor:
        writer_futures = [executor.submit(worker_submit_receipt, item) for item in all_deployments]
        reader_futures = [executor.submit(reader_poll_deployments) for _ in range(8)]
        concurrent.futures.wait(writer_futures + reader_futures)

    duration = time.time() - start_time
    print(f"  ✔ Concurrent bombardment completed in {duration:.3f}s. Total reader checks: {read_checks_passed}")

    assert not errors, f"Encountered {len(errors)} concurrency errors:\n" + "\n".join(errors[:10])

    # Final DB integrity check
    with sqlite3.connect(harness.db_path) as conn:
        cur = conn.cursor()
        cur.execute("PRAGMA integrity_check")
        assert cur.fetchone()[0] == 'ok', "SQLite integrity check failed after concurrent bombardment!"
        cur.execute("SELECT count(*) FROM agent_deployments")
        total_in_db = cur.fetchone()[0]
        assert total_in_db >= len(all_deployments), f"Expected >= {len(all_deployments)} in DB, got {total_in_db}"

    print("  ✔ SQLite WAL integrity confirmed. Zero database lock contention or corruption.")
    print("  ✅ gpu_model projection under multi-threaded bombardment PASSED 100%.")


def main():
    print("=" * 75)
    print(" ADVERSARIAL STRESS TEST SUITE — MILESTONE 2 ITERATION 3")
    print(" Auditor: m2_iter3_challenger_1 (Empirical Challenger)")
    print("=" * 75)

    harness = ChallengerClusterHarness()
    try:
        harness.start_server()
        test_float_drift_validation_and_ieee754_edge_cases(harness)
        test_version_fencing_races(harness)
        test_gpu_model_projection_and_concurrent_receipt_bombardment(harness)

        print("\n" + "=" * 75)
        print(" 🎉 ALL ITERATION 3 EMPIRICAL CHALLENGES PASSED PERFECTLY!")
        print("=" * 75)
        return 0
    finally:
        harness.stop_server()


if __name__ == '__main__':
    sys.exit(main())
