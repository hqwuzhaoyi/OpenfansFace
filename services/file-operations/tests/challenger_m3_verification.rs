use face_file_operations::agent_registry;
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
    base64_decode, claim_task, commit_task_result, list_agent_tasks, now_sec, sha256_bytes,
    sha256_hex, ClaimTaskRequest, CommitTaskResultRequest, ListTasksQuery,
};

fn generate_test_512_vector() -> Vec<f32> {
    let mut v = vec![0.0f32; 512];
    let val = 1.0f32 / (512.0f32).sqrt();
    for x in v.iter_mut() {
        *x = val;
    }
    v
}

fn create_temp_env() -> (Config, PathBuf) {
    let tmp_dir = std::env::temp_dir().join(format!("face-disp-chal-{}", uuid::Uuid::new_v4()));
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
    agent_dispatcher::init_tables(&conn).unwrap();

    (config, tmp_dir)
}

fn seed_node_full(
    config: &Config,
    node_id: &str,
    token: &str,
    os: &str,
    arch: &str,
    gpu_model: Option<&str>,
    gpu_vram_bytes: i64,
    free_vram_bytes: i64,
    cpu_cores: i64,
    status: &str,
    accepting: i32,
    model_ready: i32,
    running_tasks: i64,
    last_heartbeat_offset_s: f64,
) {
    let conn = processing_scheduler::open_or_init(config).unwrap();
    let now = now_sec();
    let token_hash = sha256_hex(token);
    let last_heartbeat = now - last_heartbeat_offset_s;

    conn.execute(
        "INSERT OR REPLACE INTO agent_nodes (
            id, token_hash, instance_id, hostname, os, arch, agent_version,
            cpu_cores, ram_bytes, gpu_model, gpu_vram_bytes, free_vram_bytes,
            status, accepting, model_ready, running_tasks, last_heartbeat_at,
            registered_at, updated_at
        ) VALUES (
            ?1, ?2, 'inst-chal-1', 'host-chal-1', ?3, ?4, '1.0.0',
            ?5, 34359738368, ?6, ?7, ?8,
            ?9, ?10, ?11, ?12, ?13,
            ?13, ?13
        )",
        params![
            node_id,
            token_hash,
            os,
            arch,
            cpu_cores,
            gpu_model,
            gpu_vram_bytes,
            free_vram_bytes,
            status,
            accepting,
            model_ready,
            running_tasks,
            last_heartbeat,
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
            ?1, ?2, ?3, 1048576, '1700000000000000', ?4, ?5, 'batch-chal-1',
            ?6, ?6, '{}'
        )",
        params![
            vid,
            format!("/isolated/media/{}.mp4", vid),
            format!("{}.mp4", vid),
            stage,
            priority,
            now,
        ],
    )
    .unwrap();
}

// =========================================================================
// SECTION 1: CAPABILITY FUZZING & HARDWARE ADMISSION
// =========================================================================

#[test]
fn challenge_01_cpu_node_detect_rejection() {
    let (config, _tmp) = create_temp_env();
    seed_node_full(
        &config, "node-cpu", "tok-cpu", "linux", "x86_64",
        None, 0, 0, 8, "accepting", 1, 1, 0, 1.0,
    );
    seed_task(&config, "vid-c01", "detect", 1);

    let res = claim_task(
        &config,
        "Bearer tok-cpu",
        ClaimTaskRequest {
            node_id: Some("node-cpu".to_string()),
            stage: "detect".to_string(),
            lease_duration_s: Some(60.0),
        },
    );

    assert!(res.is_err(), "CPU-only node must not claim detect stage");
    let err = res.unwrap_err();
    assert_eq!(err.status, 409);
    assert_eq!(err.code, "capability_mismatch");
}

#[test]
fn challenge_02_darwin_macos_metal_rejection() {
    let (config, _tmp) = create_temp_env();
    seed_node_full(
        &config, "node-darwin", "tok-darwin", "darwin", "arm64",
        Some("Apple M2 Pro"), 16_000_000_000, 12_000_000_000, 12, "accepting", 1, 1, 0, 1.0,
    );
    seed_task(&config, "vid-c02", "detect", 1);

    let res = claim_task(
        &config,
        "Bearer tok-darwin",
        ClaimTaskRequest {
            node_id: Some("node-darwin".to_string()),
            stage: "detect".to_string(),
            lease_duration_s: Some(60.0),
        },
    );

    assert!(res.is_err(), "macOS Darwin Metal node must not claim detect stage");
    let err = res.unwrap_err();
    assert_eq!(err.status, 409);
    assert_eq!(err.code, "capability_mismatch");
}

/// EMPIRICAL VULNERABILITY CHECK:
/// In `agent_dispatcher.rs` line 570:
/// `if node.os.to_lowercase() == "darwin"`
/// But on macOS, Rust's `std::env::consts::OS` returns `"macos"`, NOT `"darwin"`!
/// If an agent node registers with `os: "macos"` and non-Apple GPU name ("Metal GPU", "AMD Radeon Pro 5500M"),
/// does `agent_dispatcher.rs` reject it, or does it bypass?
#[test]
fn challenge_03_macos_string_os_bypass_audit() {
    let (config, _tmp) = create_temp_env();
    // macOS node reporting os="macos" (as std::env::consts::OS does on Apple platforms)
    // with a Metal GPU name that does not contain "apple" or "virtual"
    seed_node_full(
        &config, "node-mac-metal", "tok-mac-metal", "macos", "arm64",
        Some("Metal Graphics Acceleration Core"), 16_000_000_000, 8_000_000_000, 10, "accepting", 1, 1, 0, 1.0,
    );
    seed_task(&config, "vid-c03", "detect", 1);

    let res = claim_task(
        &config,
        "Bearer tok-mac-metal",
        ClaimTaskRequest {
            node_id: Some("node-mac-metal".to_string()),
            stage: "detect".to_string(),
            lease_duration_s: Some(60.0),
        },
    );

    assert!(res.is_err(), "macOS Metal node must not claim detect stage");
    let err = res.unwrap_err();
    assert_eq!(err.status, 409);
    assert_eq!(err.code, "capability_mismatch");
    assert!(err.error.contains("macOS 不支持 CUDA 人脸检测任务"));
}

#[test]
fn challenge_04_vram_under_600mib_rejection() {
    let (config, _tmp) = create_temp_env();
    // 599 MiB free VRAM (below 600 MiB admission floor)
    let vram_599_mib = 599 * 1024 * 1024;
    seed_node_full(
        &config, "node-low-vram", "tok-low-vram", "linux", "x86_64",
        Some("NVIDIA GeForce RTX 4070 Ti"), 12_884_901_888, vram_599_mib, 16, "accepting", 1, 1, 0, 1.0,
    );
    seed_task(&config, "vid-c04", "detect", 1);

    let res = claim_task(
        &config,
        "Bearer tok-low-vram",
        ClaimTaskRequest {
            node_id: Some("node-low-vram".to_string()),
            stage: "detect".to_string(),
            lease_duration_s: Some(60.0),
        },
    );

    assert!(res.is_err(), "Node with VRAM < 600 MiB must be rejected with 409");
    let err = res.unwrap_err();
    assert_eq!(err.status, 409);
    assert_eq!(err.code, "capability_mismatch");
}

#[test]
fn challenge_05_vram_exact_600mib_boundary() {
    let (config, _tmp) = create_temp_env();
    // Exactly 600 MiB free VRAM (meets admission floor)
    let vram_600_mib = 600 * 1024 * 1024;
    seed_node_full(
        &config, "node-600-vram", "tok-600-vram", "linux", "x86_64",
        Some("NVIDIA GeForce RTX 4070 Ti"), 12_884_901_888, vram_600_mib, 16, "accepting", 1, 1, 0, 1.0,
    );
    seed_task(&config, "vid-c05", "detect", 1);

    let res = claim_task(
        &config,
        "Bearer tok-600-vram",
        ClaimTaskRequest {
            node_id: Some("node-600-vram".to_string()),
            stage: "detect".to_string(),
            lease_duration_s: Some(60.0),
        },
    );

    assert!(res.is_ok(), "Node with VRAM >= 600 MiB must be accepted");
    let claim = res.unwrap();
    assert!(claim.is_some());
    assert_eq!(claim.unwrap().vid, "vid-c05");
}

#[test]
fn challenge_06_non_cuda_execution_provider_rejection() {
    let (config, _tmp) = create_temp_env();
    seed_node_full(
        &config, "node-prov", "tok-prov", "linux", "x86_64",
        Some("NVIDIA GeForce RTX 4070 Ti"), 12_884_901_888, 8_000_000_000, 16, "accepting", 1, 1, 0, 1.0,
    );
    seed_task(&config, "vid-c06", "detect", 1);

    // Seed agent_packages and agent_deployments with CPUExecutionProvider
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    let now = now_sec();
    conn.execute(
        "CREATE TABLE IF NOT EXISTS agent_packages (
            package_id TEXT PRIMARY KEY,
            manifest_sha256 TEXT,
            model_sha256 TEXT,
            execution_provider TEXT,
            version TEXT,
            is_pinned INTEGER
        )",
        [],
    ).unwrap();
    conn.execute(
        "CREATE TABLE IF NOT EXISTS agent_deployments (
            deployment_id TEXT PRIMARY KEY,
            node_id TEXT,
            package_id TEXT,
            status TEXT,
            updated_at REAL
        )",
        [],
    ).unwrap();

    conn.execute(
        "INSERT INTO agent_packages (package_id, manifest_sha256, model_sha256, execution_provider, version, is_pinned, created_at) VALUES ('pkg-cpu', 'm-sha', 'mod-sha', 'CPUExecutionProvider', '1.0.0', 1, ?1)",
        params![now],
    ).unwrap();
    conn.execute(
        "INSERT INTO agent_deployments (deployment_id, node_id, package_id, status, desired_version, stage, updated_at) VALUES ('dep-1', 'node-prov', 'pkg-cpu', 'ready', '1.0.0', 'ready', ?1)",
        params![now],
    ).unwrap();

    let res = claim_task(
        &config,
        "Bearer tok-prov",
        ClaimTaskRequest {
            node_id: Some("node-prov".to_string()),
            stage: "detect".to_string(),
            lease_duration_s: Some(60.0),
        },
    );

    assert!(res.is_err(), "Node with CPUExecutionProvider must not claim detect");
    let err = res.unwrap_err();
    assert_eq!(err.status, 409);
    assert_eq!(err.code, "capability_mismatch");
}

#[test]
fn challenge_07_non_accepting_node_rejection() {
    let (config, _tmp) = create_temp_env();
    // accepting = 0
    seed_node_full(
        &config, "node-no-acc", "tok-no-acc", "linux", "x86_64",
        Some("NVIDIA RTX 4070 Ti"), 12_000_000_000, 8_000_000_000, 16, "accepting", 0, 1, 0, 1.0,
    );
    seed_task(&config, "vid-c07", "detect", 1);

    let res = claim_task(
        &config,
        "Bearer tok-no-acc",
        ClaimTaskRequest {
            node_id: Some("node-no-acc".to_string()),
            stage: "detect".to_string(),
            lease_duration_s: Some(60.0),
        },
    );

    assert!(res.is_err(), "Node with accepting=0 must be rejected with 409");
    let err = res.unwrap_err();
    assert_eq!(err.status, 409);
    assert_eq!(err.code, "node_not_accepting");
}

#[test]
fn challenge_08_model_unready_node_rejection() {
    let (config, _tmp) = create_temp_env();
    // model_ready = 0
    seed_node_full(
        &config, "node-unready", "tok-unready", "linux", "x86_64",
        Some("NVIDIA RTX 4070 Ti"), 12_000_000_000, 8_000_000_000, 16, "accepting", 1, 0, 0, 1.0,
    );
    seed_task(&config, "vid-c08", "detect", 1);

    let res = claim_task(
        &config,
        "Bearer tok-unready",
        ClaimTaskRequest {
            node_id: Some("node-unready".to_string()),
            stage: "detect".to_string(),
            lease_duration_s: Some(60.0),
        },
    );

    assert!(res.is_err(), "Node with model_ready=0 must be rejected with 409");
    let err = res.unwrap_err();
    assert_eq!(err.status, 409);
    assert_eq!(err.code, "node_not_accepting");
}

#[test]
fn challenge_09_stale_heartbeat_offline_rejection() {
    let (config, _tmp) = create_temp_env();
    // Heartbeat 95 seconds ago (> 90.0s timeout)
    seed_node_full(
        &config, "node-stale-hb", "tok-stale-hb", "linux", "x86_64",
        Some("NVIDIA RTX 4070 Ti"), 12_000_000_000, 8_000_000_000, 16, "accepting", 1, 1, 0, 95.0,
    );
    seed_task(&config, "vid-c09", "detect", 1);

    let res = claim_task(
        &config,
        "Bearer tok-stale-hb",
        ClaimTaskRequest {
            node_id: Some("node-stale-hb".to_string()),
            stage: "detect".to_string(),
            lease_duration_s: Some(60.0),
        },
    );

    assert!(res.is_err(), "Node with heartbeat >90s stale must be rejected with 409");
    let err = res.unwrap_err();
    assert_eq!(err.status, 409);
    assert_eq!(err.code, "node_not_accepting");
}

#[test]
fn challenge_10_quota_saturation_rejection() {
    let (config, _tmp) = create_temp_env();
    // Node saturated (cpu_cores = 2, running_tasks = 2)
    seed_node_full(
        &config, "node-sat", "tok-sat", "linux", "x86_64",
        Some("NVIDIA RTX 4070 Ti"), 12_000_000_000, 8_000_000_000, 2, "accepting", 1, 1, 2, 1.0,
    );
    seed_task(&config, "vid-c10", "detect", 1);

    let res = claim_task(
        &config,
        "Bearer tok-sat",
        ClaimTaskRequest {
            node_id: Some("node-sat".to_string()),
            stage: "detect".to_string(),
            lease_duration_s: Some(60.0),
        },
    );

    assert!(res.is_err(), "Saturated node must be rejected with 503 node_busy");
    let err = res.unwrap_err();
    assert_eq!(err.status, 503);
    assert_eq!(err.code, "node_busy");
}

// =========================================================================
// SECTION 2: FRAME SEPARATION & PAYLOAD SECURITY
// =========================================================================

#[test]
fn challenge_11_frame_size_bound_enforced() {
    let (config, _tmp) = create_temp_env();
    seed_node_full(
        &config, "node-frame", "tok-frame", "linux", "x86_64",
        Some("NVIDIA RTX 4070 Ti"), 12_000_000_000, 8_000_000_000, 8, "accepting", 1, 1, 0, 1.0,
    );
    seed_task(&config, "vid-c11", "detect", 1);

    let claim = claim_task(
        &config,
        "Bearer tok-frame",
        ClaimTaskRequest {
            node_id: Some("node-frame".to_string()),
            stage: "detect".to_string(),
            lease_duration_s: Some(60.0),
        },
    )
    .unwrap()
    .expect("task claimed");

    assert!(!claim.frames.is_empty(), "Frames must not be empty");
    for frame in &claim.frames {
        let decoded = base64_decode(&frame.data_base64).expect("valid base64");
        assert!(
            decoded.len() <= 1_500_000,
            "Raw frame size {} bytes exceeds 1.5MB limit",
            decoded.len()
        );
        assert_eq!(
            frame.frame_sha256,
            sha256_bytes(&decoded),
            "Frame sha256 mismatch"
        );
    }
}

#[test]
fn challenge_12_no_raw_paths_or_secrets_in_claim_payload() {
    let (config, _tmp) = create_temp_env();
    seed_node_full(
        &config, "node-sec", "tok-sec", "linux", "x86_64",
        Some("NVIDIA RTX 4070 Ti"), 12_000_000_000, 8_000_000_000, 8, "accepting", 1, 1, 0, 1.0,
    );
    seed_task(&config, "vid-c12", "detect", 1);

    let claim = claim_task(
        &config,
        "Bearer tok-sec",
        ClaimTaskRequest {
            node_id: Some("node-sec".to_string()),
            stage: "detect".to_string(),
            lease_duration_s: Some(60.0),
        },
    )
    .unwrap()
    .expect("task claimed");

    let serialized = serde_json::to_string(&claim).unwrap();
    // Must NOT leak raw media paths, Peach DB paths, accounts, or ledger
    assert!(!serialized.contains("/isolated/media"), "Must not leak raw media directory");
    assert!(!serialized.contains(".mp4"), "Must not leak raw video filename");
    assert!(!serialized.contains("peach"), "Must not leak Peach DB reference");
    assert!(!serialized.contains("ledger"), "Must not leak ledger path");
    assert!(!serialized.contains("account_rules"), "Must not leak account rules");
}

#[test]
fn challenge_13_account_rule_match_skips_face() {
    let (config, _tmp) = create_temp_env();
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    conn.execute(
        "CREATE TABLE IF NOT EXISTS account_rules (
            id TEXT PRIMARY KEY,
            account_name TEXT NOT NULL,
            creator_name TEXT NOT NULL,
            alias_list TEXT NOT NULL DEFAULT '[]'
        )",
        [],
    ).unwrap();
    conn.execute(
        "INSERT INTO account_rules (id, account_name, creator_name, alias_list) VALUES ('rule-1', 'bilibili_official', 'BilibiliCreator', '[\"bili_alt\"]')",
        [],
    ).unwrap();

    seed_node_full(
        &config, "node-ocr", "tok-ocr", "linux", "x86_64",
        None, 0, 0, 8, "accepting", 1, 1, 0, 1.0,
    );
    seed_task(&config, "vid-c13", "ocr", 1);

    let claim = claim_task(
        &config,
        "Bearer tok-ocr",
        ClaimTaskRequest {
            node_id: Some("node-ocr".to_string()),
            stage: "ocr".to_string(),
            lease_duration_s: Some(60.0),
        },
    )
    .unwrap()
    .expect("task claimed");

    // Commit with text snippets matching account_rules
    let commit_res = commit_task_result(
        &config,
        "Bearer tok-ocr",
        CommitTaskResultRequest {
            node_id: Some("node-ocr".to_string()),
            attempt_id: Some(claim.attempt_id.clone()),
            vid: Some("vid-c13".to_string()),
            coordinator_generation: Some(json!(claim.coordinator_generation)),
            stage: Some("ocr".to_string()),
            detections: None,
            vectors: None,
            text_snippets: Some(vec![json!("Follow bilibili_official for more videos")]),
            execution_proof: None,
            status: Some("completed".to_string()),
            error_type: None,
            error_message: None,
        },
    )
    .unwrap();

    assert!(commit_res.ok);
    assert_eq!(commit_res.next_stage.as_deref(), Some("done"), "OCR match should advance directly to done");
    assert_eq!(commit_res.completed, Some(true));

    // Verify task data has face_skipped = true and creator recorded
    let task_data_raw: String = conn
        .query_row("SELECT data FROM tasks WHERE vid = 'vid-c13'", [], |r| r.get(0))
        .unwrap();
    let task_data: serde_json::Value = serde_json::from_str(&task_data_raw).unwrap();
    assert_eq!(task_data["face_skipped"], json!(true));
    assert_eq!(task_data["creator"], json!("BilibiliCreator"));
}

// =========================================================================
// SECTION 3: CONCURRENCY & DEDUPLICATION RACES
// =========================================================================

#[test]
fn challenge_14_multithreaded_duplicate_submission_race() {
    let (config, _tmp) = create_temp_env();
    seed_node_full(
        &config, "node-race", "tok-race", "linux", "x86_64",
        None, 0, 0, 16, "accepting", 1, 1, 0, 1.0,
    );
    seed_task(&config, "vid-c14", "feature", 1);

    let claim = claim_task(
        &config,
        "Bearer tok-race",
        ClaimTaskRequest {
            node_id: Some("node-race".to_string()),
            stage: "feature".to_string(),
            lease_duration_s: Some(60.0),
        },
    )
    .unwrap()
    .expect("task claimed");

    let num_threads = 20;
    let barrier = Arc::new(Barrier::new(num_threads));
    let mut handles = Vec::new();

    let attempt_id = claim.attempt_id.clone();
    let coord_gen = claim.coordinator_generation;

    for _ in 0..num_threads {
        let c = config.clone();
        let b = barrier.clone();
        let att = attempt_id.clone();

        handles.push(thread::spawn(move || {
            let req = CommitTaskResultRequest {
                node_id: Some("node-race".to_string()),
                attempt_id: Some(att),
                vid: Some("vid-c14".to_string()),
                coordinator_generation: Some(json!(coord_gen)),
                stage: Some("feature".to_string()),
                detections: None,
                vectors: Some(vec![json!(generate_test_512_vector())]),
                text_snippets: None,
                execution_proof: None,
                status: Some("completed".to_string()),
                error_type: None,
                error_message: None,
            };
            b.wait();
            commit_task_result(&c, "Bearer tok-race", req)
        }));
    }

    let mut first_count = 0;
    let mut idempotent_count = 0;

    for h in handles {
        let res = h.join().unwrap();
        assert!(res.is_ok(), "Commit should succeed or return idempotent response");
        let resp = res.unwrap();
        assert!(resp.ok);
        if resp.idempotent == Some(true) {
            idempotent_count += 1;
        } else {
            first_count += 1;
        }
    }

    // Exactly 1 thread performed the original commit, remaining 19 were deduplicated!
    assert_eq!(first_count, 1, "Exactly one thread must execute original commit");
    assert_eq!(idempotent_count, num_threads - 1, "All racing duplicates must receive idempotent: true");

    // CRITICAL: settings.completed_count must be EXACTLY 1, never 20!
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    let count: String = conn
        .query_row("SELECT value FROM settings WHERE key = 'completed_count'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, "1", "completed_count must be strictly 1 after 20 concurrent races");

    // agent_receipts table must have exactly 1 row
    let receipt_rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM agent_receipts WHERE node_id = 'node-race'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(receipt_rows, 1, "agent_receipts must contain exactly 1 deduplicated record");
}

#[test]
fn challenge_15_stale_coordinator_generation_rejected() {
    let (config, _tmp) = create_temp_env();
    seed_node_full(
        &config, "node-stale-g", "tok-stale-g", "linux", "x86_64",
        Some("NVIDIA RTX 4070 Ti"), 12_000_000_000, 8_000_000_000, 8, "accepting", 1, 1, 0, 1.0,
    );
    seed_task(&config, "vid-c15", "detect", 1);

    let claim = claim_task(
        &config,
        "Bearer tok-stale-g",
        ClaimTaskRequest {
            node_id: Some("node-stale-g".to_string()),
            stage: "detect".to_string(),
            lease_duration_s: Some(60.0),
        },
    )
    .unwrap()
    .expect("task claimed");

    // Coordinator generation advances (e.g. Center failover / bump)
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    conn.execute("UPDATE settings SET value = '99' WHERE key = 'coordinator_generation'", []).unwrap();

    let res = commit_task_result(
        &config,
        "Bearer tok-stale-g",
        CommitTaskResultRequest {
            node_id: Some("node-stale-g".to_string()),
            attempt_id: Some(claim.attempt_id),
            vid: Some("vid-c15".to_string()),
            coordinator_generation: Some(json!(claim.coordinator_generation)), // Gen 1 submitted
            stage: Some("detect".to_string()),
            detections: Some(vec![]),
            vectors: None,
            text_snippets: None,
            execution_proof: None,
            status: Some("completed".to_string()),
            error_type: None,
            error_message: None,
        },
    );

    assert!(res.is_err(), "Stale coordinator generation must be rejected with 409");
    let err = res.unwrap_err();
    assert_eq!(err.status, 409);
    assert_eq!(err.code, "stale_generation");
}

#[test]
fn challenge_16_out_of_order_attempt_rejected() {
    let (config, _tmp) = create_temp_env();
    seed_node_full(
        &config, "node-ooo", "tok-ooo", "linux", "x86_64",
        Some("NVIDIA RTX 4070 Ti"), 12_000_000_000, 8_000_000_000, 8, "accepting", 1, 1, 0, 1.0,
    );
    seed_task(&config, "vid-c16", "detect", 1);

    let _claim = claim_task(
        &config,
        "Bearer tok-ooo",
        ClaimTaskRequest {
            node_id: Some("node-ooo".to_string()),
            stage: "detect".to_string(),
            lease_duration_s: Some(60.0),
        },
    )
    .unwrap()
    .expect("task claimed");

    // Submit with an old/bogus attempt ID
    let res = commit_task_result(
        &config,
        "Bearer tok-ooo",
        CommitTaskResultRequest {
            node_id: Some("node-ooo".to_string()),
            attempt_id: Some("run-old-attempt-00000000".to_string()),
            vid: Some("vid-c16".to_string()),
            coordinator_generation: Some(json!(1)),
            stage: Some("detect".to_string()),
            detections: Some(vec![]),
            vectors: None,
            text_snippets: None,
            execution_proof: None,
            status: Some("completed".to_string()),
            error_type: None,
            error_message: None,
        },
    );

    assert!(res.is_err(), "Mismatched attempt ID must be rejected with 409");
    let err = res.unwrap_err();
    assert_eq!(err.status, 409);
    assert_eq!(err.code, "attempt_mismatch");
}

#[test]
fn challenge_17_expired_lease_rejected() {
    let (config, _tmp) = create_temp_env();
    seed_node_full(
        &config, "node-exp-c", "tok-exp-c", "linux", "x86_64",
        Some("NVIDIA RTX 4070 Ti"), 12_000_000_000, 8_000_000_000, 8, "accepting", 1, 1, 0, 1.0,
    );
    seed_task(&config, "vid-c17", "detect", 1);

    let claim = claim_task(
        &config,
        "Bearer tok-exp-c",
        ClaimTaskRequest {
            node_id: Some("node-exp-c".to_string()),
            stage: "detect".to_string(),
            lease_duration_s: Some(60.0),
        },
    )
    .unwrap()
    .expect("task claimed");

    // Manually expire the lease in tasks table
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    conn.execute(
        "UPDATE tasks SET lease_expires_at = ?1 WHERE vid = 'vid-c17'",
        params![now_sec() - 10.0],
    ).unwrap();

    let res = commit_task_result(
        &config,
        "Bearer tok-exp-c",
        CommitTaskResultRequest {
            node_id: Some("node-exp-c".to_string()),
            attempt_id: Some(claim.attempt_id),
            vid: Some("vid-c17".to_string()),
            coordinator_generation: Some(json!(claim.coordinator_generation)),
            stage: Some("detect".to_string()),
            detections: Some(vec![]),
            vectors: None,
            text_snippets: None,
            execution_proof: None,
            status: Some("completed".to_string()),
            error_type: None,
            error_message: None,
        },
    );

    assert!(res.is_err(), "Expired lease callback must be rejected with 409");
    let err = res.unwrap_err();
    assert_eq!(err.status, 409);
    assert_eq!(err.code, "lease_expired");
}

#[test]
fn challenge_18_manual_takeover_rejected() {
    let (config, _tmp) = create_temp_env();
    seed_node_full(
        &config, "node-man", "tok-man", "linux", "x86_64",
        Some("NVIDIA RTX 4070 Ti"), 12_000_000_000, 8_000_000_000, 8, "accepting", 1, 1, 0, 1.0,
    );
    seed_task(&config, "vid-c18", "detect", 1);

    let claim = claim_task(
        &config,
        "Bearer tok-man",
        ClaimTaskRequest {
            node_id: Some("node-man".to_string()),
            stage: "detect".to_string(),
            lease_duration_s: Some(60.0),
        },
    )
    .unwrap()
    .expect("task claimed");

    // Operator intervenes and sets manual = 1
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    conn.execute("UPDATE tasks SET manual = 1 WHERE vid = 'vid-c18'", []).unwrap();

    let res = commit_task_result(
        &config,
        "Bearer tok-man",
        CommitTaskResultRequest {
            node_id: Some("node-man".to_string()),
            attempt_id: Some(claim.attempt_id),
            vid: Some("vid-c18".to_string()),
            coordinator_generation: Some(json!(claim.coordinator_generation)),
            stage: Some("detect".to_string()),
            detections: Some(vec![]),
            vectors: None,
            text_snippets: None,
            execution_proof: None,
            status: Some("completed".to_string()),
            error_type: None,
            error_message: None,
        },
    );

    assert!(res.is_err(), "Task under manual takeover must be rejected with 409");
    let err = res.unwrap_err();
    assert_eq!(err.status, 409);
    assert_eq!(err.code, "manual_takeover");
}

#[test]
fn challenge_19_list_tasks_adversarial_queries() {
    let (config, _tmp) = create_temp_env();
    seed_task(&config, "vid-q1", "queued", 1);
    seed_task(&config, "vid-q2", "detect", 5);
    seed_task(&config, "vid-q3", "done", 10);

    let res = list_agent_tasks(
        &config,
        ListTasksQuery {
            page: Some(0),
            limit: Some(999), // clamped to 100
            stage: Some("detect".to_string()),
            ..Default::default()
        },
    )
    .unwrap();

    let tasks = res["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0]["vid"], "vid-q2");
    assert_eq!(res["total"], 1);
    assert_eq!(res["all_count"], 3);
}
