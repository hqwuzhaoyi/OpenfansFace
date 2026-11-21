"""
Tier 3: Cross-Feature Combinations E2E Test Suite.
Exhaustively tests pairwise and multi-feature lifecycle interactions:
- Pipeline: Enrollment -> Audited Deploy -> Capability Lease -> Frame Dispatch -> Atomic Commit
- Failover: Lease Expiry -> Generation Fencing -> Rejection of Dead Node -> Recovery by Healthy Node
- Rules & Accounting: OCR Watermark Text -> Account Rule Match -> Face Analysis Skip -> Monotonic Accounting
- Quota & Pooling: Physical Device Pooling -> Multi-Node Concurrency -> Dynamic Throttling -> Task View
- Operations: Canary Upgrade -> In-Flight Task Drain -> Memory-Safe Transition -> Rollback on Failure
"""
import base64
import json
from pathlib import Path
import time
import unittest
import uuid

from tests.e2e.harness import (
    FIXTURE_PORTRAIT,
    IsolatedTestEnvironment,
    SimulatedCenterService,
    generate_normalized_vector,
    sha256_bytes,
)


class BaseCrossFeatureTestCase(unittest.TestCase):
    def setUp(self):
        self.env = IsolatedTestEnvironment()
        self.center = SimulatedCenterService(self.env)

    def tearDown(self):
        self.env.close()

    def seed_task(self, vid: str, stage: str = "detect", priority: int = 0) -> str:
        with self.env.get_connection() as conn:
            conn.execute("""
            INSERT INTO tasks (
                vid, path, name, size, mtime_ns, stage, priority, created_at, updated_at, data
            ) VALUES (?, ?, ?, 1048576, '1700000000000000', ?, ?, ?, ?, '{}')
            """, (vid, f"/isolated/media/{vid}.mp4", f"{vid}.mp4", stage, priority, time.time(), time.time()))
            conn.commit()
        return vid


class TestPipelineRegistrationDeployLeaseCommit(BaseCrossFeatureTestCase):
    """
    Interaction 1: Features 1 (Enrollment) -> 5 (Deploy) -> 7 (Self-Test) -> 9 (Lease) -> 10 (Frame Dispatch) -> 11 (Commit).
    """

    def test_full_registration_deploy_lease_and_commit_cycle(self):
        # 1. Enrollment
        token = self.center.create_enrollment_token()
        s_reg, reg = self.center.register_agent({
            "enrollment_token": token,
            "hostname": "wsl-rtx4070ti",
            "os": "linux",
            "arch": "x86_64",
            "gpu": {"model": "RTX 4070 Ti", "vram_bytes": 12884901888, "device_uuid": "GPU-PIPE-1"},
            "ram_bytes": 34359738368,
            "cpu_cores": 16,
            "agent_version": "1.0.0"
        })
        self.assertEqual(s_reg, 200)
        node_id = reg["node_id"]
        node_token = reg["node_token"]

        # 2. Package Deployment
        pkg_id = "pkg-scrfd-cuda-v1"
        self.center.register_package(pkg_id, sha256_bytes(b"manifest"), sha256_bytes(b"weights"), "CUDAExecutionProvider", "1.0.0")
        s_dep, dep = self.center.deploy_package(node_id, pkg_id)
        self.assertEqual(s_dep, 200)

        # 3. Self-Test & Startup Proof
        s_rec, rec = self.center.submit_deploy_receipt({
            "deployment_id": dep["deployment_id"],
            "package_id": pkg_id,
            "status": "ready",
            "startup_proof": {"cuda_initialized": True},
            "self_test": {"passed": True, "drift": 0.0005},
            "duration_ms": 1200
        })
        self.assertEqual(s_rec, 200)

        # Node is now in 'accepting' status
        nodes = self.center.get_ui_nodes()
        node_entry = next(n for n in nodes["nodes"] if n["node_id"] == node_id)
        self.assertEqual(node_entry["status"], "accepting")

        # 4. Capability Lease
        vid = self.seed_task("vid-pipeline-1", "detect")
        s_claim, claim = self.center.claim_task(node_id, "detect")
        self.assertEqual(s_claim, 200)
        self.assertEqual(claim["vid"], vid)

        # 5. Bounded Frame Check
        self.assertEqual(len(claim["frames"]), 1)
        raw_frame = base64.b64decode(claim["frames"][0]["data_base64"])
        self.assertLessEqual(len(raw_frame), 1_572_864)

        # 6. Atomic Receipt Commit
        s_commit, commit = self.center.commit_task_result(node_token, {
            "node_id": node_id,
            "attempt_id": claim["attempt_id"],
            "vid": vid,
            "coordinator_generation": claim["coordinator_generation"],
            "stage": "detect",
            "detections": [{"box": [10, 10, 50, 50], "score": 0.99}],
            "execution_proof": {"device": "gpu"}
        })
        self.assertEqual(s_commit, 200)
        self.assertEqual(commit["next_stage"], "feature")


class TestFailoverGenerationFencingAndRecovery(BaseCrossFeatureTestCase):
    """
    Interaction 2: Features 9 (Lease) -> 17 (Lease Expiry & Fencing) -> 18 (Crash Reconcile) -> 19 (Dedup).
    """

    def test_stalled_node_timeout_generation_bump_and_takeover(self):
        # Setup two nodes
        t1 = self.center.create_enrollment_token()
        _, n1 = self.center.register_agent({"enrollment_token": t1, "hostname": "stalled-node", "os": "linux", "arch": "x86_64",
                                            "ram_bytes": 16000000000, "cpu_cores": 4, "agent_version": "1.0.0"})
        t2 = self.center.create_enrollment_token()
        _, n2 = self.center.register_agent({"enrollment_token": t2, "hostname": "healthy-node", "os": "linux", "arch": "x86_64",
                                            "ram_bytes": 16000000000, "cpu_cores": 4, "agent_version": "1.0.0"})

        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id IN (?, ?)", (n1["node_id"], n2["node_id"]))
            conn.commit()

        vid = self.seed_task("vid-failover-1", "detect")

        # 1. Node 1 claims with micro-lease
        _, claim1 = self.center.claim_task(n1["node_id"], "detect", lease_duration_s=0.01)
        time.sleep(0.05)

        # 2. Center sweeps expired leases -> generation bumps
        gen_before = self.center.get_coordinator_generation()
        reclaimed = self.center.reap_expired_leases()
        self.assertIn(vid, reclaimed)
        gen_after = self.center.get_coordinator_generation()
        self.assertEqual(gen_after, gen_before + 1)

        # 3. Healthy Node 2 takes over reclaimed task
        s_takeover, claim2 = self.center.claim_task(n2["node_id"], "detect")
        self.assertEqual(s_takeover, 200)
        self.assertEqual(claim2["vid"], vid)
        self.assertEqual(claim2["coordinator_generation"], gen_after)

        # 4. Dead Node 1 wakes up and attempts submission -> Rejected with 409 stale_generation
        s_stale, r_stale = self.center.commit_task_result(n1["node_token"], {
            "node_id": n1["node_id"], "attempt_id": claim1["attempt_id"], "vid": vid,
            "coordinator_generation": claim1["coordinator_generation"], "stage": "detect", "detections": []
        })
        self.assertEqual(s_stale, 409)
        self.assertEqual(r_stale["error"], "stale_generation")

        # 5. Healthy Node 2 submits result -> Committed successfully
        s_ok, r_ok = self.center.commit_task_result(n2["node_token"], {
            "node_id": n2["node_id"], "attempt_id": claim2["attempt_id"], "vid": vid,
            "coordinator_generation": claim2["coordinator_generation"], "stage": "detect", "detections": []
        })
        self.assertEqual(s_ok, 200)


class TestWatermarkOCRRuleMatchingFaceSkipPipeline(BaseCrossFeatureTestCase):
    """
    Interaction 3: Features 14 (Remote OCR) -> 15 (Face Skip via Rule) -> 16 (Error Distinction) -> 20 (Quota release).
    """

    def test_watermark_ocr_creator_rule_match_skips_subsequent_stages(self):
        with self.env.get_connection() as conn:
            conn.execute("""
            INSERT INTO account_rules (id, account_name, creator_name, alias_list, action)
            VALUES ('rule-auto', 'dy_creator_alice', '爱丽丝', '["alice_studio"]', 'skip_face')
            """)
            conn.commit()

        token = self.center.create_enrollment_token()
        _, node = self.center.register_agent({"enrollment_token": token, "hostname": "ocr-worker", "os": "darwin",
                                              "arch": "arm64", "ram_bytes": 32000000000, "cpu_cores": 8, "agent_version": "1.0.0"})
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (node["node_id"],))
            conn.commit()

        vid = self.seed_task("vid-rule-pipeline", "ocr")
        _, claim = self.center.claim_task(node["node_id"], "ocr")

        # OCR detects watermark matching rule
        status, resp = self.center.commit_task_result(node["node_token"], {
            "node_id": node["node_id"],
            "attempt_id": claim["attempt_id"],
            "vid": vid,
            "coordinator_generation": claim["coordinator_generation"],
            "stage": "ocr",
            "text_snippets": ["关注抖音号：dy_creator_alice 感谢支持"],
            "execution_proof": {"engine": "PaddleOCR"}
        })
        self.assertEqual(status, 200)
        self.assertEqual(resp["next_stage"], "done")
        self.assertTrue(resp["completed"])

        # Verify task data in DB
        with self.env.get_connection() as conn:
            task = conn.execute("SELECT stage, data FROM tasks WHERE vid = ?", (vid,)).fetchone()
            self.assertEqual(task["stage"], "done")
            data = json.loads(task["data"])
            self.assertTrue(data["face_skipped"])
            self.assertEqual(data["creator"], "爱丽丝")


class TestCanaryRolloutVRAMSafeSwapAndRollback(BaseCrossFeatureTestCase):
    """
    Interaction 4: Features 8 (Deploy View) -> 23 (Canary) -> 24 (VRAM Transition) -> 25 (Rollback).
    """

    def test_canary_rollout_with_vram_drain_and_recovery(self):
        token = self.center.create_enrollment_token()
        _, node = self.center.register_agent({"enrollment_token": token, "hostname": "canary-target", "os": "linux",
                                              "arch": "x86_64", "gpu": {"model": "RTX 4070 Ti", "vram_bytes": 12884901888, "device_uuid": "GPU-CANARY"},
                                              "ram_bytes": 32000000000, "cpu_cores": 8, "agent_version": "1.0.0"})
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (node["node_id"],))
            conn.commit()

        # Seed task and lease
        vid = self.seed_task("vid-in-flight", "detect")
        _, claim = self.center.claim_task(node["node_id"], "detect")

        # 1. Drain node before canary upgrade (stops accepting new work)
        self.center.control_node(node["node_id"], "drain")
        with self.env.get_connection() as conn:
            n = conn.execute("SELECT status FROM agent_nodes WHERE node_id = ?", (node["node_id"],)).fetchone()
            self.assertEqual(n["status"], "model_ready")

        # 2. Finish in-flight task to ensure VRAM release
        self.center.commit_task_result(node["node_token"], {
            "node_id": node["node_id"], "attempt_id": claim["attempt_id"], "vid": vid,
            "coordinator_generation": claim["coordinator_generation"], "stage": "detect", "detections": []
        })

        # 3. Canary package deployment
        pkg_id = "pkg-canary-experimental"
        self.center.register_package(pkg_id, "m-exp", "w-exp", "CUDAExecutionProvider", "2.0.0-rc1")
        _, dep = self.center.deploy_package(node["node_id"], pkg_id)

        # 4. Self-test fails on experimental package!
        self.center.submit_deploy_receipt({
            "deployment_id": dep["deployment_id"], "package_id": pkg_id, "status": "failed",
            "self_test": {"passed": False, "drift": 0.45}, "duration_ms": 600
        })

        # 5. One-click rollback triggered
        pkg_stable = "pkg-scrfd-stable"
        self.center.register_package(pkg_stable, "m-st", "w-st", "CUDAExecutionProvider", "1.0.0")
        s_rb, rb = self.center.deploy_package(node["node_id"], pkg_stable)
        self.assertEqual(s_rb, 200)

        # Stable package self-test passes
        self.center.submit_deploy_receipt({
            "deployment_id": rb["deployment_id"], "package_id": pkg_stable, "status": "ready",
            "self_test": {"passed": True, "drift": 0.0001}, "duration_ms": 800
        })

        # Node is back to accepting state
        with self.env.get_connection() as conn:
            n_final = conn.execute("SELECT status FROM agent_nodes WHERE node_id = ?", (node["node_id"],)).fetchone()
            self.assertEqual(n_final["status"], "accepting")


class TestPhysicalDevicePoolQuotaThrottlingAndTaskView(BaseCrossFeatureTestCase):
    """
    Interaction 5: Features 3 (Physical GPU Pool) -> 20 (Quota) -> 21 (Concurrency Reduction) -> 22 (Task View).
    """

    def test_shared_gpu_pool_throttling_and_task_view_reflection(self):
        shared_uuid = "GPU-SHARED-QUAD-TI"
        t1 = self.center.create_enrollment_token()
        t2 = self.center.create_enrollment_token()
        _, n1 = self.center.register_agent({"enrollment_token": t1, "hostname": "gpu-agent-1", "os": "linux", "arch": "x86_64",
                                            "gpu": {"model": "RTX 4070 Ti", "vram_bytes": 12884901888, "device_uuid": shared_uuid},
                                            "ram_bytes": 16000000000, "cpu_cores": 4, "agent_version": "1.0.0"})
        _, n2 = self.center.register_agent({"enrollment_token": t2, "hostname": "gpu-agent-2", "os": "linux", "arch": "x86_64",
                                            "gpu": {"model": "RTX 4070 Ti", "vram_bytes": 12884901888, "device_uuid": shared_uuid},
                                            "ram_bytes": 16000000000, "cpu_cores": 4, "agent_version": "1.0.0"})

        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE device_uuid = ?", (shared_uuid,))
            conn.commit()

        self.seed_task("vid-pool-t1", "detect", priority=1)
        self.seed_task("vid-pool-t2", "detect", priority=5)

        # Global concurrency set to 1
        self.center.set_concurrency(1)

        # Agent 1 claims higher priority task
        s1, r1 = self.center.claim_task(n1["node_id"], "detect")
        self.assertEqual(s1, 200)
        self.assertEqual(r1["vid"], "vid-pool-t2")

        # Agent 2 tries to claim -> 503 cluster_busy
        s2, r2 = self.center.claim_task(n2["node_id"], "detect")
        self.assertEqual(s2, 503)

        # Verify Operations Task View accurately reflects in-flight task assigned to agent 1
        tasks_view = self.center.get_ui_tasks()
        in_flight = next(t for t in tasks_view["tasks"] if t["vid"] == "vid-pool-t2")
        self.assertIn(n1["node_id"], in_flight["run_id"])


if __name__ == "__main__":
    unittest.main()
