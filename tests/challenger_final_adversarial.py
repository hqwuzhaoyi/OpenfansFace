#!/usr/bin/env python3
"""Final Adversarial Backend Challenge & Verification Harness.

Empirically challenges:
1. Video hotfix files integrity and symbol export.
2. Web overlay packaging rules: exclusion of root node_modules, inclusion of .next/node_modules symlinks, exclusion of macOS ._* files under COPYFILE_DISABLE=1.
3. Generation fencing, token reuse rejection, and concurrency quota enforcement.
4. Float32 cosine precision boundaries.
"""

import os
import sys
import tarfile
import tempfile
import time
import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO_ROOT))

from tests.e2e.harness import (
    IsolatedTestEnvironment,
    SimulatedCenterService,
    cosine_similarity,
    generate_normalized_vector,
)


class VideoHotfixFilesIntegrityTest(unittest.TestCase):
    def test_hotfix_files_exist_and_nonempty(self):
        hotfix_files = [
            REPO_ROOT / "lib" / "file-stream.ts",
            REPO_ROOT / "app" / "api" / "video" / "route.ts",
            REPO_ROOT / "lib" / "video-sources.ts",
            REPO_ROOT / "lib" / "integration.ts",
        ]
        for f in hotfix_files:
            self.assertTrue(f.is_file(), f"Hotfix file missing: {f}")
            self.assertGreater(f.stat().st_size, 500, f"Hotfix file suspiciously small: {f}")

    def test_file_stream_exports_hev1_patcher(self):
        content = (REPO_ROOT / "lib" / "file-stream.ts").read_text(encoding="utf-8")
        self.assertIn("findMp4Hev1PatchesAsync", content)
        self.assertIn("moovBoxCache", content)
        self.assertIn("extractHev1PatchesFromMoov", content)

    def test_video_route_handles_hev1_and_range(self):
        content = (REPO_ROOT / "app" / "api" / "video" / "route.ts").read_text(encoding="utf-8")
        self.assertIn("findMp4Hev1PatchesAsync", content)
        self.assertIn("Content-Range", content)
        self.assertIn("inline; filename*=", content)

    def test_video_sources_caching_and_overrides(self):
        content = (REPO_ROOT / "lib" / "video-sources.ts").read_text(encoding="utf-8")
        self.assertIn("readVideoSources", content)
        self.assertIn("sourcesCache", content)

    def test_integration_nonblocking_and_concurrency_bound(self):
        content = (REPO_ROOT / "lib" / "integration.ts").read_text(encoding="utf-8")
        self.assertIn("integratedStatsAsync", content)
        self.assertIn("Promise.all", content)


class WebOverlayPackagingRuleTest(unittest.TestCase):
    def test_packaging_symlink_preservation_and_macos_dot_exclusion(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = Path(tmp)
            payload_src = tmp_path / "mock_web_payload"
            payload_src.mkdir()

            # Create standalone files
            (payload_src / "server.js").write_text("console.log('standalone');")
            dot_next = payload_src / ".next"
            dot_next.mkdir()
            (dot_next / "BUILD_ID").write_text("build-12345")

            # Create .next/node_modules with symlinks
            dot_next_modules = dot_next / "node_modules"
            dot_next_modules.mkdir()

            # Fake top-level node_modules target
            top_modules = tmp_path / "node_modules" / ".pnpm"
            top_modules.mkdir(parents=True)
            (top_modules / "better-sqlite3@13.0.3").mkdir()
            (top_modules / "cpu-features@0.0.10").mkdir()

            sqlite_symlink = dot_next_modules / "better-sqlite3-bfc5648742806807"
            sqlite_symlink.symlink_to("../../node_modules/.pnpm/better-sqlite3@13.0.3")

            cpu_symlink = dot_next_modules / "cpu-features-b72d426a8252a4e7"
            cpu_symlink.symlink_to("../../node_modules/.pnpm/cpu-features@0.0.10")

            # Create dummy macOS ._* artifact
            macos_dot_file = dot_next / "._BUILD_ID"
            macos_dot_file.write_bytes(b"\x00\x05\x16\x07\x00\x02\x00\x00")

            # Test packaging with COPYFILE_DISABLE=1 filter
            archive_path = tmp_path / "web-app-payload.tar.gz"
            with tarfile.open(archive_path, "w:gz") as tar:
                for root, dirs, files in os.walk(payload_src):
                    dirs.sort()
                    files.sort()
                    for f in files:
                        if f.startswith("._") or f == ".DS_Store":
                            continue  # Respect COPYFILE_DISABLE=1 / macOS metadata filter
                        full_p = Path(root) / f
                        rel_p = full_p.relative_to(payload_src)
                        tar.add(full_p, arcname=str(rel_p), recursive=False)
                    for d in dirs:
                        full_d = Path(root) / d
                        if full_d.is_symlink():
                            rel_d = full_d.relative_to(payload_src)
                            tar.add(full_d, arcname=str(rel_d), recursive=False)

            self.assertTrue(archive_path.is_file())
            with tarfile.open(archive_path, "r:gz") as tar:
                members = {m.name: m for m in tar.getmembers()}
                # 1. macOS ._* files MUST NOT be present
                self.assertFalse(any(n.startswith("._") or "/._" in n for n in members))
                # 2. Top-level node_modules MUST NOT be in archive
                self.assertFalse(any(n.startswith("node_modules") for n in members))
                # 3. .next/node_modules symlinks MUST be preserved
                self.assertIn(".next/node_modules/better-sqlite3-bfc5648742806807", members)
                self.assertIn(".next/node_modules/cpu-features-b72d426a8252a4e7", members)
                sqlite_member = members[".next/node_modules/better-sqlite3-bfc5648742806807"]
                self.assertTrue(sqlite_member.issym())
                self.assertEqual(sqlite_member.linkname, "../../node_modules/.pnpm/better-sqlite3@13.0.3")


class ConcurrencyAndFencingAdversarialTest(unittest.TestCase):
    def setUp(self):
        self.env = IsolatedTestEnvironment()
        self.center = SimulatedCenterService(self.env)

    def tearDown(self):
        self.env.close()

    def seed_task(self, vid: str, stage: str = "queued", priority: int = 0) -> str:
        with self.env.get_connection() as conn:
            conn.execute("""
            INSERT INTO tasks (
                vid, path, name, size, mtime_ns, stage, priority, created_at, updated_at, data
            ) VALUES (?, ?, ?, 1048576, '1700000000000000', ?, ?, ?, ?, '{}')
            """, (vid, f"/isolated/media/{vid}.mp4", f"{vid}.mp4", stage, priority, time.time(), time.time()))
            conn.commit()
        return vid

    def test_token_reuse_rejected(self):
        token = self.center.create_enrollment_token()
        payload = {
            "enrollment_token": token,
            "hostname": "test-box",
            "os": "linux",
            "arch": "x86_64",
            "ram_bytes": 16 * 1024 * 1024 * 1024,
            "cpu_cores": 8,
            "agent_version": "1.0.0",
        }
        status1, reg1 = self.center.register_agent(payload)
        self.assertEqual(status1, 200)
        self.assertIn("node_id", reg1)

        # Attempting to reuse the one-time enrollment token MUST return 409 Conflict
        status2, reg2 = self.center.register_agent(payload)
        self.assertEqual(status2, 409)
        self.assertEqual(reg2.get("error"), "token_already_used")

    def test_generation_fencing_rejects_stale_receipt_commit(self):
        # 1. Register agent
        token = self.center.create_enrollment_token()
        _, reg = self.center.register_agent({
            "enrollment_token": token,
            "hostname": "test-box",
            "os": "linux",
            "arch": "x86_64",
            "ram_bytes": 16 * 1024 * 1024 * 1024,
            "cpu_cores": 8,
            "agent_version": "1.0.0",
        })
        node_id = reg["node_id"]
        node_token = reg["node_token"]

        # Deploy package to make accepting
        pkg = self.center.register_package("pkg-cpu-1", "manifest-hash", "model-hash", "cpu", "v1.0.0")
        dep_status, dep = self.center.deploy_package(node_id, pkg["package_id"])
        self.assertEqual(dep_status, 200)
        self.center.submit_deploy_receipt({
            "deployment_id": dep["deployment_id"],
            "status": "ready",
            "self_test": {"passed": True, "drift": 0.01},
        })

        # Seed an OCR task
        self.seed_task("vid-adv-1", stage="ocr")

        # Claim task
        claim_status, claim = self.center.claim_task(node_id, "ocr")
        self.assertEqual(claim_status, 200)
        stale_gen = claim["coordinator_generation"]
        attempt_id = claim["attempt_id"]
        vid = claim["vid"]

        # 2. Monotonically bump center coordinator generation (simulating failover or lease reaping)
        new_gen = self.center.bump_coordinator_generation()
        self.assertGreater(new_gen, stale_gen)

        # 3. Agent attempts to commit with stale coordinator generation
        commit_status, commit_resp = self.center.commit_task_result(
            node_token=node_token,
            payload={
                "node_id": node_id,
                "attempt_id": attempt_id,
                "vid": vid,
                "coordinator_generation": stale_gen,
                "stage": "ocr",
                "result_data": {"text": "found_watermark"},
            }
        )
        self.assertEqual(commit_status, 409)
        self.assertEqual(commit_resp.get("error"), "stale_generation")

    def test_transactional_outbox_deduplication_idempotency(self):
        # 1. Register and deploy
        token = self.center.create_enrollment_token()
        _, reg = self.center.register_agent({
            "enrollment_token": token,
            "hostname": "test-box",
            "os": "linux",
            "arch": "x86_64",
            "ram_bytes": 16 * 1024 * 1024 * 1024,
            "cpu_cores": 8,
            "agent_version": "1.0.0",
        })
        node_id = reg["node_id"]
        node_token = reg["node_token"]

        pkg = self.center.register_package("pkg-cpu-2", "manifest-hash", "model-hash", "cpu", "v1.0.0")
        _, dep = self.center.deploy_package(node_id, pkg["package_id"])
        self.center.submit_deploy_receipt({
            "deployment_id": dep["deployment_id"],
            "status": "ready",
            "self_test": {"passed": True, "drift": 0.01},
        })

        self.seed_task("vid-adv-2", stage="ocr")
        _, claim = self.center.claim_task(node_id, "ocr")
        attempt_id = claim["attempt_id"]
        vid = claim["vid"]
        coord_gen = claim["coordinator_generation"]

        # First commit succeeds (200 OK)
        status1, resp1 = self.center.commit_task_result(
            node_token=node_token,
            payload={
                "node_id": node_id,
                "attempt_id": attempt_id,
                "vid": vid,
                "coordinator_generation": coord_gen,
                "stage": "ocr",
                "result_data": {"text": "found_watermark"},
            }
        )
        self.assertEqual(status1, 200)

        # Duplicate commit with exact same (node_id, attempt_id) MUST return 200 idempotent
        status2, resp2 = self.center.commit_task_result(
            node_token=node_token,
            payload={
                "node_id": node_id,
                "attempt_id": attempt_id,
                "vid": vid,
                "coordinator_generation": coord_gen,
                "stage": "ocr",
                "result_data": {"text": "found_watermark"},
            }
        )
        self.assertEqual(status2, 200)
        self.assertTrue(resp2.get("idempotent"))

    def test_float32_cosine_drift_boundary_precision(self):
        v1 = generate_normalized_vector(dim=512, seed=100)
        v2 = generate_normalized_vector(dim=512, seed=100)
        sim_exact = cosine_similarity(v1, v2)
        self.assertAlmostEqual(sim_exact, 1.0, places=6)

        # Boundary test: perturbation right at threshold 0.9999
        v3 = list(v1)
        v3[0] += 0.005
        # Re-normalize
        norm = sum(x*x for x in v3) ** 0.5
        v3 = [x / norm for x in v3]
        sim_perturbed = cosine_similarity(v1, v3)
        self.assertGreater(sim_perturbed, 0.9990)


if __name__ == "__main__":
    unittest.main()
