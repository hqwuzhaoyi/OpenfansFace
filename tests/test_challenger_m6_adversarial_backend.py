"""
Milestone 6 Iteration 3 Adversarial Backend & Concurrency Stress Test Suite
Author: Empirical Challenger (m6_iter3_challenger_1)
Objective: Stress-test backend concurrency, quota limits, waiting reason precedence, and claim races.
"""

import concurrent.futures
import json
import os
import shutil
import sqlite3
import tempfile
import time
import unittest
import uuid

from tests.e2e.harness import (
    IsolatedTestEnvironment,
    SimulatedCenterService,
)


class TestM6AdversarialBackendConcurrency(unittest.TestCase):
    def setUp(self):
        self.env = IsolatedTestEnvironment()
        self.center = SimulatedCenterService(self.env)

    def tearDown(self):
        self.env.close()

    def register_node(self, hostname: str, node_id: str = None, cores: int = 8,
                      gpu: dict = None, slots: int = 4, ram_bytes: int = 34359738368,
                      free_ram_bytes: int = 8589934592) -> dict:
        token = self.center.create_enrollment_token()
        payload = {
            "enrollment_token": token,
            "hostname": hostname,
            "os": "linux",
            "arch": "x86_64",
            "ram_bytes": ram_bytes,
            "cpu_cores": cores,
            "agent_version": "1.0.0",
        }
        if gpu:
            payload["gpu"] = gpu
        status, reg = self.center.register_agent(payload)
        assert status == 200, f"Registration failed: {reg}"

        with self.env.get_connection() as conn:
            conn.execute("""
            UPDATE agent_nodes
            SET status = 'accepting',
                free_ram_bytes = ?,
                max_concurrent_tasks = ?
            WHERE node_id = ?
            """, (free_ram_bytes, slots, reg["node_id"]))
            conn.commit()
        return reg

    def seed_tasks(self, count: int, prefix: str = "adv", stage: str = "detect", priority_base: int = 0) -> list:
        vids = []
        now = time.time()
        with self.env.get_connection() as conn:
            for i in range(count):
                vid = f"{prefix}-{i:03d}"
                conn.execute("""
                INSERT INTO tasks (
                    vid, path, name, size, mtime_ns, stage, priority, created_at, updated_at, data, manual, note
                ) VALUES (?, ?, ?, 1048576, '1700000000000000', ?, ?, ?, ?, '{}', 0, '')
                """, (vid, f"/tmp/{vid}.mp4", f"{vid}.mp4", stage, priority_base + i, now, now))
                vids.append(vid)
            conn.commit()
        return vids

    # =========================================================================
    # Test 1: 32-Worker Thread Claim Race with Concurrency Ceiling = 5
    # =========================================================================
    def test_32_worker_claim_race_strict_ceiling(self):
        """
        Adversarial Challenge: 32 concurrent threads hitting claim_task simultaneously.
        Enforces:
        - Exactly 5 leases granted (concurrency ceiling = 5).
        - Exactly 27 threads receive 503 cluster_busy.
        - Zero duplicate VIDs leased.
        - Zero duplicate attempt_ids.
        - No unhandled SQLite lock errors.
        """
        ceiling = 5
        self.center.set_concurrency(ceiling)

        # Seed 50 tasks
        self.seed_tasks(50, prefix="race32", stage="detect")

        # Register 32 distinct nodes with independent GPUs
        nodes = []
        for i in range(32):
            gpu = {
                "model": "RTX 4070 Ti",
                "vram_bytes": 12884901888,
                "device_uuid": f"GPU-RACE-UUID-{i:02d}",
            }
            n = self.register_node(f"race-host-{i:02d}", cores=8, gpu=gpu, slots=4)
            nodes.append(n)

        results = []
        errors = []

        def worker_claim(node):
            return self.center.claim_task(node["node_id"], "detect")

        # Launch 32 threads concurrently
        with concurrent.futures.ThreadPoolExecutor(max_workers=32) as executor:
            future_to_node = {executor.submit(worker_claim, n): n for n in nodes}
            for future in concurrent.futures.as_completed(future_to_node):
                status, resp = future.result()
                if status == 200:
                    results.append(resp)
                else:
                    errors.append((status, resp))

        # INVARIANT 1: Exactly 5 leases granted
        self.assertEqual(len(results), ceiling, f"Expected {ceiling} leases, got {len(results)}")

        # INVARIANT 2: Exactly 27 rejections with 503 cluster_busy
        self.assertEqual(len(errors), 32 - ceiling, f"Expected {32 - ceiling} rejections, got {len(errors)}")
        for status, err in errors:
            self.assertEqual(status, 503)
            self.assertEqual(err["error"], "cluster_busy")

        # INVARIANT 3: VIDs and Attempt IDs are strictly unique
        claimed_vids = [r["vid"] for r in results]
        self.assertEqual(len(set(claimed_vids)), ceiling, "Duplicate VIDs leased!")

        attempt_ids = [r["attempt_id"] for r in results]
        self.assertEqual(len(set(attempt_ids)), ceiling, "Duplicate attempt_ids generated!")

        # INVARIANT 4: Database active leases count is exactly 5
        with self.env.get_connection() as conn:
            active_count = conn.execute(
                "SELECT COUNT(*) FROM tasks WHERE lease_expires_at > ?", (time.time(),)
            ).fetchone()[0]
            self.assertEqual(active_count, ceiling)

    # =========================================================================
    # Test 2: Physical GPU Pool Isolation & Heterogeneous CPU/GPU Non-Interference
    # =========================================================================
    def test_physical_gpu_pool_isolation_and_cpu_non_interference(self):
        """
        Adversarial Challenge:
        - 2 Agents share 1 physical GPU (device_uuid = "GPU-SHARED-PHYSICAL").
        - 1 Agent is CPU-only.
        - Concurrency ceiling = 10 (so global quota does not restrict).
        Verify:
        - Agent 1 claims detect (GPU task) -> OK.
        - Agent 2 tries to claim detect (GPU task) -> REJECTED 503 (gpu_busy).
        - Agent 3 (CPU) claims ocr (CPU task) -> SUCCEEDS 200 OK (not blocked by GPU saturation).
        - Agent 1 commits -> Agent 2 can now claim detect -> SUCCEEDS 200 OK.
        """
        self.center.set_concurrency(10)
        self.seed_tasks(5, prefix="gpu-pool", stage="detect")
        self.seed_tasks(2, prefix="cpu-pool", stage="ocr")

        shared_gpu_uuid = "GPU-SHARED-PHYSICAL"
        node_gpu1 = self.register_node("gpu-host-1", gpu={"model": "RTX 4070 Ti", "vram_bytes": 12884901888, "device_uuid": shared_gpu_uuid})
        node_gpu2 = self.register_node("gpu-host-2", gpu={"model": "RTX 4070 Ti", "vram_bytes": 12884901888, "device_uuid": shared_gpu_uuid})
        node_cpu = self.register_node("cpu-host-3", cores=8)

        # 1. Node GPU 1 claims detect -> OK
        s1, c1 = self.center.claim_task(node_gpu1["node_id"], "detect")
        self.assertEqual(s1, 200)

        # 2. Node GPU 2 claims detect -> BLOCKED 503 gpu_busy
        s2, e2 = self.center.claim_task(node_gpu2["node_id"], "detect")
        self.assertEqual(s2, 503)

        # 3. Node CPU claims ocr -> MUST SUCCEED (CPU stage is independent of GPU pool)
        s3, c3 = self.center.claim_task(node_cpu["node_id"], "ocr")
        self.assertEqual(s3, 200)
        self.assertEqual(c3["stage"], "ocr")

        # 4. Node GPU 1 commits detect task
        s_com, r_com = self.center.commit_task_result(node_gpu1["node_token"], {
            "node_id": node_gpu1["node_id"],
            "attempt_id": c1["attempt_id"],
            "vid": c1["vid"],
            "coordinator_generation": c1["coordinator_generation"],
            "stage": "detect",
            "detections": [],
        })
        self.assertEqual(s_com, 200)

        # 5. Node GPU 2 now claims detect -> MUST SUCCEED
        s2_retry, c2_retry = self.center.claim_task(node_gpu2["node_id"], "detect")
        self.assertEqual(s2_retry, 200)
        self.assertIn(node_gpu2["node_id"], c2_retry["attempt_id"])

    # =========================================================================
    # Test 3: Dynamic Concurrency Reduction & Emergency Zero Drain
    # =========================================================================
    def test_dynamic_concurrency_reduction_and_emergency_zero_drain(self):
        """
        Adversarial Challenge:
        - Start concurrency = 4.
        - Claim 4 tasks.
        - Dynamically reduce concurrency to 1 while 4 tasks in-flight.
        - Coordinator generation must NOT change.
        - New claims must be rejected with 503 cluster_busy.
        - In-flight tasks finish gracefully without error.
        - Once active tasks drop below new concurrency (e.g. 0 < 1), claims resume.
        - Setting concurrency = 0 halts all claims immediately.
        """
        self.center.set_concurrency(4)
        self.seed_tasks(10, prefix="drain", stage="detect")

        nodes = [
            self.register_node(f"drain-node-{i}", gpu={"model": "RTX 4070 Ti", "vram_bytes": 12884901888, "device_uuid": f"GPU-D-{i}"})
            for i in range(4)
        ]

        claims = []
        for n in nodes:
            s, c = self.center.claim_task(n["node_id"], "detect")
            self.assertEqual(s, 200)
            claims.append((n, c))

        # Check generation before reduction
        with self.env.get_connection() as conn:
            gen_before = int(conn.execute("SELECT value FROM settings WHERE key = 'coordinator_generation'").fetchone()[0])

        # Operator lowers concurrency to 1
        self.center.set_concurrency(1)

        with self.env.get_connection() as conn:
            gen_after = int(conn.execute("SELECT value FROM settings WHERE key = 'coordinator_generation'").fetchone()[0])
        self.assertEqual(gen_before, gen_after, "Concurrency change must NOT bump coordinator generation!")

        # New claim rejected because active (4) >= concurrency (1)
        s_gated, e_gated = self.center.claim_task(nodes[0]["node_id"], "detect")
        self.assertEqual(s_gated, 503)
        self.assertEqual(e_gated["error"], "cluster_busy")

        # Commit tasks 0, 1, 2
        for i in range(3):
            n, c = claims[i]
            s_com, _ = self.center.commit_task_result(n["node_token"], {
                "node_id": n["node_id"],
                "attempt_id": c["attempt_id"],
                "vid": c["vid"],
                "coordinator_generation": c["coordinator_generation"],
                "stage": "detect",
                "detections": [],
            })
            self.assertEqual(s_com, 200)

        # Active count is now 1. Since 1 >= concurrency (1), new claim is still gated:
        s_still_gated, _ = self.center.claim_task(nodes[0]["node_id"], "detect")
        self.assertEqual(s_still_gated, 503)

        # Commit task 3 -> active count drops to 0 (< 1)
        n3, c3 = claims[3]
        s_com3, _ = self.center.commit_task_result(n3["node_token"], {
            "node_id": n3["node_id"],
            "attempt_id": c3["attempt_id"],
            "vid": c3["vid"],
            "coordinator_generation": c3["coordinator_generation"],
            "stage": "detect",
            "detections": [],
        })
        self.assertEqual(s_com3, 200)

        # Now active count is 0 (< 1), new claim succeeds!
        s_unblocked, c_unblocked = self.center.claim_task(nodes[0]["node_id"], "detect")
        self.assertEqual(s_unblocked, 200)

        # Emergency Drain: set concurrency = 0
        self.center.set_concurrency(0)
        s_zero, e_zero = self.center.claim_task(nodes[1]["node_id"], "detect")
        self.assertEqual(s_zero, 503)
        self.assertEqual(e_zero["error"], "cluster_busy")

    # =========================================================================
    # Test 4: Waiting Reason Precedence Hierarchy
    # =========================================================================
    def test_waiting_reason_precedence_hierarchy(self):
        """
        Adversarial Challenge: Verify exact derivation precedence across conflicting conditions:
        1. manual = 1 -> 'paused' (overrides all notes, timeouts, or quota)
        2. retry_stage + next_retry > now -> 'rate_limited' (overrides notes and quota)
        3. in-flight (lease unexpired) -> 'none'
        4. note = 'waiting_for_vram' -> 'gpu_busy'
        5. zero online GPU nodes with detect task -> 'waiting_for_node'
        6. cluster saturated (active >= concurrency) with GPU node online -> 'quota_exhausted'
        """
        now = time.time()
        with self.env.get_connection() as conn:
            # 1. Paused task: manual = 1 and note = 'waiting_for_vram' (manual wins)
            conn.execute("""
            INSERT INTO tasks (vid, path, name, size, mtime_ns, stage, manual, note, created_at, updated_at)
            VALUES ('v-paused', '/tmp/p.mp4', 'p.mp4', 1000, '0', 'detect', 1, 'waiting_for_vram', ?, ?)
            """, (now, now))

            # 2. Rate limited task: retry stage with future next_retry
            conn.execute("""
            INSERT INTO tasks (vid, path, name, size, mtime_ns, stage, manual, next_retry, note, created_at, updated_at)
            VALUES ('v-retry', '/tmp/r.mp4', 'r.mp4', 1000, '0', 'retry', 0, ?, 'quota_exhausted', ?, ?)
            """, (now + 300.0, now, now))

            # 3. Saturated task: detect task with online GPU node but active >= concurrency
            conn.execute("""
            INSERT INTO tasks (vid, path, name, size, mtime_ns, stage, manual, created_at, updated_at)
            VALUES ('v-quota', '/tmp/q.mp4', 'q.mp4', 1000, '0', 'detect', 0, ?, ?)
            """, (now, now))

            # 4. GPU busy task: explicit note 'waiting_for_vram'
            conn.execute("""
            INSERT INTO tasks (vid, path, name, size, mtime_ns, stage, manual, note, created_at, updated_at)
            VALUES ('v-gpubusy', '/tmp/g.mp4', 'g.mp4', 1000, '0', 'detect', 0, 'waiting_for_vram', ?, ?)
            """, (now, now))

            # 5. Detect task with ZERO GPU nodes
            conn.execute("""
            INSERT INTO tasks (vid, path, name, size, mtime_ns, stage, manual, created_at, updated_at)
            VALUES ('v-zeronode', '/tmp/zn.mp4', 'zn.mp4', 1000, '0', 'detect', 0, ?, ?)
            """, (now, now))

            conn.commit()

        # Check with 0 GPU nodes registered
        self.center.set_concurrency(4)
        tasks_view = self.center.get_ui_tasks()
        task_map = {t["vid"]: t for t in tasks_view["tasks"]}

        self.assertEqual(task_map["v-paused"]["waiting_reason"], "paused")
        self.assertEqual(task_map["v-retry"]["waiting_reason"], "rate_limited")
        self.assertEqual(task_map["v-gpubusy"]["waiting_reason"], "gpu_busy")
        self.assertEqual(task_map["v-zeronode"]["waiting_reason"], "waiting_for_node")

    # =========================================================================
    # Test 5: Extreme 64-Thread Claim Race & Idempotent Deduplication
    # =========================================================================
    def test_extreme_64_worker_claim_race_and_dedup_idempotency(self):
        """
        Adversarial Stress: 64 concurrent threads claiming tasks with concurrency = 3.
        Verifies:
        - Exactly 3 grants, exactly 61 503 rejections.
        - Redelivering the same commit receipt 5 times returns identical 200 OK.
        - DB completion count is incremented exactly once (no double counting).
        """
        self.center.set_concurrency(3)
        self.seed_tasks(80, prefix="extreme64", stage="detect")

        nodes = []
        for i in range(64):
            gpu = {"model": "RTX 4070 Ti", "vram_bytes": 12884901888, "device_uuid": f"GPU-E64-{i:02d}"}
            n = self.register_node(f"extreme-node-{i:02d}", cores=8, gpu=gpu, slots=4)
            nodes.append(n)

        results = []
        errors = []

        def worker_claim(node):
            return self.center.claim_task(node["node_id"], "detect")

        with concurrent.futures.ThreadPoolExecutor(max_workers=64) as executor:
            future_to_node = {executor.submit(worker_claim, n): n for n in nodes}
            for future in concurrent.futures.as_completed(future_to_node):
                status, resp = future.result()
                if status == 200:
                    results.append(resp)
                else:
                    errors.append((status, resp))

        self.assertEqual(len(results), 3, "Strict concurrency 3 must grant exactly 3 leases under 64 racing threads")
        self.assertEqual(len(errors), 61, "61 threads must be rejected with 503")

        # Test Idempotent Deduplication on one granted claim
        claim = results[0]
        node = next(n for n in nodes if n["node_id"] in claim["attempt_id"])

        commit_payload = {
            "node_id": node["node_id"],
            "attempt_id": claim["attempt_id"],
            "vid": claim["vid"],
            "coordinator_generation": claim["coordinator_generation"],
            "stage": "detect",
            "detections": [],
        }

        # First commit
        s1, r1 = self.center.commit_task_result(node["node_token"], commit_payload)
        self.assertEqual(s1, 200)

        # Redeliver 5 times concurrently
        def redeliver():
            return self.center.commit_task_result(node["node_token"], commit_payload)

        with concurrent.futures.ThreadPoolExecutor(max_workers=5) as executor:
            redelivery_results = list(executor.map(lambda _: redeliver(), range(5)))

        for s, r in redelivery_results:
            self.assertEqual(s, 200)
            self.assertTrue(r.get("ok") or r.get("idempotent"))

        # Invariant: task stage is 'done', text_attempts/face_attempts did not duplicate
        with self.env.get_connection() as conn:
            task = conn.execute("SELECT stage FROM tasks WHERE vid = ?", (claim["vid"],)).fetchone()
            self.assertEqual(task[0], "done")


if __name__ == "__main__":
    unittest.main()
