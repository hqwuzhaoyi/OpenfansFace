"""
Empirical Challenger Verification Harness for Milestone 1 (#21: Agent 注册与节点视图).
Executes real HTTP REST requests against the compiled Rust face-file-operations binary
in a hermetic, strictly isolated test fixture environment.
"""
import argparse
import http.client
import json
import os
from pathlib import Path
import signal
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


class IsolatedClusterFixture:
    def __init__(self):
        self.root = Path(tempfile.mkdtemp(prefix='face-admin-person-fileops-challenger-', dir='/tmp')).resolve()
        (self.root / '.fileops-fixture').write_text('person file operations')
        self.base = self.root / 'base'
        self.media = self.root / 'media'
        self.base.mkdir()
        self.media.mkdir()
        self.ledger = self.root / 'ledger.db'

        # Directory for processing.db - open_or_init will create all tables automatically
        (self.base / 'processing').mkdir()
        self.db_path = self.base / 'processing' / 'processing.db'

        with sqlite3.connect(self.base / 'faces.db') as db:
            db.executescript("""
            CREATE TABLE videos(vid TEXT PRIMARY KEY, path TEXT, name TEXT, collection TEXT, size INTEGER);
            CREATE TABLE faces(vid TEXT, score REAL, thumb TEXT);
            """)

        with sqlite3.connect(self.ledger) as db:
            db.executescript("""
            CREATE TABLE asset(id INTEGER PRIMARY KEY, location TEXT, path TEXT, name TEXT);
            CREATE TABLE entity(id INTEGER PRIMARY KEY, kind TEXT, canonical_name TEXT, normalized_name TEXT, created_at TEXT, updated_at TEXT);
            CREATE TABLE asset_entity(asset_id INTEGER, entity_id INTEGER, role TEXT, source TEXT);
            CREATE TABLE entity_alias(entity_id INTEGER, alias TEXT, normalized_alias TEXT, source TEXT);
            """)

        with socket.socket() as s:
            s.bind(('127.0.0.1', 0))
            self.port = s.getsockname()[1]

        self.url = f'http://127.0.0.1:{self.port}'
        self.admin_token = 'admin-secret-token-123'
        self.env = {
            **os.environ,
            'FACES_BASE': str(self.base),
            'MEDIA2_ROOT': str(self.media),
            'PEACH_LEDGER_PATH': str(self.ledger),
            'FILE_OPERATIONS_BIND': f'127.0.0.1:{self.port}',
            'FILE_OPERATIONS_FIXTURE_ROOT': str(self.root),
            'FILE_OPERATIONS_RUST_TOKEN': self.admin_token,
        }
        self.process = None
        self.start()

    def start(self):
        self.log = (self.root / 'service.log').open('ab')
        self.process = subprocess.Popen([str(BINARY)], env=self.env, stdout=self.log, stderr=self.log)
        for _ in range(600):
            if self.process.poll() is not None:
                raise AssertionError((self.process.returncode, (self.root / 'service.log').read_text()))
            try:
                urllib.request.urlopen(self.url + '/health', timeout=0.2).close()
                return
            except OSError:
                time.sleep(0.02)
        raise AssertionError('Service did not start in time')

    def stop(self):
        if self.process and self.process.poll() is None:
            self.process.send_signal(signal.SIGINT)
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()
        self.log.close()

    def request(self, route, body=None, method=None, auth_token=None, expected_status=200):
        if method is None:
            method = 'GET' if body is None else 'POST'
        headers = {'Content-Type': 'application/json'}
        if auth_token is not None:
            headers['Authorization'] = f'Bearer {auth_token}'

        data = None if body is None else json.dumps(body).encode('utf-8')
        req = urllib.request.Request(self.url + route, data=data, headers=headers, method=method)

        code = 200
        val = None
        try:
            with urllib.request.urlopen(req, timeout=10) as resp:
                code = resp.status
                raw = resp.read()
                val = json.loads(raw) if raw else None
        except urllib.error.HTTPError as err:
            code = err.code
            raw = err.read()
            val = json.loads(raw) if raw else None

        if code != expected_status:
            raise AssertionError(f"Expected HTTP {expected_status} on {method} {route}, got {code}: {val}")
        return val


def run_all_challenger_tests():
    print("=" * 70)
    print(" EMPIRICAL CHALLENGER VERIFICATION FOR MILESTONE 1 (#21)")
    print("=" * 70)

    # 1. Compile binary check
    print("\n[Step 1] Ensuring Rust face-file-operations binary is fresh...")
    res = subprocess.run(
        ["cargo", "build", "--bin", "face-file-operations", "--manifest-path", str(REPO / "services/file-operations/Cargo.toml")],
        capture_output=True, text=True
    )
    if res.returncode != 0:
        print("FAILED to build binary:\n", res.stderr)
        sys.exit(1)
    print("✔ Binary build passed.")

    fixture = IsolatedClusterFixture()
    try:
        # TEST 1: Admin creates enrollment token
        print("\n[Test 1] Testing POST /v1/agent/enrollment-token...")
        # 1a: Missing auth token -> 401
        fixture.request('/v1/agent/enrollment-token', body={'ttl_seconds': 3600}, auth_token=None, expected_status=401)
        print("  ✔ Unauthenticated request rejected with 401.")

        # 1b: Valid admin auth -> 200
        token_data = fixture.request('/v1/agent/enrollment-token', body={'note': 'Challenger Node 1', 'ttl_seconds': 3600}, auth_token=fixture.admin_token, expected_status=200)
        enrollment_token = token_data['token']
        assert enrollment_token.startswith('et_'), f"Invalid token format: {enrollment_token}"
        print(f"  ✔ Generated valid enrollment token: {enrollment_token[:12]}...")

        # TEST 2: Agent registers with enrollment token
        print("\n[Test 2] Testing POST /v1/agent/register...")
        reg_payload = {
            "enrollment_token": enrollment_token,
            "hostname": "challenger-wsl-host",
            "os": "linux",
            "arch": "x86_64",
            "gpu": {
                "model": "NVIDIA GeForce RTX 4070 Ti",
                "vram_bytes": 12884901888,
                "device_uuid": "GPU-CHALLENGER-UUID-4070TI"
            },
            "ram_bytes": 34359738368,
            "cpu_cores": 16,
            "agent_version": "0.1.0"
        }
        reg_data = fixture.request('/v1/agent/register', body=reg_payload, expected_status=200)
        node_id = reg_data['node_id']
        node_token = reg_data['node_token']
        device_group_id = reg_data['device_group_id']
        assert node_id.startswith('node-challenger-wsl-host-')
        assert node_token.startswith('agt_')
        assert reg_data['heartbeat_interval_s'] == 15
        assert device_group_id.startswith('grp-gpu-')
        print(f"  ✔ Node successfully registered: id={node_id}, group={device_group_id}")

        # 2b: Token replay attack -> 401 (single-use consumption)
        fixture.request('/v1/agent/register', body=reg_payload, expected_status=401)
        print("  ✔ Single-use token replay correctly rejected with 401.")

        # TEST 3: Uncontacted node state in GET /v1/agent/nodes (Truth-in-UI)
        print("\n[Test 3] Testing GET /v1/agent/nodes before first heartbeat (Truth-in-UI)...")
        # 3a: Without admin token -> 401
        fixture.request('/v1/agent/nodes', auth_token=None, expected_status=401)
        print("  ✔ Unauthenticated GET /v1/agent/nodes rejected with 401.")

        # 3b: With admin token -> 200
        nodes_data = fixture.request('/v1/agent/nodes', auth_token=fixture.admin_token, expected_status=200)
        node_entry = next((n for n in nodes_data['nodes'] if n['node_id'] == node_id), None)
        assert node_entry is not None, f"Node {node_id} not in list"
        assert node_entry['status'] == 'registered'
        # TRUTH-IN-UI CHECK: Before heartbeat, telemetry metrics must be NULL, NEVER 0!
        telemetry = node_entry['telemetry']
        assert telemetry['last_heartbeat_at'] is None, f"Expected null last_heartbeat_at, got {telemetry['last_heartbeat_at']}"
        assert telemetry['free_ram_bytes'] is None, f"Expected null free_ram_bytes, got {telemetry['free_ram_bytes']}"
        assert telemetry['gpu']['free_vram_bytes'] is None, f"Expected null free_vram_bytes, got {telemetry['gpu']['free_vram_bytes']}"
        assert telemetry['gpu']['gpu_util_percent'] is None, f"Expected null gpu_util_percent, got {telemetry['gpu']['gpu_util_percent']}"
        print("  ✔ Uncontacted node telemetry metrics are strictly null (Truth-in-UI: render '未知', NEVER 0).")

        # TEST 4: Periodic Heartbeat
        print("\n[Test 4] Testing POST /v1/agent/heartbeat...")
        # 4a: Bad Bearer token -> 401
        fixture.request('/v1/agent/heartbeat', body={
            "node_id": node_id, "instance_id": "inst-1", "timestamp": time.time(),
            "free_ram_bytes": 20000000000, "free_vram_bytes": 10000000000,
            "gpu_util_percent": 18.5, "running_tasks": 0
        }, auth_token='bad_token_123', expected_status=401)
        print("  ✔ Invalid Bearer token rejected with 401.")

        # 4b: Valid Bearer token -> 200
        hb_resp = fixture.request('/v1/agent/heartbeat', body={
            "node_id": node_id, "instance_id": "inst-1", "timestamp": time.time(),
            "free_ram_bytes": 20000000000, "free_vram_bytes": 10000000000,
            "gpu_util_percent": 18.5, "running_tasks": 1
        }, auth_token=node_token, expected_status=200)
        assert hb_resp['ok'] is True
        print("  ✔ Valid heartbeat accepted.")

        # 4c: Check node status is now "online" with live metrics
        nodes_data2 = fixture.request('/v1/agent/nodes', auth_token=fixture.admin_token, expected_status=200)
        node_online = next(n for n in nodes_data2['nodes'] if n['node_id'] == node_id)
        assert node_online['status'] == 'online'
        assert node_online['telemetry']['free_ram_bytes'] == 20000000000
        assert node_online['telemetry']['gpu']['free_vram_bytes'] == 10000000000
        assert node_online['telemetry']['gpu']['gpu_util_percent'] == 18.5
        assert node_online['telemetry']['running_tasks'] == 1
        print("  ✔ Heartbeating node correctly transitions to 'online' with exact telemetry.")

        # TEST 5: Control Endpoint (Revoke and Restore Acceptance)
        print("\n[Test 5] Testing POST /v1/agent/nodes/control and /v1/agent/node-control...")
        # 5a: Invalid action -> 400
        fixture.request('/v1/agent/nodes/control', body={"node_id": node_id, "action": "reboot_machine"}, auth_token=fixture.admin_token, expected_status=400)
        print("  ✔ Invalid action rejected with 400.")

        # 5b: Non-existent node -> 404
        fixture.request('/v1/agent/nodes/control', body={"node_id": "node-nonexistent-404", "action": "revoke_acceptance"}, auth_token=fixture.admin_token, expected_status=404)
        print("  ✔ Non-existent node rejected with 404.")

        # 5c: Revoke acceptance via /v1/agent/nodes/control
        rev_resp = fixture.request('/v1/agent/nodes/control', body={"node_id": node_id, "action": "revoke_acceptance"}, auth_token=fixture.admin_token, expected_status=200)
        assert rev_resp['ok'] is True
        assert rev_resp['accepting'] is False
        print("  ✔ Revoke acceptance succeeded.")

        # Verify heartbeat receives pause_acceptance command
        hb_after_rev = fixture.request('/v1/agent/heartbeat', body={
            "node_id": node_id, "instance_id": "inst-1", "timestamp": time.time(),
            "free_ram_bytes": 20000000000, "free_vram_bytes": 10000000000,
            "gpu_util_percent": 18.5, "running_tasks": 0
        }, auth_token=node_token, expected_status=200)
        cmd_names = [c['command'] for c in hb_after_rev['commands']]
        assert 'pause_acceptance' in cmd_names, f"Expected pause_acceptance command, got {cmd_names}"
        print("  ✔ Heartbeat response contains 'pause_acceptance' command when revoked.")

        # 5d: Restore acceptance via alias /v1/agent/node-control
        rest_resp = fixture.request('/v1/agent/node-control', body={"node_id": node_id, "action": "restore_acceptance"}, auth_token=fixture.admin_token, expected_status=200)
        assert rest_resp['ok'] is True
        assert rest_resp['accepting'] is True
        print("  ✔ Restore acceptance via /v1/agent/node-control succeeded.")

        # TEST 6: Offline transition & telemetry nullification (Truth-in-UI)
        print("\n[Test 6] Testing 90s offline timeout & telemetry wiping (Truth-in-UI)...")
        with sqlite3.connect(fixture.db_path) as db:
            past_time = time.time() - 95.0
            db.execute("UPDATE agent_nodes SET last_heartbeat_at = ? WHERE id = ?", (past_time, node_id))
            db.commit()

        nodes_data3 = fixture.request('/v1/agent/nodes', auth_token=fixture.admin_token, expected_status=200)
        node_offline = next(n for n in nodes_data3['nodes'] if n['node_id'] == node_id)
        assert node_offline['status'] == 'offline', f"Expected offline status, got {node_offline['status']}"
        # CRITICAL TRUTH-IN-UI: offline telemetry must be null, NEVER 0!
        assert node_offline['telemetry']['free_ram_bytes'] is None, f"Expected null free_ram_bytes for offline node, got {node_offline['telemetry']['free_ram_bytes']}"
        assert node_offline['telemetry']['gpu']['free_vram_bytes'] is None, f"Expected null free_vram_bytes for offline node, got {node_offline['telemetry']['gpu']['free_vram_bytes']}"
        assert node_offline['telemetry']['gpu']['gpu_util_percent'] is None, f"Expected null gpu_util_percent for offline node, got {node_offline['telemetry']['gpu']['gpu_util_percent']}"
        print("  ✔ Offline node (>90s) dynamically transitions to 'offline' and telemetry is wiped to null (render '未知', NEVER 0).")

        # TEST 7: Safety & Systemd check
        print("\n[Test 7] Safety Check: indextts-server.service verification...")
        # Verify repo has no active reference running indextts
        res_grep = subprocess.run(["git", "grep", "-i", "indextts-server.service"], capture_output=True, text=True, cwd=str(REPO))
        lines = [l for l in res_grep.stdout.splitlines() if not l.startswith(".agents/") and not l.startswith("tests/challenger_")]
        for line in lines:
            assert "running" not in line.lower() and "start" not in line.lower(), f"Suspicious indextts reference: {line}"
        print("  ✔ indextts-server.service is strictly not touched or referenced as running.")

        # TEST 8: Standalone Client Daemon (face-agent binary) Enrollment & Single Heartbeat
        print("\n[Test 8] Testing standalone face-agent binary CLI enrollment & heartbeat...")
        agent_bin = REPO / 'services/file-operations/target/debug/face-agent'
        # 8a: Generate enrollment token for real client daemon
        token_for_daemon = fixture.request('/v1/agent/enrollment-token', body={'note': 'Real CLI Daemon', 'ttl_seconds': 3600}, auth_token=fixture.admin_token, expected_status=200)['token']
        agent_cfg_path = fixture.root / 'daemon-agent.json'
        res_daemon = subprocess.run([
            str(agent_bin),
            '--config', str(agent_cfg_path),
            '--center-url', fixture.url,
            '--enroll-token', token_for_daemon,
            '--once'
        ], capture_output=True, text=True)

        print("  Daemon stderr:\n", "    " + res_daemon.stderr.replace("\n", "\n    "))
        assert res_daemon.returncode == 0, f"face-agent CLI failed with code {res_daemon.returncode}: {res_daemon.stderr}"
        assert agent_cfg_path.exists(), "agent.json was not created by daemon"
        # Verify 0600 mode on unix
        mode = agent_cfg_path.stat().st_mode & 0o777
        assert mode == 0o600, f"Config file permissions must be 0600, got {oct(mode)}"

        # Check that center saw this daemon as online
        cfg_content = json.loads(agent_cfg_path.read_text())
        daemon_node_id = cfg_content['node_id']
        nodes_after_daemon = fixture.request('/v1/agent/nodes', auth_token=fixture.admin_token, expected_status=200)
        daemon_node_entry = next((n for n in nodes_after_daemon['nodes'] if n['node_id'] == daemon_node_id), None)
        assert daemon_node_entry is not None, "Daemon node not found in center nodes list"
        assert daemon_node_entry['status'] == 'online', f"Expected online status for daemon, got {daemon_node_entry['status']}"
        print(f"  ✔ Standalone face-agent enrolled (node_id={daemon_node_id}), config saved with 0600, and heartbeat delivered.")

        # EMPIRICAL OBSERVATION OF BUG ON MACOS:
        # Check what capabilities were registered by face-agent on macOS
        print(f"  Empirical check on macOS daemon capabilities:")
        print(f"    Reported OS: {daemon_node_entry['os']}")
        print(f"    Reported GPU: {daemon_node_entry['telemetry']['gpu']}")
        if sys.platform == 'darwin':
            assert daemon_node_entry['telemetry']['gpu'] is not None, (
                "macOS daemon must detect Metal GPU and register it with Center"
            )
            assert daemon_node_entry['telemetry']['gpu']['device_uuid'].startswith('METAL-'), (
                f"Expected METAL- device_uuid, got {daemon_node_entry['telemetry']['gpu']['device_uuid']}"
            )
            print(f"    ✔ macOS GPU was detected: {daemon_node_entry['telemetry']['gpu']['model']}")

        print("\n" + "=" * 70)
        print(" ALL ENDPOINT & PROTOCOL INTEGRATION TESTS PASSED SUCCESSFULLY!")
        print("=" * 70)

    finally:
        fixture.stop()


if __name__ == '__main__':
    run_all_challenger_tests()
