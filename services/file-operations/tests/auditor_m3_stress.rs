use face_file_operations::agent_registry;
use face_file_operations::config::Config;
use face_file_operations::processing_scheduler;
use rusqlite::params;
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;

pub use face_file_operations::config;
pub use face_file_operations::model;

#[path = "../src/agent_dispatcher.rs"]
mod agent_dispatcher;

use agent_dispatcher::{
    claim_task, commit_task_result, init_tables, now_sec, sha256_hex, ClaimTaskRequest,
    CommitTaskResultRequest,
};

fn generate_test_512_vector() -> Vec<f32> {
    let mut v = vec![0.0f32; 512];
    let val = 1.0f32 / (512.0f32).sqrt();
    for x in v.iter_mut() {
        *x = val;
    }
    v
}

fn create_test_env() -> (Config, PathBuf) {
    let tmp_dir = std::env::temp_dir().join(format!("auditor-m3-stress-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&tmp_dir).unwrap();
    let media = tmp_dir.join("media");
    fs::create_dir_all(&media).unwrap();
    let ledger = tmp_dir.join("ledger.db");

    let config = Config {
        base: tmp_dir.clone(),
        media,
        ledger,
        bind: "127.0.0.1:0".to_string(),
        token: Some("auditor-token".to_string()),
        fixture: None,
        parity_root: None,
    };

    let conn = processing_scheduler::open_or_init(&config).unwrap();
    init_tables(&conn).unwrap();

    (config, tmp_dir)
}

fn seed_node(
    config: &Config,
    node_id: &str,
    token: &str,
    os: &str,
    gpu_model: Option<&str>,
    vram_bytes: i64,
    status: &str,
    accepting: i32,
    model_ready: i32,
    cpu_cores: i64,
) {
    let conn = processing_scheduler::open_or_init(config).unwrap();
    let now = now_sec();
    let token_hash = sha256_hex(token);

    conn.execute(
        "INSERT INTO agent_nodes (
            id, token_hash, instance_id, hostname, os, arch, agent_version,
            cpu_cores, ram_bytes, gpu_model, gpu_vram_bytes, free_vram_bytes,
            status, accepting, model_ready, running_tasks, last_heartbeat_at,
            registered_at, updated_at
        ) VALUES (
            ?1, ?2, 'inst-1', 'host-1', ?3, 'x86_64', '1.0.0',
            ?4, 34359738368, ?5, ?6, ?6,
            ?7, ?8, ?9, 0, ?10,
            ?10, ?10
        )",
        params![
            node_id,
            token_hash,
            os,
            cpu_cores,
            gpu_model,
            vram_bytes,
            status,
            accepting,
            model_ready,
            now,
        ],
    )
    .unwrap();
}

fn seed_task(config: &Config, vid: &str, stage: &str, priority: i64) {
    let conn = processing_scheduler::open_or_init(config).unwrap();
    let now = now_sec();

    conn.execute(
        "INSERT INTO tasks (
            vid, path, name, size, mtime_ns, stage, priority, batch_id,
            created_at, updated_at, data
        ) VALUES (
            ?1, ?2, ?3, 1048576, '1700000000000000', ?4, ?5, 'batch-1',
            ?6, ?6, '{}'
        )",
        params![
            vid,
            format!("/isolated/test/{}.mp4", vid),
            format!("{}.mp4", vid),
            stage,
            priority,
            now,
        ],
    )
    .unwrap();
}

#[test]
fn stress_test_concurrent_claim_race_no_double_leasing() {
    let (config, _tmp) = create_test_env();
    let arc_config = Arc::new(config);

    // Seed 3 tasks
    seed_task(&arc_config, "vid-race-1", "feature", 10);
    seed_task(&arc_config, "vid-race-2", "feature", 20);
    seed_task(&arc_config, "vid-race-3", "feature", 30);

    // Seed 10 distinct CPU nodes
    for i in 1..=10 {
        seed_node(
            &arc_config,
            &format!("node-c-{}", i),
            &format!("token-c-{}", i),
            "linux",
            None,
            0,
            "accepting",
            1,
            1,
            4,
        );
    }

    // Set cluster concurrency high enough
    {
        let conn = processing_scheduler::open_or_init(&arc_config).unwrap();
        conn.execute(
            "UPDATE settings SET value = '10' WHERE key = 'concurrency'",
            [],
        )
        .unwrap();
    }

    // Launch 10 threads trying to claim simultaneously
    let mut handles = Vec::new();
    for i in 1..=10 {
        let cfg = Arc::clone(&arc_config);
        handles.push(thread::spawn(move || {
            let node_id = format!("node-c-{}", i);
            let token = format!("Bearer token-c-{}", i);
            claim_task(
                &cfg,
                &token,
                ClaimTaskRequest {
                    node_id: Some(node_id),
                    stage: "feature".to_string(),
                    lease_duration_s: Some(60.0),
                },
            )
        }));
    }

    let mut claimed_vids = Vec::new();
    let mut none_count = 0;

    for h in handles {
        let res = h.join().unwrap().expect("claim should succeed without panic");
        match res {
            Some(t) => claimed_vids.push(t.vid),
            None => none_count += 1,
        }
    }

    // In a 10-thread race for 3 tasks:
    // Exactly 3 tasks claimed, exactly 7 got None!
    assert_eq!(claimed_vids.len(), 3, "Exactly 3 tasks must be claimed");
    assert_eq!(none_count, 7, "Exactly 7 nodes must get None");

    // All claimed vids must be distinct (NO DOUBLE LEASING!)
    claimed_vids.sort();
    let mut deduped = claimed_vids.clone();
    deduped.dedup();
    assert_eq!(claimed_vids, deduped, "No task may be claimed by multiple nodes simultaneously!");
}

#[test]
fn stress_test_concurrent_duplicate_receipt_commit_storm() {
    let (config, _tmp) = create_test_env();
    let arc_config = Arc::new(config);

    seed_node(
        &arc_config,
        "node-commit-storm",
        "tok-storm",
        "linux",
        None,
        0,
        "accepting",
        1,
        1,
        8,
    );
    seed_task(&arc_config, "vid-storm-1", "feature", 100);

    // Claim the task
    let claim = claim_task(
        &arc_config,
        "Bearer tok-storm",
        ClaimTaskRequest {
            node_id: Some("node-commit-storm".to_string()),
            stage: "feature".to_string(),
            lease_duration_s: Some(120.0),
        },
    )
    .unwrap()
    .expect("task must be claimed");

    // 10 concurrent threads bombarding with the exact same attempt
    let mut handles = Vec::new();
    for _ in 0..10 {
        let cfg = Arc::clone(&arc_config);
        let attempt = claim.attempt_id.clone();
        let coord_gen = claim.coordinator_generation;
        handles.push(thread::spawn(move || {
            commit_task_result(
                &cfg,
                "Bearer tok-storm",
                CommitTaskResultRequest {
                    node_id: Some("node-commit-storm".to_string()),
                    attempt_id: Some(attempt),
                    vid: Some("vid-storm-1".to_string()),
                    coordinator_generation: Some(json!(coord_gen)),
                    stage: Some("feature".to_string()),
                    detections: None,
                    vectors: Some(vec![json!(generate_test_512_vector())]),
                    text_snippets: None,
                    execution_proof: None,
                    status: Some("completed".to_string()),
                    error_type: None,
                    error_message: None,
                },
            )
        }));
    }

    let mut successful_first_commits = 0;
    let mut idempotent_commits = 0;

    for h in handles {
        let res = h.join().unwrap().expect("commit should succeed");
        assert!(res.ok);
        if res.idempotent == Some(true) {
            idempotent_commits += 1;
        } else if res.completed == Some(true) {
            successful_first_commits += 1;
        }
    }

    assert_eq!(successful_first_commits, 1, "Exactly one commit is the primary monotonic commit");
    assert_eq!(idempotent_commits, 9, "All 9 redundant commits are handled idempotently");

    // Check completed_count in DB
    let conn = processing_scheduler::open_or_init(&arc_config).unwrap();
    let count: String = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'completed_count'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, "1", "Completed count must strictly equal 1, no duplicate counting!");
}

#[test]
fn stress_test_account_rules_skip_face_pipeline() {
    let (config, _tmp) = create_test_env();
    seed_node(
        &config,
        "node-ocr-skip",
        "tok-ocr",
        "linux",
        None,
        0,
        "accepting",
        1,
        1,
        4,
    );
    seed_task(&config, "vid-skip-1", "ocr", 10);

    // Setup account_rules table
    {
        let conn = processing_scheduler::open_or_init(&config).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS account_rules (
                id TEXT PRIMARY KEY,
                account_name TEXT NOT NULL,
                creator_name TEXT NOT NULL,
                alias_list TEXT NOT NULL
            );
            INSERT INTO account_rules (id, account_name, creator_name, alias_list)
            VALUES ('rule-1', 'bilibili_official', 'BiliBili', '["bilibili", "b站"]');
            "#,
        )
        .unwrap();
    }

    let claim = claim_task(
        &config,
        "Bearer tok-ocr",
        ClaimTaskRequest {
            node_id: Some("node-ocr-skip".to_string()),
            stage: "ocr".to_string(),
            lease_duration_s: Some(60.0),
        },
    )
    .unwrap()
    .expect("task claimed");

    // Commit with OCR text snippets containing creator watermark
    let commit_res = commit_task_result(
        &config,
        "Bearer tok-ocr",
        CommitTaskResultRequest {
            node_id: Some("node-ocr-skip".to_string()),
            attempt_id: Some(claim.attempt_id),
            vid: Some("vid-skip-1".to_string()),
            coordinator_generation: Some(json!(claim.coordinator_generation)),
            stage: Some("ocr".to_string()),
            detections: None,
            vectors: None,
            text_snippets: Some(vec![json!({"text": "关注我的B站账号 bilibili_official 谢谢"})]),
            execution_proof: None,
            status: Some("completed".to_string()),
            error_type: None,
            error_message: None,
        },
    )
    .unwrap();

    assert!(commit_res.ok);
    assert_eq!(commit_res.next_stage.as_deref(), Some("done"));
    assert_eq!(commit_res.completed, Some(true));

    // Verify task data in database
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    let (stage, data_raw): (String, String) = conn
        .query_row(
            "SELECT stage, data FROM tasks WHERE vid = 'vid-skip-1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(stage, "done");
    let parsed: serde_json::Value = serde_json::from_str(&data_raw).unwrap();
    assert_eq!(parsed["face_skipped"], json!(true));
    assert_eq!(parsed["creator"], json!("BiliBili"));
}

#[test]
fn stress_test_manual_takeover_fencing() {
    let (config, _tmp) = create_test_env();
    seed_node(
        &config,
        "node-takeover",
        "tok-takeover",
        "linux",
        None,
        0,
        "accepting",
        1,
        1,
        4,
    );
    seed_task(&config, "vid-takeover-1", "feature", 10);

    let claim = claim_task(
        &config,
        "Bearer tok-takeover",
        ClaimTaskRequest {
            node_id: Some("node-takeover".to_string()),
            stage: "feature".to_string(),
            lease_duration_s: Some(60.0),
        },
    )
    .unwrap()
    .expect("task claimed");

    // Human operator takes over task manually
    {
        let conn = processing_scheduler::open_or_init(&config).unwrap();
        conn.execute(
            "UPDATE tasks SET manual = 1 WHERE vid = 'vid-takeover-1'",
            [],
        )
        .unwrap();
    }

    // Agent attempts to commit result
    let commit_res = commit_task_result(
        &config,
        "Bearer tok-takeover",
        CommitTaskResultRequest {
            node_id: Some("node-takeover".to_string()),
            attempt_id: Some(claim.attempt_id),
            vid: Some("vid-takeover-1".to_string()),
            coordinator_generation: Some(json!(claim.coordinator_generation)),
            stage: Some("feature".to_string()),
            detections: None,
            vectors: Some(vec![json!([0.1])]),
            text_snippets: None,
            execution_proof: None,
            status: Some("completed".to_string()),
            error_type: None,
            error_message: None,
        },
    );

    assert!(commit_res.is_err());
    let err = commit_res.unwrap_err();
    assert_eq!(err.status, 409);
    assert_eq!(err.code, "manual_takeover");
}

#[test]
fn stress_test_tampered_and_invalid_auth() {
    let (config, _tmp) = create_test_env();
    seed_node(
        &config,
        "node-auth-test",
        "tok-real-secret",
        "linux",
        None,
        0,
        "accepting",
        1,
        1,
        4,
    );
    seed_task(&config, "vid-auth-1", "feature", 10);

    // Empty Bearer token
    let res1 = claim_task(
        &config,
        "",
        ClaimTaskRequest {
            node_id: Some("node-auth-test".to_string()),
            stage: "feature".to_string(),
            lease_duration_s: Some(60.0),
        },
    );
    assert_eq!(res1.unwrap_err().status, 401);

    // Wrong Bearer token
    let res2 = claim_task(
        &config,
        "Bearer wrong-token",
        ClaimTaskRequest {
            node_id: Some("node-auth-test".to_string()),
            stage: "feature".to_string(),
            lease_duration_s: Some(60.0),
        },
    );
    assert_eq!(res2.unwrap_err().status, 401);

    // Valid token but mismatched node_id
    let res3 = claim_task(
        &config,
        "Bearer tok-real-secret",
        ClaimTaskRequest {
            node_id: Some("node-someone-else".to_string()),
            stage: "feature".to_string(),
            lease_duration_s: Some(60.0),
        },
    );
    assert_eq!(res3.unwrap_err().status, 401);
}
