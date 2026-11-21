"""
Adversarial Stress Test Suite for Milestone 6 (#27).
Multi-Device Concurrency & Task View Wiring (Features 20, 21, 22).

Test Classes:
1. TestAdversarialM6_MultiDeviceConcurrentClaims:
   - 16-thread race against 4-tier quota hierarchy, strict single lease per task.
   - Physical GPU resource pool deduplication under concurrent claim races.
2. TestAdversarialM6_GracefulConcurrencyReductionMidFlight:
   - Mid-flight dynamic concurrency reduction (6 -> 2): zero aborted in-flight tasks, dynamic 503 gating.
   - Emergency zero-concurrency drain without orphan leases.
3. TestAdversarialM6_TaskViewTruthInUIAndWaitingReasons:
   - Dynamic 5-state waiting reason state machine (waiting_for_node, quota_exhausted, gpu_busy, rate_limited, paused, none).
   - Filter combinations (node_id, waiting_reason, batch) and Truth-in-UI invariants (null size/duration, video_path).
4. TestAdversarialM6_CrossVideoPipelineInterleaving:
   - Heterogeneous cross-video pipeline streaming across 10 videos with on-demand face skip.
   - Strictly monotonic completed_count increment from 0 to 10.
"""

from concurrent.futures import ThreadPoolExecutor, as_completed
import time
import unittest
import uuid
from tests.e2e.harness import generate_normalized_vector
from tests.e2e.test_tier1_feature_coverage import BaseFeatureTestCase


class TestAdversarialM6_MultiDeviceConcurrentClaims(BaseFeatureTestCase):
    """
    Empirical multi-threaded stress testing for Feature 20 (Multi-Tier Quota Hierarchy).
    Validates TOCTOU-free immediate transaction locking and physical GPU pool deduplication.
    """

    def test_adv_16_thread_concurrent_claim_race(self):
        """
        Race 16 worker threads claiming tasks from a pool of 25 seeded tasks under global concurrency = 4.
        Verifies:
        - Active leased tasks at any millisecond never exceed 4.
        - Every leased task is claimed strictly once (zero duplicate claims).
        - Every granted lease receives unique attempt_id and matching coordinator generation.
        - Excess claims receive HTTP 503 cluster_busy or node_busy.
        """
        # 1. Seed 25 tasks
        seeded_vids = [self.seed_task(f"vid-race-{i}", stage="detect", priority=i) for i in range(25)]

        # 2. Configure global concurrency = 4
        self.center.set_concurrency(4)

        # 3. Register 8 nodes (each with distinct physical GPU device UUID to isolate device pool)
        nodes = []
        for i in range(8):
            n = self.create_and_register_node(
                gpu={"model": f"RTX 4070 Ti #{i}", "vram_bytes": 12884901888, "device_uuid": f"GPU-UNIQUE-{i}"}
            )
            nodes.append(n)

        # Mark all nodes accepting
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting'")
            conn.commit()

        # 4. Concurrently fire 32 claim attempts across 16 threads
        claimed_tasks = []
        rejected_count = 0

        def claim_worker(worker_idx: int):
            node = nodes[worker_idx % len(nodes)]
            return self.center.claim_task(node["node_id"], "detect", lease_duration_s=60.0)

        with ThreadPoolExecutor(max_workers=16) as executor:
            futures = [executor.submit(claim_worker, i) for i in range(32)]
            for fut in as_completed(futures):
                status, resp = fut.result()
                if status == 200:
                    claimed_tasks.append(resp)
                elif status == 503:
                    rejected_count += 1
                    self.assertIn(resp.get("error"), ("cluster_busy", "node_busy", "gpu_busy"))

        # 5. Invariants Verification
        # Invariant A: Granted leases count must be exactly 4 (since concurrency ceiling = 4)
        self.assertEqual(len(claimed_tasks), 4, f"Expected exactly 4 granted leases, got {len(claimed_tasks)}")
        self.assertGreaterEqual(rejected_count, 20)

        # Invariant B: Zero duplicate tasks leased (strict single lease invariant)
        claimed_vids = [t["vid"] for t in claimed_tasks]
        self.assertEqual(len(claimed_vids), len(set(claimed_vids)), "Duplicate task leased concurrently!")

        # Invariant C: Unique attempt IDs and matching coordinator generation
        coord_gen = self.center.get_coordinator_generation()
        attempt_ids = [t["attempt_id"] for t in claimed_tasks]
        self.assertEqual(len(attempt_ids), len(set(attempt_ids)), "Duplicate attempt_id generated!")
        for t in claimed_tasks:
            self.assertEqual(t["coordinator_generation"], coord_gen)

        # Invariant D: Center DB state confirms exactly 4 active unexpired leases
        now = time.time()
        with self.env.get_connection() as conn:
            active_cnt = conn.execute(
                "SELECT COUNT(*) as cnt FROM tasks WHERE lease_expires_at > ?", (now,)
            ).fetchone()["cnt"]
            self.assertEqual(active_cnt, 4)

    def test_adv_physical_gpu_deduplication_under_high_concurrency(self):
        """
        Verify Physical GPU Pooling (Tier 3 quota):
        Multiple agents sharing the same physical GPU (e.g. WSL host + Docker container sharing an RTX 4070 Ti)
        cannot run concurrent CUDA detect tasks simultaneously, even when global cluster concurrency is 10.
        """
        gpu_uuid = "GPU-RTX4070TI-SHARED-POOL"
        node1 = self.create_and_register_node(gpu={"model": "RTX 4070 Ti", "vram_bytes": 12884901888, "device_uuid": gpu_uuid})
        node2 = self.create_and_register_node(gpu={"model": "RTX 4070 Ti", "vram_bytes": 12884901888, "device_uuid": gpu_uuid})
        node3 = self.create_and_register_node(gpu={"model": "RTX 4070 Ti", "vram_bytes": 12884901888, "device_uuid": gpu_uuid})

        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE device_uuid = ?", (gpu_uuid,))
            conn.commit()

        # Ample global concurrency
        self.center.set_concurrency(10)

        # Seed 5 detect tasks
        for i in range(5):
            self.seed_task(f"vid-gpu-pool-{i}", stage="detect")

        # Concurrently claim tasks on all 3 GPU nodes
        results = []
        with ThreadPoolExecutor(max_workers=3) as executor:
            f1 = executor.submit(self.center.claim_task, node1["node_id"], "detect")
            f2 = executor.submit(self.center.claim_task, node2["node_id"], "detect")
            f3 = executor.submit(self.center.claim_task, node3["node_id"], "detect")
            results = [f.result() for f in (f1, f2, f3)]

        statuses = [r[0] for r in results]
        # Exactly one node succeeds (200), other two are blocked by GPU pool quota (503 gpu_busy)
        self.assertEqual(statuses.count(200), 1, f"Expected exactly 1 GPU lease granted, got {statuses}")
        self.assertEqual(statuses.count(503), 2, f"Expected 2 nodes blocked by physical GPU pool, got {statuses}")

        for s, r in results:
            if s == 503:
                self.assertIn(r.get("error"), ("gpu_busy", "cluster_busy"))

        # Independent CPU task proceeds without blocking
        cpu_node = self.create_and_register_node(os_name="linux", arch="x86_64")
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (cpu_node["node_id"],))
            conn.commit()

        self.seed_task("vid-cpu-ocr-1", stage="ocr")
        cpu_status, cpu_claim = self.center.claim_task(cpu_node["node_id"], "ocr")
        self.assertEqual(cpu_status, 200, "Non-GPU CPU task should not be blocked by GPU pool!")


class TestAdversarialM6_GracefulConcurrencyReductionMidFlight(BaseFeatureTestCase):
    """
    Empirical testing for Feature 21 (Graceful Concurrency Reduction).
    Validates dynamic throttling, zero in-flight task cancellations, and emergency zero-concurrency drain.
    """

    def test_adv_midflight_dynamic_concurrency_throttling(self):
        """
        Scale concurrency down from 6 to 2 while 6 tasks are in-flight across 6 nodes.
        Verifies:
        - In-flight tasks T1..T6 complete with 200 without interruption or 409 stale generation.
        - Intermediate new claims return HTTP 503 cluster_busy until in-flight tasks drop below 2.
        - Next claim succeeds immediately once active tasks drop to 1.
        """
        # Configure initial concurrency = 6
        self.center.set_concurrency(6)

        # Register 6 nodes with distinct physical GPUs
        nodes = []
        for i in range(6):
            n = self.create_and_register_node(gpu={"model": f"GPU-{i}", "vram_bytes": 8589934592, "device_uuid": f"DEV-{i}"})
            nodes.append(n)

        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting', cpu_cores = 8")
            conn.commit()

        # Seed and claim 6 tasks
        claims = []
        for i in range(6):
            vid = self.seed_task(f"vid-flight-{i}", stage="detect")
            node = nodes[i]
            status, claim = self.center.claim_task(node["node_id"], "detect", lease_duration_s=120.0)
            self.assertEqual(status, 200)
            claims.append((node, claim))

        gen_before = self.center.get_coordinator_generation()

        # Dynamic scaling down: Operator reduces concurrency from 6 to 2
        set_res = self.center.set_concurrency(2)
        self.assertEqual(set_res["concurrency"], 2)

        # Coordinator generation must NOT be bumped by concurrency scaling
        self.assertEqual(self.center.get_coordinator_generation(), gen_before)

        # New claim attempt must immediately receive 503 cluster_busy (active 6 >= concurrency 2)
        self.seed_task("vid-blocked-1", stage="detect")
        status, resp = self.center.claim_task(nodes[0]["node_id"], "detect")
        self.assertEqual(status, 503)
        self.assertEqual(resp["error"], "cluster_busy")

        # Complete T0 -> active count becomes 5 (still > 2)
        node0, claim0 = claims[0]
        payload0 = {
            "node_id": node0["node_id"],
            "attempt_id": claim0["attempt_id"],
            "vid": claim0["vid"],
            "coordinator_generation": claim0["coordinator_generation"],
            "stage": "detect",
            "detections": []
        }
        s0, _ = self.center.commit_task_result(node0["node_token"], payload0)
        self.assertEqual(s0, 200, "In-flight task should commit cleanly despite lower concurrency!")

        # New claim must still be blocked (active 5 >= 2)
        status, _ = self.center.claim_task(nodes[0]["node_id"], "detect")
        self.assertEqual(status, 503)

        # Complete T1, T2, T3 -> active count becomes 2 (still == 2)
        for i in (1, 2, 3):
            n, c = claims[i]
            p = {
                "node_id": n["node_id"],
                "attempt_id": c["attempt_id"],
                "vid": c["vid"],
                "coordinator_generation": c["coordinator_generation"],
                "stage": "detect",
                "detections": []
            }
            s, _ = self.center.commit_task_result(n["node_token"], p)
            self.assertEqual(s, 200)

        # At exactly active = 2, claims must still be gated
        status, _ = self.center.claim_task(nodes[0]["node_id"], "detect")
        self.assertEqual(status, 503)

        # Complete T4 -> active count drops to 1 (< 2)!
        n4, c4 = claims[4]
        p4 = {
            "node_id": n4["node_id"],
            "attempt_id": c4["attempt_id"],
            "vid": c4["vid"],
            "coordinator_generation": c4["coordinator_generation"],
            "stage": "detect",
            "detections": []
        }
        s4, _ = self.center.commit_task_result(n4["node_token"], p4)
        self.assertEqual(s4, 200)

        # Now new claim must SUCCEED!
        status_unblocked, claim_unblocked = self.center.claim_task(nodes[0]["node_id"], "detect")
        self.assertEqual(status_unblocked, 200)
        self.assertEqual(claim_unblocked["vid"], "vid-blocked-1")

    def test_adv_emergency_zero_concurrency_drain(self):
        """
        Verify setting concurrency = 0 completely halts cluster leasing without leaking tasks.
        """
        node = self.create_and_register_node(
            gpu={"model": "NVIDIA GeForce RTX 4070 Ti", "vram_bytes": 12884901888, "device_uuid": "GPU-DRAIN-1"}
        )
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (node["node_id"],))
            conn.commit()

        self.seed_task("vid-drain-1", "detect")

        # Set concurrency = 0
        self.center.set_concurrency(0)
        status, resp = self.center.claim_task(node["node_id"], "detect")
        self.assertEqual(status, 503)
        self.assertEqual(resp["error"], "cluster_busy")

        # Restore concurrency = 4 -> leasing resumes immediately
        self.center.set_concurrency(4)
        status, claim = self.center.claim_task(node["node_id"], "detect")
        self.assertEqual(status, 200)
        self.assertEqual(claim["vid"], "vid-drain-1")


class TestAdversarialM6_TaskViewTruthInUIAndWaitingReasons(BaseFeatureTestCase):
    """
    Testing for Feature 22 (Operations Task View Wiring).
    Validates dynamic 5-state waiting reason derivation, Truth-in-UI metrics, and filter combinations.
    """

    def test_adv_dynamic_waiting_reason_state_machine(self):
        """
        Verifies dynamic waiting reason derivation across all operational states:
        - waiting_for_node: detect task queued with no CUDA GPU nodes online
        - none: task actively leased and running
        - quota_exhausted: unleased task when active leases >= concurrency
        - gpu_busy: unleased detect task when GPU node's physical device pool is busy
        - rate_limited: task in retry backoff window
        - paused: task manually taken over by operator
        """
        now = time.time()

        # 1. waiting_for_node: detect task with 0 GPU nodes
        self.seed_task("vid-wait-node", stage="detect")
        ui_res = self.center.get_ui_tasks()
        task_wn = next(t for t in ui_res["tasks"] if t["vid"] == "vid-wait-node")
        self.assertEqual(task_wn["waiting_reason"], "waiting_for_node")

        # 2. none: task running under active lease
        gpu_node = self.create_and_register_node(gpu={"model": "RTX 4070 Ti", "vram_bytes": 12884901888, "device_uuid": "DEV-TRUTH-1"})
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (gpu_node["node_id"],))
            conn.commit()

        self.center.set_concurrency(1)
        s, claim = self.center.claim_task(gpu_node["node_id"], "detect", lease_duration_s=60.0)
        self.assertEqual(s, 200)

        ui_res = self.center.get_ui_tasks()
        task_running = next(t for t in ui_res["tasks"] if t["vid"] == "vid-wait-node")
        self.assertEqual(task_running["waiting_reason"], "none")

        # 3. quota_exhausted: second task queued while concurrency = 1 is saturated
        self.seed_task("vid-wait-quota", stage="detect")
        ui_res = self.center.get_ui_tasks()
        task_quota = next(t for t in ui_res["tasks"] if t["vid"] == "vid-wait-quota")
        self.assertEqual(task_quota["waiting_reason"], "quota_exhausted")

        # 4. gpu_busy: explicit vram/gpu contention note
        self.seed_task("vid-wait-gpu", stage="detect")
        with self.env.get_connection() as conn:
            conn.execute("UPDATE tasks SET note = 'waiting_for_vram' WHERE vid = 'vid-wait-gpu'")
            conn.commit()
        ui_res = self.center.get_ui_tasks()
        task_gpu = next(t for t in ui_res["tasks"] if t["vid"] == "vid-wait-gpu")
        self.assertEqual(task_gpu["waiting_reason"], "gpu_busy")

        # 5. rate_limited: task in retry stage with future backoff
        self.seed_task("vid-wait-retry", stage="retry")
        with self.env.get_connection() as conn:
            conn.execute("UPDATE tasks SET next_retry = ? WHERE vid = 'vid-wait-retry'", (now + 120.0,))
            conn.commit()
        ui_res = self.center.get_ui_tasks()
        task_retry = next(t for t in ui_res["tasks"] if t["vid"] == "vid-wait-retry")
        self.assertEqual(task_retry["waiting_reason"], "rate_limited")

        # 6. paused: task with manual = 1
        self.seed_task("vid-wait-paused", stage="detect")
        with self.env.get_connection() as conn:
            conn.execute("UPDATE tasks SET manual = 1 WHERE vid = 'vid-wait-paused'")
            conn.commit()
        ui_res = self.center.get_ui_tasks()
        task_paused = next(t for t in ui_res["tasks"] if t["vid"] == "vid-wait-paused")
        self.assertEqual(task_paused["waiting_reason"], "paused")

    def test_adv_filter_combinations_and_truth_in_ui_metrics(self):
        """
        Verify:
        - Filtering by node_id returns only tasks leased to that node.
        - Filtering by waiting_reason returns only tasks matching that state.
        - Null video size and duration are preserved as None (rendered as '未知', never 0).
        - video_path is populated and accessible.
        """
        gpu_node = self.create_and_register_node(gpu={"model": "RTX 4070 Ti", "vram_bytes": 12884901888, "device_uuid": "DEV-TRUTH-2"})
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (gpu_node["node_id"],))
            conn.commit()

        self.seed_task("vid-flt-leased", stage="detect")
        self.seed_task("vid-flt-paused", stage="detect")
        with self.env.get_connection() as conn:
            conn.execute("UPDATE tasks SET manual = 1 WHERE vid = 'vid-flt-paused'")
            conn.commit()

        self.center.claim_task(gpu_node["node_id"], "detect")

        # Filter by node_id
        res_node = self.center.get_ui_tasks(node_id=gpu_node["node_id"])
        self.assertEqual(res_node["total"], 1)
        self.assertEqual(res_node["tasks"][0]["vid"], "vid-flt-leased")

        # Filter by waiting_reason
        res_paused = self.center.get_ui_tasks(waiting_reason="paused")
        self.assertEqual(res_paused["total"], 1)
        self.assertEqual(res_paused["tasks"][0]["vid"], "vid-flt-paused")

        # Truth-in-UI metrics check: Seed task with null duration and 0 size
        self.seed_task("vid-truth-nulls", stage="detect")
        with self.env.get_connection() as conn:
            conn.execute("UPDATE tasks SET size = 0, duration = NULL WHERE vid = 'vid-truth-nulls'")
            conn.commit()

        res_truth = self.center.get_ui_tasks()
        task_nulls = next(t for t in res_truth["tasks"] if t["vid"] == "vid-truth-nulls")
        self.assertIsNone(task_nulls["size"], "Zero size must be normalized to None for Truth-in-UI '未知'")
        self.assertIsNone(task_nulls["duration"], "Null duration must remain None")
        self.assertTrue(task_nulls["video_path"], "video_path must be populated")


class TestAdversarialM6_CrossVideoPipelineInterleaving(BaseFeatureTestCase):
    """
    Testing for cross-video pipeline coordination.
    Streams 10 videos across OCR -> Detect -> Feature stages with on-demand face skip,
    asserting monotonic completed_count progression and deadlock-free execution.
    """

    def test_adv_heterogeneous_10_video_pipeline_streaming(self):
        """
        10 videos streamed across 3 heterogeneous nodes (OCR, Detect GPU, Feature CPU):
        - Videos 2 and 5 trigger watermark account match -> skips face analysis entirely.
        - Other 8 videos complete full OCR -> Detect -> Feature lifecycle.
        - Monotonic completed_count invariant: strictly progresses from 0 to 10.
        """
        self.center.set_concurrency(8)

        # Register nodes: 1 CPU node for OCR, 1 GPU node for Detect, 1 CPU node for Feature
        ocr_node = self.create_and_register_node(os_name="linux", arch="x86_64")
        gpu_node = self.create_and_register_node(gpu={"model": "RTX 4070 Ti", "vram_bytes": 12884901888, "device_uuid": "DEV-STREAM-GPU"})
        feat_node = self.create_and_register_node(os_name="darwin", arch="arm64")

        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting'")
            conn.commit()

        # Seed 10 videos in OCR stage
        vids = [self.seed_task(f"vid-stream-{i}", stage="ocr", priority=10 - i) for i in range(10)]

        completed_observed = 0

        # Process all 10 videos through pipeline
        for i, vid in enumerate(vids):
            # Stage 1: Remote OCR
            s_ocr, c_ocr = self.center.claim_task(ocr_node["node_id"], "ocr")
            self.assertEqual(s_ocr, 200)

            # Videos 2 and 5 match creator watermark rule
            if i in (2, 5):
                p_ocr = {
                    "node_id": ocr_node["node_id"],
                    "attempt_id": c_ocr["attempt_id"],
                    "vid": c_ocr["vid"],
                    "coordinator_generation": c_ocr["coordinator_generation"],
                    "stage": "ocr",
                    "text_snippets": ["Creator: @authorized_creator"],
                    "creator": "@authorized_creator",
                    "face_skipped": True,
                    "skip_reason": "account_rule_match"
                }
                s_res, _ = self.center.commit_task_result(ocr_node["node_token"], p_ocr)
                self.assertEqual(s_res, 200)
                completed_observed += 1
                self.assertEqual(int(self.center._get_setting("completed_count", 0)), completed_observed)
                continue

            # Regular OCR commit -> progresses to detect
            p_ocr = {
                "node_id": ocr_node["node_id"],
                "attempt_id": c_ocr["attempt_id"],
                "vid": c_ocr["vid"],
                "coordinator_generation": c_ocr["coordinator_generation"],
                "stage": "ocr",
                "text_snippets": ["Ordinary watermark text"],
                "face_skipped": False
            }
            s_res, _ = self.center.commit_task_result(ocr_node["node_token"], p_ocr)
            self.assertEqual(s_res, 200)

            # Stage 2: CUDA Face Detect on GPU node
            s_det, c_det = self.center.claim_task(gpu_node["node_id"], "detect")
            self.assertEqual(s_det, 200)

            p_det = {
                "node_id": gpu_node["node_id"],
                "attempt_id": c_det["attempt_id"],
                "vid": c_det["vid"],
                "coordinator_generation": c_det["coordinator_generation"],
                "stage": "detect",
                "detections": [{
                    "bbox": [100, 100, 250, 250],
                    "score": 0.98,
                    "landmarks": [[120, 140], [180, 140], [150, 170], [130, 200], [170, 200]]
                }]
            }
            s_det_res, _ = self.center.commit_task_result(gpu_node["node_token"], p_det)
            self.assertEqual(s_det_res, 200)

            # Stage 3: Remote CPU Feature Extraction on Mac ARM64 node
            s_feat, c_feat = self.center.claim_task(feat_node["node_id"], "feature")
            self.assertEqual(s_feat, 200)

            embedding = generate_normalized_vector(dim=512, seed=i)
            p_feat = {
                "node_id": feat_node["node_id"],
                "attempt_id": c_feat["attempt_id"],
                "vid": c_feat["vid"],
                "coordinator_generation": c_feat["coordinator_generation"],
                "stage": "feature",
                "vectors": [embedding]
            }
            s_feat_res, _ = self.center.commit_task_result(feat_node["node_token"], p_feat)
            self.assertEqual(s_feat_res, 200)
            completed_observed += 1

            # Invariant: monotonic counter strictly follows pipeline completions
            self.assertEqual(int(self.center._get_setting("completed_count", 0)), completed_observed)

        # Final Verification: All 10 tasks completed
        self.assertEqual(completed_observed, 10)
        with self.env.get_connection() as conn:
            done_cnt = conn.execute("SELECT COUNT(*) as cnt FROM tasks WHERE stage = 'done'").fetchone()["cnt"]
            self.assertEqual(done_cnt, 10)


if __name__ == "__main__":
    unittest.main()
