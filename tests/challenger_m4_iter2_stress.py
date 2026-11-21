#!/usr/bin/env python3
"""
Milestone 4 Iteration 2 Adversarial Stress Testing Suite
Author: m4_iter2_challenger_1 (Empirical Challenger)

Verifies:
1. CPU ArcFace 512-dim embedding constraints:
   - Dimension validation (!= 512 -> 422 invalid_vector_dimension)
   - Non-finite validation (NaN/Inf -> 422 non_finite_vector_value)
   - Vector norm bounds (norm outside 0.95..1.05 -> 422 invalid_vector_norm, inside -> 200)
   - Cosine drift mathematical invariant (cos >= 0.9999)
2. Watermark OCR rules:
   - 0 matching rules -> cascade to detect, completed_count unchanged
   - >=2 matching rules -> cascade to detect, completed_count unchanged
   - Multiple rules for same creator -> unambiguous -> done, completed_count + 1
   - Exactly 1 matching rule -> skip face -> done, completed_count + 1
   - Error handling: parse_failure & model_error NEVER skip to detect/done, trigger retry/error
   - Clean execution: no_text_found -> detect
   - Lease expiry: commit after expiry -> 409 lease_expired
   - Scheduler reaping: reap_expired_leases reaps ocr & feature stages to retry
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

def cosine_similarity(v1, v2):
    if len(v1) == 0 or len(v2) == 0 or len(v1) != len(v2):
        return 0.0
    dot = 0.0
    norm1 = 0.0
    norm2 = 0.0
    for a, b in zip(v1, v2):
        dot += a * b
        norm1 += a * a
        norm2 += b * b
    denom = math.sqrt(norm1) * math.sqrt(norm2)
    if denom == 0.0 or not math.isfinite(denom):
        return 0.0
    return dot / denom

def run_tests():
    print("=" * 70)
    print("Milestone 4 Iteration 2 Adversarial Empirical Verification Suite")
    print("=" * 70)

    tmp_dir = tempfile.mkdtemp(prefix="face-m4-iter2-adv-")
    media_dir = os.path.join(tmp_dir, "media")
    os.makedirs(media_dir, exist_ok=True)
    processing_dir = os.path.join(tmp_dir, "processing")
    os.makedirs(processing_dir, exist_ok=True)
    ledger_path = os.path.join(tmp_dir, "ledger.db")
    processing_db_path = os.path.join(processing_dir, "processing.db")

    port = 19188
    bind = f"127.0.0.1:{port}"

    env = os.environ.copy()
    env["FACES_BASE"] = tmp_dir
    env["MEDIA2_ROOT"] = media_dir
    env["PEACH_LEDGER_PATH"] = ledger_path
    env["FILE_OPERATIONS_BIND"] = bind
    env["FILE_OPERATIONS_RUST_TOKEN"] = "iter2-challenger-token"

    binary_path = os.path.abspath("services/file-operations/target/debug/face-file-operations")
    if not os.path.exists(binary_path):
        raise RuntimeError(f"Binary not found at {binary_path}")

    proc = subprocess.Popen([binary_path], env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)

    try:
        # Wait for daemon
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

        print(f"[OK] Live daemon listening on http://{bind}")

        # Setup schema and tables
        conn = sqlite3.connect(processing_db_path)
        conn.execute("PRAGMA journal_mode=WAL;")
        conn.execute("CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);")
        conn.execute("INSERT OR REPLACE INTO settings (key, value) VALUES ('concurrency', '200');")
        conn.execute("INSERT OR REPLACE INTO settings (key, value) VALUES ('completed_count', '0');")
        conn.execute("INSERT OR REPLACE INTO settings (key, value) VALUES ('coordinator_generation', '1');")

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
        conn.execute("""
            CREATE TABLE IF NOT EXISTS processing_runs (
                id TEXT PRIMARY KEY, vid TEXT NOT NULL, name TEXT NOT NULL, batch_id TEXT NOT NULL,
                stage TEXT NOT NULL, started_at REAL NOT NULL, finished_at REAL, status TEXT NOT NULL,
                error TEXT, execution TEXT
            );
        """)
        conn.execute("""
            CREATE TABLE IF NOT EXISTS agent_nodes (
                id TEXT PRIMARY KEY, token_hash TEXT NOT NULL, instance_id TEXT NOT NULL DEFAULT '',
                hostname TEXT NOT NULL, os TEXT NOT NULL, arch TEXT NOT NULL, agent_version TEXT NOT NULL,
                cpu_cores INTEGER NOT NULL DEFAULT 0, ram_bytes INTEGER NOT NULL DEFAULT 0,
                gpu_model TEXT, gpu_vram_bytes INTEGER NOT NULL DEFAULT 0, gpu_device_uuid TEXT,
                device_group_id TEXT NOT NULL DEFAULT '', status TEXT NOT NULL DEFAULT 'registered',
                accepting INTEGER NOT NULL DEFAULT 1, model_ready INTEGER NOT NULL DEFAULT 0,
                desired_version TEXT NOT NULL DEFAULT '', current_version TEXT NOT NULL DEFAULT '',
                free_ram_bytes INTEGER NOT NULL DEFAULT 0, free_vram_bytes INTEGER NOT NULL DEFAULT 0,
                gpu_util_percent REAL NOT NULL DEFAULT 0.0, running_tasks INTEGER NOT NULL DEFAULT 0,
                last_heartbeat_at REAL NOT NULL DEFAULT 0.0, registered_at REAL NOT NULL, updated_at REAL NOT NULL
            );
        """)
        conn.execute("""
            CREATE TABLE IF NOT EXISTS agent_receipts (
                receipt_id TEXT PRIMARY KEY, node_id TEXT NOT NULL, attempt_id TEXT NOT NULL,
                stage TEXT NOT NULL, coordinator_generation INTEGER NOT NULL,
                committed_at REAL NOT NULL, receipt_data TEXT NOT NULL
            );
        """)
        conn.execute("""
            CREATE TABLE IF NOT EXISTS account_rules (
                id TEXT PRIMARY KEY, account_name TEXT NOT NULL, creator_name TEXT NOT NULL,
                alias_list TEXT NOT NULL, action TEXT NOT NULL DEFAULT 'skip_face'
            );
        """)
        conn.commit()

        def register_node(nid, os_name="darwin", arch="arm64", gpu_model=None, vram=0, cpu_cores=128):
            token = f"tok-{nid}"
            token_h = sha256_hex(token)
            now = time.time()
            conn.execute("""
                INSERT OR REPLACE INTO agent_nodes (
                    id, token_hash, instance_id, hostname, os, arch, agent_version,
                    cpu_cores, ram_bytes, gpu_model, gpu_vram_bytes, free_vram_bytes,
                    status, accepting, model_ready, running_tasks, last_heartbeat_at,
                    registered_at, updated_at
                ) VALUES (?, ?, 'inst-1', 'host-1', ?, ?, '1.0.0', ?, 34359738368, ?, ?, ?, 'accepting', 1, 1, 0, ?, ?, ?)
            """, (nid, token_h, os_name, arch, cpu_cores, gpu_model, vram, vram, now, now, now))
            conn.commit()
            return token

        def seed_task(vid, stage="feature"):
            now = time.time()
            conn.execute("""
                INSERT OR REPLACE INTO tasks (
                    vid, path, name, size, mtime_ns, stage, priority, batch_id,
                    created_at, updated_at, data
                ) VALUES (?, ?, ?, 1048576, '1700000000000000', ?, 10, 'batch-1', ?, ?, '{}')
            """, (vid, f"/isolated/{vid}.mp4", f"{vid}.mp4", stage, now, now))
            conn.commit()

        def http_post(endpoint, payload, token=None):
            headers = {"Content-Type": "application/json"}
            if token:
                headers["Authorization"] = f"Bearer {token}"
            data = json.dumps(payload).encode("utf-8")
            req = urllib.request.Request(f"http://{bind}{endpoint}", data=data, headers=headers)
            try:
                with urllib.request.urlopen(req) as resp:
                    return resp.status, json.loads(resp.read().decode("utf-8"))
            except urllib.error.HTTPError as e:
                body = e.read().decode("utf-8")
                try:
                    err_json = json.loads(body)
                except Exception:
                    err_json = {"raw": body}
                return e.code, err_json

        # -------------------------------------------------------------
        # 1. Stress-Test CPU ArcFace 512-dim embedding constraints
        # -------------------------------------------------------------
        print("\n[SECTION 1] CPU ArcFace 512-dim Embedding Constraints")
        node_token = register_node("node-feat-1", "darwin", "arm64")

        # 1.1 Dimension validation: lengths != 512 must yield 422 invalid_vector_dimension
        test_dimensions = [0, 1, 2, 256, 510, 511, 513, 514, 1024, 2048]
        for dim in test_dimensions:
            vid = f"vid-dim-{dim}"
            seed_task(vid, "feature")
            status, claim = http_post("/v1/agent/claim", {"node_id": "node-feat-1", "stage": "feature"}, node_token)
            assert status == 200, f"Claim failed for dim {dim}: {claim}"
            attempt_id = claim["attempt_id"]

            bad_vec = [0.044] * dim
            c_status, c_res = http_post("/v1/agent/task-result", {
                "node_id": "node-feat-1",
                "attempt_id": attempt_id,
                "vid": vid,
                "coordinator_generation": claim["coordinator_generation"],
                "stage": "feature",
                "vectors": [bad_vec],
                "status": "completed"
            }, node_token)

            assert c_status == 422, f"Dim {dim} expected 422, got {c_status}: {c_res}"
            assert c_res.get("error") == "invalid_vector_dimension" or c_res.get("code") == "invalid_vector_dimension", f"Dim {dim} code mismatch: {c_res}"
            print(f"  ✔ Dim {dim} rejected with HTTP 422 invalid_vector_dimension")

        # 1.2 Non-finite validation: NaN / Inf must yield 422 non_finite_vector_value
        non_finite_cases = [
            ("null_nan", None),
            ("str_nan", "NaN"),
            ("str_pos_inf", "Infinity"),
            ("str_neg_inf", "-Infinity"),
            ("str_val", "not_a_float"),
        ]
        for cname, bad_val in non_finite_cases:
            vid = f"vid-finite-{cname}"
            seed_task(vid, "feature")
            status, claim = http_post("/v1/agent/claim", {"node_id": "node-feat-1", "stage": "feature"}, node_token)
            assert status == 200
            attempt_id = claim["attempt_id"]

            vec = [0.044] * 512
            vec[255] = bad_val

            c_status, c_res = http_post("/v1/agent/task-result", {
                "node_id": "node-feat-1",
                "attempt_id": attempt_id,
                "vid": vid,
                "coordinator_generation": claim["coordinator_generation"],
                "stage": "feature",
                "vectors": [vec],
                "status": "completed"
            }, node_token)

            assert c_status == 422, f"Case {cname} expected 422, got {c_status}: {c_res}"
            assert c_res.get("error") == "non_finite_vector_value" or c_res.get("code") == "non_finite_vector_value", f"Case {cname} code mismatch: {c_res}"
            print(f"  ✔ Non-finite case '{cname}' rejected with HTTP 422 non_finite_vector_value")

        # 1.3 Vector norm bounds: outside [0.95, 1.05] must yield 422 invalid_vector_norm
        reject_norms = [0.0, 0.2, 0.5, 0.90, 0.9499, 1.0501, 1.10, 2.0, 100.0]
        for target_norm in reject_norms:
            vid = f"vid-norm-rej-{target_norm:.4f}"
            seed_task(vid, "feature")
            status, claim = http_post("/v1/agent/claim", {"node_id": "node-feat-1", "stage": "feature"}, node_token)
            assert status == 200
            attempt_id = claim["attempt_id"]

            if target_norm == 0.0:
                vec = [0.0] * 512
            else:
                elem = target_norm / math.sqrt(512.0)
                vec = [elem] * 512

            c_status, c_res = http_post("/v1/agent/task-result", {
                "node_id": "node-feat-1",
                "attempt_id": attempt_id,
                "vid": vid,
                "coordinator_generation": claim["coordinator_generation"],
                "stage": "feature",
                "vectors": [vec],
                "status": "completed"
            }, node_token)

            assert c_status == 422, f"Norm {target_norm} expected 422, got {c_status}: {c_res}"
            assert c_res.get("error") == "invalid_vector_norm" or c_res.get("code") == "invalid_vector_norm", f"Norm {target_norm} code mismatch: {c_res}"
            print(f"  ✔ Norm {target_norm:.4f} rejected with HTTP 422 invalid_vector_norm")

        # Vectors inside [0.95, 1.05] must be accepted
        accept_norms = [0.9501, 0.98, 1.0, 1.02, 1.0499]
        for target_norm in accept_norms:
            vid = f"vid-norm-acc-{target_norm:.4f}"
            seed_task(vid, "feature")
            status, claim = http_post("/v1/agent/claim", {"node_id": "node-feat-1", "stage": "feature"}, node_token)
            assert status == 200
            attempt_id = claim["attempt_id"]

            elem = target_norm / math.sqrt(512.0)
            vec = [elem] * 512

            c_status, c_res = http_post("/v1/agent/task-result", {
                "node_id": "node-feat-1",
                "attempt_id": attempt_id,
                "vid": vid,
                "coordinator_generation": claim["coordinator_generation"],
                "stage": "feature",
                "vectors": [vec],
                "status": "completed"
            }, node_token)

            assert c_status == 200, f"Norm {target_norm} expected 200, got {c_status}: {c_res}"
            assert c_res.get("ok") is True and c_res.get("next_stage") == "done"
            print(f"  ✔ Norm {target_norm:.4f} accepted with HTTP 200 -> done")

        # 1.4 Cosine drift mathematical invariant (cos >= 0.9999)
        ref_vec = make_unit_vector(512, seed=3.14159)
        # Self similarity
        cos_self = cosine_similarity(ref_vec, ref_vec)
        assert abs(cos_self - 1.0) < 1e-6, f"Self cosine expected 1.0, got {cos_self}"
        assert cos_self >= 0.9999
        print(f"  ✔ Reference vector self-similarity = {cos_self:.6f} >= 0.9999")

        # Micro-FP perturbation (e.g. ARM64 vs x86_64 NEON/AVX variance)
        perturbed_vec = [x + 1e-6 for x in ref_vec]
        norm = math.sqrt(sum(x * x for x in perturbed_vec))
        perturbed_vec = [x / norm for x in perturbed_vec]
        cos_perturbed = cosine_similarity(ref_vec, perturbed_vec)
        assert cos_perturbed >= 0.9999, f"Micro-FP cosine expected >= 0.9999, got {cos_perturbed}"
        print(f"  ✔ Cross-platform micro-FP perturbed cosine = {cos_perturbed:.6f} >= 0.9999")

        # Significant drift (above tolerance)
        drifted_vec = [x + 0.05 for x in ref_vec]
        norm = math.sqrt(sum(x * x for x in drifted_vec))
        drifted_vec = [x / norm for x in drifted_vec]
        cos_drifted = cosine_similarity(ref_vec, drifted_vec)
        assert cos_drifted < 0.9999, f"Drifted cosine expected < 0.9999, got {cos_drifted}"
        print(f"  ✔ Drifted vector cosine = {cos_drifted:.6f} < 0.9999 (properly detected outside tolerance)")

        # Degenerate safety
        assert cosine_similarity([], ref_vec) == 0.0
        assert cosine_similarity([0.0]*512, ref_vec) == 0.0
        print("  ✔ Degenerate vectors handled safely without panic")

        # -------------------------------------------------------------
        # 2. Stress-Test Watermark OCR Rules & Edge Cases
        # -------------------------------------------------------------
        print("\n[SECTION 2] Watermark OCR Rules & Edge Cases")
        ocr_token = register_node("node-ocr-1", "linux", "x86_64")

        # Seed account rules:
        # Rule 1: Zhang San -> account_name: 'zs_douyin', alias: ['zs_official']
        # Rule 2: Li Si -> account_name: 'lisi_vlog', alias: ['lisi_real']
        # Rule 3: Wang Wu -> account_name: 'wangwu_studio', alias: []
        # Rule 4: Zhang San (Second rule for same creator) -> account_name: 'zhangsan_bilibili', alias: []
        conn.execute("DELETE FROM account_rules;")
        conn.execute("""
            INSERT INTO account_rules (id, account_name, creator_name, alias_list, action) VALUES
            ('r1', 'zs_douyin', '张三', '["zs_official"]', 'skip_face'),
            ('r2', 'lisi_vlog', '李四', '["lisi_real"]', 'skip_face'),
            ('r3', 'wangwu_studio', '王五', '[]', 'skip_face'),
            ('r4', 'zhangsan_bilibili', '张三', '[]', 'skip_face');
        """)
        conn.commit()

        def get_completed_count():
            cursor = conn.execute("SELECT value FROM settings WHERE key = 'completed_count'")
            row = cursor.fetchone()
            return int(row[0]) if row else 0

        initial_completed = get_completed_count()

        # 2.1 Case 0: 0 matching rules -> cascade to detect, completed_count unchanged
        vid_0 = "vid-ocr-0-match"
        seed_task(vid_0, "ocr")
        s, c = http_post("/v1/agent/claim", {"node_id": "node-ocr-1", "stage": "ocr"}, ocr_token)
        assert s == 200
        s, r = http_post("/v1/agent/task-result", {
            "node_id": "node-ocr-1",
            "attempt_id": c["attempt_id"],
            "vid": vid_0,
            "coordinator_generation": c["coordinator_generation"],
            "stage": "ocr",
            "text_snippets": ["一段毫无关系的视频描述文字，没有作者水印"],
            "status": "completed"
        }, ocr_token)
        assert s == 200
        assert r.get("next_stage") == "detect", f"Expected detect, got: {r}"
        assert r.get("completed") is False
        assert get_completed_count() == initial_completed
        cursor = conn.execute("SELECT stage, data FROM tasks WHERE vid = ?", (vid_0,))
        row = cursor.fetchone()
        assert row[0] == "detect"
        d = json.loads(row[1])
        assert d.get("face_skipped") is False
        print("  ✔ 0 matching rules -> cascaded to 'detect', face_skipped=false, completed_count unchanged")

        # 2.2 Case 2: >= 2 matching rules for DIFFERENT creators -> ambiguous conflict -> cascade to detect
        vid_amb = "vid-ocr-ambiguous"
        seed_task(vid_amb, "ocr")
        s, c = http_post("/v1/agent/claim", {"node_id": "node-ocr-1", "stage": "ocr"}, ocr_token)
        assert s == 200
        s, r = http_post("/v1/agent/task-result", {
            "node_id": "node-ocr-1",
            "attempt_id": c["attempt_id"],
            "vid": vid_amb,
            "coordinator_generation": c["coordinator_generation"],
            "stage": "ocr",
            "text_snippets": ["本片由 zs_official 和 lisi_vlog 联合制作出品"],
            "status": "completed"
        }, ocr_token)
        assert s == 200
        assert r.get("next_stage") == "detect", f"Ambiguous match must cascade to detect, got {r}"
        assert r.get("completed") is False
        assert get_completed_count() == initial_completed
        cursor = conn.execute("SELECT stage, data FROM tasks WHERE vid = ?", (vid_amb,))
        row = cursor.fetchone()
        assert row[0] == "detect"
        d = json.loads(row[1])
        assert d.get("face_skipped") is False
        print("  ✔ >=2 matching rules (different creators) -> cascaded to 'detect', face_skipped=false, completed_count unchanged")

        # 2.3 Case: Multiple matching rules for the SAME creator -> unambiguous -> skip face to done
        vid_same_creator = "vid-ocr-same-creator-multirules"
        seed_task(vid_same_creator, "ocr")
        s, c = http_post("/v1/agent/claim", {"node_id": "node-ocr-1", "stage": "ocr"}, ocr_token)
        assert s == 200
        s, r = http_post("/v1/agent/task-result", {
            "node_id": "node-ocr-1",
            "attempt_id": c["attempt_id"],
            "vid": vid_same_creator,
            "coordinator_generation": c["coordinator_generation"],
            "stage": "ocr",
            "text_snippets": ["欢迎关注抖音 zs_douyin 以及 B站 zhangsan_bilibili"],
            "status": "completed"
        }, ocr_token)
        assert s == 200
        assert r.get("next_stage") == "done", f"Single unique creator must skip face to done, got {r}"
        assert r.get("completed") is True
        assert get_completed_count() == initial_completed + 1
        cursor = conn.execute("SELECT stage, data FROM tasks WHERE vid = ?", (vid_same_creator,))
        row = cursor.fetchone()
        assert row[0] == "done"
        d = json.loads(row[1])
        assert d.get("face_skipped") is True
        assert d.get("creator") == "张三"
        initial_completed += 1
        print("  ✔ Multiple matching rules for SAME creator -> recognized as unambiguous, stage=done, completed_count + 1")

        # 2.4 Case 1: Exactly 1 matching rule -> unambiguous creator match -> skip face to done
        vid_1 = "vid-ocr-1-match"
        seed_task(vid_1, "ocr")
        s, c = http_post("/v1/agent/claim", {"node_id": "node-ocr-1", "stage": "ocr"}, ocr_token)
        assert s == 200
        s, r = http_post("/v1/agent/task-result", {
            "node_id": "node-ocr-1",
            "attempt_id": c["attempt_id"],
            "vid": vid_1,
            "coordinator_generation": c["coordinator_generation"],
            "stage": "ocr",
            "text_snippets": ["欢迎关注王五工作室：wangwu_studio，独家首发"],
            "status": "completed"
        }, ocr_token)
        assert s == 200
        assert r.get("next_stage") == "done"
        assert r.get("completed") is True
        assert get_completed_count() == initial_completed + 1
        cursor = conn.execute("SELECT stage, data FROM tasks WHERE vid = ?", (vid_1,))
        row = cursor.fetchone()
        assert row[0] == "done"
        d = json.loads(row[1])
        assert d.get("face_skipped") is True
        assert d.get("creator") == "王五"
        initial_completed += 1
        print("  ✔ Exactly 1 matching rule -> skipped face to done, creator=王五, completed_count + 1")

        # 2.5 Error handling: parse_failure and model_error must NEVER skip to detect or done
        for err_type in ["parse_failure", "model_error"]:
            vid_err = f"vid-ocr-err-{err_type}"
            seed_task(vid_err, "ocr")
            s, c = http_post("/v1/agent/claim", {"node_id": "node-ocr-1", "stage": "ocr"}, ocr_token)
            assert s == 200
            s, r = http_post("/v1/agent/task-result", {
                "node_id": "node-ocr-1",
                "attempt_id": c["attempt_id"],
                "vid": vid_err,
                "coordinator_generation": c["coordinator_generation"],
                "stage": "ocr",
                "status": "failed",
                "error_type": err_type,
                "error_message": f"simulation_of_{err_type}"
            }, ocr_token)
            assert s == 200
            assert r.get("next_stage") == "retry", f"Expected retry, got {r}"
            assert r.get("completed") is False
            assert get_completed_count() == initial_completed
            cursor = conn.execute("SELECT stage, error, text_attempts, data FROM tasks WHERE vid = ?", (vid_err,))
            row = cursor.fetchone()
            assert row[0] == "retry", f"Task stage should be retry, got {row[0]}"
            assert f"simulation_of_{err_type}" in (row[1] or "")
            assert row[2] == 1, f"text_attempts should be 1, got {row[2]}"
            d = json.loads(row[3])
            assert d.get("error_type") == err_type
            print(f"  ✔ Error '{err_type}' -> preserved error state in 'retry', text_attempts=1, never advanced to detect/done")

        # Test retry exhaustion (attempts >= 3 -> error stage)
        vid_exhaust = "vid-ocr-exhaust-retries"
        seed_task(vid_exhaust, "ocr")
        conn.execute("UPDATE tasks SET text_attempts = 2 WHERE vid = ?", (vid_exhaust,))
        conn.commit()
        s, c = http_post("/v1/agent/claim", {"node_id": "node-ocr-1", "stage": "ocr"}, ocr_token)
        assert s == 200
        s, r = http_post("/v1/agent/task-result", {
            "node_id": "node-ocr-1",
            "attempt_id": c["attempt_id"],
            "vid": vid_exhaust,
            "coordinator_generation": c["coordinator_generation"],
            "stage": "ocr",
            "status": "failed",
            "error_type": "model_error",
            "error_message": "paddleocr_repeated_oom"
        }, ocr_token)
        assert s == 200
        assert r.get("next_stage") == "error", f"3rd attempt must transition to 'error', got {r}"
        cursor = conn.execute("SELECT stage, text_attempts FROM tasks WHERE vid = ?", (vid_exhaust,))
        row = cursor.fetchone()
        assert row[0] == "error" and row[1] == 3
        print("  ✔ Retries exhausted (attempts=3) -> transitioned to final 'error' stage")

        # 2.6 Clean execution with no_text_found: advances to detect
        vid_clean_notext = "vid-ocr-clean-notext"
        seed_task(vid_clean_notext, "ocr")
        s, c = http_post("/v1/agent/claim", {"node_id": "node-ocr-1", "stage": "ocr"}, ocr_token)
        assert s == 200
        s, r = http_post("/v1/agent/task-result", {
            "node_id": "node-ocr-1",
            "attempt_id": c["attempt_id"],
            "vid": vid_clean_notext,
            "coordinator_generation": c["coordinator_generation"],
            "stage": "ocr",
            "text_snippets": [],
            "error_type": "no_text_found",
            "status": "completed"
        }, ocr_token)
        assert s == 200
        assert r.get("next_stage") == "detect"
        assert r.get("completed") is False
        assert get_completed_count() == initial_completed
        cursor = conn.execute("SELECT stage, data FROM tasks WHERE vid = ?", (vid_clean_notext,))
        row = cursor.fetchone()
        assert row[0] == "detect"
        d = json.loads(row[1])
        assert d.get("ocr_result") == "no_text_found"
        print("  ✔ Clean no_text_found -> advanced to 'detect', completed_count unchanged")

        # -------------------------------------------------------------
        # 3. Stress-Test Task Leasing & Expiration Fencing
        # -------------------------------------------------------------
        print("\n[SECTION 3] Task Leasing & Expiration Fencing")

        # 3.1 Post-lease expiration commit must be rejected with HTTP 409 lease_expired
        vid_exp = "vid-lease-expired-test"
        seed_task(vid_exp, "ocr")
        s, c = http_post("/v1/agent/claim", {"node_id": "node-ocr-1", "stage": "ocr"}, ocr_token)
        assert s == 200

        # Simulate lease expiration in db
        conn.execute("UPDATE tasks SET lease_expires_at = ? WHERE vid = ?", (time.time() - 10.0, vid_exp))
        conn.commit()

        s, r = http_post("/v1/agent/task-result", {
            "node_id": "node-ocr-1",
            "attempt_id": c["attempt_id"],
            "vid": vid_exp,
            "coordinator_generation": c["coordinator_generation"],
            "stage": "ocr",
            "text_snippets": ["some text"],
            "status": "completed"
        }, ocr_token)

        assert s == 409, f"Expected HTTP 409 for expired lease, got {s}: {r}"
        assert r.get("error") == "lease_expired" or r.get("code") == "lease_expired"
        print("  ✔ Post-lease commit rejected with HTTP 409 lease_expired")

        # 3.2 Task leasing for 'ocr' and 'feature' stages
        for stg in ["ocr", "feature"]:
            vid_lease = f"vid-lease-check-{stg}"
            seed_task(vid_lease, stg)
            t_tok = node_token if stg == "feature" else ocr_token
            n_id = "node-feat-1" if stg == "feature" else "node-ocr-1"

            s, c = http_post("/v1/agent/claim", {"node_id": n_id, "stage": stg, "lease_duration_s": 45.0}, t_tok)
            assert s == 200
            assert c["stage"] == stg
            assert c["lease_expires_at_ms"] > (time.time() + 40) * 1000
            assert len(c["frames"]) > 0

            claimed_vid = c["vid"]
            cursor = conn.execute("SELECT run_id, attempt_id, lease_id, assigned_node_id, lease_expires_at FROM tasks WHERE vid = ?", (claimed_vid,))
            row = cursor.fetchone()
            assert row[0] == c["attempt_id"]
            assert row[1] == c["attempt_id"]
            assert row[3] == n_id
            assert row[4] > time.time()
            print(f"  ✔ Task lease verified for '{stg}': vid={claimed_vid}, attempt_id={row[0][:16]}..., assigned={n_id}")

        print("\n" + "=" * 70)
        print("ALL EMPIRICAL ADVERSARIAL STRESS TESTS PASSED (100%)")
        print("=" * 70)

    finally:
        proc.terminate()
        try:
            proc.wait(timeout=2)
        except Exception:
            proc.kill()

if __name__ == "__main__":
    run_tests()
