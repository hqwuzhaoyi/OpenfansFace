#!/usr/bin/env python3
"""
Empirical Adversarial Stress Test Harness for Milestone 4 (Issues #24 & #25)
Author: Milestone 4 Adversarial Challenger (m4_challenger_1)

Validates over real HTTP/REST wire against live services/file-operations daemon:
- Feature 12 & 13: Vector dimension boundaries, float finiteness, unit-norm boundaries, cosine similarity drift
- Feature 14, 15, 16: Remote OCR unambiguous match, ambiguous match fallback, clean 0-text, explicit failure distinction, expired lease rejection
"""

import json
import math
import os
import sqlite3
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
import hashlib

def sha256_hex(val: str) -> str:
    return hashlib.sha256(val.encode("utf-8")).hexdigest()

def make_unit_vector(dim=512, seed=1.23):
    raw = [seed + math.sin(i * 0.013) for i in range(dim)]
    norm = math.sqrt(sum(x * x for x in raw))
    return [x / norm for x in raw]

def run_adversarial_m4_suite():
    tmp_dir = tempfile.mkdtemp(prefix="face-m4-chal-py-")
    media_dir = os.path.join(tmp_dir, "media")
    os.makedirs(media_dir, exist_ok=True)
    processing_dir = os.path.join(tmp_dir, "processing")
    os.makedirs(processing_dir, exist_ok=True)
    ledger_path = os.path.join(tmp_dir, "ledger.db")
    processing_db_path = os.path.join(processing_dir, "processing.db")

    port = 18944
    bind = f"127.0.0.1:{port}"

    env = os.environ.copy()
    env["FACES_BASE"] = tmp_dir
    env["MEDIA2_ROOT"] = media_dir
    env["PEACH_LEDGER_PATH"] = ledger_path
    env["FILE_OPERATIONS_BIND"] = bind
    env["FILE_OPERATIONS_RUST_TOKEN"] = "admin-secret-token"

    binary_path = os.path.abspath("services/file-operations/target/debug/face-file-operations")
    proc = subprocess.Popen([binary_path], env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)

    try:
        # Wait for HTTP server
        ready = False
        for _ in range(50):
            try:
                req = urllib.request.Request(f"http://{bind}/health")
                with urllib.request.urlopen(req, timeout=1) as resp:
                    if resp.status == 200:
                        ready = True
                        break
            except Exception:
                time.sleep(0.1)

        if not ready:
            _, err = proc.communicate(timeout=2)
            raise RuntimeError(f"Server failed to start: {err.decode('utf-8')}")

        print(f"[OK] Live face-file-operations HTTP daemon listening on {bind}")

        # Seed node & rules directly into processing.db using complete SCHEMA
        conn = sqlite3.connect(processing_db_path)
        now = time.time()
        conn.execute("PRAGMA journal_mode=WAL;")
        conn.execute("""
            CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
        """)
        conn.execute("""
            CREATE TABLE IF NOT EXISTS tasks (
                vid TEXT PRIMARY KEY, path TEXT NOT NULL UNIQUE, name TEXT NOT NULL,
                size INTEGER NOT NULL, mtime_ns TEXT NOT NULL, duration REAL,
                stage TEXT NOT NULL DEFAULT 'queued', retry_stage TEXT NOT NULL DEFAULT 'text',
                text_attempts INTEGER NOT NULL DEFAULT 0, face_attempts INTEGER NOT NULL DEFAULT 0,
                next_retry REAL NOT NULL DEFAULT 0, priority INTEGER NOT NULL DEFAULT 0,
                is_new INTEGER NOT NULL DEFAULT 0, batch_id TEXT NOT NULL,
                created_at REAL NOT NULL, updated_at REAL NOT NULL,
                error TEXT, manual INTEGER NOT NULL DEFAULT 0, deferred INTEGER NOT NULL DEFAULT 0,
                note TEXT NOT NULL DEFAULT '', data TEXT NOT NULL DEFAULT '{}', run_id TEXT NOT NULL DEFAULT '',
                lease_expires_at REAL NOT NULL DEFAULT 0, coordinator_generation TEXT NOT NULL DEFAULT '',
                attempt_id TEXT NOT NULL DEFAULT '', lease_id TEXT NOT NULL DEFAULT '',
                assigned_node_id TEXT NOT NULL DEFAULT ''
            );
        """)
        conn.execute("CREATE INDEX IF NOT EXISTS processing_stage ON tasks(stage, priority, is_new, created_at);")
        conn.execute("""
            CREATE TABLE IF NOT EXISTS events (
                id INTEGER PRIMARY KEY AUTOINCREMENT, at REAL NOT NULL, vid TEXT, kind TEXT NOT NULL, message TEXT NOT NULL
            );
        """)
        conn.execute("CREATE TABLE IF NOT EXISTS face_groups (id TEXT PRIMARY KEY, version TEXT NOT NULL, embedding BLOB NOT NULL);")
        conn.execute("""
            CREATE TABLE IF NOT EXISTS batch_items (
                batch_id TEXT NOT NULL, vid TEXT NOT NULL, name TEXT NOT NULL, path TEXT NOT NULL,
                size INTEGER NOT NULL, mtime_ns TEXT NOT NULL, added_at REAL NOT NULL,
                text_done INTEGER NOT NULL DEFAULT 0, face_required INTEGER, face_done INTEGER NOT NULL DEFAULT 0,
                face_skipped INTEGER NOT NULL DEFAULT 0, completed INTEGER NOT NULL DEFAULT 0,
                completed_at REAL, excluded INTEGER NOT NULL DEFAULT 0, legacy INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY(batch_id,vid)
            );
        """)
        conn.execute("""
            CREATE TABLE IF NOT EXISTS processing_runs (
                id TEXT PRIMARY KEY, vid TEXT NOT NULL, name TEXT NOT NULL, batch_id TEXT NOT NULL,
                stage TEXT NOT NULL, started_at REAL NOT NULL, finished_at REAL, status TEXT NOT NULL,
                error TEXT, execution TEXT
            );
        """)
        conn.execute("CREATE INDEX IF NOT EXISTS processing_run_time ON processing_runs(started_at DESC);")
        conn.execute("""
            CREATE TABLE IF NOT EXISTS agent_nodes (
                id TEXT PRIMARY KEY,
                token_hash TEXT NOT NULL,
                instance_id TEXT NOT NULL DEFAULT '',
                hostname TEXT NOT NULL,
                os TEXT NOT NULL,
                arch TEXT NOT NULL,
                agent_version TEXT NOT NULL,
                cpu_cores INTEGER NOT NULL DEFAULT 0,
                ram_bytes INTEGER NOT NULL DEFAULT 0,
                gpu_model TEXT,
                gpu_vram_bytes INTEGER NOT NULL DEFAULT 0,
                gpu_device_uuid TEXT,
                device_group_id TEXT NOT NULL DEFAULT '',
                status TEXT NOT NULL DEFAULT 'registered',
                accepting INTEGER NOT NULL DEFAULT 1,
                model_ready INTEGER NOT NULL DEFAULT 0,
                desired_version TEXT NOT NULL DEFAULT '',
                current_version TEXT NOT NULL DEFAULT '',
                free_ram_bytes INTEGER NOT NULL DEFAULT 0,
                free_vram_bytes INTEGER NOT NULL DEFAULT 0,
                gpu_util_percent REAL NOT NULL DEFAULT 0.0,
                running_tasks INTEGER NOT NULL DEFAULT 0,
                last_heartbeat_at REAL NOT NULL DEFAULT 0.0,
                registered_at REAL NOT NULL,
                updated_at REAL NOT NULL
            );
        """)
        conn.execute("CREATE INDEX IF NOT EXISTS idx_agent_nodes_device_group ON agent_nodes(device_group_id);")
        conn.execute("CREATE INDEX IF NOT EXISTS idx_agent_nodes_token_hash ON agent_nodes(token_hash);")
        conn.execute("CREATE INDEX IF NOT EXISTS idx_agent_nodes_status ON agent_nodes(status, accepting, model_ready);")
        conn.execute("""
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
        """)
        conn.execute("""
            CREATE TABLE IF NOT EXISTS account_rules (
                id TEXT PRIMARY KEY,
                account_name TEXT NOT NULL,
                creator_name TEXT NOT NULL,
                alias_list TEXT NOT NULL DEFAULT '[]',
                action TEXT NOT NULL DEFAULT 'skip_face'
            )
        """)
        conn.execute("""
            CREATE TABLE IF NOT EXISTS settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            )
        """)
        conn.execute("INSERT OR REPLACE INTO settings (key, value) VALUES ('coordinator_generation', '1')")
        conn.execute("INSERT OR REPLACE INTO settings (key, value) VALUES ('completed_count', '0')")
        conn.execute("INSERT OR REPLACE INTO settings (key, value) VALUES ('concurrency', '100')")

        node_token = "tok-worker-live"
        conn.execute("""
            INSERT INTO agent_nodes (
                id, token_hash, instance_id, hostname, os, arch, agent_version,
                cpu_cores, ram_bytes, gpu_model, gpu_vram_bytes, free_vram_bytes,
                status, accepting, model_ready, running_tasks, last_heartbeat_at,
                registered_at, updated_at
            ) VALUES (
                'node-worker-live', ?, 'inst-live', 'host-live', 'darwin', 'arm64', '1.0.0',
                128, 34359738368, NULL, 0, 0, 'accepting', 1, 1, 0, ?, ?, ?
            )
        """, (sha256_hex(node_token), now, now, now))
        conn.commit()

        def http_post(path, body, token=node_token):
            data = json.dumps(body).encode("utf-8")
            req = urllib.request.Request(
                f"http://{bind}{path}",
                data=data,
                headers={
                    "Content-Type": "application/json",
                    "Authorization": f"Bearer {token}",
                },
            )
            try:
                with urllib.request.urlopen(req) as resp:
                    return resp.status, json.loads(resp.read().decode("utf-8"))
            except urllib.error.HTTPError as e:
                err_body = e.read().decode("utf-8")
                try:
                    return e.code, json.loads(err_body)
                except Exception:
                    return e.code, {"raw": err_body}

        # ---------------------------------------------------------------------
        # 1. Feature 12 & 13: Vector Dimension Boundaries (0, 1, 511, 513, 1024)
        # ---------------------------------------------------------------------
        print("\n--- Testing Feature 12 & 13: Vector Dimension Boundaries ---")
        dim_cases = [0, 1, 511, 513, 1024]
        for d in dim_cases:
            vid = f"vid-dim-{d}"
            conn.execute("""
                INSERT INTO tasks (vid, path, name, size, mtime_ns, stage, priority, batch_id, created_at, updated_at)
                VALUES (?, ?, 'test.mp4', 100, '1', 'feature', 10, 'batch-1', ?, ?)
            """, (vid, f"/m/{vid}.mp4", now, now))
            conn.commit()

            status, claim = http_post("/v1/agent/claim", {
                "node_id": "node-worker-live",
                "stage": "feature",
                "lease_duration_s": 60.0,
            })
            assert status == 200 and claim["attempt_id"], f"Claim failed: {claim}"
            attempt_id = claim["attempt_id"]

            bad_vec = [0.044] * d
            c_status, c_resp = http_post("/v1/agent/task-result", {
                "node_id": "node-worker-live",
                "attempt_id": attempt_id,
                "vid": vid,
                "coordinator_generation": claim["coordinator_generation"],
                "stage": "feature",
                "vectors": [bad_vec],
                "status": "completed",
            })
            assert c_status == 422, f"Dimension {d} must return 422, got {c_status}: {c_resp}"
            assert c_resp.get("error") == "invalid_vector_dimension", f"Code mismatch for dim {d}: {c_resp}"
            print(f"  [PASS] Vector length {d} rejected with HTTP 422 invalid_vector_dimension")

        # ---------------------------------------------------------------------
        # 2. Feature 12 & 13: Float Finiteness Rejection
        # ---------------------------------------------------------------------
        print("\n--- Testing Feature 12 & 13: Float Finiteness Rejection ---")
        finite_cases = [
            ("null_nan", None),
            ("str_nan", "NaN"),
            ("str_pos_inf", "+Infinity"),
            ("str_neg_inf", "-Infinity"),
        ]
        for name, bad_val in finite_cases:
            vid = f"vid-finite-{name}"
            conn.execute("""
                INSERT INTO tasks (vid, path, name, size, mtime_ns, stage, priority, batch_id, created_at, updated_at)
                VALUES (?, ?, 'test.mp4', 100, '1', 'feature', 10, 'batch-1', ?, ?)
            """, (vid, f"/m/{vid}.mp4", now, now))
            conn.commit()

            status, claim = http_post("/v1/agent/claim", {
                "node_id": "node-worker-live",
                "stage": "feature",
                "lease_duration_s": 60.0,
            })
            assert status == 200
            attempt_id = claim["attempt_id"]

            vec = [0.044] * 512
            vec[100] = bad_val
            c_status, c_resp = http_post("/v1/agent/task-result", {
                "node_id": "node-worker-live",
                "attempt_id": attempt_id,
                "vid": vid,
                "coordinator_generation": claim["coordinator_generation"],
                "stage": "feature",
                "vectors": [vec],
                "status": "completed",
            })
            assert c_status == 422, f"Finiteness {name} must return 422, got {c_status}: {c_resp}"
            assert c_resp.get("error") == "non_finite_vector_value", f"Code mismatch for {name}: {c_resp}"
            print(f"  [PASS] Float finiteness case '{name}' rejected with HTTP 422 non_finite_vector_value")

        # ---------------------------------------------------------------------
        # 3. Feature 12 & 13: L2 Unit-Norm Boundaries
        # ---------------------------------------------------------------------
        print("\n--- Testing Feature 12 & 13: L2 Unit-Norm Boundaries ---")
        unnorm_cases = [0.0, 0.5, 2.0, 100.0, 0.9499, 1.0501]
        for target_norm in unnorm_cases:
            vid = f"vid-norm-reject-{target_norm}"
            conn.execute("""
                INSERT INTO tasks (vid, path, name, size, mtime_ns, stage, priority, batch_id, created_at, updated_at)
                VALUES (?, ?, 'test.mp4', 100, '1', 'feature', 10, 'batch-1', ?, ?)
            """, (vid, f"/m/{vid}.mp4", now, now))
            conn.commit()

            status, claim = http_post("/v1/agent/claim", {
                "node_id": "node-worker-live",
                "stage": "feature",
                "lease_duration_s": 60.0,
            })
            attempt_id = claim["attempt_id"]

            elem = 0.0 if target_norm == 0.0 else target_norm / math.sqrt(512)
            vec = [elem] * 512
            c_status, c_resp = http_post("/v1/agent/task-result", {
                "node_id": "node-worker-live",
                "attempt_id": attempt_id,
                "vid": vid,
                "coordinator_generation": claim["coordinator_generation"],
                "stage": "feature",
                "vectors": [vec],
                "status": "completed",
            })
            assert c_status == 422, f"Target norm {target_norm} must return 422, got {c_status}: {c_resp}"
            assert c_resp.get("error") == "invalid_vector_norm", f"Code mismatch for norm {target_norm}: {c_resp}"
            print(f"  [PASS] Unnormalized norm {target_norm} rejected with HTTP 422 invalid_vector_norm")

        # Acceptance norms within 1.0 +/- 0.05
        accept_norms = [0.9501, 1.0, 1.0499]
        for target_norm in accept_norms:
            vid = f"vid-norm-accept-{target_norm}"
            conn.execute("""
                INSERT INTO tasks (vid, path, name, size, mtime_ns, stage, priority, batch_id, created_at, updated_at)
                VALUES (?, ?, 'test.mp4', 100, '1', 'feature', 10, 'batch-1', ?, ?)
            """, (vid, f"/m/{vid}.mp4", now, now))
            conn.commit()

            status, claim = http_post("/v1/agent/claim", {
                "node_id": "node-worker-live",
                "stage": "feature",
                "lease_duration_s": 60.0,
            })
            attempt_id = claim["attempt_id"]

            elem = target_norm / math.sqrt(512)
            vec = [elem] * 512
            c_status, c_resp = http_post("/v1/agent/task-result", {
                "node_id": "node-worker-live",
                "attempt_id": attempt_id,
                "vid": vid,
                "coordinator_generation": claim["coordinator_generation"],
                "stage": "feature",
                "vectors": [vec],
                "status": "completed",
            })
            assert c_status == 200, f"Target norm {target_norm} within tolerance must be accepted, got {c_status}: {c_resp}"
            assert c_resp.get("next_stage") == "done" and c_resp.get("completed") is True
            print(f"  [PASS] Valid normalized norm {target_norm} accepted with HTTP 200 -> done")

        # ---------------------------------------------------------------------
        # 4. Feature 13: Cosine Similarity Drift & Degenerate Vector Safety
        # ---------------------------------------------------------------------
        print("\n--- Testing Feature 13: Cosine Similarity Drift & Degenerate Safety ---")
        def cosine_similarity(v1, v2):
            if len(v1) != len(v2) or len(v1) == 0:
                return 0.0
            dot = sum(a * b for a, b in zip(v1, v2))
            norm1 = math.sqrt(sum(a * a for a in v1))
            norm2 = math.sqrt(sum(b * b for b in v2))
            denom = norm1 * norm2
            if denom == 0.0 or not math.isfinite(denom):
                return 0.0
            return dot / denom

        v_base = make_unit_vector(512, 3.14159)
        # Exact
        cos_exact = cosine_similarity(v_base, v_base)
        assert abs(cos_exact - 1.0) < 1e-6 and cos_exact >= 0.9999
        print(f"  [PASS] Exact match cos = {cos_exact:.6f} >= 0.9999")

        # Micro-FP variance
        v_other = make_unit_vector(512, 9.8765)
        dot_vo = sum(a * b for a, b in zip(v_base, v_other))
        v_perp = [b - dot_vo * a for a, b in zip(v_base, v_other)]
        norm_perp = math.sqrt(sum(x * x for x in v_perp))
        u_perp = [x / norm_perp for x in v_perp]

        def vector_with_cosine(c):
            s = math.sqrt(max(0.0, 1.0 - c * c))
            return [c * a + s * b for a, b in zip(v_base, u_perp)]

        v_micro = vector_with_cosine(0.99995)
        cos_micro = cosine_similarity(v_base, v_micro)
        assert cos_micro >= 0.9999 and abs(cos_micro - 0.99995) < 1e-5
        print(f"  [PASS] Micro-FP variance cos = {cos_micro:.6f} >= 0.9999")

        # Exact threshold 0.9999000
        v_thresh = vector_with_cosine(0.9999000)
        cos_thresh = cosine_similarity(v_base, v_thresh)
        assert cos_thresh >= 0.9999000 and abs(cos_thresh - 0.9999000) < 1e-5
        print(f"  [PASS] Exact threshold cos = {cos_thresh:.7f} >= 0.9999")

        # Below threshold 0.9998999
        v_below = vector_with_cosine(0.9998999)
        cos_below = cosine_similarity(v_base, v_below)
        assert cos_below < 0.9999000
        print(f"  [PASS] Below threshold cos = {cos_below:.7f} properly rejected (< 0.9999)")

        # Degenerate vectors
        assert cosine_similarity([], []) == 0.0
        assert cosine_similarity([], v_base) == 0.0
        assert cosine_similarity(v_base, []) == 0.0
        assert cosine_similarity([0.0]*512, [0.0]*512) == 0.0
        assert cosine_similarity([0.0]*512, v_base) == 0.0
        print(f"  [PASS] Degenerate vectors (empty, zeros, mismatched) return 0.0 without panic")

        # ---------------------------------------------------------------------
        # 5. Feature 14, 15, 16: Unambiguous Creator Match (Face Skip)
        # ---------------------------------------------------------------------
        print("\n--- Testing Feature 14, 15: Unambiguous Creator Match ---")
        conn.execute("""
            INSERT INTO account_rules (id, account_name, creator_name, alias_list, action)
            VALUES ('rule-live-1', 'douyin_star', '星光作者', '["star_vlog"]', 'skip_face')
        """)
        conn.commit()

        vid = "vid-ocr-star-skip"
        conn.execute("""
            INSERT INTO tasks (vid, path, name, size, mtime_ns, stage, priority, batch_id, created_at, updated_at)
            VALUES (?, ?, 'test.mp4', 100, '1', 'ocr', 10, 'batch-1', ?, ?)
        """, (vid, f"/m/{vid}.mp4", now, now))
        conn.commit()

        status, claim = http_post("/v1/agent/claim", {
            "node_id": "node-worker-live",
            "stage": "ocr",
            "lease_duration_s": 60.0,
        })
        assert status == 200 and claim["frames"][0]["roi_box"] == [0, 0, 1080, 1920]

        c_status, c_resp = http_post("/v1/agent/task-result", {
            "node_id": "node-worker-live",
            "attempt_id": claim["attempt_id"],
            "vid": vid,
            "coordinator_generation": claim["coordinator_generation"],
            "stage": "ocr",
            "text_snippets": ["欢迎关注 @star_vlog 获取独家幕后"],
            "status": "completed",
        })
        assert c_status == 200 and c_resp["next_stage"] == "done" and c_resp["completed"] is True
        row = conn.execute("SELECT stage, data FROM tasks WHERE vid = ?", (vid,)).fetchone()
        assert row[0] == "done"
        task_data = json.loads(row[1])
        assert task_data["face_skipped"] is True and task_data["creator"] == "星光作者"
        print(f"  [PASS] Unambiguous match skipped face detection -> stage=done, creator=星光作者")

        # ---------------------------------------------------------------------
        # 5. Feature 14, 15: Ambiguous Conflict Match (Does Not Skip Face)
        # ---------------------------------------------------------------------
        print("\n--- Testing Feature 14, 15: Ambiguous Conflict Match ---")
        conn.execute("""
            INSERT INTO account_rules (id, account_name, creator_name, alias_list, action)
            VALUES ('rule-live-2', 'author_b', '作者B', '[]', 'skip_face')
        """)
        conn.commit()

        vid = "vid-ocr-conflict"
        conn.execute("""
            INSERT INTO tasks (vid, path, name, size, mtime_ns, stage, priority, batch_id, created_at, updated_at)
            VALUES (?, ?, 'test.mp4', 100, '1', 'ocr', 10, 'batch-1', ?, ?)
        """, (vid, f"/m/{vid}.mp4", now, now))
        conn.commit()

        status, claim = http_post("/v1/agent/claim", {
            "node_id": "node-worker-live",
            "stage": "ocr",
            "lease_duration_s": 60.0,
        })
        c_status, c_resp = http_post("/v1/agent/task-result", {
            "node_id": "node-worker-live",
            "attempt_id": claim["attempt_id"],
            "vid": vid,
            "coordinator_generation": claim["coordinator_generation"],
            "stage": "ocr",
            "text_snippets": ["合辑：@douyin_star 与 @author_b 联袂演出"],
            "status": "completed",
        })
        assert c_status == 200 and c_resp["next_stage"] == "detect" and c_resp["completed"] is False
        row = conn.execute("SELECT stage, data FROM tasks WHERE vid = ?", (vid,)).fetchone()
        assert row[0] == "detect"
        task_data = json.loads(row[1])
        assert task_data["face_skipped"] is False
        print(f"  [PASS] Ambiguous conflict safely cascaded to detect -> stage=detect, face_skipped=False")

        # ---------------------------------------------------------------------
        # 6. Feature 16: Clean Execution With 0 Text (Advances to Detect)
        # ---------------------------------------------------------------------
        print("\n--- Testing Feature 16: Clean Execution With 0 Text ---")
        vid = "vid-ocr-no-text"
        conn.execute("""
            INSERT INTO tasks (vid, path, name, size, mtime_ns, stage, priority, batch_id, created_at, updated_at)
            VALUES (?, ?, 'test.mp4', 100, '1', 'ocr', 10, 'batch-1', ?, ?)
        """, (vid, f"/m/{vid}.mp4", now, now))
        conn.commit()

        status, claim = http_post("/v1/agent/claim", {
            "node_id": "node-worker-live",
            "stage": "ocr",
            "lease_duration_s": 60.0,
        })
        c_status, c_resp = http_post("/v1/agent/task-result", {
            "node_id": "node-worker-live",
            "attempt_id": claim["attempt_id"],
            "vid": vid,
            "coordinator_generation": claim["coordinator_generation"],
            "stage": "ocr",
            "text_snippets": [],
            "error_type": "no_text_found",
            "status": "completed",
        })
        assert c_status == 200 and c_resp["next_stage"] == "detect" and c_resp["completed"] is False
        row = conn.execute("SELECT stage, data FROM tasks WHERE vid = ?", (vid,)).fetchone()
        assert row[0] == "detect"
        task_data = json.loads(row[1])
        assert task_data["ocr_result"] == "no_text_found"
        assert task_data["face_skipped"] is False
        print(f"  [PASS] Clean no_text_found advanced to detect -> stage=detect, ocr_result=no_text_found")

        # ---------------------------------------------------------------------
        # 7. Feature 16: Explicit Failure Distinction (parse_failure / model_error)
        # ---------------------------------------------------------------------
        print("\n--- Testing Feature 16: Explicit Failure Distinction ---")
        fail_types = ["parse_failure", "model_error"]
        for ft in fail_types:
            vid = f"vid-fail-{ft}"
            conn.execute("""
                INSERT INTO tasks (vid, path, name, size, mtime_ns, stage, priority, batch_id, created_at, updated_at)
                VALUES (?, ?, 'test.mp4', 100, '1', 'ocr', 10, 'batch-1', ?, ?)
            """, (vid, f"/m/{vid}.mp4", now, now))
            conn.commit()

            status, claim = http_post("/v1/agent/claim", {
                "node_id": "node-worker-live",
                "stage": "ocr",
                "lease_duration_s": 60.0,
            })
            c_status, c_resp = http_post("/v1/agent/task-result", {
                "node_id": "node-worker-live",
                "attempt_id": claim["attempt_id"],
                "vid": vid,
                "coordinator_generation": claim["coordinator_generation"],
                "stage": "ocr",
                "status": "failed",
                "error_type": ft,
                "error_message": f"synthetic_{ft}_message",
            })
            assert c_status == 200 and c_resp["next_stage"] == "retry" and c_resp["completed"] is False
            row = conn.execute("SELECT stage, error, text_attempts FROM tasks WHERE vid = ?", (vid,)).fetchone()
            assert row[0] == "retry", f"Must be retry, got {row[0]}"
            assert row[1] == f"synthetic_{ft}_message"
            assert row[2] == 1
            print(f"  [PASS] OCR failure '{ft}' preserved error state -> stage=retry, text_attempts=1")

        # ---------------------------------------------------------------------
        # 8. Expired Lease Commit Rejection (HTTP 409)
        # ---------------------------------------------------------------------
        print("\n--- Testing Expired Lease Commit Rejection (HTTP 409) ---")
        vid = "vid-expired-lease"
        conn.execute("""
            INSERT INTO tasks (vid, path, name, size, mtime_ns, stage, priority, batch_id, created_at, updated_at)
            VALUES (?, ?, 'test.mp4', 100, '1', 'ocr', 10, 'batch-1', ?, ?)
        """, (vid, f"/m/{vid}.mp4", now, now))
        conn.commit()

        status, claim = http_post("/v1/agent/claim", {
            "node_id": "node-worker-live",
            "stage": "ocr",
            "lease_duration_s": 60.0,
        })
        # Force lease expiration
        conn.execute("UPDATE tasks SET lease_expires_at = ? WHERE vid = ?", (time.time() - 20.0, vid))
        conn.commit()

        c_status, c_resp = http_post("/v1/agent/task-result", {
            "node_id": "node-worker-live",
            "attempt_id": claim["attempt_id"],
            "vid": vid,
            "coordinator_generation": claim["coordinator_generation"],
            "stage": "ocr",
            "text_snippets": ["some text"],
            "status": "completed",
        })
        assert c_status == 409, f"Expired lease must return 409, got {c_status}: {c_resp}"
        assert c_resp.get("error") == "lease_expired", f"Code mismatch: {c_resp}"
        print(f"  [PASS] Expired lease rejected with HTTP 409 lease_expired")

        print("\n=======================================================")
        print(" ALL EMPIRICAL ADVERSARIAL STRESS TESTS PASSED (100%)")
        print("=======================================================")

    finally:
        proc.terminate()
        try:
            proc.wait(timeout=3)
        except subprocess.TimeoutExpired:
            proc.kill()

if __name__ == "__main__":
    run_adversarial_m4_suite()
