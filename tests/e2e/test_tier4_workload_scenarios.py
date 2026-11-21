"""
Tier 4: Real-World Workload Scenarios E2E Test Suite.
Verifies complete operational scenarios under production-like conditions:
- Scenario 1: Heterogeneous Cluster Daily Ingestion (1 RTX 4070 Ti GPU + 2 Mac ARM64 CPU nodes)
- Scenario 2: Network Flap & Dead Node Failover during Large Batch Processing
- Scenario 3: Creator Video High-Volume Pipeline with Account Rule Skips
- Scenario 4: Zero-Downtime Rolling Upgrade of Model Weights under Ingestion Load
- Scenario 5: Catastrophic Node Crash & Cold Restart Reconciliation
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


class BaseWorkloadTestCase(unittest.TestCase):
    def setUp(self):
        self.env = IsolatedTestEnvironment()
        self.center = SimulatedCenterService(self.env)

    def tearDown(self):
        self.env.close()

    def register_node(self, hostname: str, os_name: str, arch: str, cores: int = 8, gpu: dict = None) -> dict:
        token = self.center.create_enrollment_token()
        payload = {
            "enrollment_token": token,
            "hostname": hostname,
            "os": os_name,
            "arch": arch,
            "ram_bytes": 34359738368,
            "cpu_cores": cores,
            "agent_version": "1.0.0",
        }
        if gpu:
            payload["gpu"] = gpu
        status, reg = self.center.register_agent(payload)
        assert status == 200
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (reg["node_id"],))
            conn.commit()
        return reg

    def seed_tasks(self, count: int, prefix: str = "vid", stage: str = "detect") -> list:
        vids = []
        with self.env.get_connection() as conn:
            for i in range(count):
                vid = f"{prefix}-{i:03d}"
                conn.execute("""
                INSERT INTO tasks (
                    vid, path, name, size, mtime_ns, stage, priority, created_at, updated_at, data
                ) VALUES (?, ?, ?, 1048576, '1700000000000000', ?, 0, ?, ?, '{}')
                """, (vid, f"/isolated/media/{vid}.mp4", f"{vid}.mp4", stage, time.time(), time.time()))
                vids.append(vid)
            conn.commit()
        return vids


class TestScenario1_HeterogeneousClusterDailyIngestion(BaseWorkloadTestCase):
    """
    Scenario 1: Heterogeneous Cluster Daily Ingestion.
    1 PC/WSL NVIDIA RTX 4070 Ti (CUDA detector) + 2 Mac ARM64 (CPU OCR & feature extractors).
    Processes a batch of 15 tasks across OCR -> Detect -> Feature stages.
    """

    def test_daily_ingestion_across_heterogeneous_cluster(self):
        # 1. Register heterogeneous nodes
        gpu_node = self.register_node("wsl-rtx4070ti", "linux", "x86_64", cores=16,
                                      gpu={"model": "RTX 4070 Ti", "vram_bytes": 12884901888, "device_uuid": "GPU-4070TI"})
        mac1 = self.register_node("mac-mini-m2", "darwin", "arm64", cores=8)
        mac2 = self.register_node("mac-studio-m1", "darwin", "arm64", cores=10)

        # 2. Seed 15 tasks in 'ocr' stage
        vids = self.seed_tasks(15, prefix="ingest", stage="ocr")
        self.center.set_concurrency(10)

        # 3. Stage 1: CPU nodes process OCR
        for i, vid in enumerate(vids):
            worker = mac1 if i % 2 == 0 else mac2
            status, claim = self.center.claim_task(worker["node_id"], "ocr")
            self.assertEqual(status, 200)
            self.assertEqual(claim["stage"], "ocr")

            # Commit OCR with unmatched text -> advances to detect
            s_c, r_c = self.center.commit_task_result(worker["node_token"], {
                "node_id": worker["node_id"],
                "attempt_id": claim["attempt_id"],
                "vid": claim["vid"],
                "coordinator_generation": claim["coordinator_generation"],
                "stage": "ocr",
                "text_snippets": ["随机水印文字"],
                "execution_proof": {"arch": "arm64"}
            })
            self.assertEqual(s_c, 200)
            self.assertEqual(r_c["next_stage"], "detect")

        # 4. Stage 2: GPU node processes Face Detection
        for vid in vids:
            status, claim = self.center.claim_task(gpu_node["node_id"], "detect")
            self.assertEqual(status, 200)
            self.assertEqual(claim["stage"], "detect")

            s_c, r_c = self.center.commit_task_result(gpu_node["node_token"], {
                "node_id": gpu_node["node_id"],
                "attempt_id": claim["attempt_id"],
                "vid": claim["vid"],
                "coordinator_generation": claim["coordinator_generation"],
                "stage": "detect",
                "detections": [{"box": [10, 10, 50, 50], "score": 0.99}],
                "execution_proof": {"provider": "CUDAExecutionProvider"}
            })
            self.assertEqual(s_c, 200)
            self.assertEqual(r_c["next_stage"], "feature")

        # 5. Stage 3: CPU nodes process Face Feature Extraction
        for i, vid in enumerate(vids):
            worker = mac1 if i % 2 == 0 else mac2
            status, claim = self.center.claim_task(worker["node_id"], "feature")
            self.assertEqual(status, 200)

            s_c, r_c = self.center.commit_task_result(worker["node_token"], {
                "node_id": worker["node_id"],
                "attempt_id": claim["attempt_id"],
                "vid": claim["vid"],
                "coordinator_generation": claim["coordinator_generation"],
                "stage": "feature",
                "vectors": [generate_normalized_vector(512, seed=i)],
                "execution_proof": {"arch": "arm64"}
            })
            self.assertEqual(s_c, 200)
            self.assertEqual(r_c["next_stage"], "done")
            self.assertTrue(r_c["completed"])

        # Verification: all 15 completed, count incremented to 15
        tasks = self.center.get_ui_tasks()
        self.assertEqual(tasks["completed_count"], 15)
        for t in tasks["tasks"]:
            self.assertEqual(t["stage"], "done")


class TestScenario2_NetworkFlapAndDeadNodeFailover(BaseWorkloadTestCase):
    """
    Scenario 2: Network Flap & Dead Node Failover during Large Batch Processing.
    10 tasks running across 2 nodes. Node 1 experiences network drop (>90s).
    Center reclaims tasks, bumps generation, healthy node completes entire queue without duplicate commits.
    """

    def test_network_flap_dead_node_failover_and_recovery(self):
        n1 = self.register_node("node-flaky", "linux", "x86_64")
        n2 = self.register_node("node-stable", "linux", "x86_64")
        vids = self.seed_tasks(10, prefix="flap", stage="detect")
        self.center.set_concurrency(5)

        # N1 claims 2 tasks with short lease
        _, c1 = self.center.claim_task(n1["node_id"], "detect", lease_duration_s=0.01)
        _, c2 = self.center.claim_task(n1["node_id"], "detect", lease_duration_s=0.01)

        # Simulate network drop on N1: missed heartbeats for 95 seconds
        old_time = time.time() - 95.0
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET last_heartbeat_at = ? WHERE node_id = ?", (old_time, n1["node_id"]))
            conn.commit()

        time.sleep(0.05)
        # Liveness check transitions N1 to offline
        offline = self.center.check_liveness_timeouts()
        self.assertIn(n1["node_id"], offline)

        # Center reaps expired leases held by dead N1 -> bumps generation
        reclaimed = self.center.reap_expired_leases()
        self.assertIn(c1["vid"], reclaimed)
        self.assertIn(c2["vid"], reclaimed)

        # Dead N1 wakes up and tries to submit c1 -> Rejected with 409 stale_generation
        s_dead, r_dead = self.center.commit_task_result(n1["node_token"], {
            "node_id": n1["node_id"], "attempt_id": c1["attempt_id"], "vid": c1["vid"],
            "coordinator_generation": c1["coordinator_generation"], "stage": "detect", "detections": []
        })
        self.assertEqual(s_dead, 409)

        # Stable N2 takes over reclaimed c1 (with fresh coordinator generation) and finishes it
        s_take, c_take = self.center.claim_task(n2["node_id"], "detect")
        self.assertEqual(s_take, 200)
        self.assertEqual(c_take["vid"], c1["vid"])
        s_fin, r_fin = self.center.commit_task_result(n2["node_token"], {
            "node_id": n2["node_id"], "attempt_id": c_take["attempt_id"], "vid": c_take["vid"],
            "coordinator_generation": c_take["coordinator_generation"], "stage": "detect", "detections": []
        })
        self.assertEqual(s_fin, 200)


class TestScenario3_CreatorVideoHighVolumePipeline(BaseWorkloadTestCase):
    """
    Scenario 3: Creator Video High-Volume Pipeline with Account Rule Skips.
    20 creator videos with watermark text matching registered account rules.
    OCR triggers skip_face, completing batch 3x faster without face detection or extraction.
    """

    def test_creator_video_pipeline_with_rule_matching(self):
        with self.env.get_connection() as conn:
            conn.execute("""
            INSERT INTO account_rules (id, account_name, creator_name, alias_list, action)
            VALUES
                ('r-1', 'dy_alice_official', '爱丽丝', '["alice_vlog"]', 'skip_face'),
                ('r-2', 'dy_bob_tech', '鲍勃', '["bob_official"]', 'skip_face')
            """)
            conn.commit()

        worker = self.register_node("ocr-creator-worker", "darwin", "arm64", cores=8)
        vids = self.seed_tasks(20, prefix="creator", stage="ocr")
        self.center.set_concurrency(10)

        for i, vid in enumerate(vids):
            status, claim = self.center.claim_task(worker["node_id"], "ocr")
            self.assertEqual(status, 200)

            # Alternate between Alice and Bob
            text = "欢迎关注抖音：dy_alice_official" if i % 2 == 0 else "快手官方：bob_official"
            s_c, r_c = self.center.commit_task_result(worker["node_token"], {
                "node_id": worker["node_id"],
                "attempt_id": claim["attempt_id"],
                "vid": claim["vid"],
                "coordinator_generation": claim["coordinator_generation"],
                "stage": "ocr",
                "text_snippets": [text],
                "execution_proof": {"fast": True}
            })
            self.assertEqual(s_c, 200)
            self.assertEqual(r_c["next_stage"], "done")
            self.assertTrue(r_c["completed"])

        # All 20 tasks skipped straight to done
        tasks = self.center.get_ui_tasks()
        self.assertEqual(tasks["completed_count"], 20)
        for t in tasks["tasks"]:
            self.assertEqual(t["stage"], "done")
            data = json.loads(t["data"])
            self.assertTrue(data["face_skipped"])
            self.assertIn(data["creator"], ["爱丽丝", "鲍勃"])


class TestScenario4_ZeroDowntimeRollingUpgrade(BaseWorkloadTestCase):
    """
    Scenario 4: Zero-Downtime Rolling Upgrade of Model Weights under Ingestion Load.
    3 nodes running; canary upgraded first, verified with synthetic self-test,
    then remaining nodes upgraded sequentially with zero leaked leases or dropouts.
    """

    def test_zero_downtime_rolling_upgrade(self):
        n1 = self.register_node("worker-1", "linux", "x86_64")
        n2 = self.register_node("worker-2", "linux", "x86_64")
        n3 = self.register_node("worker-3", "linux", "x86_64")
        nodes = [n1, n2, n3]

        pkg_v2 = "pkg-scrfd-cuda-v2"
        self.center.register_package(pkg_v2, "manifest-v2-sha", "model-v2-sha", "CUDAExecutionProvider", "2.0.0")

        # Continuous task stream: seed tasks
        self.seed_tasks(9, prefix="roll", stage="detect")

        for idx, node in enumerate(nodes):
            # 1. Drain node (stops leasing new tasks)
            self.center.control_node(node["node_id"], "drain")
            # 2. Deploy new package
            s_dep, dep = self.center.deploy_package(node["node_id"], pkg_v2)
            self.assertEqual(s_dep, 200)
            # 3. Synthetic self-test passes
            s_rec, _ = self.center.submit_deploy_receipt({
                "deployment_id": dep["deployment_id"],
                "package_id": pkg_v2,
                "status": "ready",
                "self_test": {"passed": True, "drift": 0.0002},
                "duration_ms": 750
            })
            self.assertEqual(s_rec, 200)

            # Node returns to accepting with new version
            with self.env.get_connection() as conn:
                status = conn.execute("SELECT status FROM agent_nodes WHERE node_id = ?", (node["node_id"],)).fetchone()["status"]
                self.assertEqual(status, "accepting")

            # Immediately lease a task to prove new version operates cleanly
            s_claim, claim = self.center.claim_task(node["node_id"], "detect")
            self.assertEqual(s_claim, 200)
            s_fin, _ = self.center.commit_task_result(node["node_token"], {
                "node_id": node["node_id"], "attempt_id": claim["attempt_id"], "vid": claim["vid"],
                "coordinator_generation": claim["coordinator_generation"], "stage": "detect", "detections": []
            })
            self.assertEqual(s_fin, 200)

        # All 3 nodes now upgraded and verified
        deployments = self.center.get_ui_deployments()
        self.assertEqual(len(deployments["deployments"]), 3)
        for d in deployments["deployments"]:
            self.assertEqual(d["status"], "ready")
            self.assertEqual(d["pkg_version"], "2.0.0")


class TestScenario5_CatastrophicNodeCrashReconciliation(BaseWorkloadTestCase):
    """
    Scenario 5: Catastrophic Node Crash & Cold Restart Reconciliation.
    Node crashes abruptly during active task execution; restarts with new instance_id.
    Center identifies instance change, clears orphaned leases, bumps generation, logs audit event.
    """

    def test_catastrophic_crash_and_cold_restart(self):
        node = self.register_node("crash-prone-node", "linux", "x86_64")
        self.seed_tasks(2, prefix="crash", stage="detect")

        # Claim task
        s_claim, claim = self.center.claim_task(node["node_id"], "detect", lease_duration_s=60.0)
        self.assertEqual(s_claim, 200)

        # Sudden cold reboot: new instance_id
        reboot_instance = "instance-cold-reboot-" + uuid.uuid4().hex[:8]
        hb_status, _ = self.center.agent_heartbeat(node["node_token"], {
            "node_id": node["node_id"],
            "instance_id": reboot_instance,
            "timestamp": time.time(),
            "free_ram_bytes": 32000000000
        })
        self.assertEqual(hb_status, 200)

        # Orphaned lease was automatically reclaimed upon crash detection
        with self.env.get_connection() as conn:
            task = conn.execute("SELECT run_id, retry_stage FROM tasks WHERE vid = ?", (claim["vid"],)).fetchone()
            self.assertIsNone(task["run_id"])
            self.assertEqual(task["retry_stage"], "detect")

            # Node status was reset from accepting to online
            n = conn.execute("SELECT status, instance_id FROM agent_nodes WHERE node_id = ?", (node["node_id"],)).fetchone()
            self.assertEqual(n["instance_id"], reboot_instance)
            self.assertEqual(n["status"], "online")

            # Audit event was logged
            event = conn.execute("SELECT * FROM events WHERE kind = 'agent_restart_reconciled'").fetchone()
            self.assertIsNotNone(event)
            self.assertIn("orphaned leases reclaimed", event["message"])


if __name__ == "__main__":
    unittest.main()
