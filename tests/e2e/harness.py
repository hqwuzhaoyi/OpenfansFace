"""
Opaque-Box E2E Test Harness for OpenfansFace Multi-Agent Registration & Operations Tri-View (#20–#28).
Provides strictly isolated test environments, in-memory/temp SQLite WAL engines,
protocol validators, mathematical tolerance checks, and mock Center/Agent actors.
"""
from contextlib import contextmanager
import base64
import hashlib
import json
import math
import os
from pathlib import Path
import sqlite3
import struct
import tempfile
import time
from typing import Any, Dict, Generator, List, Optional, Tuple
import uuid

# Base repo path
REPO_ROOT = Path(__file__).resolve().parents[2]
FIXTURE_PORTRAIT = REPO_ROOT / "tests/fixtures/gpu-inference/synthetic-portrait.jpg"

assert FIXTURE_PORTRAIT.exists(), f"Synthetic portrait fixture missing at {FIXTURE_PORTRAIT}"


def sha256_bytes(data: bytes) -> str:
    """Calculate hex-encoded SHA-256 digest of byte payload."""
    return hashlib.sha256(data).hexdigest()


def sha256_file(path: Path) -> str:
    """Calculate hex-encoded SHA-256 digest of file."""
    h = hashlib.sha256()
    with open(path, "rb") as f:
        while chunk := f.read(65536):
            h.update(chunk)
    return h.hexdigest()


def cosine_similarity(v1: List[float], v2: List[float]) -> float:
    """Compute exact cosine similarity between two float vectors."""
    assert len(v1) == len(v2), f"Vector dimension mismatch: {len(v1)} vs {len(v2)}"
    dot = sum(a * b for a, b in zip(v1, v2))
    norm1 = math.sqrt(sum(a * a for a in v1))
    norm2 = math.sqrt(sum(b * b for b in v2))
    if norm1 == 0.0 or norm2 == 0.0:
        return 0.0
    return dot / (norm1 * norm2)


def generate_normalized_vector(dim: int = 512, seed: int = 42) -> List[float]:
    """Deterministic normalized float vector generator."""
    # Deterministic LCG
    vals = []
    state = seed & 0xFFFFFFFF
    for _ in range(dim):
        state = (state * 1664525 + 1013904223) & 0xFFFFFFFF
        f = (state / 0xFFFFFFFF) * 2.0 - 1.0
        vals.append(f)
    norm = math.sqrt(sum(x * x for x in vals))
    return [x / norm for x in vals]


def float_vector_to_blob(v: List[float]) -> bytes:
    """Pack float32 vector into little-endian bytes."""
    return struct.pack(f"<{len(v)}f", *v)


def blob_to_float_vector(b: bytes) -> List[float]:
    """Unpack little-endian byte blob into float32 vector."""
    count = len(b) // 4
    return list(struct.unpack(f"<{count}f", b))


class IsolatedTestEnvironment:
    """
    Hermetic test fixture environment.
    Creates a temporary scratch directory and an isolated SQLite WAL database.
    Guarantees no reads or writes to production databases or /mnt/media2.
    """

    def __init__(self, prefix: str = "face-admin-e2e-"):
        self.tmpdir = tempfile.TemporaryDirectory(prefix=prefix)
        self.root = Path(self.tmpdir.name).resolve()
        
        # Write isolation marker file
        (self.root / ".fixture-marker").write_text("openfansface-isolated-e2e-fixture")
        
        self.base_dir = self.root / "base"
        self.media_dir = self.root / "media"
        self.scratch_dir = self.root / "scratch"
        self.db_path = self.base_dir / "processing" / "processing.db"
        
        self.base_dir.mkdir(parents=True)
        self.media_dir.mkdir(parents=True)
        self.scratch_dir.mkdir(parents=True)
        (self.base_dir / "processing").mkdir(parents=True)

        self._init_db()

    def _init_db(self):
        conn = sqlite3.connect(self.db_path)
        try:
            conn.execute("PRAGMA journal_mode=WAL;")
            conn.execute("PRAGMA foreign_keys=ON;")
            
            # Core scheduling state machine tables
            conn.executescript("""
            CREATE TABLE IF NOT EXISTS settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS tasks (
                vid TEXT PRIMARY KEY,
                path TEXT NOT NULL UNIQUE,
                name TEXT NOT NULL,
                size INTEGER NOT NULL,
                mtime_ns TEXT NOT NULL,
                duration REAL,
                stage TEXT NOT NULL,
                retry_stage TEXT,
                text_attempts INTEGER NOT NULL DEFAULT 0,
                face_attempts INTEGER NOT NULL DEFAULT 0,
                next_retry REAL,
                priority INTEGER NOT NULL DEFAULT 0,
                is_new INTEGER NOT NULL DEFAULT 1,
                batch_id TEXT,
                created_at REAL NOT NULL,
                updated_at REAL NOT NULL,
                error TEXT,
                manual INTEGER NOT NULL DEFAULT 0,
                deferred INTEGER NOT NULL DEFAULT 0,
                note TEXT,
                data TEXT NOT NULL DEFAULT '{}',
                run_id TEXT,
                video_path TEXT,
                assigned_node_id TEXT,
                attempt_id TEXT,
                lease_expires_at REAL,
                coordinator_generation INTEGER NOT NULL DEFAULT 1
            );

            CREATE TABLE IF NOT EXISTS events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                at REAL NOT NULL,
                vid TEXT,
                kind TEXT NOT NULL,
                message TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS face_groups (
                id TEXT PRIMARY KEY,
                version TEXT NOT NULL,
                embedding BLOB NOT NULL
            );

            -- Multi-Agent cluster management tables (Issue #21–#28)
            CREATE TABLE IF NOT EXISTS enrollment_tokens (
                token TEXT PRIMARY KEY,
                created_at REAL NOT NULL,
                expires_at REAL NOT NULL,
                used_by_node TEXT,
                is_revoked INTEGER NOT NULL DEFAULT 0
            );

            CREATE TABLE IF NOT EXISTS agent_nodes (
                node_id TEXT PRIMARY KEY,
                hostname TEXT NOT NULL,
                os TEXT NOT NULL,
                arch TEXT NOT NULL,
                gpu_model TEXT,
                vram_bytes INTEGER,
                device_uuid TEXT,
                gpu_device_uuid TEXT,
                device_group_id TEXT NOT NULL DEFAULT '',
                max_concurrent_tasks INTEGER NOT NULL DEFAULT 0,
                ram_bytes INTEGER NOT NULL,
                cpu_cores INTEGER NOT NULL,
                agent_version TEXT NOT NULL,
                status TEXT NOT NULL, -- registered, online, model_ready, accepting, offline, revoked
                instance_id TEXT NOT NULL,
                last_heartbeat_at REAL NOT NULL,
                free_ram_bytes INTEGER,
                free_vram_bytes INTEGER,
                gpu_util_percent REAL,
                running_tasks INTEGER NOT NULL DEFAULT 0,
                node_token TEXT NOT NULL UNIQUE,
                enrolled_at REAL NOT NULL
            );

            CREATE TABLE IF NOT EXISTS agent_packages (
                package_id TEXT PRIMARY KEY,
                manifest_sha256 TEXT NOT NULL,
                model_sha256 TEXT NOT NULL,
                execution_provider TEXT NOT NULL,
                version TEXT NOT NULL,
                is_pinned INTEGER NOT NULL DEFAULT 1,
                created_at REAL NOT NULL
            );

            CREATE TABLE IF NOT EXISTS agent_deployments (
                deployment_id TEXT PRIMARY KEY,
                node_id TEXT NOT NULL,
                package_id TEXT NOT NULL,
                status TEXT NOT NULL, -- pending, downloading, verifying, starting, warming_up, self_testing, ready, failed, rolled_back
                desired_version TEXT NOT NULL,
                actual_version TEXT,
                stage TEXT NOT NULL,
                startup_proof TEXT,
                self_test_passed INTEGER,
                self_test_drift REAL,
                duration_ms INTEGER,
                error TEXT,
                updated_at REAL NOT NULL,
                FOREIGN KEY(node_id) REFERENCES agent_nodes(node_id),
                FOREIGN KEY(package_id) REFERENCES agent_packages(package_id)
            );

            CREATE TABLE IF NOT EXISTS agent_receipts (
                receipt_id TEXT PRIMARY KEY,
                node_id TEXT NOT NULL,
                attempt_id TEXT NOT NULL,
                stage TEXT NOT NULL,
                coordinator_generation INTEGER NOT NULL,
                committed_at REAL NOT NULL,
                receipt_data TEXT NOT NULL,
                UNIQUE(node_id, attempt_id)
            );

            CREATE TABLE IF NOT EXISTS account_rules (
                id TEXT PRIMARY KEY,
                account_name TEXT NOT NULL UNIQUE,
                creator_name TEXT NOT NULL,
                alias_list TEXT NOT NULL DEFAULT '[]',
                action TEXT NOT NULL DEFAULT 'skip_face'
            );
            """)

            # Seed initial configuration
            conn.executemany("INSERT OR REPLACE INTO settings (key, value) VALUES (?, ?)", [
                ("concurrency", "4"),
                ("coordinator_generation", "1"),
                ("paused", "0"),
                ("bootstrapped", "1"),
                ("completed_count", "0"),
            ])
            conn.commit()
        finally:
            conn.close()

    @contextmanager
    def get_connection(self) -> Generator[sqlite3.Connection, None, None]:
        """Open a raw sqlite3 connection with WAL mode and guarantee closing."""
        conn = sqlite3.connect(self.db_path)
        conn.row_factory = sqlite3.Row
        try:
            yield conn
        finally:
            conn.close()

    def close(self):
        """Cleanup temporary files."""
        self.tmpdir.cleanup()


class SimulatedCenterService:
    """
    Opaque-box implementation of the Center service orchestrating
    Multi-Agent Registration, Leasing, Operations Tri-View, and Receipt Commits.
    Directly enforces the REST interfaces from PROJECT.md § Interface Contracts.
    """

    def __init__(self, env: IsolatedTestEnvironment):
        self.env = env
        self.max_frame_bytes = 1_572_864  # 1.5MB hard bound (PROJECT.md § Feature 10)
        self.heartbeat_timeout_s = 90.0   # 90s timeout to OFFLINE (PROJECT.md § Feature 2)
        self.drift_tolerance = 0.05       # Max landmark/feature self-test tolerance (PROJECT.md § Feature 7)
        self.cosine_drift_floor = 0.9999   # Cosine similarity floor (PROJECT.md § Feature 13)

    # --------------------------------------------------------------------------
    # Database helpers
    # --------------------------------------------------------------------------
    def _get_setting(self, key: str, default: Any = None) -> Any:
        with self.env.get_connection() as conn:
            row = conn.execute("SELECT value FROM settings WHERE key = ?", (key,)).fetchone()
            if row:
                return row["value"]
            return default

    def _set_setting(self, key: str, value: Any):
        with self.env.get_connection() as conn:
            conn.execute("INSERT OR REPLACE INTO settings (key, value) VALUES (?, ?)", (key, str(value)))
            conn.commit()

    def get_coordinator_generation(self) -> int:
        return int(self._get_setting("coordinator_generation", 1))

    def bump_coordinator_generation(self) -> int:
        with self.env.get_connection() as conn:
            gen = int(self._get_setting("coordinator_generation", 1)) + 1
            conn.execute("INSERT OR REPLACE INTO settings (key, value) VALUES ('coordinator_generation', ?)", (str(gen),))
            conn.commit()
            return gen

    # --------------------------------------------------------------------------
    # 1. Enrollment & Registration (Feature 1, Issue #21)
    # --------------------------------------------------------------------------
    def create_enrollment_token(self, ttl_s: float = 3600.0) -> str:
        """Create a single-use cryptographically random enrollment token."""
        token = "enroll-" + uuid.uuid4().hex
        now = time.time()
        with self.env.get_connection() as conn:
            conn.execute(
                "INSERT INTO enrollment_tokens (token, created_at, expires_at, is_revoked) VALUES (?, ?, ?, 0)",
                (token, now, now + ttl_s)
            )
            conn.commit()
        return token

    def register_agent(self, payload: Dict[str, Any]) -> Tuple[int, Dict[str, Any]]:
        """
        POST /v1/agent/register
        Request: { enrollment_token, hostname, os, arch, gpu?, ram_bytes, cpu_cores, agent_version }
        Response: { node_id, node_token, heartbeat_interval_s }
        """
        now = time.time()
        enrollment_token = payload.get("enrollment_token")
        if not enrollment_token:
            return 400, {"error": "missing_token", "message": "缺少注册 Token"}

        with self.env.get_connection() as conn:
            token_row = conn.execute(
                "SELECT * FROM enrollment_tokens WHERE token = ?", (enrollment_token,)
            ).fetchone()

            if not token_row:
                return 401, {"error": "invalid_token", "message": "注册 Token 无效"}
            if token_row["is_revoked"] == 1:
                return 403, {"error": "token_revoked", "message": "注册 Token 已被撤回"}
            if token_row["expires_at"] < now:
                return 401, {"error": "token_expired", "message": "注册 Token 已过期"}
            if token_row["used_by_node"] is not None:
                return 409, {"error": "token_already_used", "message": "一次性 Token 已被使用"}

            # Validate required hardware fields
            for field in ["hostname", "os", "arch", "ram_bytes", "cpu_cores", "agent_version"]:
                if field not in payload:
                    return 400, {"error": "missing_field", "message": f"缺少硬件字段: {field}"}

            gpu = payload.get("gpu", {})
            gpu_model = gpu.get("model") if gpu else None
            vram_bytes = gpu.get("vram_bytes") if gpu else None
            device_uuid = gpu.get("device_uuid") if gpu else None

            # Physical device pooling check (Feature 3):
            # Check if an existing node already owns this device_uuid
            if device_uuid:
                existing_node = conn.execute(
                    "SELECT node_id, status FROM agent_nodes WHERE device_uuid = ? AND status != 'offline'",
                    (device_uuid,)
                ).fetchone()
                if existing_node:
                    # Device already claimed by an active node; agent can register but joins pool without duplicating quota
                    pass

            node_id = f"node-{payload['os']}-{uuid.uuid4().hex[:8]}"
            node_token = "node-token-" + uuid.uuid4().hex
            instance_id = uuid.uuid4().hex
            device_group_id = f"gpu-{hashlib.sha256(device_uuid.encode()).hexdigest()[:12]}" if device_uuid else ""
            max_tasks = int(payload.get("max_concurrent_tasks", 0))

            conn.execute("""
            INSERT INTO agent_nodes (
                node_id, hostname, os, arch, gpu_model, vram_bytes, device_uuid,
                gpu_device_uuid, device_group_id, max_concurrent_tasks,
                ram_bytes, cpu_cores, agent_version, status, instance_id,
                last_heartbeat_at, node_token, enrolled_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'registered', ?, ?, ?, ?)
            """, (
                node_id, payload["hostname"], payload["os"], payload["arch"],
                gpu_model, vram_bytes, device_uuid, device_uuid, device_group_id, max_tasks,
                payload["ram_bytes"], payload["cpu_cores"], payload["agent_version"],
                instance_id, now, node_token, now
            ))

            # Mark token consumed
            conn.execute(
                "UPDATE enrollment_tokens SET used_by_node = ? WHERE token = ?",
                (node_id, enrollment_token)
            )
            conn.commit()

            return 200, {
                "node_id": node_id,
                "node_token": node_token,
                "instance_id": instance_id,
                "heartbeat_interval_s": 15
            }

    # --------------------------------------------------------------------------
    # 2. Heartbeat & Liveness (Feature 2, 17, 18, Issue #21, #26)
    # --------------------------------------------------------------------------
    def agent_heartbeat(self, node_token: str, payload: Dict[str, Any]) -> Tuple[int, Dict[str, Any]]:
        """
        POST /v1/agent/heartbeat
        Headers: Authorization: Bearer <node_token>
        Payload: { node_id, instance_id, timestamp, free_ram_bytes, free_vram_bytes, gpu_util_percent, running_tasks }
        """
        now = time.time()
        node_id = payload.get("node_id")
        instance_id = payload.get("instance_id")

        if not node_id or not instance_id:
            return 400, {"error": "invalid_payload", "message": "缺少 node_id 或 instance_id"}

        with self.env.get_connection() as conn:
            node = conn.execute("SELECT * FROM agent_nodes WHERE node_id = ?", (node_id,)).fetchone()
            if not node:
                return 404, {"error": "node_not_found", "message": "节点不存在"}
            if node["node_token"] != node_token:
                return 401, {"error": "unauthorized", "message": "节点凭据无效"}
            if node["status"] == "revoked":
                return 403, {"error": "node_revoked", "message": "节点权限已被撤销"}

            # Agent Crash Reconciliation check (Feature 18)
            # If instance_id differs, this node has restarted! Reclaim previous leases and bump generation.
            if node["instance_id"] != instance_id:
                old_instance = node["instance_id"]
                conn.execute(
                    "UPDATE agent_nodes SET instance_id = ?, status = 'registered' WHERE node_id = ?",
                    (instance_id, node_id)
                )
                # Reclaim orphaned tasks held by the previous instance
                conn.execute("""
                UPDATE tasks
                SET run_id = NULL, lease_expires_at = NULL, retry_stage = stage
                WHERE run_id LIKE ?
                """, (f"%{node_id}%",))
                conn.execute("""
                INSERT INTO events (at, vid, kind, message)
                VALUES (?, NULL, 'agent_restart_reconciled', ?)
                """, (now, f"Node {node_id} restarted (instance {old_instance} -> {instance_id}); orphaned leases reclaimed"))

            # Update telemetry and online status
            new_status = "registered" if node["instance_id"] != instance_id else node["status"]
            if new_status in ("registered", "offline"):
                new_status = "online"

            conn.execute("""
            UPDATE agent_nodes
            SET status = ?, last_heartbeat_at = ?, free_ram_bytes = ?, free_vram_bytes = ?,
                gpu_util_percent = ?, running_tasks = ?
            WHERE node_id = ?
            """, (
                new_status, now, payload.get("free_ram_bytes"), payload.get("free_vram_bytes"),
                payload.get("gpu_util_percent"), payload.get("running_tasks", 0), node_id
            ))
            conn.commit()

            return 200, {
                "ok": True,
                "desired_version": self._get_setting("desired_agent_version", "1.0.0"),
                "commands": []
            }

    def check_liveness_timeouts(self) -> List[str]:
        """Mark nodes with heartbeat older than 90s as OFFLINE (Feature 2)."""
        now = time.time()
        cutoff = now - self.heartbeat_timeout_s
        offline_nodes = []
        with self.env.get_connection() as conn:
            rows = conn.execute(
                "SELECT node_id FROM agent_nodes WHERE status != 'offline' AND status != 'revoked' AND last_heartbeat_at < ?",
                (cutoff,)
            ).fetchall()
            for r in rows:
                nid = r["node_id"]
                conn.execute("UPDATE agent_nodes SET status = 'offline' WHERE node_id = ?", (nid,))
                conn.execute("""
                INSERT INTO events (at, vid, kind, message)
                VALUES (?, NULL, 'node_offline', ?)
                """, (now, f"Node {nid} missed heartbeat for >90s; marked OFFLINE"))
                offline_nodes.append(nid)
            conn.commit()
        return offline_nodes

    # --------------------------------------------------------------------------
    # 3. Audited Runtime Package Distribution (Features 5, 7, 8, Issue #22)
    # --------------------------------------------------------------------------
    def register_package(self, package_id: str, manifest_sha256: str, model_sha256: str,
                         execution_provider: str, version: str) -> Dict[str, Any]:
        """Register an audited immutable package."""
        now = time.time()
        with self.env.get_connection() as conn:
            conn.execute("""
            INSERT OR REPLACE INTO agent_packages (
                package_id, manifest_sha256, model_sha256, execution_provider, version, created_at
            ) VALUES (?, ?, ?, ?, ?, ?)
            """, (package_id, manifest_sha256, model_sha256, execution_provider, version, now))
            conn.commit()
        return {"ok": True, "package_id": package_id}

    def deploy_package(self, node_id: str, package_id: str) -> Tuple[int, Dict[str, Any]]:
        """
        POST /v1/agent/deploy
        Triggers deployment of pinned package to node.
        """
        with self.env.get_connection() as conn:
            node = conn.execute("SELECT * FROM agent_nodes WHERE node_id = ?", (node_id,)).fetchone()
            if not node:
                return 404, {"error": "node_not_found"}
            if node["status"] == "offline":
                return 409, {"error": "node_offline", "message": "离线节点无法部署"}

            pkg = conn.execute("SELECT * FROM agent_packages WHERE package_id = ?", (package_id,)).fetchone()
            if not pkg:
                return 404, {"error": "package_not_found"}

            deployment_id = f"dep-{node_id}-{uuid.uuid4().hex[:6]}"
            conn.execute("""
            INSERT INTO agent_deployments (
                deployment_id, node_id, package_id, status, desired_version, stage, updated_at
            ) VALUES (?, ?, ?, 'downloading', ?, 'download', ?)
            """, (deployment_id, node_id, package_id, pkg["version"], time.time()))
            conn.commit()

            return 200, {
                "status": "accepted",
                "deployment_id": deployment_id,
                "package_id": package_id,
                "manifest_sha256": pkg["manifest_sha256"],
                "model_sha256": pkg["model_sha256"],
                "execution_provider": pkg["execution_provider"]
            }

    def submit_deploy_receipt(self, payload: Dict[str, Any]) -> Tuple[int, Dict[str, Any]]:
        """
        POST /v1/agent/deploy-receipt
        Receipt verifying package verification, startup profile, and self-test drift.
        """
        deployment_id = payload.get("deployment_id")
        status = payload.get("status")
        self_test = payload.get("self_test", {})
        passed = self_test.get("passed", False)
        drift = self_test.get("drift", 1.0)

        with self.env.get_connection() as conn:
            dep = conn.execute("SELECT * FROM agent_deployments WHERE deployment_id = ?", (deployment_id,)).fetchone()
            if not dep:
                return 404, {"error": "deployment_not_found"}

            if status == "ready":
                # Validate self test tolerance (Feature 7)
                if not passed or drift > self.drift_tolerance:
                    conn.execute("""
                    UPDATE agent_deployments
                    SET status = 'failed', stage = 'self_test', self_test_passed = 0,
                        self_test_drift = ?, error = 'self_test_drift_exceeded', updated_at = ?
                    WHERE deployment_id = ?
                    """, (drift, time.time(), deployment_id))
                    conn.commit()
                    return 422, {"ok": False, "error": "自检偏差超出阈值 (drift > 0.05)"}

                conn.execute("""
                UPDATE agent_deployments
                SET status = 'ready', stage = 'ready', actual_version = desired_version,
                    self_test_passed = 1, self_test_drift = ?, startup_proof = ?, duration_ms = ?, updated_at = ?
                WHERE deployment_id = ?
                """, (drift, json.dumps(payload.get("startup_proof", {})), payload.get("duration_ms", 0), time.time(), deployment_id))
                
                # Mark node model_ready and accepting
                conn.execute(
                    "UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?",
                    (dep["node_id"],)
                )
                conn.commit()
                return 200, {"ok": True}
            else:
                conn.execute("""
                UPDATE agent_deployments
                SET status = 'failed', stage = ?, error = ?, updated_at = ?
                WHERE deployment_id = ?
                """, (payload.get("stage", "failed"), payload.get("error", "unknown"), time.time(), deployment_id))
                conn.commit()
                return 200, {"ok": True, "marked_failed": True}

    # --------------------------------------------------------------------------
    # 4. Capability-Based Task Lease & Control Separation (Features 9, 10, Issue #23)
    # --------------------------------------------------------------------------
    def claim_task(self, node_id: str, stage_capability: str,
                   lease_duration_s: float = 60.0) -> Tuple[int, Dict[str, Any]]:
        """
        POST /v1/agent/claim
        Center claims task and leases to qualified node with fencing tokens.
        """
        now = time.time()
        with self.env.get_connection() as conn:
            conn.execute("BEGIN IMMEDIATE")
            try:
                node = conn.execute("SELECT * FROM agent_nodes WHERE node_id = ?", (node_id,)).fetchone()
                if not node:
                    conn.commit()
                    return 404, {"error": "node_not_found"}
                if node["status"] != "accepting":
                    conn.commit()
                    return 409, {"error": "node_not_accepting", "message": f"节点状态为 {node['status']}，不能接单"}

                # Quota Hierarchy check (Feature 20)
                # Tier 1: Global Cluster Concurrency
                concurrency = int(self._get_setting("concurrency", 4))
                active_tasks = conn.execute(
                    "SELECT COUNT(*) as cnt FROM tasks WHERE lease_expires_at > ?", (now,)
                ).fetchone()["cnt"]

                if active_tasks >= concurrency:
                    conn.commit()
                    return 503, {"error": "cluster_busy", "message": "达到集群配额上限"}

                # Tier 2 & Tier 4: Node CPU cores, worker slots, and RAM limits
                cpu_cores = node["cpu_cores"] if node["cpu_cores"] is not None else 0
                node_keys = node.keys()
                max_tasks = node["max_concurrent_tasks"] if "max_concurrent_tasks" in node_keys and node["max_concurrent_tasks"] is not None else 0
                effective_slots = min(max_tasks, cpu_cores) if max_tasks > 0 and cpu_cores > 0 else (max_tasks if max_tasks > 0 else cpu_cores)

                if effective_slots > 0 and node["running_tasks"] >= effective_slots:
                    conn.commit()
                    return 503, {"error": "node_busy", "message": "达到节点任务配额上限"}

                free_ram = node["free_ram_bytes"] if "free_ram_bytes" in node_keys and node["free_ram_bytes"] is not None else 0
                if free_ram > 0 and free_ram < 256 * 1024 * 1024:
                    conn.commit()
                    return 503, {"error": "node_busy", "message": "主机可用内存低于 256 MiB 准入门槛"}

                # Tier 3: Physical GPU Quota Grouping
                dev_uuid = node["device_uuid"] if "device_uuid" in node_keys and node["device_uuid"] else (node["gpu_device_uuid"] if "gpu_device_uuid" in node_keys and node["gpu_device_uuid"] else "")
                dev_grp = node["device_group_id"] if "device_group_id" in node_keys and node["device_group_id"] else ""
                if stage_capability == "detect" or (dev_grp and dev_grp.startswith("gpu-")):
                    if dev_uuid or dev_grp:
                        gpu_active = conn.execute("""
                            SELECT COALESCE(SUM(running_tasks), 0) as cnt
                            FROM agent_nodes
                            WHERE ((device_group_id != '' AND device_group_id = ?) OR (device_uuid != '' AND device_uuid = ?))
                              AND status IN ('model_ready', 'accepting')
                        """, (dev_grp, dev_uuid)).fetchone()["cnt"]
                        if gpu_active >= 1:
                            conn.commit()
                            return 503, {"error": "gpu_busy", "message": "达到物理 GPU 资源池配额上限"}

                # Find matching task for this capability
                task = conn.execute("""
                SELECT * FROM tasks
                WHERE stage = ? AND (lease_expires_at IS NULL OR lease_expires_at < ?)
                ORDER BY priority DESC, created_at ASC
                LIMIT 1
                """, (stage_capability, now)).fetchone()

                if not task:
                    conn.commit()
                    return 204, {}  # No task available

                attempt_id = f"run-{node_id}-{uuid.uuid4().hex[:8]}"
                lease_expires_at = now + lease_duration_s
                coord_gen = self.get_coordinator_generation()

                conn.execute("""
                UPDATE tasks
                SET run_id = ?, lease_expires_at = ?, coordinator_generation = ?, updated_at = ?
                WHERE vid = ?
                """, (attempt_id, lease_expires_at, coord_gen, now, task["vid"]))

                conn.execute(
                    "UPDATE agent_nodes SET running_tasks = running_tasks + 1 WHERE node_id = ?",
                    (node_id,)
                )
                conn.commit()
            except Exception:
                try:
                    conn.rollback()
                except Exception:
                    pass
                raise

            # Prepare bounded frame payload (Feature 10: bounded frames <= 1.5MB)
            # Read synthetic portrait fixture
            frame_bytes = FIXTURE_PORTRAIT.read_bytes()
            assert len(frame_bytes) <= self.max_frame_bytes, "Fixture exceeds 1.5MB limit!"

            frame_data = {
                "index": 0,
                "timestamp_ms": 1000,
                "frame_sha256": sha256_bytes(frame_bytes),
                "data_base64": base64.b64encode(frame_bytes).decode("ascii")
            }

            return 200, {
                "attempt_id": attempt_id,
                "vid": task["vid"],
                "lease_id": str(uuid.uuid4()),
                "coordinator_generation": coord_gen,
                "lease_expires_at_ms": int(lease_expires_at * 1000),
                "stage": stage_capability,
                "frames": [frame_data]
            }

    # --------------------------------------------------------------------------
    # 5. Native Receipt Commit & Generation Fencing (Features 11, 17, 19, Issue #23, #26)
    # --------------------------------------------------------------------------
    def commit_task_result(self, node_token: str, payload: Dict[str, Any]) -> Tuple[int, Dict[str, Any]]:
        """
        POST /v1/agent/task-result
        Validates attempt_id == tasks.run_id, coordinator_generation == current,
        now < lease_expires_at, and deduplicates via (node_id, attempt_id).
        """
        now = time.time()
        node_id = payload.get("node_id")
        attempt_id = payload.get("attempt_id")
        vid = payload.get("vid")
        coord_gen = payload.get("coordinator_generation")
        stage = payload.get("stage")

        if not all([node_id, attempt_id, vid, coord_gen is not None, stage]):
            return 400, {"error": "missing_fields", "message": "缺少必要回执字段"}

        with self.env.get_connection() as conn:
            node = conn.execute("SELECT * FROM agent_nodes WHERE node_id = ?", (node_id,)).fetchone()
            if not node or node["node_token"] != node_token:
                return 401, {"error": "unauthorized"}

            # Transactional Outbox & Receipt Dedup (Feature 19)
            existing_receipt = conn.execute(
                "SELECT * FROM agent_receipts WHERE node_id = ? AND attempt_id = ?",
                (node_id, attempt_id)
            ).fetchone()
            if existing_receipt:
                # Idempotent response: return success without duplicate increment
                return 200, {
                    "ok": True,
                    "idempotent": True,
                    "message": "已成功处理的回执（幂等响应）"
                }

            task = conn.execute("SELECT * FROM tasks WHERE vid = ?", (vid,)).fetchone()
            if not task:
                return 404, {"error": "task_not_found"}

            current_gen = self.get_coordinator_generation()
            # Fencing check 1: Generation check (Feature 17)
            if coord_gen != current_gen:
                return 409, {
                    "error": "stale_generation",
                    "current_generation": current_gen,
                    "submitted_generation": coord_gen,
                    "message": f"代际已失效 (中心当前代际: {current_gen}, 提交代际: {coord_gen})"
                }

            # Fencing check 2: Attempt ID match
            if task["run_id"] != attempt_id:
                return 409, {
                    "error": "attempt_mismatch",
                    "current_run_id": task["run_id"],
                    "submitted_attempt_id": attempt_id,
                    "message": "尝试编号不匹配，任务可能已被接管"
                }

            # Fencing check 3: Lease expiration check
            if task["lease_expires_at"] and task["lease_expires_at"] < now:
                return 409, {
                    "error": "lease_expired",
                    "expired_at": task["lease_expires_at"],
                    "now": now,
                    "message": "任务租约已超时过期"
                }

            # Fencing check 4: Operator manual takeover check
            if task["manual"] != 0:
                return 409, {
                    "error": "manual_takeover",
                    "message": "任务已被人工接管"
                }

            # Advance stage or complete
            receipt_id = f"rec-{uuid.uuid4().hex[:10]}"
            next_stage = "done"
            data = json.loads(task["data"])

            # Check if this is OCR stage and if account rule can skip face stage (Feature 15)
            if stage == "ocr":
                text_snippets = payload.get("text_snippets", [])
                skip_face, matched_creator = self._match_account_rules(conn, text_snippets)
                if skip_face or payload.get("face_skipped"):
                    data["face_skipped"] = True
                    data["creator"] = matched_creator or payload.get("creator")
                    next_stage = "done"
                else:
                    data["face_skipped"] = False
                    next_stage = "detect"
            elif stage == "detect":
                detections = payload.get("detections", [])
                data["detections"] = detections
                next_stage = "feature" if detections else "done"
            elif stage == "feature":
                vectors = payload.get("vectors", [])
                data["vectors_count"] = len(vectors)
                next_stage = "done"

            # Monotonic completion count increment
            is_completed = (next_stage == "done")
            completed_increment = 1 if is_completed else 0

            # Atomic database commit
            conn.execute("""
            INSERT INTO agent_receipts (
                receipt_id, node_id, attempt_id, stage, coordinator_generation, committed_at, receipt_data
            ) VALUES (?, ?, ?, ?, ?, ?, ?)
            """, (receipt_id, node_id, attempt_id, stage, coord_gen, now, json.dumps(payload)))

            conn.execute("""
            UPDATE tasks
            SET stage = ?, run_id = NULL, lease_expires_at = NULL, data = ?, updated_at = ?
            WHERE vid = ?
            """, (next_stage, json.dumps(data), now, vid))

            if completed_increment > 0:
                conn.execute(
                    "UPDATE settings SET value = CAST(CAST(value AS INTEGER) + 1 AS TEXT) WHERE key = 'completed_count'"
                )

            conn.execute(
                "UPDATE agent_nodes SET running_tasks = MAX(0, running_tasks - 1) WHERE node_id = ?",
                (node_id,)
            )
            conn.commit()

            return 200, {
                "ok": True,
                "receipt_id": receipt_id,
                "next_stage": next_stage,
                "completed": is_completed
            }

    def _match_account_rules(self, conn: sqlite3.Connection, text_snippets: List[str]) -> Tuple[bool, Optional[str]]:
        """Evaluate OCR snippets against account_rules table (Feature 15)."""
        all_text = " ".join(text_snippets).lower()
        rules = conn.execute("SELECT * FROM account_rules").fetchall()
        for r in rules:
            acc = r["account_name"].lower()
            if acc in all_text:
                return True, r["creator_name"]
            aliases = json.loads(r["alias_list"])
            for alias in aliases:
                if alias.lower() in all_text:
                    return True, r["creator_name"]
        return False, None

    # --------------------------------------------------------------------------
    # 6. Lease Expiry & Dead Node Reaping (Feature 17, Issue #26)
    # --------------------------------------------------------------------------
    def reap_expired_leases(self) -> List[str]:
        """
        Reclaims tasks whose lease_expires_at < now.
        Increments coordinator generation so that in-flight callbacks are rejected with 409.
        """
        now = time.time()
        reclaimed_vids = []
        with self.env.get_connection() as conn:
            expired_tasks = conn.execute(
                "SELECT vid, run_id FROM tasks WHERE lease_expires_at IS NOT NULL AND lease_expires_at < ?",
                (now,)
            ).fetchall()

            if expired_tasks:
                # Increment coordinator generation
                new_gen = self.bump_coordinator_generation()
                for t in expired_tasks:
                    vid = t["vid"]
                    conn.execute("""
                    UPDATE tasks
                    SET run_id = NULL, lease_expires_at = NULL, coordinator_generation = ?, updated_at = ?
                    WHERE vid = ?
                    """, (new_gen, now, vid))
                    conn.execute("""
                    INSERT INTO events (at, vid, kind, message)
                    VALUES (?, ?, 'lease_expired_reclaimed', ?)
                    """, (now, vid, f"Task {vid} lease expired (run_id: {t['run_id']}); generation bumped to {new_gen}"))
                    reclaimed_vids.append(vid)
                conn.commit()

        return reclaimed_vids

    # --------------------------------------------------------------------------
    # 7. Quota & Concurrency Reduction (Features 20, 21, Issue #27)
    # --------------------------------------------------------------------------
    def set_concurrency(self, new_concurrency: int) -> Dict[str, Any]:
        """
        Gracefully reduces or increases concurrency.
        Lowering concurrency halts new task leases while letting in-flight tasks finish.
        """
        assert new_concurrency >= 0, "Concurrency cannot be negative"
        self._set_setting("concurrency", new_concurrency)
        return {"ok": True, "concurrency": new_concurrency}

    # --------------------------------------------------------------------------
    # 8. Node Revocation & Control (Features 4, 21, 23, 25, Issue #21, #28)
    # --------------------------------------------------------------------------
    def control_node(self, node_id: str, action: str) -> Tuple[int, Dict[str, Any]]:
        """
        POST /api/processing/nodes/control
        Actions: revoke_acceptance, restore_acceptance, drain
        """
        now = time.time()
        with self.env.get_connection() as conn:
            node = conn.execute("SELECT * FROM agent_nodes WHERE node_id = ?", (node_id,)).fetchone()
            if not node:
                return 404, {"error": "node_not_found"}

            if action == "revoke_acceptance":
                conn.execute("UPDATE agent_nodes SET status = 'revoked' WHERE node_id = ?", (node_id,))
                conn.execute("""
                INSERT INTO events (at, vid, kind, message)
                VALUES (?, NULL, 'node_revoked', ?)
                """, (now, f"Node {node_id} acceptance privilege revoked by operator"))
            elif action == "restore_acceptance":
                conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (node_id,))
                conn.execute("""
                INSERT INTO events (at, vid, kind, message)
                VALUES (?, NULL, 'node_restored', ?)
                """, (now, f"Node {node_id} restored to accepting state"))
            elif action == "drain":
                # Mark as model_ready (stop leasing new tasks) while waiting for running_tasks to reach 0
                conn.execute("UPDATE agent_nodes SET status = 'model_ready' WHERE node_id = ?", (node_id,))
            else:
                return 400, {"error": "unknown_action", "action": action}

            conn.commit()
            return 200, {"ok": True, "node_id": node_id, "action": action}

    # --------------------------------------------------------------------------
    # 9. Operations UI Endpoints (Features 4, 8, 22, Issue #21, #22, #27)
    # --------------------------------------------------------------------------
    def get_ui_tasks(self, stage: Optional[str] = None, node_id: Optional[str] = None,
                     waiting_reason: Optional[str] = None, batch: Optional[str] = None,
                     batch_id: Optional[str] = None) -> Dict[str, Any]:
        """GET /api/processing/tasks"""
        now = time.time()
        with self.env.get_connection() as conn:
            concurrency = int(self._get_setting("concurrency", 4))
            active_tasks = conn.execute(
                "SELECT COUNT(*) as cnt FROM tasks WHERE lease_expires_at > ?", (now,)
            ).fetchone()["cnt"]

            online_gpu_nodes = conn.execute("""
                SELECT COUNT(*) as cnt FROM agent_nodes
                WHERE (gpu_model IS NOT NULL AND gpu_model != '')
                  AND status IN ('model_ready', 'accepting')
                  AND (? - last_heartbeat_at) < 90.0
            """, (now,)).fetchone()["cnt"]

            query = "SELECT * FROM tasks WHERE 1=1"
            params = []
            if stage:
                query += " AND stage = ?"
                params.append(stage)
            target_batch = batch_id or batch
            if target_batch:
                query += " AND batch_id = ?"
                params.append(target_batch)
            query += " ORDER BY priority DESC, created_at ASC"

            rows = conn.execute(query, params).fetchall()
            tasks = []
            for r in rows:
                t = dict(r)
                if t.get("size") is not None and t["size"] <= 0:
                    t["size"] = None
                if t.get("duration") is not None and t["duration"] <= 0:
                    t["duration"] = None

                manual = bool(t.get("manual", 0))
                t_stage = t.get("stage", "queued")
                retry_stage = t.get("retry_stage") or ""
                next_retry = float(t.get("next_retry") or 0.0)
                lease_exp = float(t.get("lease_expires_at") or 0.0)
                note = t.get("note") or ""

                if manual:
                    wr = "paused"
                elif t_stage == "retry" and next_retry > now:
                    wr = "rate_limited"
                elif lease_exp > now:
                    wr = "none"
                elif note and (note.startswith("waiting_") or note in ("quota_exhausted", "quota_limit", "gpu_busy", "rate_limited")):
                    if note in ("waiting_for_vram", "gpu_busy"):
                        wr = "gpu_busy"
                    elif note in ("waiting_for_node", "waiting_node"):
                        wr = "waiting_for_node"
                    elif note in ("quota_limit", "quota_exhausted"):
                        wr = "quota_exhausted"
                    else:
                        wr = note
                elif (t_stage == "detect" or retry_stage == "detect") and online_gpu_nodes == 0:
                    wr = "waiting_for_node"
                elif active_tasks >= concurrency and (t_stage in ("queued", "face_queued", "detect", "ocr", "feature")):
                    wr = "quota_exhausted"
                elif t_stage in ("queued", "face_queued"):
                    wr = "waiting_for_node"
                else:
                    wr = "none"

                t["waiting_reason"] = wr
                t["video_path"] = t.get("video_path") or t.get("path") or f"/storage/videos/{t['vid']}.mp4"
                if "assigned_node_id" not in t or not t["assigned_node_id"]:
                    run_id = t.get("run_id") or ""
                    if run_id.startswith("run-"):
                        parts = run_id.split("-")
                        if len(parts) >= 2:
                            t["assigned_node_id"] = parts[1]
                        else:
                            t["assigned_node_id"] = None
                    else:
                        t["assigned_node_id"] = None

                if node_id:
                    matched_node = (t.get("assigned_node_id") == node_id) or (t.get("run_id") and node_id in t["run_id"])
                    if not matched_node:
                        continue

                if waiting_reason and waiting_reason != "all":
                    if wr != waiting_reason:
                        synonyms = {
                            "quota_exhausted": ["quota_limit", "quota_exhausted"],
                            "quota_limit": ["quota_limit", "quota_exhausted"],
                            "waiting_for_node": ["waiting_node", "waiting_for_node"],
                            "waiting_node": ["waiting_node", "waiting_for_node"],
                            "rate_limited": ["rate_limited", "retry_backoff"],
                            "retry_backoff": ["rate_limited", "retry_backoff"],
                        }
                        allowed = synonyms.get(waiting_reason, [waiting_reason])
                        if wr not in allowed:
                            continue

                tasks.append(t)

            return {
                "tasks": tasks,
                "total": len(tasks),
                "completed_count": int(self._get_setting("completed_count", 0)),
                "concurrency": int(self._get_setting("concurrency", 4))
            }

    def get_ui_nodes(self) -> Dict[str, Any]:
        """
        GET /api/processing/nodes
        Shows all registered nodes with distinct statuses and hardware telemetry.
        Never converts unknown resources to zero.
        """
        with self.env.get_connection() as conn:
            rows = conn.execute("SELECT * FROM agent_nodes ORDER BY enrolled_at DESC").fetchall()
            nodes = []
            for r in rows:
                d = dict(r)
                # Ensure unknown metrics remain None (to be rendered as "未知" per AGENTS.md)
                nodes.append(d)
            return {"nodes": nodes, "total": len(nodes)}

    def get_ui_deployments(self) -> Dict[str, Any]:
        """GET /api/processing/deployments"""
        with self.env.get_connection() as conn:
            rows = conn.execute("""
            SELECT d.*, p.version as pkg_version, p.execution_provider, p.manifest_sha256
            FROM agent_deployments d
            JOIN agent_packages p ON d.package_id = p.package_id
            ORDER BY d.updated_at DESC
            """).fetchall()
            return {"deployments": [dict(r) for r in rows]}
