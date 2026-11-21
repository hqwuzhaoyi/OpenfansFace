use face_file_operations::agent::cleanup::is_target_worker_process;
use face_file_operations::agent::outbox::{AgentOutbox, OutboxItem};
use face_file_operations::agent_registry::{self, HeartbeatRequest};
use face_file_operations::config::Config;
use face_file_operations::processing_scheduler;
use rusqlite::params;
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Barrier};
use std::thread;

pub use face_file_operations::config;
pub use face_file_operations::model;

#[path = "../src/agent_dispatcher.rs"]
mod agent_dispatcher;

use agent_dispatcher::{
    claim_task, commit_task_result, get_coordinator_generation,
    init_tables, now_sec, reap_leases, sha256_hex, ClaimTaskRequest, CommitTaskResultRequest,
};

fn create_temp_env() -> (Config, PathBuf) {
    let tmp_dir = std::env::temp_dir().join(format!("face-m5-adv-stress-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&tmp_dir).unwrap();
    let media = tmp_dir.join("media");
    fs::create_dir_all(&media).unwrap();
    let ledger = tmp_dir.join("ledger.db");

    let config = Config {
        base: tmp_dir.clone(),
        media,
        ledger,
        bind: "127.0.0.1:0".to_string(),
        token: Some("test-admin-token".to_string()),
        fixture: None,
        parity_root: None,
    };

    let conn = processing_scheduler::open_or_init(&config).unwrap();
    init_tables(&conn).unwrap();
    conn.execute("UPDATE settings SET value = '10' WHERE key = 'concurrency'", []).unwrap();

    (config, tmp_dir)
}

fn enroll_node(config: &Config, node_id: &str, hostname: &str, instance_id: &str) -> String {
    let conn = processing_scheduler::open_or_init(config).unwrap();
    agent_registry::init_tables(&conn).unwrap();

    let now = now_sec();
    let token = format!("token_{}_{}", node_id, uuid::Uuid::new_v4().simple());
    let token_hash = sha256_hex(&token);

    conn.execute(
        "INSERT OR REPLACE INTO agent_nodes (
            id, token_hash, instance_id, hostname, os, arch, agent_version,
            cpu_cores, ram_bytes, gpu_model, gpu_vram_bytes, free_vram_bytes,
            status, accepting, model_ready, running_tasks, last_heartbeat_at,
            registered_at, updated_at
        ) VALUES (
            ?1, ?2, ?3, ?4, 'linux', 'x86_64', '0.1.0',
            16, 34359738368, 'NVIDIA GeForce RTX 4070 Ti', 12884901888, 8589934592,
            'accepting', 1, 1, 0, ?5,
            ?5, ?5
        )",
        params![node_id, token_hash, instance_id, hostname, now],
    ).unwrap();

    token
}

fn generate_unit_vector(dim: usize, seed_val: f32) -> Vec<f32> {
    let mut vec = Vec::with_capacity(dim);
    let mut norm_sq = 0.0f32;
    for i in 0..dim {
        let v = seed_val + (i as f32 * 0.013).sin();
        vec.push(v);
        norm_sq += v * v;
    }
    let norm = norm_sq.sqrt();
    for v in vec.iter_mut() {
        *v /= norm;
    }
    vec
}

// =========================================================================
// FEATURE 17: GENERATION FENCING & LEASE EXPIRY ADVERSARIAL STRESS TESTS
// =========================================================================

#[test]
fn challenge_subsecond_lease_expiry_and_monotonic_generation_bump() {
    let (config, _tmp) = create_temp_env();
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    init_tables(&conn).unwrap();

    let token = enroll_node(&config, "node-sub-1", "host-sub-1", "inst-sub-1");
    let now = now_sec();

    // Seed task
    conn.execute(
        "INSERT INTO tasks (vid, path, name, size, mtime_ns, stage, batch_id, created_at, updated_at)
         VALUES ('vid-sub-exp', '/tmp/sub.mp4', 'sub.mp4', 1000, '0', 'detect', 'b1', ?1, ?1)",
        params![now],
    ).unwrap();

    let gen_start = get_coordinator_generation(&config).unwrap();

    // Claim with 0.01s micro-lease
    let claim = claim_task(&config, &token, ClaimTaskRequest {
        node_id: Some("node-sub-1".to_string()),
        stage: "detect".to_string(),
        lease_duration_s: Some(0.01),
    }).unwrap().expect("Claim task succeeds");

    assert_eq!(claim.coordinator_generation, gen_start);

    // Immediate reap right after claim: task lease should NOT be expired yet (or barely)
    // But after a small sleep of 25ms, it MUST be expired
    thread::sleep(std::time::Duration::from_millis(25));

    let reap_res = reap_leases(&config).unwrap();
    assert_eq!(reap_res.count, 1, "Must reap exactly the expired task");
    assert_eq!(reap_res.reaped_vids, vec!["vid-sub-exp".to_string()]);
    assert_eq!(reap_res.coordinator_generation, gen_start + 1, "Must bump coordinator generation monotonically");

    // Second reap immediately after: count should be 0, generation must NOT bump again
    let reap_res2 = reap_leases(&config).unwrap();
    assert_eq!(reap_res2.count, 0, "No additional tasks to reap");
    assert_eq!(reap_res2.coordinator_generation, gen_start + 1, "Generation must stay stable when 0 reaped");

    // Verify task state in database: reclaimed to retry, lease cleared
    let (stage, assigned, lease_exp): (String, String, f64) = conn.query_row(
        "SELECT stage, assigned_node_id, lease_expires_at FROM tasks WHERE vid = 'vid-sub-exp'",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    ).unwrap();
    assert_eq!(stage, "retry");
    assert_eq!(assigned, "");
    assert_eq!(lease_exp, 0.0);
}

#[test]
fn challenge_stale_and_future_generation_fencing_rejections() {
    let (config, _tmp) = create_temp_env();
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    init_tables(&conn).unwrap();

    let token = enroll_node(&config, "node-fence-1", "host-fence", "inst-fence-1");
    let now = now_sec();

    conn.execute(
        "INSERT INTO tasks (vid, path, name, size, mtime_ns, stage, batch_id, created_at, updated_at)
         VALUES ('vid-fence-1', '/tmp/f.mp4', 'f.mp4', 1000, '0', 'ocr', 'b1', ?1, ?1)",
        params![now],
    ).unwrap();

    let claim = claim_task(&config, &token, ClaimTaskRequest {
        node_id: Some("node-fence-1".to_string()),
        stage: "ocr".to_string(),
        lease_duration_s: Some(60.0),
    }).unwrap().expect("Claim succeeds");

    let current_gen = claim.coordinator_generation;

    // 1. Attempt commit with stale generation (current_gen - 1)
    let stale_req = CommitTaskResultRequest {
        node_id: Some("node-fence-1".to_string()),
        vid: Some("vid-fence-1".to_string()),
        attempt_id: Some(claim.attempt_id.clone()),
        stage: Some("ocr".to_string()),
        coordinator_generation: Some(json!(current_gen - 1)),
        detections: None,
        vectors: None,
        text_snippets: Some(vec![json!("sample")]),
        execution_proof: None,
        status: Some("completed".to_string()),
        error_type: None,
        error_message: None,
    };
    let stale_err = commit_task_result(&config, &token, stale_req).unwrap_err();
    assert_eq!(stale_err.status, 409);
    assert_eq!(stale_err.code, "stale_generation");

    // 2. Attempt commit with future generation (current_gen + 10)
    let future_req = CommitTaskResultRequest {
        node_id: Some("node-fence-1".to_string()),
        vid: Some("vid-fence-1".to_string()),
        attempt_id: Some(claim.attempt_id.clone()),
        stage: Some("ocr".to_string()),
        coordinator_generation: Some(json!(current_gen + 10)),
        detections: None,
        vectors: None,
        text_snippets: Some(vec![json!("sample")]),
        execution_proof: None,
        status: Some("completed".to_string()),
        error_type: None,
        error_message: None,
    };
    let future_err = commit_task_result(&config, &token, future_req).unwrap_err();
    assert_eq!(future_err.status, 409);
    assert_eq!(future_err.code, "stale_generation");
}

#[test]
fn challenge_late_commit_with_expired_lease_returns_lease_expired_409() {
    let (config, _tmp) = create_temp_env();
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    init_tables(&conn).unwrap();

    let token = enroll_node(&config, "node-exp-late", "host-exp", "inst-exp-1");
    let now = now_sec();

    conn.execute(
        "INSERT INTO tasks (vid, path, name, size, mtime_ns, stage, batch_id, created_at, updated_at)
         VALUES ('vid-exp-late', '/tmp/el.mp4', 'el.mp4', 1000, '0', 'ocr', 'b1', ?1, ?1)",
        params![now],
    ).unwrap();

    let claim = claim_task(&config, &token, ClaimTaskRequest {
        node_id: Some("node-exp-late".to_string()),
        stage: "ocr".to_string(),
        lease_duration_s: Some(0.02), // 20ms micro lease
    }).unwrap().expect("Claim succeeds");

    // We do NOT call reap_expired_leases! Coordinator generation is untouched.
    // Instead, we wait 40ms so the lease time has passed.
    thread::sleep(std::time::Duration::from_millis(40));

    // Agent attempts to commit with valid generation, but lease expired
    let req = CommitTaskResultRequest {
        node_id: Some("node-exp-late".to_string()),
        vid: Some("vid-exp-late".to_string()),
        attempt_id: Some(claim.attempt_id),
        stage: Some("ocr".to_string()),
        coordinator_generation: Some(json!(claim.coordinator_generation)),
        detections: None,
        vectors: None,
        text_snippets: Some(vec![json!("too late text")]),
        execution_proof: None,
        status: Some("completed".to_string()),
        error_type: None,
        error_message: None,
    };

    let err = commit_task_result(&config, &token, req).unwrap_err();
    assert_eq!(err.status, 409);
    assert_eq!(err.code, "lease_expired");
}

#[test]
fn challenge_manual_takeover_rejects_commit_and_preserves_task_state() {
    let (config, _tmp) = create_temp_env();
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    init_tables(&conn).unwrap();

    let token = enroll_node(&config, "node-manual-1", "host-man", "inst-man-1");
    let now = now_sec();

    conn.execute(
        "INSERT INTO tasks (vid, path, name, size, mtime_ns, stage, batch_id, created_at, updated_at, manual, data)
         VALUES ('vid-manual-test', '/tmp/m.mp4', 'm.mp4', 1000, '0', 'ocr', 'b1', ?1, ?1, 0, '{\"original\":\"data\"}')",
        params![now],
    ).unwrap();

    let claim = claim_task(&config, &token, ClaimTaskRequest {
        node_id: Some("node-manual-1".to_string()),
        stage: "ocr".to_string(),
        lease_duration_s: Some(60.0),
    }).unwrap().expect("Claim succeeds");

    // Human operator takes over task: sets manual = 1 in database
    conn.execute("UPDATE tasks SET manual = 1 WHERE vid = 'vid-manual-test'", []).unwrap();

    // Agent attempts to commit result
    let req = CommitTaskResultRequest {
        node_id: Some("node-manual-1".to_string()),
        vid: Some("vid-manual-test".to_string()),
        attempt_id: Some(claim.attempt_id),
        stage: Some("ocr".to_string()),
        coordinator_generation: Some(json!(claim.coordinator_generation)),
        detections: None,
        vectors: None,
        text_snippets: Some(vec![json!("agent text")]),
        execution_proof: None,
        status: Some("completed".to_string()),
        error_type: None,
        error_message: None,
    };

    let err = commit_task_result(&config, &token, req).unwrap_err();
    assert_eq!(err.status, 409);
    assert_eq!(err.code, "manual_takeover");

    // Verify task in database is completely untouched by the rejected commit
    let (manual_val, data_val): (i32, String) = conn.query_row(
        "SELECT manual, data FROM tasks WHERE vid = 'vid-manual-test'",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    ).unwrap();
    assert_eq!(manual_val, 1);
    assert_eq!(data_val, "{\"original\":\"data\"}");

    // Verify no receipt was inserted
    let receipt_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM agent_receipts WHERE vid = 'vid-manual-test'",
        [],
        |r| r.get(0),
    ).unwrap();
    assert_eq!(receipt_count, 0);
}

// =========================================================================
// FEATURE 18: CRASH RECONCILIATION & PEER ISOLATION STRESS TESTS
// =========================================================================

#[test]
fn challenge_node_crash_reconciliation_and_exhausted_retry_transitions() {
    let (config, _tmp) = create_temp_env();
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    init_tables(&conn).unwrap();

    let token_a = enroll_node(&config, "node-crash-a", "host-a", "inst-a-1");
    let now = now_sec();

    // Task 1: attempts = 1 (should reclaim to 'retry')
    conn.execute(
        "INSERT INTO tasks (vid, path, name, size, mtime_ns, stage, text_attempts, batch_id, created_at, updated_at)
         VALUES ('vid-retry-cand', '/tmp/r.mp4', 'r.mp4', 1000, '0', 'ocr', 1, 'b1', ?1, ?1)",
        params![now],
    ).unwrap();

    // Task 2: attempts = 3 (should transition to 'error' because retries exhausted)
    conn.execute(
        "INSERT INTO tasks (vid, path, name, size, mtime_ns, stage, text_attempts, batch_id, created_at, updated_at)
         VALUES ('vid-error-cand', '/tmp/e.mp4', 'e.mp4', 1000, '0', 'ocr', 3, 'b1', ?1, ?1)",
        params![now],
    ).unwrap();

    // Claim both tasks to Node A
    let _c1 = claim_task(&config, &token_a, ClaimTaskRequest {
        node_id: Some("node-crash-a".to_string()),
        stage: "ocr".to_string(),
        lease_duration_s: Some(60.0),
    }).unwrap().expect("Claim 1 succeeds");

    let _c2 = claim_task(&config, &token_a, ClaimTaskRequest {
        node_id: Some("node-crash-a".to_string()),
        stage: "ocr".to_string(),
        lease_duration_s: Some(60.0),
    }).unwrap().expect("Claim 2 succeeds");

    // Verify Node A has running_tasks = 2
    let running: i32 = conn.query_row(
        "SELECT running_tasks FROM agent_nodes WHERE id = 'node-crash-a'",
        [],
        |r| r.get(0),
    ).unwrap();
    assert_eq!(running, 2);

    // Node A crashes and restarts with new instance_id 'inst-a-2'
    let hb = HeartbeatRequest {
        node_id: "node-crash-a".to_string(),
        instance_id: "inst-a-2".to_string(),
        timestamp: now + 5.0,
        uptime_s: Some(2.0),
        free_ram_bytes: 16 * 1024 * 1024 * 1024,
        free_vram_bytes: 8 * 1024 * 1024 * 1024,
        gpu_util_percent: 0.0,
        running_tasks: 0,
    };
    let resp = agent_registry::handle_heartbeat(&config, &token_a, hb).unwrap();
    assert!(resp.reconciliation.is_some(), "Must return reconciliation summary on restart");
    let summary = resp.reconciliation.unwrap();
    assert_eq!(summary.previous_instance_id, "inst-a-1");
    assert_eq!(summary.leases_reclaimed, 2);
    assert!(summary.reclaimed_vids.contains(&"vid-retry-cand".to_string()));
    assert!(summary.reclaimed_vids.contains(&"vid-error-cand".to_string()));

    // Verify running_tasks is reset to 0
    let running_after: i32 = conn.query_row(
        "SELECT running_tasks FROM agent_nodes WHERE id = 'node-crash-a'",
        [],
        |r| r.get(0),
    ).unwrap();
    assert_eq!(running_after, 0);

    // Verify task 1 is in 'retry', task 2 is in 'error'
    let stage1: String = conn.query_row("SELECT stage FROM tasks WHERE vid = 'vid-retry-cand'", [], |r| r.get(0)).unwrap();
    let stage2: String = conn.query_row("SELECT stage FROM tasks WHERE vid = 'vid-error-cand'", [], |r| r.get(0)).unwrap();
    assert_eq!(stage1, "retry");
    assert_eq!(stage2, "error");
}

#[test]
fn challenge_peer_isolation_during_node_crash() {
    let (config, _tmp) = create_temp_env();
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    init_tables(&conn).unwrap();

    let token_a = enroll_node(&config, "node-peer-a", "host-a", "inst-a-1");
    let token_b = enroll_node(&config, "node-peer-b", "host-b", "inst-b-1");
    let now = now_sec();

    // Seed task A and task B
    conn.execute(
        "INSERT INTO tasks (vid, path, name, size, mtime_ns, stage, batch_id, created_at, updated_at)
         VALUES ('vid-task-a', '/tmp/a.mp4', 'a.mp4', 1000, '0', 'ocr', 'b1', ?1, ?1)",
        params![now],
    ).unwrap();
    conn.execute(
        "INSERT INTO tasks (vid, path, name, size, mtime_ns, stage, batch_id, created_at, updated_at)
         VALUES ('vid-task-b', '/tmp/b.mp4', 'b.mp4', 1000, '0', 'ocr', 'b1', ?1, ?1)",
        params![now],
    ).unwrap();

    // Node A claims task A, Node B claims task B
    let claim_a = claim_task(&config, &token_a, ClaimTaskRequest {
        node_id: Some("node-peer-a".to_string()),
        stage: "ocr".to_string(),
        lease_duration_s: Some(120.0),
    }).unwrap().expect("Claim A succeeds");
    assert_eq!(claim_a.vid, "vid-task-a");

    let claim_b = claim_task(&config, &token_b, ClaimTaskRequest {
        node_id: Some("node-peer-b".to_string()),
        stage: "ocr".to_string(),
        lease_duration_s: Some(120.0),
    }).unwrap().expect("Claim B succeeds");
    assert_eq!(claim_b.vid, "vid-task-b");

    let gen_before_crash = get_coordinator_generation(&config).unwrap();

    // Node A crashes! Reconnects with instance 'inst-a-2'
    let hb_a = HeartbeatRequest {
        node_id: "node-peer-a".to_string(),
        instance_id: "inst-a-2".to_string(),
        timestamp: now + 5.0,
        uptime_s: Some(1.0),
        free_ram_bytes: 16 * 1024 * 1024 * 1024,
        free_vram_bytes: 8 * 1024 * 1024 * 1024,
        gpu_util_percent: 0.0,
        running_tasks: 0,
    };
    let hb_resp_a = agent_registry::handle_heartbeat(&config, &token_a, hb_a).unwrap();
    assert_eq!(hb_resp_a.reconciliation.unwrap().leases_reclaimed, 1);

    // PEER ISOLATION INVARIANTS:
    // 1. Global coordinator generation MUST NOT be bumped by a single node crash
    let gen_after_crash = get_coordinator_generation(&config).unwrap();
    assert_eq!(gen_after_crash, gen_before_crash, "Node A crash must NOT bump coordinator generation");

    // 2. Node B's task MUST remain leased to Node B
    let (b_stage, b_assigned, b_attempt): (String, String, String) = conn.query_row(
        "SELECT stage, assigned_node_id, attempt_id FROM tasks WHERE vid = 'vid-task-b'",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    ).unwrap();
    assert_eq!(b_stage, "ocr");
    assert_eq!(b_assigned, "node-peer-b");
    assert_eq!(b_attempt, claim_b.attempt_id);

    // 3. Node B MUST be able to complete its task without error
    let commit_b = commit_task_result(&config, &token_b, CommitTaskResultRequest {
        node_id: Some("node-peer-b".to_string()),
        vid: Some("vid-task-b".to_string()),
        attempt_id: Some(claim_b.attempt_id),
        stage: Some("ocr".to_string()),
        coordinator_generation: Some(json!(claim_b.coordinator_generation)),
        detections: None,
        vectors: None,
        text_snippets: Some(vec![json!("OCR OK from B")]),
        execution_proof: None,
        status: Some("completed".to_string()),
        error_type: None,
        error_message: None,
    }).unwrap();
    assert_eq!(commit_b.ok, true);

    // 4. Re-dispatch invariant: Node A's reclaimed task (now in 'retry') can be claimed immediately by Node B!
    let claim_reclaimed = claim_task(&config, &token_b, ClaimTaskRequest {
        node_id: Some("node-peer-b".to_string()),
        stage: "ocr".to_string(),
        lease_duration_s: Some(60.0),
    }).unwrap().expect("Reclaimed task from A must be claimable by B immediately");
    assert_eq!(claim_reclaimed.vid, "vid-task-a");
}

// =========================================================================
// FEATURE 19: OUTBOX IDEMPOTENCE & MONOTONIC COUNTER STRESS TESTS
// =========================================================================

#[test]
fn challenge_receipt_deduplication_and_strictly_monotonic_completion_counter() {
    let (config, _tmp) = create_temp_env();
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    init_tables(&conn).unwrap();

    let token = enroll_node(&config, "node-dedup-mono", "host-mono", "inst-mono-1");
    let now = now_sec();

    // Seed task in feature stage (final stage that marks task done and increments completed_count)
    conn.execute(
        "INSERT INTO tasks (vid, path, name, size, mtime_ns, stage, batch_id, created_at, updated_at)
         VALUES ('vid-mono-cnt', '/tmp/mono.mp4', 'mono.mp4', 1000, '0', 'feature', 'batch-m5', ?1, ?1)",
        params![now],
    ).unwrap();

    let claim = claim_task(&config, &token, ClaimTaskRequest {
        node_id: Some("node-dedup-mono".to_string()),
        stage: "feature".to_string(),
        lease_duration_s: Some(60.0),
    }).unwrap().expect("Claim succeeds");

    let sample_vector = generate_unit_vector(512, 0.5);

    let req = CommitTaskResultRequest {
        node_id: Some("node-dedup-mono".to_string()),
        vid: Some("vid-mono-cnt".to_string()),
        attempt_id: Some(claim.attempt_id.clone()),
        stage: Some("feature".to_string()),
        coordinator_generation: Some(json!(claim.coordinator_generation)),
        detections: None,
        vectors: Some(vec![json!(sample_vector)]),
        text_snippets: None,
        execution_proof: None,
        status: Some("completed".to_string()),
        error_type: None,
        error_message: None,
    };

    // Commit 1: Initial commit
    let r1 = commit_task_result(&config, &token, req.clone()).unwrap();
    assert_eq!(r1.ok, true);
    assert_eq!(r1.idempotent, None);
    assert_eq!(r1.completed, Some(true));

    // Verify completed_count == 1
    let cnt1: String = conn.query_row(
        "SELECT value FROM settings WHERE key = 'completed_count'",
        [],
        |r| r.get(0),
    ).unwrap();
    assert_eq!(cnt1, "1");

    // Commit 2: Duplicate redelivery
    let r2 = commit_task_result(&config, &token, req.clone()).unwrap();
    assert_eq!(r2.ok, true);
    assert_eq!(r2.idempotent, Some(true), "Duplicate commit must return idempotent: true");

    // Commit 3: Triplicate redelivery
    let r3 = commit_task_result(&config, &token, req.clone()).unwrap();
    assert_eq!(r3.ok, true);
    assert_eq!(r3.idempotent, Some(true));

    // INVARIANT: completed_count MUST NOT double-increment!
    let cnt3: String = conn.query_row(
        "SELECT value FROM settings WHERE key = 'completed_count'",
        [],
        |r| r.get(0),
    ).unwrap();
    assert_eq!(cnt3, "1", "completed_count must strictly remain 1 after repeated commits");

    // INVARIANT: delivery_count in agent_receipts must track each delivery attempt
    let (delivery_cnt, receipt_stage): (i64, String) = conn.query_row(
        "SELECT delivery_count, stage FROM agent_receipts WHERE attempt_id = ?",
        params![claim.attempt_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    ).unwrap();
    assert_eq!(delivery_cnt, 3, "Delivery count must record all 3 deliveries");
    assert_eq!(receipt_stage, "feature");
}

#[test]
fn challenge_concurrent_multithreaded_commits_exact_once_guarantee() {
    let (config, _tmp) = create_temp_env();
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    init_tables(&conn).unwrap();

    let token = enroll_node(&config, "node-race-1", "host-race", "inst-race-1");
    let now = now_sec();

    conn.execute(
        "INSERT INTO tasks (vid, path, name, size, mtime_ns, stage, batch_id, created_at, updated_at)
         VALUES ('vid-race-task', '/tmp/race.mp4', 'race.mp4', 1000, '0', 'feature', 'b-race', ?1, ?1)",
        params![now],
    ).unwrap();

    let claim = claim_task(&config, &token, ClaimTaskRequest {
        node_id: Some("node-race-1".to_string()),
        stage: "feature".to_string(),
        lease_duration_s: Some(60.0),
    }).unwrap().expect("Claim succeeds");

    let sample_vector = generate_unit_vector(512, 0.42);

    let num_threads = 10;
    let barrier = Arc::new(Barrier::new(num_threads));
    let mut handles = Vec::new();

    for _ in 0..num_threads {
        let b = Arc::clone(&barrier);
        let cfg = config.clone();
        let tok = token.clone();
        let att_id = claim.attempt_id.clone();
        let coord_gen = claim.coordinator_generation;
        let vec_data = sample_vector.clone();

        handles.push(thread::spawn(move || {
            let req = CommitTaskResultRequest {
                node_id: Some("node-race-1".to_string()),
                vid: Some("vid-race-task".to_string()),
                attempt_id: Some(att_id),
                stage: Some("feature".to_string()),
                coordinator_generation: Some(json!(coord_gen)),
                detections: None,
                vectors: Some(vec![json!(vec_data)]),
                text_snippets: None,
                execution_proof: None,
                status: Some("completed".to_string()),
                error_type: None,
                error_message: None,
            };

            // Synchronize all threads so they fire at the EXACT same instant
            b.wait();
            commit_task_result(&cfg, &tok, req)
        }));
    }

    let mut initial_commit_count = 0;
    let mut idempotent_commit_count = 0;

    for h in handles {
        let res = h.join().unwrap().expect("Concurrent commit call must succeed without panic or conflict error");
        assert_eq!(res.ok, true);
        if res.idempotent == Some(true) {
            idempotent_commit_count += 1;
        } else {
            initial_commit_count += 1;
        }
    }

    // Exactly 1 thread must be the initial committer; the remaining 9 must receive idempotent responses!
    assert_eq!(initial_commit_count, 1, "Exactly one thread must execute initial commit");
    assert_eq!(idempotent_commit_count, num_threads - 1, "Remaining threads must receive idempotent response");

    // Verify completed_count is strictly 1 in settings
    let final_cnt: String = conn.query_row(
        "SELECT value FROM settings WHERE key = 'completed_count'",
        [],
        |r| r.get(0),
    ).unwrap();
    assert_eq!(final_cnt, "1", "Completed count must strictly be 1 under concurrent race conditions");

    // Verify total delivery count in receipts is num_threads (10)
    let total_deliveries: i64 = conn.query_row(
        "SELECT delivery_count FROM agent_receipts WHERE attempt_id = ?",
        params![claim.attempt_id],
        |r| r.get(0),
    ).unwrap();
    assert_eq!(total_deliveries, num_threads as i64);
}

// =========================================================================
// AGENT OUTBOX & CLEANUP ADVERSARIAL CHECKS
// =========================================================================

#[test]
fn challenge_agent_outbox_exponential_backoff_and_terminal_abandonment() {
    let tmp_db = std::env::temp_dir().join(format!("adv-outbox-{}.db", uuid::Uuid::new_v4()));
    let outbox = AgentOutbox::open(&tmp_db).unwrap();
    let now = now_sec();

    let item = OutboxItem {
        attempt_id: "run-adv-outbox".to_string(),
        node_id: "node-1".to_string(),
        vid: "vid-adv-1".to_string(),
        stage: "ocr".to_string(),
        payload_json: json!({"vid": "vid-adv-1"}).to_string(),
        retry_count: 0,
        created_at: now,
        last_sent_at: None,
        next_retry_at: now,
        status: "pending".to_string(),
        last_error: None,
    };
    outbox.enqueue(&item).unwrap();

    // Verify backoff growth: 1s, 2s, 4s, 8s, 16s, 30s (capped)
    let expected_delays = [1.0, 2.0, 4.0, 8.0, 16.0, 30.0, 30.0];
    let mut current_time = now;
    for &expected in &expected_delays {
        let delay = outbox.record_transient_failure("run-adv-outbox", "Flaky network", current_time).unwrap();
        assert_eq!(delay, expected);
        current_time += delay;
    }

    // Terminal rejection (e.g. Center returned 409 stale_generation): outbox marks abandoned
    outbox.mark_abandoned("run-adv-outbox", "Stale generation", current_time).unwrap();
    let item_abandoned = outbox.get_item("run-adv-outbox").unwrap().expect("Item exists");
    assert_eq!(item_abandoned.status, "abandoned");
    assert_eq!(item_abandoned.last_error, Some("Stale generation".to_string()));

    let _ = fs::remove_file(&tmp_db);
}

#[test]
fn challenge_agent_cleanup_prohibited_patterns_never_kill_tts() {
    let forbidden = [
        "python -m indextts-server.service",
        "/usr/bin/python3 -m indextts",
        "systemctl restart indextts",
        "indextts_worker",
        "tts_daemon --port 8000",
        "python tts_server.py",
    ];

    for cmd in &forbidden {
        assert!(
            !is_target_worker_process(cmd),
            "Process '{}' contains forbidden pattern and MUST NEVER be classified as target",
            cmd
        );
    }
}
