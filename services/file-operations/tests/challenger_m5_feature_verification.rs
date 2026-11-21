use face_file_operations::agent::cleanup::is_target_worker_process;
use face_file_operations::agent::outbox::{AgentOutbox, OutboxItem};
use face_file_operations::agent_registry::{self, HeartbeatRequest};
use face_file_operations::config::Config;
use face_file_operations::processing_scheduler;
use rusqlite::params;
use serde_json::json;
use std::fs;
use std::path::PathBuf;

pub use face_file_operations::config;
pub use face_file_operations::model;

#[path = "../src/agent_dispatcher.rs"]
mod agent_dispatcher;

use agent_dispatcher::{
    claim_task, commit_task_result, get_coordinator_generation, get_task_attempts,
    init_tables, now_sec, reap_leases, sha256_hex, ClaimTaskRequest, CommitTaskResultRequest,
};

fn create_temp_env() -> (Config, PathBuf) {
    let tmp_dir = std::env::temp_dir().join(format!("face-m5-test-{}", uuid::Uuid::new_v4()));
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
    (config, tmp_dir)
}

fn enroll_test_node(config: &Config, node_id: &str, _hostname: &str) -> String {
    let conn = processing_scheduler::open_or_init(config).unwrap();
    agent_registry::init_tables(&conn).unwrap();

    let now = now_sec();
    let token = format!("agt_{}", uuid::Uuid::new_v4().simple());
    let token_hash = sha256_hex(&token);

    conn.execute(
        "INSERT OR REPLACE INTO agent_nodes (
            id, token_hash, instance_id, hostname, os, arch, agent_version,
            cpu_cores, ram_bytes, gpu_model, gpu_vram_bytes, free_vram_bytes,
            status, accepting, model_ready, running_tasks, last_heartbeat_at,
            registered_at, updated_at
        ) VALUES (
            ?1, ?2, 'inst-1111', 'host-1', 'linux', 'x86_64', '0.1.0',
            16, 34359738368, NULL, 0, 0,
            'accepting', 1, 1, 0, ?3,
            ?3, ?3
        )",
        params![node_id, token_hash, now],
    ).unwrap();

    token
}

#[test]
fn test_reap_expired_leases_bumps_coordinator_generation_monotonically() {
    let (config, _tmp) = create_temp_env();
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    init_tables(&conn).unwrap();

    let gen_before = get_coordinator_generation(&config).unwrap();
    let now = now_sec();

    // Seed 2 expired tasks and 1 unexpired task
    conn.execute(
        "INSERT INTO tasks (vid, path, name, size, mtime_ns, stage, lease_expires_at, run_id, attempt_id, batch_id, created_at, updated_at)
         VALUES ('vid-exp-1', '/tmp/1.mp4', '1.mp4', 1000, '0', 'detect', ?1, 'run-1', 'run-1', 'b1', ?2, ?2)",
        params![now - 10.0, now],
    ).unwrap();
    conn.execute(
        "INSERT INTO tasks (vid, path, name, size, mtime_ns, stage, lease_expires_at, run_id, attempt_id, batch_id, created_at, updated_at)
         VALUES ('vid-exp-2', '/tmp/2.mp4', '2.mp4', 1000, '0', 'ocr', ?1, 'run-2', 'run-2', 'b1', ?2, ?2)",
        params![now - 5.0, now],
    ).unwrap();
    conn.execute(
        "INSERT INTO tasks (vid, path, name, size, mtime_ns, stage, lease_expires_at, run_id, attempt_id, batch_id, created_at, updated_at)
         VALUES ('vid-active-3', '/tmp/3.mp4', '3.mp4', 1000, '0', 'feature', ?1, 'run-3', 'run-3', 'b1', ?2, ?2)",
        params![now + 300.0, now],
    ).unwrap();

    let reap_res = reap_leases(&config).unwrap();
    assert_eq!(reap_res.count, 2);
    assert!(reap_res.reaped_vids.contains(&"vid-exp-1".to_string()));
    assert!(reap_res.reaped_vids.contains(&"vid-exp-2".to_string()));
    assert_eq!(reap_res.coordinator_generation, gen_before + 1);

    let gen_after = get_coordinator_generation(&config).unwrap();
    assert_eq!(gen_after, gen_before + 1);

    // Verify reaped tasks transitioned to retry with lease_expires_at = 0
    let stage_1: String = conn.query_row("SELECT stage FROM tasks WHERE vid = 'vid-exp-1'", [], |r| r.get(0)).unwrap();
    let stage_2: String = conn.query_row("SELECT stage FROM tasks WHERE vid = 'vid-exp-2'", [], |r| r.get(0)).unwrap();
    let stage_3: String = conn.query_row("SELECT stage FROM tasks WHERE vid = 'vid-active-3'", [], |r| r.get(0)).unwrap();
    assert_eq!(stage_1, "retry");
    assert_eq!(stage_2, "retry");
    assert_eq!(stage_3, "feature");
}

#[test]
fn test_stale_generation_callback_rejected_with_409() {
    let (config, _tmp) = create_temp_env();
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    init_tables(&conn).unwrap();

    let node_token = enroll_test_node(&config, "node-gen-1", "host-gen");
    let now = now_sec();

    // Seed task
    conn.execute(
        "INSERT INTO tasks (vid, path, name, size, mtime_ns, stage, batch_id, created_at, updated_at)
         VALUES ('vid-gen-test', '/tmp/g.mp4', 'g.mp4', 1000, '0', 'ocr', 'b1', ?1, ?1)",
        params![now],
    ).unwrap();

    // Claim task with 0.05s micro lease
    let claim_res = claim_task(&config, &node_token, ClaimTaskRequest {
        node_id: Some("node-gen-1".to_string()),
        stage: "ocr".to_string(),
        lease_duration_s: Some(0.05),
    }).unwrap().expect("Task claimed");

    let submitted_gen = claim_res.coordinator_generation;
    let attempt_id = claim_res.attempt_id;

    // Center bumps generation via reaping an expired lease
    let _ = reap_leases(&config);
    let next_gen = processing_scheduler::bump_coordinator_generation(&conn).unwrap();
    assert!(next_gen > submitted_gen);

    // Node attempts to commit task with older generation
    let commit_err = commit_task_result(&config, &node_token, CommitTaskResultRequest {
        node_id: Some("node-gen-1".to_string()),
        vid: Some("vid-gen-test".to_string()),
        attempt_id: Some(attempt_id),
        stage: Some("ocr".to_string()),
        coordinator_generation: Some(json!(submitted_gen)),
        detections: None,
        vectors: None,
        text_snippets: Some(vec![json!("text")]),
        execution_proof: None,
        status: Some("completed".to_string()),
        error_type: None,
        error_message: None,
    }).unwrap_err();

    assert_eq!(commit_err.status, 409);
    assert_eq!(commit_err.code, "stale_generation");
}

#[test]
fn test_liveness_check_marks_offline_and_reclaims_stranded_tasks() {
    let (config, _tmp) = create_temp_env();
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    init_tables(&conn).unwrap();

    let node_token = enroll_test_node(&config, "node-dead-1", "host-dead");
    let now = now_sec();

    // Set node last_heartbeat to 120s ago (>90s)
    conn.execute(
        "UPDATE agent_nodes SET last_heartbeat_at = ?1, running_tasks = 1, status = 'accepting' WHERE id = 'node-dead-1'",
        params![now - 120.0],
    ).unwrap();

    // Seed task assigned to node-dead-1
    conn.execute(
        "INSERT INTO tasks (vid, path, name, size, mtime_ns, stage, assigned_node_id, run_id, attempt_id, lease_expires_at, batch_id, created_at, updated_at)
         VALUES ('vid-dead-1', '/tmp/d.mp4', 'd.mp4', 1000, '0', 'ocr', 'node-dead-1', 'run-dead', 'run-dead', ?1, 'b1', ?2, ?2)",
        params![now + 500.0, now],
    ).unwrap();

    // Execute liveness check
    let offline_nodes = agent_registry::liveness_check(&config).unwrap();
    assert!(offline_nodes.contains(&"node-dead-1".to_string()));

    // Verify node is marked offline and running_tasks = 0
    let (status, running): (String, i32) = conn.query_row(
        "SELECT status, running_tasks FROM agent_nodes WHERE id = 'node-dead-1'",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    ).unwrap();
    assert_eq!(status, "offline");
    assert_eq!(running, 0);

    // Verify task was reclaimed
    let (task_stage, assigned_node, lease_exp): (String, String, f64) = conn.query_row(
        "SELECT stage, assigned_node_id, lease_expires_at FROM tasks WHERE vid = 'vid-dead-1'",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    ).unwrap();
    assert_eq!(task_stage, "retry");
    assert_eq!(assigned_node, "");
    assert_eq!(lease_exp, 0.0);

    // Dead node's late callback rejected with 409
    let commit_err = commit_task_result(&config, &node_token, CommitTaskResultRequest {
        node_id: Some("node-dead-1".to_string()),
        vid: Some("vid-dead-1".to_string()),
        attempt_id: Some("run-dead".to_string()),
        stage: Some("ocr".to_string()),
        coordinator_generation: Some(json!(1)),
        detections: None,
        vectors: None,
        text_snippets: Some(vec![json!("text")]),
        execution_proof: None,
        status: Some("completed".to_string()),
        error_type: None,
        error_message: None,
    }).unwrap_err();
    assert_eq!(commit_err.status, 409);
}

#[test]
fn test_agent_crash_reconciliation_reclaims_leases_and_reports_summary() {
    let (config, _tmp) = create_temp_env();
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    init_tables(&conn).unwrap();

    let node_token = enroll_test_node(&config, "node-crash-1", "host-crash");
    let now = now_sec();

    // First heartbeat with instance-1
    let hb1 = HeartbeatRequest {
        node_id: "node-crash-1".to_string(),
        instance_id: "inst-1111".to_string(),
        timestamp: now,
        uptime_s: Some(100.0),
        free_ram_bytes: 16 * 1024 * 1024 * 1024,
        free_vram_bytes: 8 * 1024 * 1024 * 1024,
        gpu_util_percent: 10.0,
        running_tasks: 0,
    };
    let resp1 = agent_registry::handle_heartbeat(&config, &node_token, hb1).unwrap();
    assert!(resp1.reconciliation.is_none(), "First heartbeat should not trigger reconciliation");

    // Seed task and claim it under instance-1
    conn.execute(
        "INSERT INTO tasks (vid, path, name, size, mtime_ns, stage, batch_id, created_at, updated_at)
         VALUES ('vid-crash-task', '/tmp/c.mp4', 'c.mp4', 1000, '0', 'ocr', 'b1', ?1, ?1)",
        params![now],
    ).unwrap();

    let claim_res = claim_task(&config, &node_token, ClaimTaskRequest {
        node_id: Some("node-crash-1".to_string()),
        stage: "ocr".to_string(),
        lease_duration_s: Some(600.0),
    }).unwrap().expect("Claim task succeeds");

    // Agent crashes and restarts with brand new instance-2
    let hb2 = HeartbeatRequest {
        node_id: "node-crash-1".to_string(),
        instance_id: "inst-2222".to_string(),
        timestamp: now + 5.0,
        uptime_s: Some(5.0),
        free_ram_bytes: 16 * 1024 * 1024 * 1024,
        free_vram_bytes: 8 * 1024 * 1024 * 1024,
        gpu_util_percent: 0.0,
        running_tasks: 0,
    };
    let resp2 = agent_registry::handle_heartbeat(&config, &node_token, hb2).unwrap();
    assert!(resp2.reconciliation.is_some(), "Crash must trigger reconciliation summary");
    let summary = resp2.reconciliation.unwrap();
    assert_eq!(summary.previous_instance_id, "inst-1111");
    assert_eq!(summary.leases_reclaimed, 1);
    assert_eq!(summary.reclaimed_vids, vec!["vid-crash-task".to_string()]);

    // Verify task is in 'retry' stage with cleared lease
    let (stage, assigned, lease_exp): (String, String, f64) = conn.query_row(
        "SELECT stage, assigned_node_id, lease_expires_at FROM tasks WHERE vid = 'vid-crash-task'",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    ).unwrap();
    assert_eq!(stage, "retry");
    assert_eq!(assigned, "");
    assert_eq!(lease_exp, 0.0);

    // Old attempt callback is rejected with 409
    let commit_err = commit_task_result(&config, &node_token, CommitTaskResultRequest {
        node_id: Some("node-crash-1".to_string()),
        vid: Some("vid-crash-task".to_string()),
        attempt_id: Some(claim_res.attempt_id),
        stage: Some("ocr".to_string()),
        coordinator_generation: Some(json!(claim_res.coordinator_generation)),
        detections: None,
        vectors: None,
        text_snippets: Some(vec![json!("text")]),
        execution_proof: None,
        status: Some("completed".to_string()),
        error_type: None,
        error_message: None,
    }).unwrap_err();
    assert_eq!(commit_err.status, 409);

    // Reclaimed task is immediately re-claimable by healthy/restarted node
    let claim_res2 = claim_task(&config, &node_token, ClaimTaskRequest {
        node_id: Some("node-crash-1".to_string()),
        stage: "ocr".to_string(),
        lease_duration_s: Some(60.0),
    }).unwrap().expect("Reclaimed task should be claimable immediately");
    assert_eq!(claim_res2.vid, "vid-crash-task");
}

#[test]
fn test_agent_process_cleanup_patterns_and_indextts_guard() {
    // Prohibited TTS patterns MUST NEVER be treated as targets
    assert!(!is_target_worker_process("python -m indextts-server.service"));
    assert!(!is_target_worker_process("/usr/bin/python indextts_worker.py"));
    assert!(!is_target_worker_process("indextts-server"));
    assert!(!is_target_worker_process("tts_engine_daemon"));

    // Allowed operator targets
    assert!(is_target_worker_process("python -m maintenance.watermark_worker"));
    assert!(is_target_worker_process("python maintenance/processing_faces.py"));
    assert!(is_target_worker_process("python maintenance/processing_compute.py"));
    assert!(is_target_worker_process("/opt/venv/bin/python -m services.gpu_inference.deploy.launch"));

    // Arbitrary other processes ignored
    assert!(!is_target_worker_process("/usr/bin/bash"));
    assert!(!is_target_worker_process("node server.js"));
    assert!(!is_target_worker_process("cargo test"));
}

#[test]
fn test_idempotent_receipt_deduplication_exact_cached_response_and_delivery_count() {
    let (config, _tmp) = create_temp_env();
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    init_tables(&conn).unwrap();

    let node_token = enroll_test_node(&config, "node-dedup-1", "host-dedup");
    let now = now_sec();

    // Seed task in OCR stage
    conn.execute(
        "INSERT INTO tasks (vid, path, name, size, mtime_ns, stage, batch_id, created_at, updated_at)
         VALUES ('vid-dedup-1', '/tmp/dd.mp4', 'dd.mp4', 1000, '0', 'ocr', 'batch-test', ?1, ?1)",
        params![now],
    ).unwrap();

    let claim_res = claim_task(&config, &node_token, ClaimTaskRequest {
        node_id: Some("node-dedup-1".to_string()),
        stage: "ocr".to_string(),
        lease_duration_s: Some(60.0),
    }).unwrap().expect("Claim task succeeds");

    let req = CommitTaskResultRequest {
        node_id: Some("node-dedup-1".to_string()),
        vid: Some("vid-dedup-1".to_string()),
        attempt_id: Some(claim_res.attempt_id.clone()),
        stage: Some("ocr".to_string()),
        coordinator_generation: Some(json!(claim_res.coordinator_generation)),
        detections: None,
        vectors: None,
        text_snippets: Some(vec![json!("Test Watermark")]),
        execution_proof: None,
        status: Some("completed".to_string()),
        error_type: None,
        error_message: None,
    };

    // First commit: initial delivery
    let resp1 = commit_task_result(&config, &node_token, req.clone()).unwrap();
    assert_eq!(resp1.ok, true);
    assert_eq!(resp1.idempotent, None);

    // Initial delivery count in agent_receipts should be 1
    let (delivery_cnt_1, vid_saved): (i64, String) = conn.query_row(
        "SELECT delivery_count, vid FROM agent_receipts WHERE attempt_id = ?1",
        params![claim_res.attempt_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    ).unwrap();
    assert_eq!(delivery_cnt_1, 1);
    assert_eq!(vid_saved, "vid-dedup-1");

    // Second commit: duplicate redelivery of same attempt_id
    let resp2 = commit_task_result(&config, &node_token, req.clone()).unwrap();
    assert_eq!(resp2.ok, true);
    assert_eq!(resp2.idempotent, Some(true), "Duplicate must return idempotent: true");

    // Delivery count in agent_receipts should increment to 2
    let delivery_cnt_2: i64 = conn.query_row(
        "SELECT delivery_count FROM agent_receipts WHERE attempt_id = ?1",
        params![claim_res.attempt_id],
        |r| r.get(0),
    ).unwrap();
    assert_eq!(delivery_cnt_2, 2);

    // Third commit: duplicate redelivery
    let resp3 = commit_task_result(&config, &node_token, req).unwrap();
    assert_eq!(resp3.ok, true);
    assert_eq!(resp3.idempotent, Some(true));

    let delivery_cnt_3: i64 = conn.query_row(
        "SELECT delivery_count FROM agent_receipts WHERE attempt_id = ?1",
        params![claim_res.attempt_id],
        |r| r.get(0),
    ).unwrap();
    assert_eq!(delivery_cnt_3, 3);

    // Verify task attempts query
    let attempts_val = get_task_attempts(&config, "vid-dedup-1").unwrap();
    let attempts_arr = attempts_val["attempts"].as_array().expect("attempts array");
    assert_eq!(attempts_arr.len(), 1);
    assert_eq!(attempts_arr[0]["attempt_id"], claim_res.attempt_id);
    assert_eq!(attempts_arr[0]["delivery_count"], 3);
    assert_eq!(attempts_arr[0]["is_authoritative"], true);
}

#[test]
fn test_agent_outbox_enqueue_poll_exponential_backoff_and_commit() {
    let tmp_db = std::env::temp_dir().join(format!("outbox-test-{}.db", uuid::Uuid::new_v4()));
    let outbox = AgentOutbox::open(&tmp_db).unwrap();
    let now = now_sec();

    let item = OutboxItem {
        attempt_id: "run-node1-abc".to_string(),
        node_id: "node-1".to_string(),
        vid: "vid-outbox-1".to_string(),
        stage: "ocr".to_string(),
        payload_json: json!({"vid": "vid-outbox-1"}).to_string(),
        retry_count: 0,
        created_at: now,
        last_sent_at: None,
        next_retry_at: now,
        status: "pending".to_string(),
        last_error: None,
    };

    outbox.enqueue(&item).unwrap();

    // Poll pending
    let pending = outbox.poll_pending(now + 1.0, 10).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].attempt_id, "run-node1-abc");

    // Simulate transient failure 1
    let delay1 = outbox.record_transient_failure("run-node1-abc", "Network timeout", now).unwrap();
    assert_eq!(delay1, 1.0); // 1 * 2^0 = 1s

    // At now + 0.5s, item is not yet ready for retry
    let pending_early = outbox.poll_pending(now + 0.5, 10).unwrap();
    assert_eq!(pending_early.len(), 0);

    // At now + 1.1s, item is ready
    let pending_ready = outbox.poll_pending(now + 1.1, 10).unwrap();
    assert_eq!(pending_ready.len(), 1);
    assert_eq!(pending_ready[0].retry_count, 1);

    // Simulate transient failure 2 -> exponential backoff: 2s
    let delay2 = outbox.record_transient_failure("run-node1-abc", "Connection reset", now + 1.1).unwrap();
    assert_eq!(delay2, 2.0); // 1 * 2^1 = 2s

    // Mark committed
    outbox.mark_committed("run-node1-abc", now + 5.0).unwrap();
    let item_committed = outbox.get_item("run-node1-abc").unwrap().expect("Item exists");
    assert_eq!(item_committed.status, "committed");

    // Prune committed older than 1s
    let pruned = outbox.prune_committed(1.0, now + 10.0).unwrap();
    assert_eq!(pruned, 1);
    assert!(outbox.get_item("run-node1-abc").unwrap().is_none());

    let _ = fs::remove_file(&tmp_db);
}
