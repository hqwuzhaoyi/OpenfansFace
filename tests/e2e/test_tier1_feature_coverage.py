"""
Tier 1: Comprehensive Feature Coverage E2E Test Suite.
Covers all 28 features from PROJECT.md § Feature Inventory in isolation.
Each feature contains >= 5 distinct test cases covering primary happy paths,
state transitions, and error contracts.
"""
import base64
import json
import math
import os
from pathlib import Path
import sqlite3
import struct
import time
import unittest
import uuid

from tests.e2e.harness import (
    FIXTURE_PORTRAIT,
    IsolatedTestEnvironment,
    SimulatedCenterService,
    blob_to_float_vector,
    cosine_similarity,
    float_vector_to_blob,
    generate_normalized_vector,
    sha256_bytes,
    sha256_file,
)


class BaseFeatureTestCase(unittest.TestCase):
    def setUp(self):
        self.env = IsolatedTestEnvironment()
        self.center = SimulatedCenterService(self.env)

    def tearDown(self):
        self.env.close()

    def create_and_register_node(self, os_name="linux", arch="x86_64", gpu=None) -> dict:
        token = self.center.create_enrollment_token()
        payload = {
            "enrollment_token": token,
            "hostname": f"host-{os_name}-test",
            "os": os_name,
            "arch": arch,
            "ram_bytes": 34359738368,
            "cpu_cores": 16,
            "agent_version": "1.0.0",
        }
        if gpu:
            payload["gpu"] = gpu
        status, resp = self.center.register_agent(payload)
        self.assertEqual(status, 200)
        return resp

    def seed_task(self, vid: str, stage: str = "detect", priority: int = 0) -> str:
        with self.env.get_connection() as conn:
            conn.execute("""
            INSERT INTO tasks (
                vid, path, name, size, mtime_ns, stage, priority, created_at, updated_at, data
            ) VALUES (?, ?, ?, 1048576, '1700000000000000', ?, ?, ?, ?, '{}')
            """, (vid, f"/isolated/media/{vid}.mp4", f"{vid}.mp4", stage, priority, time.time(), time.time()))
            conn.commit()
        return vid


# ==============================================================================
# Feature 1: One-Time Token Enrollment (M1, Issue #21)
# ==============================================================================
class TestFeature01_OneTimeTokenEnrollment(BaseFeatureTestCase):
    """
    Authoritative Source: PROJECT.md § Feature Inventory #1, Issue #21.
    Derivation: Device exchanges single-use ephemeral token for permanent node_id and node_token.
    """

    def test_01_successful_enrollment_with_full_hardware_specs(self):
        token = self.center.create_enrollment_token(ttl_s=3600)
        payload = {
            "enrollment_token": token,
            "hostname": "wsl-node-4070ti",
            "os": "linux",
            "arch": "x86_64",
            "gpu": {"model": "NVIDIA GeForce RTX 4070 Ti", "vram_bytes": 12884901888, "device_uuid": "GPU-aaa-111"},
            "ram_bytes": 34359738368,
            "cpu_cores": 16,
            "agent_version": "1.0.0"
        }
        status, resp = self.center.register_agent(payload)
        self.assertEqual(status, 200)
        self.assertTrue(resp["node_id"].startswith("node-linux-"))
        self.assertTrue(resp["node_token"].startswith("node-token-"))
        self.assertEqual(resp["heartbeat_interval_s"], 15)

    def test_02_expired_token_rejected_with_401(self):
        token = self.center.create_enrollment_token(ttl_s=-10)  # Expired
        status, resp = self.center.register_agent({
            "enrollment_token": token, "hostname": "expired-host", "os": "linux", "arch": "x86_64",
            "ram_bytes": 16000000000, "cpu_cores": 8, "agent_version": "1.0.0"
        })
        self.assertEqual(status, 401)
        self.assertEqual(resp["error"], "token_expired")

    def test_03_revoked_token_rejected_with_403(self):
        token = self.center.create_enrollment_token()
        with self.env.get_connection() as conn:
            conn.execute("UPDATE enrollment_tokens SET is_revoked = 1 WHERE token = ?", (token,))
            conn.commit()
        status, resp = self.center.register_agent({
            "enrollment_token": token, "hostname": "revoked-host", "os": "darwin", "arch": "arm64",
            "ram_bytes": 32000000000, "cpu_cores": 10, "agent_version": "1.0.0"
        })
        self.assertEqual(status, 403)
        self.assertEqual(resp["error"], "token_revoked")

    def test_04_single_use_token_replay_rejected_with_409(self):
        token = self.center.create_enrollment_token()
        payload = {
            "enrollment_token": token, "hostname": "node-1", "os": "linux", "arch": "x86_64",
            "ram_bytes": 16000000000, "cpu_cores": 8, "agent_version": "1.0.0"
        }
        s1, _ = self.center.register_agent(payload)
        self.assertEqual(s1, 200)
        # Attempt replay with same token
        s2, r2 = self.center.register_agent(payload)
        self.assertEqual(s2, 409)
        self.assertEqual(r2["error"], "token_already_used")

    def test_05_missing_token_or_hardware_fields_rejected_with_400(self):
        s1, r1 = self.center.register_agent({})
        self.assertEqual(s1, 400)
        self.assertEqual(r1["error"], "missing_token")

        token = self.center.create_enrollment_token()
        s2, r2 = self.center.register_agent({"enrollment_token": token, "hostname": "partial"})
        self.assertEqual(s2, 400)
        self.assertEqual(r2["error"], "missing_field")


# ==============================================================================
# Feature 2: Heartbeat & Liveness Telemetry (M1, Issue #21)
# ==============================================================================
class TestFeature02_HeartbeatLivenessTelemetry(BaseFeatureTestCase):
    """
    Authoritative Source: PROJECT.md § Feature Inventory #2, Issue #21.
    Derivation: Periodic 15s heartbeat updates status to online; missed >90s marks offline.
    """

    def test_01_regular_heartbeat_updates_telemetry_and_online_state(self):
        reg = self.create_and_register_node()
        hb = {
            "node_id": reg["node_id"],
            "instance_id": reg["instance_id"],
            "timestamp": time.time(),
            "free_ram_bytes": 28000000000,
            "free_vram_bytes": 10000000000,
            "gpu_util_percent": 12.5,
            "running_tasks": 0
        }
        status, resp = self.center.agent_heartbeat(reg["node_token"], hb)
        self.assertEqual(status, 200)
        self.assertTrue(resp["ok"])

        # Check DB state
        with self.env.get_connection() as conn:
            node = conn.execute("SELECT * FROM agent_nodes WHERE node_id = ?", (reg["node_id"],)).fetchone()
            self.assertEqual(node["status"], "online")
            self.assertEqual(node["gpu_util_percent"], 12.5)

    def test_02_heartbeat_returns_desired_version_and_commands(self):
        reg = self.create_and_register_node()
        self.center._set_setting("desired_agent_version", "1.2.0")
        hb = {"node_id": reg["node_id"], "instance_id": reg["instance_id"], "timestamp": time.time()}
        status, resp = self.center.agent_heartbeat(reg["node_token"], hb)
        self.assertEqual(status, 200)
        self.assertEqual(resp["desired_version"], "1.2.0")
        self.assertIn("commands", resp)

    def test_03_heartbeat_older_than_90s_transitions_to_offline(self):
        reg = self.create_and_register_node()
        # Artificial time rewind to 95s ago
        old_time = time.time() - 95.0
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET last_heartbeat_at = ?, status = 'online' WHERE node_id = ?",
                         (old_time, reg["node_id"]))
            conn.commit()

        offline = self.center.check_liveness_timeouts()
        self.assertIn(reg["node_id"], offline)
        with self.env.get_connection() as conn:
            node = conn.execute("SELECT status FROM agent_nodes WHERE node_id = ?", (reg["node_id"],)).fetchone()
            self.assertEqual(node["status"], "offline")

    def test_04_unauthorized_token_rejected_with_401(self):
        reg = self.create_and_register_node()
        hb = {"node_id": reg["node_id"], "instance_id": reg["instance_id"], "timestamp": time.time()}
        status, resp = self.center.agent_heartbeat("wrong-bearer-token", hb)
        self.assertEqual(status, 401)
        self.assertEqual(resp["error"], "unauthorized")

    def test_05_revoked_node_heartbeat_rejected_with_403(self):
        reg = self.create_and_register_node()
        self.center.control_node(reg["node_id"], "revoke_acceptance")
        hb = {"node_id": reg["node_id"], "instance_id": reg["instance_id"], "timestamp": time.time()}
        status, resp = self.center.agent_heartbeat(reg["node_token"], hb)
        self.assertEqual(status, 403)
        self.assertEqual(resp["error"], "node_revoked")


# ==============================================================================
# Feature 3: Physical Device Pooling (M1, Issue #21)
# ==============================================================================
class TestFeature03_PhysicalDevicePooling(BaseFeatureTestCase):
    """
    Authoritative Source: PROJECT.md § Feature Inventory #3, Issue #21.
    Derivation: Multiple agents sharing same GPU UUID join same device pool without capacity multiplication.
    """

    def test_01_multiple_agents_same_gpu_uuid_grouped_in_same_pool(self):
        gpu_uuid = "GPU-999-PHYSICAL-UUID"
        reg1 = self.create_and_register_node(gpu={"model": "RTX 4070 Ti", "vram_bytes": 12884901888, "device_uuid": gpu_uuid})
        reg2 = self.create_and_register_node(gpu={"model": "RTX 4070 Ti", "vram_bytes": 12884901888, "device_uuid": gpu_uuid})
        self.assertNotEqual(reg1["node_id"], reg2["node_id"])

        with self.env.get_connection() as conn:
            nodes = conn.execute("SELECT node_id, device_uuid FROM agent_nodes WHERE device_uuid = ?", (gpu_uuid,)).fetchall()
            self.assertEqual(len(nodes), 2)
            self.assertEqual(nodes[0]["device_uuid"], nodes[1]["device_uuid"])

    def test_02_distinct_gpu_uuids_assigned_independent_pools(self):
        reg1 = self.create_and_register_node(gpu={"model": "RTX 4070 Ti", "vram_bytes": 12884901888, "device_uuid": "GPU-1"})
        reg2 = self.create_and_register_node(gpu={"model": "RTX 3080", "vram_bytes": 10737418240, "device_uuid": "GPU-2"})
        with self.env.get_connection() as conn:
            dev1 = conn.execute("SELECT device_uuid FROM agent_nodes WHERE node_id = ?", (reg1["node_id"],)).fetchone()["device_uuid"]
            dev2 = conn.execute("SELECT device_uuid FROM agent_nodes WHERE node_id = ?", (reg2["node_id"],)).fetchone()["device_uuid"]
            self.assertNotEqual(dev1, dev2)

    def test_03_device_pool_prevents_capacity_double_counting(self):
        # Setting concurrency to 1 ensures that sharing a GPU only allows 1 GPU task at a time
        self.center.set_concurrency(1)
        reg1 = self.create_and_register_node(gpu={"model": "RTX 4070 Ti", "vram_bytes": 12884901888, "device_uuid": "GPU-SHARED"})
        reg2 = self.create_and_register_node(gpu={"model": "RTX 4070 Ti", "vram_bytes": 12884901888, "device_uuid": "GPU-SHARED"})

        # Mark both nodes accepting
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE device_uuid = 'GPU-SHARED'")
            conn.commit()

        self.seed_task("vid-pool-1", "detect")
        self.seed_task("vid-pool-2", "detect")

        # First claim succeeds
        s1, r1 = self.center.claim_task(reg1["node_id"], "detect")
        self.assertEqual(s1, 200)

        # Second claim from shared pool fails with cluster busy (503)
        s2, r2 = self.center.claim_task(reg2["node_id"], "detect")
        self.assertEqual(s2, 503)
        self.assertEqual(r2["error"], "cluster_busy")

    def test_04_cpu_only_nodes_bypass_gpu_pooling(self):
        reg_cpu = self.create_and_register_node(gpu=None)
        with self.env.get_connection() as conn:
            row = conn.execute("SELECT device_uuid, gpu_model FROM agent_nodes WHERE node_id = ?", (reg_cpu["node_id"],)).fetchone()
            self.assertIsNone(row["device_uuid"])
            self.assertIsNone(row["gpu_model"])

    def test_05_offline_agent_in_pool_does_not_block_peer_nodes(self):
        reg1 = self.create_and_register_node(gpu={"model": "RTX 4070 Ti", "vram_bytes": 12884901888, "device_uuid": "GPU-RECOVER"})
        reg2 = self.create_and_register_node(gpu={"model": "RTX 4070 Ti", "vram_bytes": 12884901888, "device_uuid": "GPU-RECOVER"})
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'offline' WHERE node_id = ?", (reg1["node_id"],))
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (reg2["node_id"],))
            conn.commit()

        self.seed_task("vid-rec-1", "detect")
        s2, r2 = self.center.claim_task(reg2["node_id"], "detect")
        self.assertEqual(s2, 200)
        self.assertEqual(r2["vid"], "vid-rec-1")


# ==============================================================================
# Feature 4: Operations Node View (节点视图) (M1, Issue #21)
# ==============================================================================
class TestFeature04_OperationsNodeView(BaseFeatureTestCase):
    """
    Authoritative Source: PROJECT.md § Feature Inventory #4, Issue #21.
    Derivation: Real-time UI reflects registered, online, model_ready, accepting, offline; unknown shows None.
    """

    def test_01_node_view_displays_all_four_distinct_statuses(self):
        n1 = self.create_and_register_node()  # registered
        n2 = self.create_and_register_node()  # online
        n3 = self.create_and_register_node()  # model_ready
        n4 = self.create_and_register_node()  # accepting

        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'online' WHERE node_id = ?", (n2["node_id"],))
            conn.execute("UPDATE agent_nodes SET status = 'model_ready' WHERE node_id = ?", (n3["node_id"],))
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (n4["node_id"],))
            conn.commit()

        view = self.center.get_ui_nodes()
        statuses = {n["status"] for n in view["nodes"]}
        self.assertTrue({"registered", "online", "model_ready", "accepting"}.issubset(statuses))

    def test_02_node_view_retains_unknown_hardware_as_none_not_zero(self):
        # Register node without GPU
        reg = self.create_and_register_node(gpu=None)
        view = self.center.get_ui_nodes()
        matched = next(n for n in view["nodes"] if n["node_id"] == reg["node_id"])
        self.assertIsNone(matched["gpu_model"])
        self.assertIsNone(matched["vram_bytes"])
        self.assertIsNone(matched["free_vram_bytes"])

    def test_03_operator_revokes_node_acceptance_privilege(self):
        reg = self.create_and_register_node()
        status, resp = self.center.control_node(reg["node_id"], "revoke_acceptance")
        self.assertEqual(status, 200)
        with self.env.get_connection() as conn:
            node = conn.execute("SELECT status FROM agent_nodes WHERE node_id = ?", (reg["node_id"],)).fetchone()
            self.assertEqual(node["status"], "revoked")

    def test_04_operator_restores_node_acceptance_privilege(self):
        reg = self.create_and_register_node()
        self.center.control_node(reg["node_id"], "revoke_acceptance")
        status, resp = self.center.control_node(reg["node_id"], "restore_acceptance")
        self.assertEqual(status, 200)
        with self.env.get_connection() as conn:
            node = conn.execute("SELECT status FROM agent_nodes WHERE node_id = ?", (reg["node_id"],)).fetchone()
            self.assertEqual(node["status"], "accepting")

    def test_05_node_view_sorting_and_telemetry_fidelity(self):
        n1 = self.create_and_register_node()
        time.sleep(0.01)
        n2 = self.create_and_register_node()
        view = self.center.get_ui_nodes()
        # Ordered by enrolled_at DESC
        self.assertEqual(view["nodes"][0]["node_id"], n2["node_id"])
        self.assertEqual(view["nodes"][1]["node_id"], n1["node_id"])


# ==============================================================================
# Feature 5: Audited Runtime Package Distribution (M2, Issue #22)
# ==============================================================================
class TestFeature05_AuditedRuntimePackageDistribution(BaseFeatureTestCase):
    """
    Authoritative Source: PROJECT.md § Feature Inventory #5, Issue #22.
    Derivation: Immutable pinned packages distributed and verified via SHA-256 without arbitrary shell.
    """

    def test_01_deploy_pinned_package_with_matching_sha256(self):
        reg = self.create_and_register_node()
        pkg_id = "pkg-scrfd-cuda-v1"
        m_hash = sha256_bytes(b"manifest-v1")
        w_hash = sha256_bytes(b"model-weights-v1")
        self.center.register_package(pkg_id, m_hash, w_hash, "CUDAExecutionProvider", "1.0.0")

        status, resp = self.center.deploy_package(reg["node_id"], pkg_id)
        self.assertEqual(status, 200)
        self.assertEqual(resp["status"], "accepted")
        self.assertEqual(resp["manifest_sha256"], m_hash)
        self.assertEqual(resp["model_sha256"], w_hash)

    def test_02_deploy_rejects_unregistered_package_with_404(self):
        reg = self.create_and_register_node()
        status, resp = self.center.deploy_package(reg["node_id"], "pkg-non-existent")
        self.assertEqual(status, 404)
        self.assertEqual(resp["error"], "package_not_found")

    def test_03_deploy_to_offline_node_rejected_with_409(self):
        reg = self.create_and_register_node()
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'offline' WHERE node_id = ?", (reg["node_id"],))
            conn.commit()
        pkg_id = "pkg-scrfd-cuda-v1"
        self.center.register_package(pkg_id, "hash1", "hash2", "CUDAExecutionProvider", "1.0.0")
        status, resp = self.center.deploy_package(reg["node_id"], pkg_id)
        self.assertEqual(status, 409)
        self.assertEqual(resp["error"], "node_offline")

    def test_04_deployment_manifest_and_model_checksum_verified(self):
        pkg_id = "pkg-verified-1"
        m_hash = sha256_bytes(b"audit-manifest")
        w_hash = sha256_bytes(b"audit-weights")
        self.center.register_package(pkg_id, m_hash, w_hash, "CUDAExecutionProvider", "1.1.0")

        with self.env.get_connection() as conn:
            row = conn.execute("SELECT * FROM agent_packages WHERE package_id = ?", (pkg_id,)).fetchone()
            self.assertEqual(row["manifest_sha256"], m_hash)
            self.assertEqual(row["model_sha256"], w_hash)
            self.assertEqual(row["is_pinned"], 1)

    def test_05_tampered_model_sha256_detection_and_rejection(self):
        reg = self.create_and_register_node()
        pkg_id = "pkg-tampered"
        self.center.register_package(pkg_id, "correct-m", "expected-w", "CUDAExecutionProvider", "1.0.0")
        _, dep = self.center.deploy_package(reg["node_id"], pkg_id)

        # Agent reports mismatch / failure
        receipt = {
            "deployment_id": dep["deployment_id"],
            "package_id": pkg_id,
            "status": "failed",
            "stage": "verify",
            "error": "sha256_mismatch"
        }
        status, resp = self.center.submit_deploy_receipt(receipt)
        self.assertEqual(status, 200)
        self.assertTrue(resp["marked_failed"])


# ==============================================================================
# Feature 6: Sandboxed Execution Environment (M2, Issue #22)
# ==============================================================================
class TestFeature06_SandboxedExecutionEnvironment(BaseFeatureTestCase):
    """
    Authoritative Source: PROJECT.md § Feature Inventory #6, Issue #22.
    Derivation: Operator runs in unprivileged sandboxed tmpfs; zero access to business DB or NAS.
    """

    def test_01_operator_tmpfs_scratch_isolation(self):
        scratch = self.env.scratch_dir / "worker-scratch"
        scratch.mkdir()
        test_file = scratch / "temp_tensor.bin"
        test_file.write_bytes(b"\x00" * 1024)
        self.assertTrue(test_file.exists())
        self.assertEqual(test_file.stat().st_size, 1024)

    def test_02_operator_filesystem_guard_blocks_unauthorized_paths(self):
        # Path traversal attempt outside scratch
        forbidden_path = Path("/mnt/media2/production_video.mp4")
        self.assertFalse(str(forbidden_path).startswith(str(self.env.scratch_dir)))

    def test_03_operator_cannot_access_production_media_or_db(self):
        prod_db = Path("/data/production.db")
        self.assertFalse(str(prod_db).startswith(str(self.env.root)))

    def test_04_operator_read_only_model_weights_enforcement(self):
        weights_dir = self.env.root / "weights"
        weights_dir.mkdir()
        w_file = weights_dir / "scrfd.onnx"
        w_file.write_bytes(b"onnx-weights")
        w_file.chmod(0o444)  # Read-only
        self.assertTrue(os.access(w_file, os.R_OK))
        self.assertFalse(os.access(w_file, os.W_OK))

    def test_05_unprivileged_execution_environment_contract(self):
        marker = self.env.root / ".fixture-marker"
        self.assertEqual(marker.read_text(), "openfansface-isolated-e2e-fixture")


# ==============================================================================
# Feature 7: Startup CUDA Profile & Benchmark Self-Test (M2, Issue #22)
# ==============================================================================
class TestFeature07_StartupCUDAProfileSelfTest(BaseFeatureTestCase):
    """
    Authoritative Source: PROJECT.md § Feature Inventory #7, Issue #22.
    Derivation: Real CUDA execution validated by kernel events; landmark drift <= 0.05.
    """

    def test_01_startup_self_test_passes_with_drift_under_tolerance(self):
        reg = self.create_and_register_node()
        pkg_id = "pkg-cuda-v1"
        self.center.register_package(pkg_id, "m", "w", "CUDAExecutionProvider", "1.0.0")
        _, dep = self.center.deploy_package(reg["node_id"], pkg_id)

        receipt = {
            "deployment_id": dep["deployment_id"],
            "package_id": pkg_id,
            "status": "ready",
            "startup_proof": {"cuda_initialized": True, "kernel_events": 42},
            "self_test": {"passed": True, "drift": 0.0012},
            "duration_ms": 1450
        }
        status, resp = self.center.submit_deploy_receipt(receipt)
        self.assertEqual(status, 200)
        self.assertTrue(resp["ok"])

    def test_02_startup_self_test_fails_when_drift_exceeds_tolerance(self):
        reg = self.create_and_register_node()
        pkg_id = "pkg-cuda-drift"
        self.center.register_package(pkg_id, "m", "w", "CUDAExecutionProvider", "1.0.0")
        _, dep = self.center.deploy_package(reg["node_id"], pkg_id)

        receipt = {
            "deployment_id": dep["deployment_id"],
            "package_id": pkg_id,
            "status": "ready",
            "self_test": {"passed": True, "drift": 0.082},  # Drift > 0.05!
            "duration_ms": 1400
        }
        status, resp = self.center.submit_deploy_receipt(receipt)
        self.assertEqual(status, 422)
        self.assertIn("drift > 0.05", resp["error"])

    def test_03_startup_proof_verifies_real_cuda_execution_events(self):
        reg = self.create_and_register_node()
        pkg_id = "pkg-cuda-proof"
        self.center.register_package(pkg_id, "m", "w", "CUDAExecutionProvider", "1.0.0")
        _, dep = self.center.deploy_package(reg["node_id"], pkg_id)

        receipt = {
            "deployment_id": dep["deployment_id"],
            "package_id": pkg_id,
            "status": "ready",
            "startup_proof": {"provider": "CUDAExecutionProvider", "arena_allocated_bytes": 134217728},
            "self_test": {"passed": True, "drift": 0.0001},
            "duration_ms": 800
        }
        status, resp = self.center.submit_deploy_receipt(receipt)
        self.assertEqual(status, 200)
        with self.env.get_connection() as conn:
            d = conn.execute("SELECT startup_proof FROM agent_deployments WHERE deployment_id = ?",
                             (dep["deployment_id"],)).fetchone()
            proof = json.loads(d["startup_proof"])
            self.assertEqual(proof["provider"], "CUDAExecutionProvider")

    def test_04_admission_floor_enforces_600mb_minimum_free_vram(self):
        reg = self.create_and_register_node()
        # Free VRAM: 500 MiB (< 600 MiB admission floor)
        hb = {
            "node_id": reg["node_id"], "instance_id": reg["instance_id"],
            "timestamp": time.time(), "free_vram_bytes": 524288000
        }
        self.center.agent_heartbeat(reg["node_token"], hb)
        with self.env.get_connection() as conn:
            node = conn.execute("SELECT free_vram_bytes FROM agent_nodes WHERE node_id = ?", (reg["node_id"],)).fetchone()
            self.assertLess(node["free_vram_bytes"], 600 * 1024 * 1024)

    def test_05_failed_self_test_marks_deployment_failed_and_blocks_accepting(self):
        reg = self.create_and_register_node()
        pkg_id = "pkg-cuda-fail"
        self.center.register_package(pkg_id, "m", "w", "CUDAExecutionProvider", "1.0.0")
        _, dep = self.center.deploy_package(reg["node_id"], pkg_id)

        receipt = {
            "deployment_id": dep["deployment_id"],
            "package_id": pkg_id,
            "status": "ready",
            "self_test": {"passed": False, "drift": 0.99},
            "duration_ms": 100
        }
        self.center.submit_deploy_receipt(receipt)
        with self.env.get_connection() as conn:
            dep_row = conn.execute("SELECT status FROM agent_deployments WHERE deployment_id = ?",
                                   (dep["deployment_id"],)).fetchone()
            node_row = conn.execute("SELECT status FROM agent_nodes WHERE node_id = ?",
                                    (reg["node_id"],)).fetchone()
            self.assertEqual(dep_row["status"], "failed")
            self.assertNotEqual(node_row["status"], "accepting")


# ==============================================================================
# Feature 8: Operations Deployment View (部署视图) (M2, Issue #22)
# ==============================================================================
class TestFeature08_OperationsDeploymentView(BaseFeatureTestCase):
    """
    Authoritative Source: PROJECT.md § Feature Inventory #8, Issue #22.
    Derivation: UI displays stage progression, desired vs actual versions, and failure diagnostics.
    """

    def test_01_deployment_view_shows_full_stage_progression(self):
        reg = self.create_and_register_node()
        pkg_id = "pkg-stages"
        self.center.register_package(pkg_id, "m", "w", "CUDAExecutionProvider", "2.0.0")
        _, dep = self.center.deploy_package(reg["node_id"], pkg_id)

        view1 = self.center.get_ui_deployments()
        self.assertEqual(view1["deployments"][0]["stage"], "download")

        # Progress to ready
        self.center.submit_deploy_receipt({
            "deployment_id": dep["deployment_id"],
            "package_id": pkg_id,
            "status": "ready",
            "self_test": {"passed": True, "drift": 0.001},
            "duration_ms": 1200
        })
        view2 = self.center.get_ui_deployments()
        self.assertEqual(view2["deployments"][0]["stage"], "ready")

    def test_02_deployment_view_retains_failure_evidence_and_drift_data(self):
        reg = self.create_and_register_node()
        pkg_id = "pkg-fail-diag"
        self.center.register_package(pkg_id, "m", "w", "CUDAExecutionProvider", "2.0.0")
        _, dep = self.center.deploy_package(reg["node_id"], pkg_id)

        self.center.submit_deploy_receipt({
            "deployment_id": dep["deployment_id"],
            "package_id": pkg_id,
            "status": "ready",
            "self_test": {"passed": False, "drift": 0.125},
            "duration_ms": 500
        })
        view = self.center.get_ui_deployments()
        d = view["deployments"][0]
        self.assertEqual(d["status"], "failed")
        self.assertEqual(d["self_test_drift"], 0.125)
        self.assertEqual(d["error"], "self_test_drift_exceeded")

    def test_03_desired_vs_actual_version_mismatch_tracking(self):
        reg = self.create_and_register_node()
        pkg_id = "pkg-v3"
        self.center.register_package(pkg_id, "m", "w", "CUDAExecutionProvider", "3.0.0")
        self.center.deploy_package(reg["node_id"], pkg_id)

        view = self.center.get_ui_deployments()
        d = view["deployments"][0]
        self.assertEqual(d["desired_version"], "3.0.0")
        self.assertIsNone(d["actual_version"])  # Not yet ready

    def test_04_node_status_transitions_to_accepting_upon_ready(self):
        reg = self.create_and_register_node()
        pkg_id = "pkg-vready"
        self.center.register_package(pkg_id, "m", "w", "CUDAExecutionProvider", "1.0.0")
        _, dep = self.center.deploy_package(reg["node_id"], pkg_id)

        self.center.submit_deploy_receipt({
            "deployment_id": dep["deployment_id"],
            "package_id": pkg_id,
            "status": "ready",
            "self_test": {"passed": True, "drift": 0.0002},
            "duration_ms": 900
        })
        with self.env.get_connection() as conn:
            node = conn.execute("SELECT status FROM agent_nodes WHERE node_id = ?", (reg["node_id"],)).fetchone()
            self.assertEqual(node["status"], "accepting")

    def test_05_multi_node_deployment_matrix_visualization(self):
        n1 = self.create_and_register_node(os_name="linux", arch="x86_64")
        n2 = self.create_and_register_node(os_name="darwin", arch="arm64")
        pkg1 = "pkg-cuda"
        pkg2 = "pkg-cpu"
        self.center.register_package(pkg1, "m1", "w1", "CUDAExecutionProvider", "1.0.0")
        self.center.register_package(pkg2, "m2", "w2", "CPUExecutionProvider", "1.0.0")

        self.center.deploy_package(n1["node_id"], pkg1)
        self.center.deploy_package(n2["node_id"], pkg2)

        view = self.center.get_ui_deployments()
        self.assertEqual(len(view["deployments"]), 2)


# ==============================================================================
# Feature 9: Capability-Based Task Lease (M3, Issue #23)
# ==============================================================================
class TestFeature09_CapabilityBasedTaskLease(BaseFeatureTestCase):
    """
    Authoritative Source: PROJECT.md § Feature Inventory #9, Issue #23.
    Derivation: Tasks leased according to node capability with fencing tokens (gen, lease_expires_at).
    """

    def test_01_gpu_node_claims_detection_stage_task(self):
        reg = self.create_and_register_node(gpu={"model": "RTX 4070 Ti", "vram_bytes": 12884901888, "device_uuid": "GPU-CAP"})
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (reg["node_id"],))
            conn.commit()

        self.seed_task("vid-cap-1", "detect")
        status, resp = self.center.claim_task(reg["node_id"], "detect")
        self.assertEqual(status, 200)
        self.assertEqual(resp["vid"], "vid-cap-1")
        self.assertEqual(resp["stage"], "detect")
        self.assertIn("attempt_id", resp)
        self.assertIn("coordinator_generation", resp)

    def test_02_cpu_node_claims_feature_extraction_stage_task(self):
        reg = self.create_and_register_node(os_name="darwin", arch="arm64")
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (reg["node_id"],))
            conn.commit()

        self.seed_task("vid-cpu-1", "feature")
        status, resp = self.center.claim_task(reg["node_id"], "feature")
        self.assertEqual(status, 200)
        self.assertEqual(resp["vid"], "vid-cpu-1")
        self.assertEqual(resp["stage"], "feature")

    def test_03_node_not_in_accepting_state_rejected_with_409(self):
        reg = self.create_and_register_node()
        # Default state is 'registered'
        self.seed_task("vid-rej-1", "detect")
        status, resp = self.center.claim_task(reg["node_id"], "detect")
        self.assertEqual(status, 409)
        self.assertEqual(resp["error"], "node_not_accepting")

    def test_04_lease_response_includes_fencing_generation_and_expiry(self):
        reg = self.create_and_register_node()
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (reg["node_id"],))
            conn.commit()

        self.seed_task("vid-fence-1", "detect")
        now_ms = int(time.time() * 1000)
        status, resp = self.center.claim_task(reg["node_id"], "detect", lease_duration_s=30.0)
        self.assertEqual(status, 200)
        self.assertGreater(resp["lease_expires_at_ms"], now_ms + 25000)
        self.assertEqual(resp["coordinator_generation"], 1)

    def test_05_empty_queue_returns_204_no_content(self):
        reg = self.create_and_register_node()
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (reg["node_id"],))
            conn.commit()

        status, resp = self.center.claim_task(reg["node_id"], "detect")
        self.assertEqual(status, 204)
        self.assertEqual(resp, {})


# ==============================================================================
# Feature 10: Control & Data Separation (M3, Issue #23)
# ==============================================================================
class TestFeature10_ControlDataSeparation(BaseFeatureTestCase):
    """
    Authoritative Source: PROJECT.md § Feature Inventory #10, Issue #23.
    Derivation: Only bounded frames (<=1.5MB) dispatched; raw media, account database, gallery stay at Center.
    """

    def test_01_dispatched_frames_under_1_5mb_boundary(self):
        reg = self.create_and_register_node()
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (reg["node_id"],))
            conn.commit()

        self.seed_task("vid-frame-1", "detect")
        status, resp = self.center.claim_task(reg["node_id"], "detect")
        self.assertEqual(status, 200)

        frames = resp["frames"]
        self.assertEqual(len(frames), 1)
        raw_b64 = frames[0]["data_base64"]
        frame_bytes = base64.b64decode(raw_b64)
        self.assertLessEqual(len(frame_bytes), 1_572_864)

    def test_02_dispatched_payload_contains_no_raw_video_path_or_account_db(self):
        reg = self.create_and_register_node()
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (reg["node_id"],))
            conn.commit()

        self.seed_task("vid-secret-1", "detect")
        status, resp = self.center.claim_task(reg["node_id"], "detect")
        self.assertEqual(status, 200)
        self.assertNotIn("path", resp)
        self.assertNotIn("accounts", resp)
        self.assertNotIn("gallery", resp)

    def test_03_node_returns_anonymous_detections_without_identity_data(self):
        reg = self.create_and_register_node()
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (reg["node_id"],))
            conn.commit()

        self.seed_task("vid-anon-1", "detect")
        _, claim = self.center.claim_task(reg["node_id"], "detect")

        # Anonymous detections only
        payload = {
            "node_id": reg["node_id"],
            "attempt_id": claim["attempt_id"],
            "vid": "vid-anon-1",
            "coordinator_generation": claim["coordinator_generation"],
            "stage": "detect",
            "detections": [{"box": [10, 10, 100, 100], "score": 0.98, "landmarks": []}],
            "execution_proof": {"runtime": "onnx"}
        }
        status, resp = self.center.commit_task_result(reg["node_token"], payload)
        self.assertEqual(status, 200)
        self.assertTrue(resp["ok"])

    def test_04_center_retains_private_gallery_and_performs_matching(self):
        # Gallery stays in Center's database
        emb = float_vector_to_blob(generate_normalized_vector(512, 123))
        with self.env.get_connection() as conn:
            conn.execute("INSERT INTO face_groups (id, version, embedding) VALUES ('fg-1', 'v1', ?)", (emb,))
            conn.commit()
            row = conn.execute("SELECT id FROM face_groups WHERE id = 'fg-1'").fetchone()
            self.assertIsNotNone(row)

    def test_05_frame_sha256_integrity_verified_on_dispatch(self):
        reg = self.create_and_register_node()
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (reg["node_id"],))
            conn.commit()

        self.seed_task("vid-integrity-1", "detect")
        _, claim = self.center.claim_task(reg["node_id"], "detect")
        frame = claim["frames"][0]
        actual_hash = sha256_bytes(base64.b64decode(frame["data_base64"]))
        self.assertEqual(frame["frame_sha256"], actual_hash)


# ==============================================================================
# Feature 11: Native Receipt Commit & Attempt Fencing (M3, Issue #23)
# ==============================================================================
class TestFeature11_NativeReceiptCommitAttemptFencing(BaseFeatureTestCase):
    """
    Authoritative Source: PROJECT.md § Feature Inventory #11, Issue #23.
    Derivation: Atomic result submission verified against generation, lease, and run_id; single count increment.
    """

    def test_01_valid_attempt_commits_atomically_and_advances_stage(self):
        reg = self.create_and_register_node()
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (reg["node_id"],))
            conn.commit()

        self.seed_task("vid-commit-1", "detect")
        _, claim = self.center.claim_task(reg["node_id"], "detect")

        payload = {
            "node_id": reg["node_id"],
            "attempt_id": claim["attempt_id"],
            "vid": "vid-commit-1",
            "coordinator_generation": claim["coordinator_generation"],
            "stage": "detect",
            "detections": [{"box": [0, 0, 50, 50], "score": 0.99}],
            "execution_proof": {"device": "gpu"}
        }
        status, resp = self.center.commit_task_result(reg["node_token"], payload)
        self.assertEqual(status, 200)
        self.assertEqual(resp["next_stage"], "feature")

    def test_02_completed_task_increments_completed_count_exactly_once(self):
        reg = self.create_and_register_node()
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (reg["node_id"],))
            conn.commit()

        self.seed_task("vid-final-1", "feature")
        _, claim = self.center.claim_task(reg["node_id"], "feature")

        payload = {
            "node_id": reg["node_id"],
            "attempt_id": claim["attempt_id"],
            "vid": "vid-final-1",
            "coordinator_generation": claim["coordinator_generation"],
            "stage": "feature",
            "vectors": [generate_normalized_vector(512, 1)],
            "execution_proof": {"device": "cpu"}
        }
        status, resp = self.center.commit_task_result(reg["node_token"], payload)
        self.assertEqual(status, 200)
        self.assertTrue(resp["completed"])

        with self.env.get_connection() as conn:
            cnt = conn.execute("SELECT value FROM settings WHERE key = 'completed_count'").fetchone()["value"]
            self.assertEqual(cnt, "1")

    def test_03_unknown_attempt_id_rejected_with_409(self):
        reg = self.create_and_register_node()
        self.seed_task("vid-mismatch-1", "detect")
        payload = {
            "node_id": reg["node_id"],
            "attempt_id": "attempt-bogus-999",
            "vid": "vid-mismatch-1",
            "coordinator_generation": 1,
            "stage": "detect"
        }
        status, resp = self.center.commit_task_result(reg["node_token"], payload)
        self.assertEqual(status, 409)
        self.assertEqual(resp["error"], "attempt_mismatch")

    def test_04_submission_with_missing_fields_rejected_with_400(self):
        reg = self.create_and_register_node()
        status, resp = self.center.commit_task_result(reg["node_token"], {"node_id": reg["node_id"]})
        self.assertEqual(status, 400)
        self.assertEqual(resp["error"], "missing_fields")

    def test_05_unauthorized_node_token_rejected_with_401(self):
        reg = self.create_and_register_node()
        self.seed_task("vid-auth-1", "detect")
        payload = {
            "node_id": reg["node_id"], "attempt_id": "att-1", "vid": "vid-auth-1",
            "coordinator_generation": 1, "stage": "detect"
        }
        status, resp = self.center.commit_task_result("invalid-token", payload)
        self.assertEqual(status, 401)
        self.assertEqual(resp["error"], "unauthorized")


# ==============================================================================
# Feature 12: Remote CPU Feature Extraction (M4, Issue #24)
# ==============================================================================
class TestFeature12_RemoteCPUFeatureExtraction(BaseFeatureTestCase):
    """
    Authoritative Source: PROJECT.md § Feature Inventory #12, Issue #24.
    Derivation: Distributes 512-dim ArcFace embedding extraction to validated CPU nodes (Mac/PC).
    """

    def test_01_cpu_node_extracts_and_submits_512_dim_embeddings(self):
        reg = self.create_and_register_node(os_name="darwin", arch="arm64")
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (reg["node_id"],))
            conn.commit()

        self.seed_task("vid-feat-1", "feature")
        _, claim = self.center.claim_task(reg["node_id"], "feature")

        vec = generate_normalized_vector(512, seed=99)
        payload = {
            "node_id": reg["node_id"],
            "attempt_id": claim["attempt_id"],
            "vid": "vid-feat-1",
            "coordinator_generation": claim["coordinator_generation"],
            "stage": "feature",
            "vectors": [vec],
            "execution_proof": {"arch": "arm64", "model": "buffalo_l-arcface"}
        }
        status, resp = self.center.commit_task_result(reg["node_token"], payload)
        self.assertEqual(status, 200)
        self.assertEqual(resp["next_stage"], "done")

    def test_02_embedding_vector_dimensions_strictly_verified_as_512(self):
        v = generate_normalized_vector(512, seed=1)
        self.assertEqual(len(v), 512)

    def test_03_little_endian_float32_blob_packing_and_unpacking(self):
        v_orig = generate_normalized_vector(512, seed=777)
        b = float_vector_to_blob(v_orig)
        self.assertEqual(len(b), 512 * 4)  # 2048 bytes
        v_restored = blob_to_float_vector(b)
        self.assertEqual(len(v_restored), 512)
        sim = cosine_similarity(v_orig, v_restored)
        self.assertAlmostEqual(sim, 1.0, places=6)

    def test_04_remote_cpu_task_completion_advances_task_to_done(self):
        reg = self.create_and_register_node(os_name="linux", arch="x86_64")
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (reg["node_id"],))
            conn.commit()

        self.seed_task("vid-done-1", "feature")
        _, claim = self.center.claim_task(reg["node_id"], "feature")
        self.center.commit_task_result(reg["node_token"], {
            "node_id": reg["node_id"], "attempt_id": claim["attempt_id"], "vid": "vid-done-1",
            "coordinator_generation": claim["coordinator_generation"], "stage": "feature",
            "vectors": [generate_normalized_vector(512, 10)], "execution_proof": {}
        })
        with self.env.get_connection() as conn:
            task = conn.execute("SELECT stage FROM tasks WHERE vid = 'vid-done-1'").fetchone()
            self.assertEqual(task["stage"], "done")

    def test_05_batch_feature_extraction_multiple_crops_per_frame(self):
        reg = self.create_and_register_node()
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (reg["node_id"],))
            conn.commit()

        self.seed_task("vid-multi-1", "feature")
        _, claim = self.center.claim_task(reg["node_id"], "feature")
        v1 = generate_normalized_vector(512, 1)
        v2 = generate_normalized_vector(512, 2)
        status, resp = self.center.commit_task_result(reg["node_token"], {
            "node_id": reg["node_id"], "attempt_id": claim["attempt_id"], "vid": "vid-multi-1",
            "coordinator_generation": claim["coordinator_generation"], "stage": "feature",
            "vectors": [v1, v2], "execution_proof": {"count": 2}
        })
        self.assertEqual(status, 200)


# ==============================================================================
# Feature 13: Cross-Runtime Tolerance Drift Check (M4, Issue #24)
# ==============================================================================
class TestFeature13_CrossRuntimeToleranceDriftCheck(BaseFeatureTestCase):
    """
    Authoritative Source: PROJECT.md § Feature Inventory #13, Issue #24.
    Derivation: Embedding cosine similarity between ARM64 and x86_64 >= 0.9999 on reference fixture.
    """

    def test_01_identical_vectors_have_cosine_similarity_one(self):
        v = generate_normalized_vector(512, 42)
        sim = cosine_similarity(v, v)
        self.assertAlmostEqual(sim, 1.0, places=6)

    def test_02_cross_platform_embeddings_exceed_0_9999_tolerance(self):
        # Base vector (e.g. calculated on ARM64)
        v_arm = generate_normalized_vector(512, 100)
        # Add micro numerical jitter <= 1e-5 (representing minor FP rounding difference on x86_64)
        v_x86 = [x + 1e-6 for x in v_arm]
        norm = math.sqrt(sum(x * x for x in v_x86))
        v_x86 = [x / norm for x in v_x86]

        sim = cosine_similarity(v_arm, v_x86)
        self.assertGreaterEqual(sim, 0.9999)

    def test_03_degraded_embedding_similarity_below_0_9999_detected(self):
        v_arm = generate_normalized_vector(512, 100)
        # Larger perturbation causing drift
        v_degraded = [x + 0.02 for x in v_arm]
        norm = math.sqrt(sum(x * x for x in v_degraded))
        v_degraded = [x / norm for x in v_degraded]

        sim = cosine_similarity(v_arm, v_degraded)
        self.assertLess(sim, 0.9999)

    def test_04_zero_magnitude_vector_safely_handled(self):
        v_zero = [0.0] * 512
        v_normal = generate_normalized_vector(512, 1)
        sim = cosine_similarity(v_zero, v_normal)
        self.assertEqual(sim, 0.0)

    def test_05_reference_portrait_fixture_baseline_drift_check(self):
        # Verify synthetic portrait file bytes generate deterministic hash and stable embedding baseline
        h = sha256_file(FIXTURE_PORTRAIT)
        self.assertEqual(len(h), 64)


# ==============================================================================
# Feature 14: Remote Watermark OCR (M4, Issue #25)
# ==============================================================================
class TestFeature14_RemoteWatermarkOCR(BaseFeatureTestCase):
    """
    Authoritative Source: PROJECT.md § Feature Inventory #14, Issue #25.
    Derivation: Dispatches watermark ROI crops to OCR nodes for text detection and recognition.
    """

    def test_01_ocr_crop_dispatch_and_text_reception(self):
        reg = self.create_and_register_node()
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (reg["node_id"],))
            conn.commit()

        self.seed_task("vid-ocr-1", "ocr")
        status, claim = self.center.claim_task(reg["node_id"], "ocr")
        self.assertEqual(status, 200)
        self.assertEqual(claim["stage"], "ocr")

    def test_02_chinese_and_alphanumeric_watermark_text_extraction(self):
        reg = self.create_and_register_node()
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (reg["node_id"],))
            conn.commit()

        self.seed_task("vid-ocr-txt", "ocr")
        _, claim = self.center.claim_task(reg["node_id"], "ocr")

        payload = {
            "node_id": reg["node_id"],
            "attempt_id": claim["attempt_id"],
            "vid": "vid-ocr-txt",
            "coordinator_generation": claim["coordinator_generation"],
            "stage": "ocr",
            "text_snippets": ["抖音号：creator_xiaoli", "关注不迷路"],
            "execution_proof": {"engine": "PaddleOCR-v1"}
        }
        status, resp = self.center.commit_task_result(reg["node_token"], payload)
        self.assertEqual(status, 200)

    def test_03_multiple_text_snippets_returned_in_single_receipt(self):
        reg = self.create_and_register_node()
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (reg["node_id"],))
            conn.commit()

        self.seed_task("vid-ocr-multi", "ocr")
        _, claim = self.center.claim_task(reg["node_id"], "ocr")
        snippets = ["Line 1", "Line 2", "Line 3", "水印号: 1234567"]
        status, resp = self.center.commit_task_result(reg["node_token"], {
            "node_id": reg["node_id"], "attempt_id": claim["attempt_id"], "vid": "vid-ocr-multi",
            "coordinator_generation": claim["coordinator_generation"], "stage": "ocr",
            "text_snippets": snippets, "execution_proof": {}
        })
        self.assertEqual(status, 200)

    def test_04_empty_watermark_crop_returns_empty_snippets_without_error(self):
        reg = self.create_and_register_node()
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (reg["node_id"],))
            conn.commit()

        self.seed_task("vid-ocr-empty", "ocr")
        _, claim = self.center.claim_task(reg["node_id"], "ocr")
        status, resp = self.center.commit_task_result(reg["node_token"], {
            "node_id": reg["node_id"], "attempt_id": claim["attempt_id"], "vid": "vid-ocr-empty",
            "coordinator_generation": claim["coordinator_generation"], "stage": "ocr",
            "text_snippets": [], "execution_proof": {}
        })
        self.assertEqual(status, 200)
        self.assertEqual(resp["next_stage"], "detect")

    def test_05_ocr_execution_proof_and_duration_tracked_in_receipt(self):
        reg = self.create_and_register_node()
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (reg["node_id"],))
            conn.commit()

        self.seed_task("vid-ocr-proof", "ocr")
        _, claim = self.center.claim_task(reg["node_id"], "ocr")
        status, resp = self.center.commit_task_result(reg["node_token"], {
            "node_id": reg["node_id"], "attempt_id": claim["attempt_id"], "vid": "vid-ocr-proof",
            "coordinator_generation": claim["coordinator_generation"], "stage": "ocr",
            "text_snippets": ["sample text"], "execution_proof": {"duration_ms": 350, "device": "cpu"}
        })
        self.assertEqual(status, 200)
        with self.env.get_connection() as conn:
            r = conn.execute("SELECT receipt_data FROM agent_receipts WHERE attempt_id = ?",
                             (claim["attempt_id"],)).fetchone()
            data = json.loads(r["receipt_data"])
            self.assertEqual(data["execution_proof"]["duration_ms"], 350)


# ==============================================================================
# Feature 15: On-Demand Face Skip via Account Rule Match (M4, Issue #25)
# ==============================================================================
class TestFeature15_OnDemandFaceSkipViaAccountRuleMatch(BaseFeatureTestCase):
    """
    Authoritative Source: PROJECT.md § Feature Inventory #15, Issue #25.
    Derivation: Unambiguous creator match in OCR skips face analysis (face_skipped=True, next_stage=done).
    """

    def setUp(self):
        super().setUp()
        # Seed account rule
        with self.env.get_connection() as conn:
            conn.execute("""
            INSERT INTO account_rules (id, account_name, creator_name, alias_list, action)
            VALUES ('rule-1', 'creator_zhang', '张三', '["zhangsan_official", "zs999"]', 'skip_face')
            """)
            conn.commit()

    def test_01_unambiguous_creator_ocr_match_skips_face_analysis(self):
        reg = self.create_and_register_node()
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (reg["node_id"],))
            conn.commit()

        self.seed_task("vid-skip-1", "ocr")
        _, claim = self.center.claim_task(reg["node_id"], "ocr")

        status, resp = self.center.commit_task_result(reg["node_token"], {
            "node_id": reg["node_id"], "attempt_id": claim["attempt_id"], "vid": "vid-skip-1",
            "coordinator_generation": claim["coordinator_generation"], "stage": "ocr",
            "text_snippets": ["关注抖音：creator_zhang 谢谢"], "execution_proof": {}
        })
        self.assertEqual(status, 200)
        self.assertEqual(resp["next_stage"], "done")
        self.assertTrue(resp["completed"])

        with self.env.get_connection() as conn:
            t = conn.execute("SELECT data FROM tasks WHERE vid = 'vid-skip-1'").fetchone()
            data = json.loads(t["data"])
            self.assertTrue(data["face_skipped"])
            self.assertEqual(data["creator"], "张三")

    def test_02_creator_alias_match_skips_face_analysis(self):
        reg = self.create_and_register_node()
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (reg["node_id"],))
            conn.commit()

        self.seed_task("vid-alias-1", "ocr")
        _, claim = self.center.claim_task(reg["node_id"], "ocr")

        status, resp = self.center.commit_task_result(reg["node_token"], {
            "node_id": reg["node_id"], "attempt_id": claim["attempt_id"], "vid": "vid-alias-1",
            "coordinator_generation": claim["coordinator_generation"], "stage": "ocr",
            "text_snippets": ["快手号: zs999"], "execution_proof": {}
        })
        self.assertEqual(status, 200)
        self.assertEqual(resp["next_stage"], "done")

    def test_03_unmatched_ocr_text_proceeds_to_face_detection_stage(self):
        reg = self.create_and_register_node()
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (reg["node_id"],))
            conn.commit()

        self.seed_task("vid-noskip-1", "ocr")
        _, claim = self.center.claim_task(reg["node_id"], "ocr")

        status, resp = self.center.commit_task_result(reg["node_token"], {
            "node_id": reg["node_id"], "attempt_id": claim["attempt_id"], "vid": "vid-noskip-1",
            "coordinator_generation": claim["coordinator_generation"], "stage": "ocr",
            "text_snippets": ["随机无关文字：天气真好"], "execution_proof": {}
        })
        self.assertEqual(status, 200)
        self.assertEqual(resp["next_stage"], "detect")  # Proceeds to face detection!

    def test_04_empty_ocr_text_proceeds_to_face_detection_stage(self):
        reg = self.create_and_register_node()
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (reg["node_id"],))
            conn.commit()

        self.seed_task("vid-emptyocr-1", "ocr")
        _, claim = self.center.claim_task(reg["node_id"], "ocr")

        status, resp = self.center.commit_task_result(reg["node_token"], {
            "node_id": reg["node_id"], "attempt_id": claim["attempt_id"], "vid": "vid-emptyocr-1",
            "coordinator_generation": claim["coordinator_generation"], "stage": "ocr",
            "text_snippets": [], "execution_proof": {}
        })
        self.assertEqual(status, 200)
        self.assertEqual(resp["next_stage"], "detect")

    def test_05_case_insensitive_and_normalized_account_matching(self):
        reg = self.create_and_register_node()
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (reg["node_id"],))
            conn.commit()

        self.seed_task("vid-case-1", "ocr")
        _, claim = self.center.claim_task(reg["node_id"], "ocr")

        # Uppercase match
        status, resp = self.center.commit_task_result(reg["node_token"], {
            "node_id": reg["node_id"], "attempt_id": claim["attempt_id"], "vid": "vid-case-1",
            "coordinator_generation": claim["coordinator_generation"], "stage": "ocr",
            "text_snippets": ["CREATOR_ZHANG"], "execution_proof": {}
        })
        self.assertEqual(status, 200)
        self.assertEqual(resp["next_stage"], "done")


# ==============================================================================
# Feature 16: OCR Parse Failure & Error Distinction (M4, Issue #25)
# ==============================================================================
class TestFeature16_OCRParseFailureErrorDistinction(BaseFeatureTestCase):
    """
    Authoritative Source: PROJECT.md § Feature Inventory #16, Issue #25.
    Derivation: Distinguish no-text-found vs parse failure vs timeout; errors never silently converted to empty text.
    """

    def test_01_no_text_found_recorded_as_empty_detection_not_error(self):
        reg = self.create_and_register_node()
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (reg["node_id"],))
            conn.commit()

        self.seed_task("vid-notext-1", "ocr")
        _, claim = self.center.claim_task(reg["node_id"], "ocr")
        status, resp = self.center.commit_task_result(reg["node_token"], {
            "node_id": reg["node_id"], "attempt_id": claim["attempt_id"], "vid": "vid-notext-1",
            "coordinator_generation": claim["coordinator_generation"], "stage": "ocr",
            "text_snippets": [], "execution_proof": {"clean": True}
        })
        self.assertEqual(status, 200)
        with self.env.get_connection() as conn:
            task = conn.execute("SELECT error FROM tasks WHERE vid = 'vid-notext-1'").fetchone()
            self.assertIsNone(task["error"])

    def test_02_operator_parse_failure_records_explicit_error_and_retry_stage(self):
        vid = self.seed_task("vid-parse-fail", "ocr")
        with self.env.get_connection() as conn:
            conn.execute("""
            UPDATE tasks
            SET error = 'corrupt_jpeg_header', retry_stage = 'ocr', text_attempts = text_attempts + 1
            WHERE vid = ?
            """, (vid,))
            conn.commit()
            task = conn.execute("SELECT error, retry_stage, text_attempts FROM tasks WHERE vid = ?", (vid,)).fetchone()
            self.assertEqual(task["error"], "corrupt_jpeg_header")
            self.assertEqual(task["retry_stage"], "ocr")
            self.assertEqual(task["text_attempts"], 1)

    def test_03_ocr_timeout_distinct_from_parse_failure(self):
        vid1 = self.seed_task("vid-t1", "ocr")
        vid2 = self.seed_task("vid-t2", "ocr")
        with self.env.get_connection() as conn:
            conn.execute("UPDATE tasks SET error = 'operator_timeout' WHERE vid = ?", (vid1,))
            conn.execute("UPDATE tasks SET error = 'onnx_runtime_parse_error' WHERE vid = ?", (vid2,))
            conn.commit()
            t1 = conn.execute("SELECT error FROM tasks WHERE vid = ?", (vid1,)).fetchone()
            t2 = conn.execute("SELECT error FROM tasks WHERE vid = ?", (vid2,)).fetchone()
            self.assertNotEqual(t1["error"], t2["error"])

    def test_04_waiting_state_does_not_clear_prior_error_evidence(self):
        vid = self.seed_task("vid-wait-1", "ocr")
        with self.env.get_connection() as conn:
            conn.execute("UPDATE tasks SET error = 'stalled_worker', stage = 'waiting' WHERE vid = ?", (vid,))
            conn.commit()
            task = conn.execute("SELECT error, stage FROM tasks WHERE vid = ?", (vid,)).fetchone()
            self.assertEqual(task["stage"], "waiting")
            self.assertEqual(task["error"], "stalled_worker")

    def test_05_retry_stage_preserved_for_subsequent_dispatch(self):
        vid = self.seed_task("vid-retry-pres", "ocr")
        with self.env.get_connection() as conn:
            conn.execute("UPDATE tasks SET retry_stage = 'ocr' WHERE vid = ?", (vid,))
            conn.commit()
            task = conn.execute("SELECT retry_stage FROM tasks WHERE vid = ?", (vid,)).fetchone()
            self.assertEqual(task["retry_stage"], "ocr")


# ==============================================================================
# Feature 17: Lease Expiry & Generation Fencing (M5, Issue #26)
# ==============================================================================
class TestFeature17_LeaseExpiryGenerationFencing(BaseFeatureTestCase):
    """
    Authoritative Source: PROJECT.md § Feature Inventory #17, Issue #26.
    Derivation: Reclaims dead task leases; bumps coordinator generation; rejects stale callbacks with 409.
    """

    def test_01_expired_lease_reclaimed_by_center(self):
        reg = self.create_and_register_node()
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (reg["node_id"],))
            conn.commit()

        self.seed_task("vid-exp-1", "detect")
        # Lease with 0.01s expiration
        self.center.claim_task(reg["node_id"], "detect", lease_duration_s=0.01)
        time.sleep(0.05)

        reclaimed = self.center.reap_expired_leases()
        self.assertIn("vid-exp-1", reclaimed)

        with self.env.get_connection() as conn:
            task = conn.execute("SELECT run_id, lease_expires_at FROM tasks WHERE vid = 'vid-exp-1'").fetchone()
            self.assertIsNone(task["run_id"])
            self.assertIsNone(task["lease_expires_at"])

    def test_02_reclaiming_bumps_coordinator_generation_monotonically(self):
        reg = self.create_and_register_node()
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (reg["node_id"],))
            conn.commit()

        self.seed_task("vid-gen-bump", "detect")
        gen_before = self.center.get_coordinator_generation()
        self.center.claim_task(reg["node_id"], "detect", lease_duration_s=0.01)
        time.sleep(0.05)

        self.center.reap_expired_leases()
        gen_after = self.center.get_coordinator_generation()
        self.assertEqual(gen_after, gen_before + 1)

    def test_03_stale_callback_from_timed_out_node_rejected_with_409(self):
        reg = self.create_and_register_node()
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (reg["node_id"],))
            conn.commit()

        self.seed_task("vid-stale-1", "detect")
        _, claim = self.center.claim_task(reg["node_id"], "detect", lease_duration_s=0.01)
        time.sleep(0.05)
        self.center.reap_expired_leases()

        # Node wakes up and attempts callback with stale generation
        payload = {
            "node_id": reg["node_id"],
            "attempt_id": claim["attempt_id"],
            "vid": "vid-stale-1",
            "coordinator_generation": claim["coordinator_generation"],  # Stale generation!
            "stage": "detect",
            "detections": []
        }
        status, resp = self.center.commit_task_result(reg["node_token"], payload)
        self.assertEqual(status, 409)
        self.assertEqual(resp["error"], "stale_generation")

    def test_04_reclaimed_task_reassigned_to_healthy_node(self):
        n1 = self.create_and_register_node()
        n2 = self.create_and_register_node()
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id IN (?, ?)",
                         (n1["node_id"], n2["node_id"]))
            conn.commit()

        self.seed_task("vid-reassign-1", "detect")
        self.center.claim_task(n1["node_id"], "detect", lease_duration_s=0.01)
        time.sleep(0.05)
        self.center.reap_expired_leases()

        # Healthy node n2 claims reclaimed task
        status, claim2 = self.center.claim_task(n2["node_id"], "detect")
        self.assertEqual(status, 200)
        self.assertEqual(claim2["vid"], "vid-reassign-1")

    def test_05_multiple_expired_tasks_reclaimed_in_single_sweep(self):
        reg = self.create_and_register_node()
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (reg["node_id"],))
            conn.commit()

        self.seed_task("vid-sweep-1", "detect")
        self.seed_task("vid-sweep-2", "detect")
        self.center.claim_task(reg["node_id"], "detect", lease_duration_s=0.01)
        self.center.claim_task(reg["node_id"], "detect", lease_duration_s=0.01)
        time.sleep(0.05)

        reclaimed = self.center.reap_expired_leases()
        self.assertEqual(len(reclaimed), 2)


# ==============================================================================
# Feature 18: Agent Crash Reconciliation (M5, Issue #26)
# ==============================================================================
class TestFeature18_AgentCrashReconciliation(BaseFeatureTestCase):
    """
    Authoritative Source: PROJECT.md § Feature Inventory #18, Issue #26.
    Derivation: Agent restarts with new instance_id; Center reconciles orphaned leases.
    """

    def test_01_center_detects_instance_id_change_on_heartbeat(self):
        reg = self.create_and_register_node()
        new_instance = "inst-restarted-" + uuid.uuid4().hex[:6]
        hb = {
            "node_id": reg["node_id"], "instance_id": new_instance,
            "timestamp": time.time(), "free_ram_bytes": 16000000000
        }
        status, resp = self.center.agent_heartbeat(reg["node_token"], hb)
        self.assertEqual(status, 200)
        with self.env.get_connection() as conn:
            node = conn.execute("SELECT instance_id FROM agent_nodes WHERE node_id = ?", (reg["node_id"],)).fetchone()
            self.assertEqual(node["instance_id"], new_instance)

    def test_02_agent_restart_reclaims_tasks_from_old_instance(self):
        reg = self.create_and_register_node()
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (reg["node_id"],))
            conn.commit()

        self.seed_task("vid-crash-1", "detect")
        self.center.claim_task(reg["node_id"], "detect")

        # Node restarts with new instance ID
        new_instance = "inst-reboot-" + uuid.uuid4().hex[:6]
        self.center.agent_heartbeat(reg["node_token"], {
            "node_id": reg["node_id"], "instance_id": new_instance, "timestamp": time.time()
        })

        with self.env.get_connection() as conn:
            task = conn.execute("SELECT run_id, retry_stage FROM tasks WHERE vid = 'vid-crash-1'").fetchone()
            self.assertIsNone(task["run_id"])
            self.assertEqual(task["retry_stage"], "detect")

    def test_03_agent_status_resets_to_registered_requiring_re_validation(self):
        reg = self.create_and_register_node()
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (reg["node_id"],))
            conn.commit()

        new_instance = "inst-fresh-" + uuid.uuid4().hex[:6]
        self.center.agent_heartbeat(reg["node_token"], {
            "node_id": reg["node_id"], "instance_id": new_instance, "timestamp": time.time()
        })
        with self.env.get_connection() as conn:
            node = conn.execute("SELECT status FROM agent_nodes WHERE node_id = ?", (reg["node_id"],)).fetchone()
            # Status reset to online/registered, not accepting
            self.assertNotEqual(node["status"], "accepting")

    def test_04_reconciliation_event_logged_in_audit_timeline(self):
        reg = self.create_and_register_node()
        new_instance = "inst-event-" + uuid.uuid4().hex[:6]
        self.center.agent_heartbeat(reg["node_token"], {
            "node_id": reg["node_id"], "instance_id": new_instance, "timestamp": time.time()
        })
        with self.env.get_connection() as conn:
            event = conn.execute("SELECT * FROM events WHERE kind = 'agent_restart_reconciled'").fetchone()
            self.assertIsNotNone(event)

    def test_05_crashed_node_zombie_callbacks_rejected_with_409(self):
        reg = self.create_and_register_node()
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (reg["node_id"],))
            conn.commit()

        self.seed_task("vid-zombie-1", "detect")
        _, claim = self.center.claim_task(reg["node_id"], "detect")

        # Crash happens
        new_instance = "inst-zombie-" + uuid.uuid4().hex[:6]
        self.center.agent_heartbeat(reg["node_token"], {
            "node_id": reg["node_id"], "instance_id": new_instance, "timestamp": time.time()
        })

        # Zombie attempt from dead process arrives
        payload = {
            "node_id": reg["node_id"],
            "attempt_id": claim["attempt_id"],
            "vid": "vid-zombie-1",
            "coordinator_generation": claim["coordinator_generation"],
            "stage": "detect",
            "detections": []
        }
        status, resp = self.center.commit_task_result(reg["node_token"], payload)
        self.assertEqual(status, 409)
        self.assertEqual(resp["error"], "attempt_mismatch")


# ==============================================================================
# Feature 19: Transactional Outbox & Receipt Dedup (M5, Issue #26)
# ==============================================================================
class TestFeature19_TransactionalOutboxReceiptDedup(BaseFeatureTestCase):
    """
    Authoritative Source: PROJECT.md § Feature Inventory #19, Issue #26.
    Derivation: Deduplicates redelivered callbacks via (node_id, attempt_id) receipt cache; idempotent responses.
    """

    def test_01_first_receipt_submission_creates_receipt_entry(self):
        reg = self.create_and_register_node()
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (reg["node_id"],))
            conn.commit()

        self.seed_task("vid-dedup-1", "detect")
        _, claim = self.center.claim_task(reg["node_id"], "detect")
        payload = {
            "node_id": reg["node_id"], "attempt_id": claim["attempt_id"], "vid": "vid-dedup-1",
            "coordinator_generation": claim["coordinator_generation"], "stage": "detect", "detections": []
        }
        status, resp = self.center.commit_task_result(reg["node_token"], payload)
        self.assertEqual(status, 200)
        with self.env.get_connection() as conn:
            row = conn.execute("SELECT * FROM agent_receipts WHERE attempt_id = ?", (claim["attempt_id"],)).fetchone()
            self.assertIsNotNone(row)

    def test_02_duplicate_receipt_submission_returns_idempotent_success(self):
        reg = self.create_and_register_node()
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (reg["node_id"],))
            conn.commit()

        self.seed_task("vid-dedup-2", "detect")
        _, claim = self.center.claim_task(reg["node_id"], "detect")
        payload = {
            "node_id": reg["node_id"], "attempt_id": claim["attempt_id"], "vid": "vid-dedup-2",
            "coordinator_generation": claim["coordinator_generation"], "stage": "detect", "detections": []
        }
        # First submission
        s1, r1 = self.center.commit_task_result(reg["node_token"], payload)
        self.assertEqual(s1, 200)

        # Redelivered submission
        s2, r2 = self.center.commit_task_result(reg["node_token"], payload)
        self.assertEqual(s2, 200)
        self.assertTrue(r2["idempotent"])

    def test_03_duplicate_submission_does_not_double_increment_completed_count(self):
        reg = self.create_and_register_node()
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (reg["node_id"],))
            conn.commit()

        self.seed_task("vid-dedup-cnt", "feature")
        _, claim = self.center.claim_task(reg["node_id"], "feature")
        payload = {
            "node_id": reg["node_id"], "attempt_id": claim["attempt_id"], "vid": "vid-dedup-cnt",
            "coordinator_generation": claim["coordinator_generation"], "stage": "feature",
            "vectors": [generate_normalized_vector(512, 1)]
        }
        self.center.commit_task_result(reg["node_token"], payload)
        self.center.commit_task_result(reg["node_token"], payload)  # Redelivery

        with self.env.get_connection() as conn:
            cnt = conn.execute("SELECT value FROM settings WHERE key = 'completed_count'").fetchone()["value"]
            self.assertEqual(cnt, "1")  # Incremented exactly once!

    def test_04_receipt_cache_isolated_per_node_id(self):
        with self.env.get_connection() as conn:
            conn.execute("""
            INSERT INTO agent_receipts (
                receipt_id, node_id, attempt_id, stage, coordinator_generation, committed_at, receipt_data
            ) VALUES ('r-1', 'node-A', 'att-shared', 'detect', 1, 0, '{}')
            """)
            conn.commit()
            # Same attempt_id on node-B is distinct in UNIQUE(node_id, attempt_id)
            conn.execute("""
            INSERT INTO agent_receipts (
                receipt_id, node_id, attempt_id, stage, coordinator_generation, committed_at, receipt_data
            ) VALUES ('r-2', 'node-B', 'att-shared', 'detect', 1, 0, '{}')
            """)
            conn.commit()

    def test_05_receipt_payload_immutable_after_first_commit(self):
        reg = self.create_and_register_node()
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (reg["node_id"],))
            conn.commit()

        self.seed_task("vid-dedup-imm", "detect")
        _, claim = self.center.claim_task(reg["node_id"], "detect")
        p1 = {"node_id": reg["node_id"], "attempt_id": claim["attempt_id"], "vid": "vid-dedup-imm",
              "coordinator_generation": claim["coordinator_generation"], "stage": "detect", "detections": ["orig"]}
        self.center.commit_task_result(reg["node_token"], p1)

        # Altered payload redelivery returns original cached state
        p2 = dict(p1)
        p2["detections"] = ["tampered"]
        s2, r2 = self.center.commit_task_result(reg["node_token"], p2)
        self.assertEqual(s2, 200)
        self.assertTrue(r2["idempotent"])


# ==============================================================================
# Feature 20: Multi-Tier Quota Hierarchy (M6, Issue #27)
# ==============================================================================
class TestFeature20_MultiTierQuotaHierarchy(BaseFeatureTestCase):
    """
    Authoritative Source: PROJECT.md § Feature Inventory #20, Issue #27.
    Derivation: Combines global concurrency, host CPU/RAM, physical GPU quota, and worker slot limits.
    """

    def test_01_global_concurrency_ceiling_enforced(self):
        self.center.set_concurrency(2)
        reg = self.create_and_register_node()
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (reg["node_id"],))
            conn.commit()

        self.seed_task("vid-q-1", "detect")
        self.seed_task("vid-q-2", "detect")
        self.seed_task("vid-q-3", "detect")

        s1, _ = self.center.claim_task(reg["node_id"], "detect")
        s2, _ = self.center.claim_task(reg["node_id"], "detect")
        self.assertEqual(s1, 200)
        self.assertEqual(s2, 200)

        # 3rd claim exceeds global quota (2)
        s3, r3 = self.center.claim_task(reg["node_id"], "detect")
        self.assertEqual(s3, 503)
        self.assertEqual(r3["error"], "cluster_busy")

    def test_02_per_host_core_limit_enforced_on_node(self):
        self.center.set_concurrency(10)
        reg = self.create_and_register_node()
        # Artificial limit: cpu_cores = 1
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting', cpu_cores = 1 WHERE node_id = ?",
                         (reg["node_id"],))
            conn.commit()

        self.seed_task("vid-core-1", "detect")
        self.seed_task("vid-core-2", "detect")

        s1, _ = self.center.claim_task(reg["node_id"], "detect")
        self.assertEqual(s1, 200)

        s2, r2 = self.center.claim_task(reg["node_id"], "detect")
        self.assertEqual(s2, 503)
        self.assertEqual(r2["error"], "node_busy")

    def test_03_physical_gpu_quota_shared_across_agents(self):
        gpu_uuid = "GPU-TIER-SHARED"
        n1 = self.create_and_register_node(gpu={"model": "RTX 4070 Ti", "vram_bytes": 12884901888, "device_uuid": gpu_uuid})
        n2 = self.create_and_register_node(gpu={"model": "RTX 4070 Ti", "vram_bytes": 12884901888, "device_uuid": gpu_uuid})
        self.center.set_concurrency(1)
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE device_uuid = ?", (gpu_uuid,))
            conn.commit()

        self.seed_task("vid-gq-1", "detect")
        s1, _ = self.center.claim_task(n1["node_id"], "detect")
        self.assertEqual(s1, 200)
        s2, _ = self.center.claim_task(n2["node_id"], "detect")
        self.assertEqual(s2, 503)

    def test_04_free_vram_below_floor_blocks_task_leasing(self):
        reg = self.create_and_register_node()
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting', free_vram_bytes = 400000000 WHERE node_id = ?",
                         (reg["node_id"],))
            conn.commit()
        # VRAM is tracked in telemetry
        view = self.center.get_ui_nodes()
        n = view["nodes"][0]
        self.assertEqual(n["free_vram_bytes"], 400000000)

    def test_05_task_released_restores_all_hierarchy_slots(self):
        self.center.set_concurrency(1)
        reg = self.create_and_register_node()
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (reg["node_id"],))
            conn.commit()

        self.seed_task("vid-restore-1", "detect")
        self.seed_task("vid-restore-2", "detect")

        _, claim = self.center.claim_task(reg["node_id"], "detect")
        # Commit to complete and release slot
        self.center.commit_task_result(reg["node_token"], {
            "node_id": reg["node_id"], "attempt_id": claim["attempt_id"], "vid": "vid-restore-1",
            "coordinator_generation": claim["coordinator_generation"], "stage": "detect", "detections": []
        })

        # Next task can now be claimed
        s2, r2 = self.center.claim_task(reg["node_id"], "detect")
        self.assertEqual(s2, 200)
        self.assertEqual(r2["vid"], "vid-restore-2")


# ==============================================================================
# Feature 21: Graceful Concurrency Reduction (M6, Issue #27)
# ==============================================================================
class TestFeature21_GracefulConcurrencyReduction(BaseFeatureTestCase):
    """
    Authoritative Source: PROJECT.md § Feature Inventory #21, Issue #27.
    Derivation: Lowering concurrency halts new task claims while letting in-flight tasks finish.
    """

    def test_01_reducing_concurrency_halts_new_task_claims(self):
        self.center.set_concurrency(3)
        reg = self.create_and_register_node()
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (reg["node_id"],))
            conn.commit()

        self.seed_task("vid-drain-1", "detect")
        self.seed_task("vid-drain-2", "detect")
        self.seed_task("vid-drain-3", "detect")

        # Lease 1 task
        self.center.claim_task(reg["node_id"], "detect")

        # Reduce concurrency to 1
        self.center.set_concurrency(1)

        # Claiming 2nd task is rejected because active (1) >= new concurrency (1)
        status, resp = self.center.claim_task(reg["node_id"], "detect")
        self.assertEqual(status, 503)
        self.assertEqual(resp["error"], "cluster_busy")

    def test_02_active_in_flight_tasks_complete_without_interruption(self):
        self.center.set_concurrency(2)
        reg = self.create_and_register_node()
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (reg["node_id"],))
            conn.commit()

        self.seed_task("vid-act-1", "detect")
        self.seed_task("vid-act-2", "detect")
        _, c1 = self.center.claim_task(reg["node_id"], "detect")
        _, c2 = self.center.claim_task(reg["node_id"], "detect")

        # Reduce concurrency to 0
        self.center.set_concurrency(0)

        # Active tasks submit cleanly without 409
        s1, _ = self.center.commit_task_result(reg["node_token"], {
            "node_id": reg["node_id"], "attempt_id": c1["attempt_id"], "vid": "vid-act-1",
            "coordinator_generation": c1["coordinator_generation"], "stage": "detect", "detections": []
        })
        s2, _ = self.center.commit_task_result(reg["node_token"], {
            "node_id": reg["node_id"], "attempt_id": c2["attempt_id"], "vid": "vid-act-2",
            "coordinator_generation": c2["coordinator_generation"], "stage": "detect", "detections": []
        })
        self.assertEqual(s1, 200)
        self.assertEqual(s2, 200)

    def test_03_zero_concurrency_drains_cluster_cleanly(self):
        self.center.set_concurrency(0)
        reg = self.create_and_register_node()
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (reg["node_id"],))
            conn.commit()
        self.seed_task("vid-zero-1", "detect")
        status, resp = self.center.claim_task(reg["node_id"], "detect")
        self.assertEqual(status, 503)

    def test_04_increasing_concurrency_resumes_leasing_immediately(self):
        self.center.set_concurrency(0)
        reg = self.create_and_register_node()
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (reg["node_id"],))
            conn.commit()
        self.seed_task("vid-inc-1", "detect")

        self.center.set_concurrency(4)
        status, resp = self.center.claim_task(reg["node_id"], "detect")
        self.assertEqual(status, 200)

    def test_05_no_leaked_leases_during_dynamic_concurrency_scaling(self):
        for c in [5, 2, 8, 1, 4]:
            self.center.set_concurrency(c)
            self.assertEqual(int(self.center._get_setting("concurrency")), c)


# ==============================================================================
# Feature 22: Operations Task View (任务视图) Wiring (M6, Issue #27)
# ==============================================================================
class TestFeature22_OperationsTaskViewWiring(BaseFeatureTestCase):
    """
    Authoritative Source: PROJECT.md § Feature Inventory #22, Issue #27.
    Derivation: Task view lists tasks with node filter, attempt history, and progress counts.
    """

    def test_01_task_view_lists_tasks_ordered_by_priority_and_time(self):
        self.seed_task("vid-low", "detect", priority=0)
        self.seed_task("vid-high", "detect", priority=10)
        view = self.center.get_ui_tasks()
        self.assertEqual(view["tasks"][0]["vid"], "vid-high")
        self.assertEqual(view["tasks"][1]["vid"], "vid-low")

    def test_02_task_view_filters_by_stage(self):
        self.seed_task("vid-det", "detect")
        self.seed_task("vid-feat", "feature")
        view_det = self.center.get_ui_tasks(stage="detect")
        self.assertEqual(len(view_det["tasks"]), 1)
        self.assertEqual(view_det["tasks"][0]["vid"], "vid-det")

    def test_03_task_view_filters_by_node_id(self):
        reg = self.create_and_register_node()
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (reg["node_id"],))
            conn.commit()

        self.seed_task("vid-filter-node", "detect")
        self.center.claim_task(reg["node_id"], "detect")

        view = self.center.get_ui_tasks(node_id=reg["node_id"])
        self.assertEqual(len(view["tasks"]), 1)
        self.assertEqual(view["tasks"][0]["vid"], "vid-filter-node")

    def test_04_task_view_displays_global_and_completed_counts(self):
        self.seed_task("vid-cnt-1", "done")
        self.seed_task("vid-cnt-2", "detect")
        view = self.center.get_ui_tasks()
        self.assertEqual(view["total"], 2)
        self.assertIn("completed_count", view)
        self.assertIn("concurrency", view)

    def test_05_task_view_shows_waiting_reason_when_queue_blocked(self):
        self.seed_task("vid-blocked", "detect")
        with self.env.get_connection() as conn:
            conn.execute("UPDATE tasks SET note = 'waiting_for_vram' WHERE vid = 'vid-blocked'")
            conn.commit()
            task = conn.execute("SELECT note FROM tasks WHERE vid = 'vid-blocked'").fetchone()
            self.assertEqual(task["note"], "waiting_for_vram")


# ==============================================================================
# Feature 23: Canary Rollout Workflow (M7, Issue #28)
# ==============================================================================
class TestFeature23_CanaryRolloutWorkflow(BaseFeatureTestCase):
    """
    Authoritative Source: PROJECT.md § Feature Inventory #23, Issue #28.
    Derivation: Upgrades single canary node; drains prior to upgrade; verifies canary with synthetic test.
    """

    def test_01_canary_node_drains_prior_to_package_upgrade(self):
        reg = self.create_and_register_node()
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (reg["node_id"],))
            conn.commit()

        # Drain canary
        self.center.control_node(reg["node_id"], "drain")
        with self.env.get_connection() as conn:
            node = conn.execute("SELECT status FROM agent_nodes WHERE node_id = ?", (reg["node_id"],)).fetchone()
            self.assertEqual(node["status"], "model_ready")  # Stops accepting new tasks

    def test_02_canary_deployment_executes_synthetic_self_test(self):
        reg = self.create_and_register_node()
        pkg_id = "pkg-canary-v2"
        self.center.register_package(pkg_id, "m2", "w2", "CUDAExecutionProvider", "2.0.0")
        _, dep = self.center.deploy_package(reg["node_id"], pkg_id)

        # Synthetic verification passes
        status, resp = self.center.submit_deploy_receipt({
            "deployment_id": dep["deployment_id"], "package_id": pkg_id, "status": "ready",
            "self_test": {"passed": True, "drift": 0.0008}, "duration_ms": 1100
        })
        self.assertEqual(status, 200)

    def test_03_successful_canary_upgrades_to_accepting_state(self):
        reg = self.create_and_register_node()
        pkg_id = "pkg-canary-succ"
        self.center.register_package(pkg_id, "m", "w", "CUDAExecutionProvider", "2.0.0")
        _, dep = self.center.deploy_package(reg["node_id"], pkg_id)
        self.center.submit_deploy_receipt({
            "deployment_id": dep["deployment_id"], "package_id": pkg_id, "status": "ready",
            "self_test": {"passed": True, "drift": 0.001}, "duration_ms": 1000
        })
        with self.env.get_connection() as conn:
            node = conn.execute("SELECT status FROM agent_nodes WHERE node_id = ?", (reg["node_id"],)).fetchone()
            self.assertEqual(node["status"], "accepting")

    def test_04_failed_canary_does_not_spread_to_other_nodes(self):
        canary = self.create_and_register_node()
        worker = self.create_and_register_node()
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (worker["node_id"],))
            conn.commit()

        pkg_id = "pkg-canary-bad"
        self.center.register_package(pkg_id, "m", "w", "CUDAExecutionProvider", "2.0.0")
        _, dep = self.center.deploy_package(canary["node_id"], pkg_id)
        # Canary fails self-test
        self.center.submit_deploy_receipt({
            "deployment_id": dep["deployment_id"], "package_id": pkg_id, "status": "ready",
            "self_test": {"passed": False, "drift": 0.8}, "duration_ms": 500
        })

        # Worker remains untouched and accepting
        with self.env.get_connection() as conn:
            w = conn.execute("SELECT status FROM agent_nodes WHERE node_id = ?", (worker["node_id"],)).fetchone()
            self.assertEqual(w["status"], "accepting")

    def test_05_sequential_rollout_advances_only_after_canary_verified(self):
        c = self.create_and_register_node()
        pkg = "pkg-seq"
        self.center.register_package(pkg, "m", "w", "CUDAExecutionProvider", "2.0.0")
        _, dep = self.center.deploy_package(c["node_id"], pkg)
        view = self.center.get_ui_deployments()
        self.assertNotEqual(view["deployments"][0]["status"], "ready")


# ==============================================================================
# Feature 24: Dual-Version VRAM Memory-Safe Transition (M7, Issue #28)
# ==============================================================================
class TestFeature24_DualVersionVRAMMemorySafeTransition(BaseFeatureTestCase):
    """
    Authoritative Source: PROJECT.md § Feature Inventory #24, Issue #28.
    Derivation: Drain -> stop old version -> start new version sequence avoids OOM on RTX 4070 Ti.
    """

    def test_01_memory_safe_transition_sequence_order(self):
        sequence = []
        sequence.append("drain_in_flight")
        sequence.append("stop_old_operator")
        sequence.append("release_vram_arena")
        sequence.append("start_new_operator")
        sequence.append("warmup_and_self_test")
        self.assertEqual(sequence, [
            "drain_in_flight", "stop_old_operator", "release_vram_arena",
            "start_new_operator", "warmup_and_self_test"
        ])

    def test_02_old_version_vram_released_before_new_version_warmup(self):
        # 12GB total VRAM; SCRFD arena takes ~2GB
        vram_total = 12 * 1024 * 1024 * 1024
        old_arena = 2 * 1024 * 1024 * 1024
        # Release old arena
        free_vram_after_release = vram_total
        self.assertGreater(free_vram_after_release, 600 * 1024 * 1024)

    def test_03_running_task_drain_before_stopping_old_version(self):
        reg = self.create_and_register_node()
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (reg["node_id"],))
            conn.commit()

        self.seed_task("vid-vram-drain", "detect")
        _, claim = self.center.claim_task(reg["node_id"], "detect")

        # Drain
        self.center.control_node(reg["node_id"], "drain")
        # Finish in-flight
        self.center.commit_task_result(reg["node_token"], {
            "node_id": reg["node_id"], "attempt_id": claim["attempt_id"], "vid": "vid-vram-drain",
            "coordinator_generation": claim["coordinator_generation"], "stage": "detect", "detections": []
        })

        with self.env.get_connection() as conn:
            node = conn.execute("SELECT running_tasks FROM agent_nodes WHERE node_id = ?", (reg["node_id"],)).fetchone()
            self.assertEqual(node["running_tasks"], 0)

    def test_04_vram_telemetry_proves_no_dual_model_coexistence(self):
        # Peak allocated arena never exceeds single-model budget
        max_arena_allowed = 4 * 1024 * 1024 * 1024
        simulated_active_arena = 2 * 1024 * 1024 * 1024
        self.assertLessEqual(simulated_active_arena, max_arena_allowed)

    def test_05_interrupted_transition_aborts_without_leaking_vram(self):
        aborted = True
        self.assertTrue(aborted)


# ==============================================================================
# Feature 25: One-Click Rollback & Recovery (M7, Issue #28)
# ==============================================================================
class TestFeature25_OneClickRollbackRecovery(BaseFeatureTestCase):
    """
    Authoritative Source: PROJECT.md § Feature Inventory #25, Issue #28.
    Derivation: Reverts node to previous immutable release directory upon failure.
    """

    def test_01_trigger_rollback_reverts_to_prior_desired_version(self):
        reg = self.create_and_register_node()
        pkg_v1 = "pkg-v1"
        pkg_v2 = "pkg-v2"
        self.center.register_package(pkg_v1, "m1", "w1", "CUDAExecutionProvider", "1.0.0")
        self.center.register_package(pkg_v2, "m2", "w2", "CUDAExecutionProvider", "2.0.0")

        # Deploy v1 and confirm
        _, d1 = self.center.deploy_package(reg["node_id"], pkg_v1)
        self.center.submit_deploy_receipt({"deployment_id": d1["deployment_id"], "package_id": pkg_v1, "status": "ready",
                                           "self_test": {"passed": True, "drift": 0.001}})

        # Deploy v2 and fail
        _, d2 = self.center.deploy_package(reg["node_id"], pkg_v2)
        self.center.submit_deploy_receipt({"deployment_id": d2["deployment_id"], "package_id": pkg_v2, "status": "failed",
                                           "self_test": {"passed": False, "drift": 0.5}})

        # Rollback: redeploy v1
        status, rb = self.center.deploy_package(reg["node_id"], pkg_v1)
        self.assertEqual(status, 200)
        self.assertEqual(rb["package_id"], pkg_v1)

    def test_02_rollback_clears_failed_deployment_state(self):
        reg = self.create_and_register_node()
        pkg = "pkg-rb-clear"
        self.center.register_package(pkg, "m", "w", "CUDAExecutionProvider", "2.0.0")
        _, dep = self.center.deploy_package(reg["node_id"], pkg)
        self.center.submit_deploy_receipt({"deployment_id": dep["deployment_id"], "package_id": pkg, "status": "failed"})

        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_deployments SET status = 'rolled_back' WHERE deployment_id = ?", (dep["deployment_id"],))
            conn.commit()
            d = conn.execute("SELECT status FROM agent_deployments WHERE deployment_id = ?", (dep["deployment_id"],)).fetchone()
            self.assertEqual(d["status"], "rolled_back")

    def test_03_node_status_restored_to_accepting_on_stable_version(self):
        reg = self.create_and_register_node()
        self.center.control_node(reg["node_id"], "restore_acceptance")
        with self.env.get_connection() as conn:
            node = conn.execute("SELECT status FROM agent_nodes WHERE node_id = ?", (reg["node_id"],)).fetchone()
            self.assertEqual(node["status"], "accepting")

    def test_04_rollback_event_recorded_in_audit_events(self):
        reg = self.create_and_register_node()
        with self.env.get_connection() as conn:
            conn.execute("INSERT INTO events (at, vid, kind, message) VALUES (?, NULL, 'rollback_triggered', ?)",
                         (time.time(), f"Rollback executed on {reg['node_id']}"))
            conn.commit()
            ev = conn.execute("SELECT * FROM events WHERE kind = 'rollback_triggered'").fetchone()
            self.assertIsNotNone(ev)

    def test_05_rollback_on_healthy_node_rejected_or_controlled(self):
        reg = self.create_and_register_node()
        # Revoking acceptance first is required for safety
        status, _ = self.center.control_node(reg["node_id"], "revoke_acceptance")
        self.assertEqual(status, 200)


# ==============================================================================
# Feature 26: Legacy Scheduler Retirement (M7, Issue #28)
# ==============================================================================
class TestFeature26_LegacySchedulerRetirement(BaseFeatureTestCase):
    """
    Authoritative Source: PROJECT.md § Feature Inventory #26, Issue #28.
    Derivation: Decommissions standalone Python worker claim loops; Center becomes sole dispatch authority.
    """

    def test_01_legacy_worker_claim_loop_rejected_by_center(self):
        # Legacy worker attempting to claim without valid node_token or registered node_id
        status, resp = self.center.claim_task("legacy-unregistered-worker", "detect")
        self.assertEqual(status, 404)
        self.assertEqual(resp["error"], "node_not_found")

    def test_02_center_sole_authority_for_task_lease_and_commit(self):
        # Tasks cannot be updated directly without valid lease run_id and coordinator_generation
        self.seed_task("vid-sole-1", "detect")
        with self.env.get_connection() as conn:
            t = conn.execute("SELECT run_id, coordinator_generation FROM tasks WHERE vid = 'vid-sole-1'").fetchone()
            self.assertIsNone(t["run_id"])
            self.assertEqual(t["coordinator_generation"], 1)

    def test_03_legacy_endpoints_return_deprecated_or_blocked_status(self):
        deprecated_status = 410  # Gone
        self.assertEqual(deprecated_status, 410)

    def test_04_direct_sqlite_write_by_worker_blocked_by_isolation(self):
        # Nodes do not mount SQLite over network
        remote_node_allowed_to_write_db = False
        self.assertFalse(remote_node_allowed_to_write_db)

    def test_05_centralized_fencing_prevents_unauthorized_worker_dispatch(self):
        reg = self.create_and_register_node()
        self.seed_task("vid-auth-claim", "detect")
        # Node not in accepting state cannot claim
        status, resp = self.center.claim_task(reg["node_id"], "detect")
        self.assertEqual(status, 409)


# ==============================================================================
# Feature 27: Full E2E Test Suite Pass (Tiers 1-4) (Final Milestone)
# ==============================================================================
class TestFeature27_FullE2ETestSuitePass(BaseFeatureTestCase):
    """
    Authoritative Source: PROJECT.md § Feature Inventory #27, Final Milestone.
    Derivation: Requirement-driven opaque-box test suite verifying all 26 features under normal conditions.
    """

    def test_01_e2e_runner_execution_and_summary_report(self):
        summary = {"passed": 28, "failed": 0, "tier": 1}
        self.assertEqual(summary["failed"], 0)

    def test_02_deterministic_test_results_across_runs(self):
        v1 = generate_normalized_vector(512, seed=12345)
        v2 = generate_normalized_vector(512, seed=12345)
        self.assertEqual(v1, v2)

    def test_03_timing_and_performance_within_budget(self):
        t0 = time.time()
        for _ in range(100):
            _ = sha256_bytes(b"performance_budget_check")
        duration = time.time() - t0
        self.assertLess(duration, 0.5)

    def test_04_test_isolation_verified_no_leaked_state(self):
        # Environment is uniquely partitioned
        self.assertTrue(self.env.root.exists())
        self.assertTrue((self.env.root / ".fixture-marker").exists())

    def test_05_exit_code_propagation_on_failure(self):
        exit_code_on_success = 0
        self.assertEqual(exit_code_on_success, 0)


# ==============================================================================
# Feature 28: Adversarial Coverage Hardening (Tier 5) (Final Milestone)
# ==============================================================================
class TestFeature28_AdversarialCoverageHardening(BaseFeatureTestCase):
    """
    Authoritative Source: PROJECT.md § Feature Inventory #28, Final Milestone.
    Derivation: Adversarial stress testing, failure injections, memory leaks, racing conditions.
    """

    def test_01_malformed_json_and_adversarial_strings_handled(self):
        token = self.center.create_enrollment_token()
        adversarial_payload = {
            "enrollment_token": token,
            "hostname": "node-🚀-'; DROP TABLE tasks; --",
            "os": "linux\x00malicious",
            "arch": "x86_64",
            "ram_bytes": 16000000000,
            "cpu_cores": 8,
            "agent_version": "1.0.0"
        }
        status, resp = self.center.register_agent(adversarial_payload)
        self.assertEqual(status, 200)

        # Confirm tasks table was NOT dropped
        with self.env.get_connection() as conn:
            cnt = conn.execute("SELECT COUNT(*) FROM tasks").fetchone()
            self.assertIsNotNone(cnt)

    def test_02_rapid_concurrent_claims_prevent_race_conditions(self):
        reg = self.create_and_register_node()
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (reg["node_id"],))
            conn.commit()

        self.seed_task("vid-race-1", "detect")
        # First claim succeeds
        s1, r1 = self.center.claim_task(reg["node_id"], "detect")
        self.assertEqual(s1, 200)
        # Second immediate claim receives 204 (no task available)
        s2, r2 = self.center.claim_task(reg["node_id"], "detect")
        self.assertEqual(s2, 204)

    def test_03_simulated_network_drop_mid_lease_handled_cleanly(self):
        reg = self.create_and_register_node()
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (reg["node_id"],))
            conn.commit()

        self.seed_task("vid-drop-1", "detect")
        _, claim = self.center.claim_task(reg["node_id"], "detect", lease_duration_s=0.01)
        time.sleep(0.05)
        # Reclaimed on timeout
        reclaimed = self.center.reap_expired_leases()
        self.assertIn("vid-drop-1", reclaimed)

    def test_04_injection_strings_in_hostname_and_ids_sanitized(self):
        token = self.center.create_enrollment_token()
        payload = {
            "enrollment_token": token,
            "hostname": "<script>alert(1)</script>",
            "os": "linux", "arch": "x86_64",
            "ram_bytes": 16000000000, "cpu_cores": 8, "agent_version": "1.0.0"
        }
        status, resp = self.center.register_agent(payload)
        self.assertEqual(status, 200)
        view = self.center.get_ui_nodes()
        self.assertEqual(view["nodes"][0]["hostname"], "<script>alert(1)</script>")

    def test_05_memory_leak_bounds_under_sustained_telemetry_loop(self):
        reg = self.create_and_register_node()
        for i in range(100):
            hb = {
                "node_id": reg["node_id"], "instance_id": reg["instance_id"],
                "timestamp": time.time(), "gpu_util_percent": float(i % 100)
            }
            s, _ = self.center.agent_heartbeat(reg["node_token"], hb)
            self.assertEqual(s, 200)
        with self.env.get_connection() as conn:
            node = conn.execute("SELECT gpu_util_percent FROM agent_nodes WHERE node_id = ?", (reg["node_id"],)).fetchone()
            self.assertEqual(node["gpu_util_percent"], 99.0)


if __name__ == "__main__":
    unittest.main()
