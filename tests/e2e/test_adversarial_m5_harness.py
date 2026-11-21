"""
Adversarial empirical stress tests for Milestone 5 (#26).
Covers:
- Feature 17: Generation Fencing & Lease Expiry (sub-second lease, stale gen, expired lease, manual takeover)
- Feature 18: Crash Reconciliation Isolation & Re-dispatch (peer isolation, retry vs error on exhausted attempts)
- Feature 19: Outbox Idempotence & Monotonic Counters (dedup caching, strictly once completed_count)
"""

import time
import unittest
import uuid
from concurrent.futures import ThreadPoolExecutor
from tests.e2e.harness import generate_normalized_vector
from tests.e2e.test_tier1_feature_coverage import BaseFeatureTestCase


class TestAdversarialM5_Feature17_GenerationFencingLeaseExpiry(BaseFeatureTestCase):
    def test_adv_subsecond_lease_expiry_and_monotonic_generation_bump(self):
        reg = self.create_and_register_node()
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (reg["node_id"],))
            conn.commit()

        self.seed_task("vid-adv-sub-1", "detect")
        gen_before = self.center.get_coordinator_generation()

        # Claim with 0.01s lease
        status, claim = self.center.claim_task(reg["node_id"], "detect", lease_duration_s=0.01)
        self.assertEqual(status, 200)

        # Before expiration: reap should find 0 expired
        reaped_early = self.center.reap_expired_leases()
        self.assertNotIn("vid-adv-sub-1", reaped_early)
        self.assertEqual(self.center.get_coordinator_generation(), gen_before)

        # Wait 30ms for lease to expire
        time.sleep(0.03)

        # Now reap: must reclaim task and bump coordinator generation
        reaped = self.center.reap_expired_leases()
        self.assertIn("vid-adv-sub-1", reaped)
        gen_after = self.center.get_coordinator_generation()
        self.assertEqual(gen_after, gen_before + 1)

        # Verify task lease is cleared
        with self.env.get_connection() as conn:
            row = conn.execute("SELECT run_id, lease_expires_at FROM tasks WHERE vid = 'vid-adv-sub-1'").fetchone()
            self.assertIsNone(row["run_id"])
            self.assertIsNone(row["lease_expires_at"])

    def test_adv_late_commit_with_stale_generation_rejected_409(self):
        reg = self.create_and_register_node()
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (reg["node_id"],))
            conn.commit()

        self.seed_task("vid-adv-stale-1", "detect")
        _, claim = self.center.claim_task(reg["node_id"], "detect", lease_duration_s=60.0)

        # Center generation is bumped (e.g. by another lease expiration)
        self.seed_task("vid-other-exp", "detect")
        self.center.claim_task(reg["node_id"], "detect", lease_duration_s=0.01)
        time.sleep(0.03)
        self.center.reap_expired_leases()

        # Submit commit with the stale generation
        payload = {
            "node_id": reg["node_id"],
            "attempt_id": claim["attempt_id"],
            "vid": "vid-adv-stale-1",
            "coordinator_generation": claim["coordinator_generation"],
            "stage": "detect",
            "detections": []
        }
        status, resp = self.center.commit_task_result(reg["node_token"], payload)
        self.assertEqual(status, 409)
        self.assertEqual(resp["error"], "stale_generation")

    def test_adv_late_commit_with_expired_lease_rejected_409(self):
        reg = self.create_and_register_node()
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (reg["node_id"],))
            conn.commit()

        self.seed_task("vid-adv-lease-exp", "detect")
        _, claim = self.center.claim_task(reg["node_id"], "detect", lease_duration_s=0.02)

        # We DO NOT reap. Generation is NOT bumped. But time passes past lease_expires_at.
        time.sleep(0.04)

        payload = {
            "node_id": reg["node_id"],
            "attempt_id": claim["attempt_id"],
            "vid": "vid-adv-lease-exp",
            "coordinator_generation": claim["coordinator_generation"],
            "stage": "detect",
            "detections": []
        }
        status, resp = self.center.commit_task_result(reg["node_token"], payload)
        self.assertEqual(status, 409)
        self.assertEqual(resp["error"], "lease_expired")

    def test_adv_manual_takeover_rejects_commit_and_preserves_task(self):
        reg = self.create_and_register_node()
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (reg["node_id"],))
            conn.commit()

        self.seed_task("vid-adv-manual", "detect")
        _, claim = self.center.claim_task(reg["node_id"], "detect", lease_duration_s=60.0)

        # Human operator marks task manual = 1
        with self.env.get_connection() as conn:
            conn.execute("UPDATE tasks SET manual = 1 WHERE vid = 'vid-adv-manual'")
            conn.commit()

        payload = {
            "node_id": reg["node_id"],
            "attempt_id": claim["attempt_id"],
            "vid": "vid-adv-manual",
            "coordinator_generation": claim["coordinator_generation"],
            "stage": "detect",
            "detections": []
        }
        status, resp = self.center.commit_task_result(reg["node_token"], payload)
        self.assertEqual(status, 409)
        self.assertEqual(resp["error"], "manual_takeover")

        # Task in DB must remain manual = 1, unmodified
        with self.env.get_connection() as conn:
            task = conn.execute("SELECT manual, stage FROM tasks WHERE vid = 'vid-adv-manual'").fetchone()
            self.assertEqual(task["manual"], 1)
            self.assertEqual(task["stage"], "detect")


class TestAdversarialM5_Feature18_AgentCrashReconciliationPeerIsolation(BaseFeatureTestCase):
    def test_adv_crash_reconciliation_isolates_peers_and_redispatches(self):
        node_a = self.create_and_register_node(os_name="linux", arch="x86_64")
        node_b = self.create_and_register_node(os_name="linux", arch="x86_64")
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id IN (?, ?)",
                         (node_a["node_id"], node_b["node_id"]))
            conn.commit()

        self.seed_task("vid-task-a", "detect")
        self.seed_task("vid-task-b", "detect")

        # Node A claims task A, Node B claims task B
        s_a, claim_a = self.center.claim_task(node_a["node_id"], "detect", lease_duration_s=120.0)
        s_b, claim_b = self.center.claim_task(node_b["node_id"], "detect", lease_duration_s=120.0)
        self.assertEqual(s_a, 200)
        self.assertEqual(s_b, 200)

        gen_before = self.center.get_coordinator_generation()

        # Node A crashes! Reboots with new instance ID
        new_inst_a = "inst-reboot-" + uuid.uuid4().hex[:6]
        hb_status, hb_resp = self.center.agent_heartbeat(node_a["node_token"], {
            "node_id": node_a["node_id"],
            "instance_id": new_inst_a,
            "timestamp": time.time(),
            "running_tasks": 0,
        })
        self.assertEqual(hb_status, 200)

        # Peer Isolation Invariant 1: Coordinator generation MUST NOT change due to single-node crash
        gen_after = self.center.get_coordinator_generation()
        self.assertEqual(gen_after, gen_before, "Single node crash must NOT bump cluster generation")

        # Peer Isolation Invariant 2: Node B's task lease MUST remain active and valid
        with self.env.get_connection() as conn:
            task_b = conn.execute("SELECT run_id, stage FROM tasks WHERE vid = 'vid-task-b'").fetchone()
            self.assertEqual(task_b["run_id"], claim_b["attempt_id"])
            self.assertEqual(task_b["stage"], "detect")

        # Peer Isolation Invariant 3: Node B commits successfully
        payload_b = {
            "node_id": node_b["node_id"],
            "attempt_id": claim_b["attempt_id"],
            "vid": "vid-task-b",
            "coordinator_generation": claim_b["coordinator_generation"],
            "stage": "detect",
            "detections": []
        }
        commit_b_status, commit_b_resp = self.center.commit_task_result(node_b["node_token"], payload_b)
        self.assertEqual(commit_b_status, 200)
        self.assertTrue(commit_b_resp["ok"])

        # Re-dispatch Invariant: Task A reclaimed from Node A can now be claimed by Node B immediately
        status_reclaim, claim_reclaim = self.center.claim_task(node_b["node_id"], "detect")
        self.assertEqual(status_reclaim, 200)
        self.assertEqual(claim_reclaim["vid"], "vid-task-a")


class TestAdversarialM5_Feature19_OutboxIdempotenceMonotonicCounters(BaseFeatureTestCase):
    def test_adv_repeated_commits_idempotent_and_completion_count_monotonic(self):
        reg = self.create_and_register_node()
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (reg["node_id"],))
            conn.commit()

        self.seed_task("vid-adv-mono-cnt", "feature")
        _, claim = self.center.claim_task(reg["node_id"], "feature")

        payload = {
            "node_id": reg["node_id"],
            "attempt_id": claim["attempt_id"],
            "vid": "vid-adv-mono-cnt",
            "coordinator_generation": claim["coordinator_generation"],
            "stage": "feature",
            "vectors": [generate_normalized_vector(512, 1)]
        }

        # Initial commit
        s1, r1 = self.center.commit_task_result(reg["node_token"], payload)
        self.assertEqual(s1, 200)
        self.assertFalse(r1.get("idempotent", False))
        self.assertTrue(r1.get("completed", False))

        # Repeated commit 2
        s2, r2 = self.center.commit_task_result(reg["node_token"], payload)
        self.assertEqual(s2, 200)
        self.assertTrue(r2.get("idempotent", False))

        # Repeated commit 3
        s3, r3 = self.center.commit_task_result(reg["node_token"], payload)
        self.assertEqual(s3, 200)
        self.assertTrue(r3.get("idempotent", False))

        # Invariant: completed_count is strictly 1
        with self.env.get_connection() as conn:
            cnt = conn.execute("SELECT value FROM settings WHERE key = 'completed_count'").fetchone()["value"]
            self.assertEqual(cnt, "1", "Completed count must strictly equal 1")


if __name__ == "__main__":
    unittest.main()
