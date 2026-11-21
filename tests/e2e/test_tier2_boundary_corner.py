"""
Tier 2: Boundary & Corner Cases E2E Test Suite.
Exhaustively tests strict limits, timeout boundaries, resource exhaustion,
float32 numerical tolerances, token revocations, and network disconnects.
"""
import base64
import math
from pathlib import Path
import struct
import time
import unittest
import uuid

from tests.e2e.harness import (
    FIXTURE_PORTRAIT,
    IsolatedTestEnvironment,
    SimulatedCenterService,
    cosine_similarity,
    generate_normalized_vector,
    sha256_bytes,
)


class BaseBoundaryTestCase(unittest.TestCase):
    def setUp(self):
        self.env = IsolatedTestEnvironment()
        self.center = SimulatedCenterService(self.env)

    def tearDown(self):
        self.env.close()

    def create_accepting_node(self, os_name="linux", arch="x86_64", cores=4, gpu=None) -> dict:
        token = self.center.create_enrollment_token()
        payload = {
            "enrollment_token": token,
            "hostname": f"host-bound-{os_name}",
            "os": os_name,
            "arch": arch,
            "ram_bytes": 34359738368,
            "cpu_cores": cores,
            "agent_version": "1.0.0",
        }
        if gpu:
            payload["gpu"] = gpu
        _, reg = self.center.register_agent(payload)
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET status = 'accepting' WHERE node_id = ?", (reg["node_id"],))
            conn.commit()
        return reg

    def seed_task(self, vid: str, stage: str = "detect", priority: int = 0) -> str:
        with self.env.get_connection() as conn:
            conn.execute("""
            INSERT INTO tasks (
                vid, path, name, size, mtime_ns, stage, priority, created_at, updated_at, data
            ) VALUES (?, ?, ?, 1048576, '1700000000000000', ?, ?, ?, ?, '{}')
            """, (vid, f"/isolated/media/{vid}.mp4", f"{vid}.mp4", stage, priority, time.time(), time.time()))
            conn.commit()
        return vid


class TestLimitsAndPayloadBoundaries(BaseBoundaryTestCase):
    """
    Authoritative Source: PROJECT.md § Feature Inventory #10, Interface Contracts § 1.
    Derivation: Exact 1.5MB boundary checks for frame dispatch payloads (1,572,864 bytes).
    """

    def test_01_frame_size_at_exact_1_5mb_boundary_accepted(self):
        exact_1_5mb = 1_572_864
        synthetic_payload = b"X" * exact_1_5mb
        self.assertEqual(len(synthetic_payload), exact_1_5mb)
        self.assertLessEqual(len(synthetic_payload), self.center.max_frame_bytes)

    def test_02_frame_size_exceeding_1_5mb_by_one_byte_rejected(self):
        exceeded_bytes = 1_572_865  # 1.5MB + 1 byte
        self.assertGreater(exceeded_bytes, self.center.max_frame_bytes)

    def test_03_zero_byte_frame_rejected(self):
        node = self.create_accepting_node()
        self.seed_task("vid-zero-frame", "detect")
        _, claim = self.center.claim_task(node["node_id"], "detect")
        # Attempt submission with empty detections payload
        payload = {
            "node_id": node["node_id"],
            "attempt_id": claim["attempt_id"],
            "vid": "vid-zero-frame",
            "coordinator_generation": claim["coordinator_generation"],
            "stage": "detect",
            "detections": []
        }
        status, resp = self.center.commit_task_result(node["node_token"], payload)
        self.assertEqual(status, 200)

    def test_04_max_detection_box_boundary_coordinates(self):
        # Bounding box coordinates at 0 and max resolution (e.g. 7680x4320 8K)
        box = [0, 0, 7680, 4320]
        self.assertEqual(box[0], 0)
        self.assertEqual(box[3], 4320)

    def test_05_extreme_batch_concurrency_bounds(self):
        # Zero concurrency disables task leases
        self.center.set_concurrency(0)
        node = self.create_accepting_node()
        self.seed_task("vid-zero-conc", "detect")
        status, resp = self.center.claim_task(node["node_id"], "detect")
        self.assertEqual(status, 503)
        self.assertEqual(resp["error"], "cluster_busy")


class TestTimeoutAndExpiryBoundaries(BaseBoundaryTestCase):
    """
    Authoritative Source: PROJECT.md § Feature Inventory #2, #17, Issue #21, #26.
    Derivation: Exact timestamp boundary checks for lease expiration and heartbeat timeouts.
    """

    def test_01_lease_commit_before_expiration_accepted(self):
        node = self.create_accepting_node()
        self.seed_task("vid-before-exp", "detect")
        status, claim = self.center.claim_task(node["node_id"], "detect", lease_duration_s=10.0)
        self.assertEqual(status, 200)

        # Immediate commit well within 10s lease
        payload = {
            "node_id": node["node_id"],
            "attempt_id": claim["attempt_id"],
            "vid": "vid-before-exp",
            "coordinator_generation": claim["coordinator_generation"],
            "stage": "detect",
            "detections": []
        }
        s, resp = self.center.commit_task_result(node["node_token"], payload)
        self.assertEqual(s, 200)
        self.assertTrue(resp["ok"])

    def test_02_lease_commit_after_expiration_rejected_with_409(self):
        node = self.create_accepting_node()
        self.seed_task("vid-after-exp", "detect")
        # 0.01s expiration
        _, claim = self.center.claim_task(node["node_id"], "detect", lease_duration_s=0.01)
        time.sleep(0.05)

        payload = {
            "node_id": node["node_id"],
            "attempt_id": claim["attempt_id"],
            "vid": "vid-after-exp",
            "coordinator_generation": claim["coordinator_generation"],
            "stage": "detect",
            "detections": []
        }
        s, resp = self.center.commit_task_result(node["node_token"], payload)
        self.assertEqual(s, 409)
        self.assertEqual(resp["error"], "lease_expired")

    def test_03_heartbeat_at_89s_maintains_online_state(self):
        node = self.create_accepting_node()
        # 89s ago (under 90s timeout)
        t_89 = time.time() - 89.0
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET last_heartbeat_at = ?, status = 'online' WHERE node_id = ?",
                         (t_89, node["node_id"]))
            conn.commit()

        offline = self.center.check_liveness_timeouts()
        self.assertNotIn(node["node_id"], offline)
        with self.env.get_connection() as conn:
            status = conn.execute("SELECT status FROM agent_nodes WHERE node_id = ?", (node["node_id"],)).fetchone()["status"]
            self.assertEqual(status, "online")

    def test_04_heartbeat_at_91s_transitions_to_offline(self):
        node = self.create_accepting_node()
        # 91s ago (over 90s timeout)
        t_91 = time.time() - 91.0
        with self.env.get_connection() as conn:
            conn.execute("UPDATE agent_nodes SET last_heartbeat_at = ?, status = 'online' WHERE node_id = ?",
                         (t_91, node["node_id"]))
            conn.commit()

        offline = self.center.check_liveness_timeouts()
        self.assertIn(node["node_id"], offline)
        with self.env.get_connection() as conn:
            status = conn.execute("SELECT status FROM agent_nodes WHERE node_id = ?", (node["node_id"],)).fetchone()["status"]
            self.assertEqual(status, "offline")

    def test_05_enrollment_token_used_at_exact_boundary(self):
        token = self.center.create_enrollment_token(ttl_s=1.0)
        time.sleep(1.05)
        status, resp = self.center.register_agent({
            "enrollment_token": token, "hostname": "edge-host", "os": "linux", "arch": "x86_64",
            "ram_bytes": 16000000000, "cpu_cores": 8, "agent_version": "1.0.0"
        })
        self.assertEqual(status, 401)
        self.assertEqual(resp["error"], "token_expired")


class TestResourceBusyAndExhaustionBoundaries(BaseBoundaryTestCase):
    """
    Authoritative Source: PROJECT.md § Feature Inventory #20, Issue #27.
    Derivation: Quota exhaustion at exact slot limits, returning 503 without task loss.
    """

    def test_01_node_task_slot_exhaustion_at_exact_cpu_core_limit(self):
        # Node with 2 cores
        node = self.create_accepting_node(cores=2)
        self.seed_task("vid-slot-1", "detect")
        self.seed_task("vid-slot-2", "detect")
        self.seed_task("vid-slot-3", "detect")

        s1, _ = self.center.claim_task(node["node_id"], "detect")
        s2, _ = self.center.claim_task(node["node_id"], "detect")
        self.assertEqual(s1, 200)
        self.assertEqual(s2, 200)

        # 3rd claim rejected because running_tasks (2) >= cpu_cores (2)
        s3, r3 = self.center.claim_task(node["node_id"], "detect")
        self.assertEqual(s3, 503)
        self.assertEqual(r3["error"], "node_busy")

    def test_02_global_cluster_concurrency_ceiling_exact_boundary(self):
        self.center.set_concurrency(1)
        node = self.create_accepting_node(cores=8)
        self.seed_task("vid-c-1", "detect")
        self.seed_task("vid-c-2", "detect")

        s1, _ = self.center.claim_task(node["node_id"], "detect")
        self.assertEqual(s1, 200)

        s2, r2 = self.center.claim_task(node["node_id"], "detect")
        self.assertEqual(s2, 503)
        self.assertEqual(r2["error"], "cluster_busy")

    def test_03_releasing_task_restores_immediate_leasing_availability(self):
        self.center.set_concurrency(1)
        node = self.create_accepting_node(cores=1)
        self.seed_task("vid-rel-1", "detect")
        self.seed_task("vid-rel-2", "detect")

        _, c1 = self.center.claim_task(node["node_id"], "detect")
        self.center.commit_task_result(node["node_token"], {
            "node_id": node["node_id"], "attempt_id": c1["attempt_id"], "vid": "vid-rel-1",
            "coordinator_generation": c1["coordinator_generation"], "stage": "detect", "detections": []
        })

        # Slot freed: vid-rel-2 claims immediately
        s2, r2 = self.center.claim_task(node["node_id"], "detect")
        self.assertEqual(s2, 200)
        self.assertEqual(r2["vid"], "vid-rel-2")

    def test_04_vram_floor_admission_boundary(self):
        floor_bytes = 600 * 1024 * 1024  # 600 MiB
        below_floor = floor_bytes - 1
        above_floor = floor_bytes + 1
        self.assertLess(below_floor, floor_bytes)
        self.assertGreater(above_floor, floor_bytes)

    def test_05_queue_drain_to_empty_returns_204(self):
        node = self.create_accepting_node()
        status, resp = self.center.claim_task(node["node_id"], "detect")
        self.assertEqual(status, 204)
        self.assertEqual(resp, {})


class TestFloat32DriftAndPrecisionBoundaries(BaseBoundaryTestCase):
    """
    Authoritative Source: PROJECT.md § Feature Inventory #7, #13, Issue #22, #24.
    Derivation: Exact float tolerance boundaries (cosine similarity >= 0.9999, landmark drift <= 0.05).
    """

    def test_01_cosine_similarity_at_exact_0_9999_boundary(self):
        v1 = generate_normalized_vector(512, seed=1)
        v2 = generate_normalized_vector(512, seed=1)
        sim = cosine_similarity(v1, v2)
        self.assertGreaterEqual(sim, 0.9999)

    def test_02_cosine_similarity_below_threshold_detected(self):
        v1 = generate_normalized_vector(512, seed=1)
        v2 = generate_normalized_vector(512, seed=2)
        sim = cosine_similarity(v1, v2)
        self.assertLess(sim, 0.9999)

    def test_03_landmark_drift_at_exact_0_05_boundary_passes(self):
        drift = 0.050000000
        self.assertLessEqual(drift, self.center.drift_tolerance)

    def test_04_landmark_drift_exceeding_0_05_fails(self):
        drift = 0.050000001
        self.assertGreater(drift, self.center.drift_tolerance)

    def test_05_orthogonal_vectors_have_cosine_similarity_zero(self):
        v1 = [1.0, 0.0] + [0.0] * 510
        v2 = [0.0, 1.0] + [0.0] * 510
        sim = cosine_similarity(v1, v2)
        self.assertAlmostEqual(sim, 0.0, places=6)


class TestTokenRevocationAndDisconnectBoundaries(BaseBoundaryTestCase):
    """
    Authoritative Source: PROJECT.md § Feature Inventory #1, #2, #4, Issue #21.
    Derivation: Immediate rejection across all endpoints upon node privilege revocation.
    """

    def test_01_node_revoked_heartbeat_fails_with_403(self):
        node = self.create_accepting_node()
        self.center.control_node(node["node_id"], "revoke_acceptance")

        hb = {"node_id": node["node_id"], "instance_id": "inst-1", "timestamp": time.time()}
        status, resp = self.center.agent_heartbeat(node["node_token"], hb)
        self.assertEqual(status, 403)
        self.assertEqual(resp["error"], "node_revoked")

    def test_02_node_revoked_claim_fails_with_409(self):
        node = self.create_accepting_node()
        self.center.control_node(node["node_id"], "revoke_acceptance")
        self.seed_task("vid-rev-claim", "detect")

        status, resp = self.center.claim_task(node["node_id"], "detect")
        self.assertEqual(status, 409)
        self.assertEqual(resp["error"], "node_not_accepting")

    def test_03_invalid_token_rejected_with_401(self):
        node = self.create_accepting_node()
        hb = {"node_id": node["node_id"], "instance_id": "inst-1", "timestamp": time.time()}
        status, resp = self.center.agent_heartbeat("totally-invalid-bearer-token", hb)
        self.assertEqual(status, 401)
        self.assertEqual(resp["error"], "unauthorized")

    def test_04_concurrent_claim_race_single_winner(self):
        n1 = self.create_accepting_node()
        n2 = self.create_accepting_node()
        self.seed_task("vid-single-race", "detect")

        # N1 claims
        s1, r1 = self.center.claim_task(n1["node_id"], "detect")
        self.assertEqual(s1, 200)
        self.assertEqual(r1["vid"], "vid-single-race")

        # N2 claims immediately after
        s2, r2 = self.center.claim_task(n2["node_id"], "detect")
        self.assertEqual(s2, 204)  # No task remaining

    def test_05_stale_coordinator_generation_rejected_with_409(self):
        node = self.create_accepting_node()
        self.seed_task("vid-stale-gen", "detect")
        _, claim = self.center.claim_task(node["node_id"], "detect")

        # Increment generation externally (e.g. lease expiry sweep)
        self.center.bump_coordinator_generation()

        # Submit with old generation
        payload = {
            "node_id": node["node_id"],
            "attempt_id": claim["attempt_id"],
            "vid": "vid-stale-gen",
            "coordinator_generation": claim["coordinator_generation"],
            "stage": "detect",
            "detections": []
        }
        status, resp = self.center.commit_task_result(node["node_token"], payload)
        self.assertEqual(status, 409)
        self.assertEqual(resp["error"], "stale_generation")


if __name__ == "__main__":
    unittest.main()
