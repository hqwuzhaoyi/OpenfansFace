use face_file_operations::agent_registry;
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
    base64_decode, claim_task, commit_task_result, list_agent_tasks, sha256_bytes,
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
    let tmp_dir = std::env::temp_dir().join(format!("face-disp-test-{}", uuid::Uuid::new_v4()));
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
) {
    let conn = processing_scheduler::open_or_init(config).unwrap();
    let now = agent_dispatcher::now_sec();
    let token_hash = sha256_hex(token);

    conn.execute(
        "INSERT INTO agent_nodes (
            id, token_hash, instance_id, hostname, os, arch, agent_version,
            cpu_cores, ram_bytes, gpu_model, gpu_vram_bytes, free_vram_bytes,
            status, accepting, model_ready, running_tasks, last_heartbeat_at,
            registered_at, updated_at
        ) VALUES (
            ?1, ?2, 'inst-1', 'host-1', ?3, 'x86_64', '1.0.0',
            8, 34359738368, ?4, ?5, ?5,
            ?6, ?7, ?8, 0, ?9,
            ?9, ?9
        )",
        params![
            node_id,
            token_hash,
            os,
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
    let now = agent_dispatcher::now_sec();

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
            format!("/isolated/media/{}.mp4", vid),
            format!("{}.mp4", vid),
            stage,
            priority,
            now,
        ],
    )
    .unwrap();
}

#[test]
fn test_01_gpu_node_claims_detection_stage_task() {
    let (config, _tmp) = create_temp_env();
    seed_node(
        &config,
        "node-gpu-1",
        "tok-gpu-1",
        "linux",
        Some("NVIDIA GeForce RTX 4070 Ti"),
        12_884_901_888,
        "accepting",
        1,
        1,
    );
    seed_task(&config, "vid-detect-1", "detect", 10);

    let claim_res = claim_task(
        &config,
        "Bearer tok-gpu-1",
        ClaimTaskRequest {
            node_id: Some("node-gpu-1".to_string()),
            stage: "detect".to_string(),
            lease_duration_s: Some(60.0),
        },
    );

    assert!(claim_res.is_ok());
    let task_opt = claim_res.unwrap();
    assert!(task_opt.is_some());
    let task = task_opt.unwrap();

    assert_eq!(task.vid, "vid-detect-1");
    assert_eq!(task.stage, "detect");
    assert!(task.attempt_id.starts_with("run-node-gpu-1-"));
    assert!(!task.lease_id.is_empty());
    assert_eq!(task.coordinator_generation, 1);
    assert_eq!(task.frames.len(), 1);
}

#[test]
fn test_02_cpu_node_claims_feature_stage_task() {
    let (config, _tmp) = create_temp_env();
    seed_node(
        &config,
        "node-mac-1",
        "tok-mac-1",
        "darwin",
        None,
        0,
        "accepting",
        1,
        1,
    );
    seed_task(&config, "vid-feat-1", "feature", 5);

    let claim_res = claim_task(
        &config,
        "Bearer tok-mac-1",
        ClaimTaskRequest {
            node_id: Some("node-mac-1".to_string()),
            stage: "feature".to_string(),
            lease_duration_s: Some(60.0),
        },
    );

    assert!(claim_res.is_ok());
    let task = claim_res.unwrap().expect("task should be leased");
    assert_eq!(task.vid, "vid-feat-1");
    assert_eq!(task.stage, "feature");
}

#[test]
fn test_03_cpu_node_rejected_from_claiming_detect() {
    let (config, _tmp) = create_temp_env();
    seed_node(
        &config,
        "node-cpu-only",
        "tok-cpu",
        "darwin",
        None,
        0,
        "accepting",
        1,
        1,
    );
    seed_task(&config, "vid-detect-cpu", "detect", 1);

    let claim_res = claim_task(
        &config,
        "Bearer tok-cpu",
        ClaimTaskRequest {
            node_id: Some("node-cpu-only".to_string()),
            stage: "detect".to_string(),
            lease_duration_s: Some(60.0),
        },
    );

    assert!(claim_res.is_err());
    let err = claim_res.unwrap_err();
    assert_eq!(err.status, 409);
    assert_eq!(err.code, "capability_mismatch");
}

#[test]
fn test_04_node_not_in_accepting_state_rejected_with_409() {
    let (config, _tmp) = create_temp_env();
    seed_node(
        &config,
        "node-reg",
        "tok-reg",
        "linux",
        Some("RTX 4070"),
        12_000_000_000,
        "registered",
        0,
        0,
    );
    seed_task(&config, "vid-rej-1", "detect", 1);

    let claim_res = claim_task(
        &config,
        "Bearer tok-reg",
        ClaimTaskRequest {
            node_id: Some("node-reg".to_string()),
            stage: "detect".to_string(),
            lease_duration_s: Some(60.0),
        },
    );

    assert!(claim_res.is_err());
    let err = claim_res.unwrap_err();
    assert_eq!(err.status, 409);
    assert_eq!(err.code, "node_not_accepting");
}

#[test]
fn test_05_empty_queue_returns_no_content() {
    let (config, _tmp) = create_temp_env();
    seed_node(
        &config,
        "node-empty",
        "tok-empty",
        "linux",
        Some("RTX 4070"),
        12_000_000_000,
        "accepting",
        1,
        1,
    );

    let claim_res = claim_task(
        &config,
        "Bearer tok-empty",
        ClaimTaskRequest {
            node_id: Some("node-empty".to_string()),
            stage: "detect".to_string(),
            lease_duration_s: Some(60.0),
        },
    );

    assert!(claim_res.is_ok());
    assert!(claim_res.unwrap().is_none());
}

#[test]
fn test_06_frame_data_bound_and_no_leak() {
    let (config, _tmp) = create_temp_env();
    seed_node(
        &config,
        "node-bound",
        "tok-bound",
        "linux",
        Some("RTX 4070"),
        12_000_000_000,
        "accepting",
        1,
        1,
    );
    seed_task(&config, "vid-secret-1", "detect", 1);

    let claim_res = claim_task(
        &config,
        "Bearer tok-bound",
        ClaimTaskRequest {
            node_id: Some("node-bound".to_string()),
            stage: "detect".to_string(),
            lease_duration_s: Some(60.0),
        },
    )
    .unwrap()
    .expect("task should be leased");

    assert_eq!(claim_res.frames.len(), 1);
    let frame = &claim_res.frames[0];
    let decoded = base64_decode(&frame.data_base64).expect("valid base64");
    assert!(decoded.len() <= 1_500_000, "Frame must be <= 1.5MB");
    assert_eq!(frame.frame_sha256, sha256_bytes(&decoded));

    // Serialize to JSON and verify no raw media paths or credentials leaked
    let json_val = serde_json::to_value(&claim_res).unwrap();
    let json_str = json_val.to_string();
    assert!(!json_str.contains("path"));
    assert!(!json_str.contains("/isolated/media"));
    assert!(!json_str.contains("accounts"));
    assert!(!json_str.contains("gallery"));
}

#[test]
fn test_07_atomic_commit_advances_stage() {
    let (config, _tmp) = create_temp_env();
    seed_node(
        &config,
        "node-commit",
        "tok-commit",
        "linux",
        Some("RTX 4070"),
        12_000_000_000,
        "accepting",
        1,
        1,
    );
    seed_task(&config, "vid-commit-1", "detect", 1);

    let claim = claim_task(
        &config,
        "Bearer tok-commit",
        ClaimTaskRequest {
            node_id: Some("node-commit".to_string()),
            stage: "detect".to_string(),
            lease_duration_s: Some(60.0),
        },
    )
    .unwrap()
    .expect("task leased");

    let commit_res = commit_task_result(
        &config,
        "Bearer tok-commit",
        CommitTaskResultRequest {
            node_id: Some("node-commit".to_string()),
            attempt_id: Some(claim.attempt_id.clone()),
            vid: Some("vid-commit-1".to_string()),
            coordinator_generation: Some(json!(claim.coordinator_generation)),
            stage: Some("detect".to_string()),
            detections: Some(vec![json!({"box": [0, 0, 50, 50], "score": 0.99})]),
            vectors: None,
            text_snippets: None,
            execution_proof: Some(json!({"device": "gpu", "duration_ms": 120})),
            status: Some("completed".to_string()),
            error_type: None,
            error_message: None,
        },
    );

    assert!(commit_res.is_ok());
    let resp = commit_res.unwrap();
    assert!(resp.ok);
    assert_eq!(resp.next_stage.as_deref(), Some("feature"));
    assert_eq!(resp.completed, Some(false));

    // Verify task state in DB
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    let stage: String = conn
        .query_row("SELECT stage FROM tasks WHERE vid = 'vid-commit-1'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(stage, "feature");
}

#[test]
fn test_08_atomic_commit_monotonic_completed_count() {
    let (config, _tmp) = create_temp_env();
    seed_node(
        &config,
        "node-mono",
        "tok-mono",
        "linux",
        None,
        0,
        "accepting",
        1,
        1,
    );
    seed_task(&config, "vid-mono-1", "feature", 1);

    let claim = claim_task(
        &config,
        "Bearer tok-mono",
        ClaimTaskRequest {
            node_id: Some("node-mono".to_string()),
            stage: "feature".to_string(),
            lease_duration_s: Some(60.0),
        },
    )
    .unwrap()
    .expect("task leased");

    let commit_res = commit_task_result(
        &config,
        "Bearer tok-mono",
        CommitTaskResultRequest {
            node_id: Some("node-mono".to_string()),
            attempt_id: Some(claim.attempt_id.clone()),
            vid: Some("vid-mono-1".to_string()),
            coordinator_generation: Some(json!(claim.coordinator_generation)),
            stage: Some("feature".to_string()),
            detections: None,
            vectors: Some(vec![json!(generate_test_512_vector())]),
            text_snippets: None,
            execution_proof: Some(json!({"device": "cpu"})),
            status: Some("completed".to_string()),
            error_type: None,
            error_message: None,
        },
    );

    assert!(commit_res.is_ok());
    let resp = commit_res.unwrap();
    assert!(resp.ok);
    assert_eq!(resp.next_stage.as_deref(), Some("done"));
    assert_eq!(resp.completed, Some(true));

    // Verify settings completed_count incremented to 1
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    let count: String = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'completed_count'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, "1");
}

#[test]
fn test_09_stale_generation_fencing_rejected() {
    let (config, _tmp) = create_temp_env();
    seed_node(
        &config,
        "node-stale",
        "tok-stale",
        "linux",
        Some("RTX 4070"),
        12_000_000_000,
        "accepting",
        1,
        1,
    );
    seed_task(&config, "vid-stale-1", "detect", 1);

    let claim = claim_task(
        &config,
        "Bearer tok-stale",
        ClaimTaskRequest {
            node_id: Some("node-stale".to_string()),
            stage: "detect".to_string(),
            lease_duration_s: Some(60.0),
        },
    )
    .unwrap()
    .expect("task leased");

    // Coordinator bump generation to 2
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    conn.execute(
        "UPDATE settings SET value = '2' WHERE key = 'coordinator_generation'",
        [],
    )
    .unwrap();

    let commit_res = commit_task_result(
        &config,
        "Bearer tok-stale",
        CommitTaskResultRequest {
            node_id: Some("node-stale".to_string()),
            attempt_id: Some(claim.attempt_id.clone()),
            vid: Some("vid-stale-1".to_string()),
            coordinator_generation: Some(json!(claim.coordinator_generation)), // Still gen 1
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

    assert!(commit_res.is_err());
    let err = commit_res.unwrap_err();
    assert_eq!(err.status, 409);
    assert_eq!(err.code, "stale_generation");
}

#[test]
fn test_10_attempt_mismatch_fencing_rejected() {
    let (config, _tmp) = create_temp_env();
    seed_node(
        &config,
        "node-att",
        "tok-att",
        "linux",
        Some("RTX 4070"),
        12_000_000_000,
        "accepting",
        1,
        1,
    );
    seed_task(&config, "vid-att-1", "detect", 1);

    let _claim = claim_task(
        &config,
        "Bearer tok-att",
        ClaimTaskRequest {
            node_id: Some("node-att".to_string()),
            stage: "detect".to_string(),
            lease_duration_s: Some(60.0),
        },
    )
    .unwrap()
    .expect("task leased");

    let commit_res = commit_task_result(
        &config,
        "Bearer tok-att",
        CommitTaskResultRequest {
            node_id: Some("node-att".to_string()),
            attempt_id: Some("run-bogus-attempt-999".to_string()),
            vid: Some("vid-att-1".to_string()),
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

    assert!(commit_res.is_err());
    let err = commit_res.unwrap_err();
    assert_eq!(err.status, 409);
    assert_eq!(err.code, "attempt_mismatch");
}

#[test]
fn test_11_lease_expired_fencing_rejected() {
    let (config, _tmp) = create_temp_env();
    seed_node(
        &config,
        "node-exp",
        "tok-exp",
        "linux",
        Some("RTX 4070"),
        12_000_000_000,
        "accepting",
        1,
        1,
    );
    seed_task(&config, "vid-exp-1", "detect", 1);

    let claim = claim_task(
        &config,
        "Bearer tok-exp",
        ClaimTaskRequest {
            node_id: Some("node-exp".to_string()),
            stage: "detect".to_string(),
            lease_duration_s: Some(60.0),
        },
    )
    .unwrap()
    .expect("task leased");

    // Fast-forward lease_expires_at into past
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    conn.execute(
        "UPDATE tasks SET lease_expires_at = ?1 WHERE vid = 'vid-exp-1'",
        params![agent_dispatcher::now_sec() - 100.0],
    )
    .unwrap();

    let commit_res = commit_task_result(
        &config,
        "Bearer tok-exp",
        CommitTaskResultRequest {
            node_id: Some("node-exp".to_string()),
            attempt_id: Some(claim.attempt_id.clone()),
            vid: Some("vid-exp-1".to_string()),
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

    assert!(commit_res.is_err());
    let err = commit_res.unwrap_err();
    assert_eq!(err.status, 409);
    assert_eq!(err.code, "lease_expired");
}

#[test]
fn test_12_idempotent_receipt_deduplication() {
    let (config, _tmp) = create_temp_env();
    seed_node(
        &config,
        "node-dedup",
        "tok-dedup",
        "linux",
        None,
        0,
        "accepting",
        1,
        1,
    );
    seed_task(&config, "vid-dedup-1", "feature", 1);

    let claim = claim_task(
        &config,
        "Bearer tok-dedup",
        ClaimTaskRequest {
            node_id: Some("node-dedup".to_string()),
            stage: "feature".to_string(),
            lease_duration_s: Some(60.0),
        },
    )
    .unwrap()
    .expect("task leased");

    let req = CommitTaskResultRequest {
        node_id: Some("node-dedup".to_string()),
        attempt_id: Some(claim.attempt_id.clone()),
        vid: Some("vid-dedup-1".to_string()),
        coordinator_generation: Some(json!(claim.coordinator_generation)),
        stage: Some("feature".to_string()),
        detections: None,
        vectors: Some(vec![json!(generate_test_512_vector())]),
        text_snippets: None,
        execution_proof: None,
        status: Some("completed".to_string()),
        error_type: None,
        error_message: None,
    };

    // First commit
    let first = commit_task_result(&config, "Bearer tok-dedup", req.clone()).unwrap();
    assert!(first.ok);
    assert_eq!(first.completed, Some(true));

    // Verify counter is 1
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    let count1: String = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'completed_count'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count1, "1");

    // Second commit (replay / redelivery)
    let second = commit_task_result(&config, "Bearer tok-dedup", req).unwrap();
    assert!(second.ok);
    assert_eq!(second.idempotent, Some(true));

    // Counter must NOT be incremented twice!
    let count2: String = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'completed_count'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count2, "1");
}

#[test]
fn test_13_list_agent_tasks_paged_and_filtered() {
    let (config, _tmp) = create_temp_env();
    seed_task(&config, "vid-list-1", "queued", 1);
    seed_task(&config, "vid-list-2", "detect", 2);
    seed_task(&config, "vid-list-3", "review", 3);

    let list_res = list_agent_tasks(
        &config,
        ListTasksQuery {
            page: Some(0),
            limit: Some(10),
            stage: Some("detect".to_string()),
            ..Default::default()
        },
    )
    .unwrap();

    let tasks = list_res["tasks"].as_array().expect("tasks array");
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0]["vid"], "vid-list-2");
    assert_eq!(tasks[0]["stage"], "detect");
    assert_eq!(list_res["total"], 1);
    assert_eq!(list_res["all_count"], 3);
}

#[test]
fn test_14_macos_string_os_with_metal_gpu_rejected_from_claiming_detect() {
    let (config, _tmp) = create_temp_env();
    seed_node(
        &config,
        "node-mac-metal-1",
        "tok-mac-metal-1",
        "macos",
        Some("Metal Graphics Acceleration Core"),
        16_000_000_000,
        "accepting",
        1,
        1,
    );
    seed_task(&config, "vid-mac-cuda-reject", "detect", 10);

    let claim_res = claim_task(
        &config,
        "Bearer tok-mac-metal-1",
        ClaimTaskRequest {
            node_id: Some("node-mac-metal-1".to_string()),
            stage: "detect".to_string(),
            lease_duration_s: Some(60.0),
        },
    );

    assert!(claim_res.is_err(), "macOS node must be rejected from claiming CUDA detect task");
    let err = claim_res.unwrap_err();
    assert_eq!(err.status, 409);
    assert_eq!(err.code, "capability_mismatch");
    assert!(err.error.contains("macOS 不支持 CUDA 人脸检测任务"));
}

#[test]
fn test_15_non_cuda_execution_provider_rejected_from_claiming_detect() {
    let (config, _tmp) = create_temp_env();
    let now = agent_dispatcher::now_sec();
    seed_node(
        &config,
        "node-linux-cpu-prov",
        "tok-linux-cpu-prov",
        "linux",
        Some("NVIDIA GeForce RTX 4070 Ti"),
        12_884_901_888,
        "accepting",
        1,
        1,
    );
    seed_task(&config, "vid-non-cuda-reject", "detect", 10);

    let conn = processing_scheduler::open_or_init(&config).unwrap();
    conn.execute(
        "CREATE TABLE IF NOT EXISTS agent_packages (
            package_id TEXT PRIMARY KEY,
            manifest_sha256 TEXT,
            model_sha256 TEXT,
            execution_provider TEXT,
            version TEXT,
            is_pinned INTEGER,
            created_at REAL
        )",
        [],
    ).unwrap();
    conn.execute(
        "CREATE TABLE IF NOT EXISTS agent_deployments (
            deployment_id TEXT PRIMARY KEY,
            node_id TEXT,
            package_id TEXT,
            status TEXT,
            desired_version TEXT,
            stage TEXT,
            updated_at REAL
        )",
        [],
    ).unwrap();

    conn.execute(
        "INSERT INTO agent_packages (package_id, manifest_sha256, model_sha256, execution_provider, version, is_pinned, created_at)
         VALUES ('pkg-cpu-test', 'sha-m', 'sha-mod', 'CPUExecutionProvider', '1.0.0', 1, ?1)",
        params![now],
    ).unwrap();
    conn.execute(
        "INSERT INTO agent_deployments (deployment_id, node_id, package_id, status, desired_version, stage, updated_at)
         VALUES ('dep-cpu-test', 'node-linux-cpu-prov', 'pkg-cpu-test', 'ready', '1.0.0', 'ready', ?1)",
        params![now],
    ).unwrap();

    let claim_res = claim_task(
        &config,
        "Bearer tok-linux-cpu-prov",
        ClaimTaskRequest {
            node_id: Some("node-linux-cpu-prov".to_string()),
            stage: "detect".to_string(),
            lease_duration_s: Some(60.0),
        },
    );

    assert!(claim_res.is_err(), "Node deployed with CPUExecutionProvider must not claim detect");
    let err = claim_res.unwrap_err();
    assert_eq!(err.status, 409);
    assert_eq!(err.code, "capability_mismatch");
    assert!(err.error.contains("并非 CUDAExecutionProvider"));
}

#[test]
fn test_16_macos_node_allowed_to_claim_feature_stage() {
    let (config, _tmp) = create_temp_env();
    seed_node(
        &config,
        "node-mac-feat-ok",
        "tok-mac-feat-ok",
        "macos",
        Some("Apple M2 Pro"),
        16_000_000_000,
        "accepting",
        1,
        1,
    );
    seed_task(&config, "vid-mac-feat-1", "feature", 5);

    let claim_res = claim_task(
        &config,
        "Bearer tok-mac-feat-ok",
        ClaimTaskRequest {
            node_id: Some("node-mac-feat-ok".to_string()),
            stage: "feature".to_string(),
            lease_duration_s: Some(60.0),
        },
    );

    assert!(claim_res.is_ok(), "macOS node must successfully claim feature stage");
    let claim = claim_res.unwrap().expect("task leased");
    assert_eq!(claim.vid, "vid-mac-feat-1");
    assert_eq!(claim.stage, "feature");
}

#[test]
fn test_17_cpu_node_feature_stage_dispatches_landmarks_and_bbox() {
    let (config, _tmp) = create_temp_env();
    seed_node(
        &config,
        "node-cpu-feat",
        "tok-cpu-feat",
        "darwin",
        None,
        0,
        "accepting",
        1,
        1,
    );
    seed_task(&config, "vid-feat-landmarks", "feature", 10);

    let claim_res = claim_task(
        &config,
        "Bearer tok-cpu-feat",
        ClaimTaskRequest {
            node_id: Some("node-cpu-feat".to_string()),
            stage: "feature".to_string(),
            lease_duration_s: Some(60.0),
        },
    );

    assert!(claim_res.is_ok());
    let claim = claim_res.unwrap().expect("task leased");
    assert_eq!(claim.stage, "feature");
    assert!(!claim.frames.is_empty());
    let frame = &claim.frames[0];
    assert!(frame.landmarks.is_some(), "Landmarks must be populated for feature extraction");
    assert_eq!(frame.landmarks.as_ref().unwrap().len(), 5);
    assert!(frame.bbox.is_some(), "Bbox must be populated");
    assert!(frame.det_score.is_some(), "Detection score must be populated");
}

#[test]
fn test_18_feature_commit_validates_512_dim_and_rejects_dimension_mismatch() {
    let (config, _tmp) = create_temp_env();
    seed_node(
        &config,
        "node-dim-val",
        "tok-dim-val",
        "linux",
        None,
        0,
        "accepting",
        1,
        1,
    );
    seed_task(&config, "vid-dim-val", "feature", 10);

    let claim = claim_task(
        &config,
        "Bearer tok-dim-val",
        ClaimTaskRequest {
            node_id: Some("node-dim-val".to_string()),
            stage: "feature".to_string(),
            lease_duration_s: Some(60.0),
        },
    )
    .unwrap()
    .expect("task leased");

    // Submit 511-dim vector (too short)
    let vec_511: Vec<f64> = vec![0.044; 511];
    let res = commit_task_result(
        &config,
        "Bearer tok-dim-val",
        CommitTaskResultRequest {
            node_id: Some("node-dim-val".to_string()),
            attempt_id: Some(claim.attempt_id.clone()),
            vid: Some("vid-dim-val".to_string()),
            coordinator_generation: Some(json!(claim.coordinator_generation)),
            stage: Some("feature".to_string()),
            detections: None,
            vectors: Some(vec![json!(vec_511)]),
            text_snippets: None,
            execution_proof: None,
            status: Some("completed".to_string()),
            error_type: None,
            error_message: None,
        },
    );

    assert!(res.is_err(), "511-dim vector must be rejected with 422");
    let err = res.unwrap_err();
    assert_eq!(err.status, 422);
    assert_eq!(err.code, "invalid_vector_dimension");
}

#[test]
fn test_19_feature_commit_rejects_nan_and_infinity_values() {
    let (config, _tmp) = create_temp_env();
    seed_node(
        &config,
        "node-nan-val",
        "tok-nan-val",
        "linux",
        None,
        0,
        "accepting",
        1,
        1,
    );
    seed_task(&config, "vid-nan-val", "feature", 10);

    let claim = claim_task(
        &config,
        "Bearer tok-nan-val",
        ClaimTaskRequest {
            node_id: Some("node-nan-val".to_string()),
            stage: "feature".to_string(),
            lease_duration_s: Some(60.0),
        },
    )
    .unwrap()
    .expect("task leased");

    // Vector with NaN/string non-finite
    let mut vec_nan: Vec<serde_json::Value> = (0..511).map(|_| json!(0.044)).collect();
    vec_nan.push(json!("NaN"));

    let res = commit_task_result(
        &config,
        "Bearer tok-nan-val",
        CommitTaskResultRequest {
            node_id: Some("node-nan-val".to_string()),
            attempt_id: Some(claim.attempt_id),
            vid: Some("vid-nan-val".to_string()),
            coordinator_generation: Some(json!(claim.coordinator_generation)),
            stage: Some("feature".to_string()),
            detections: None,
            vectors: Some(vec![json!(vec_nan)]),
            text_snippets: None,
            execution_proof: None,
            status: Some("completed".to_string()),
            error_type: None,
            error_message: None,
        },
    );

    assert!(res.is_err());
    let err = res.unwrap_err();
    assert_eq!(err.status, 422);
    assert_eq!(err.code, "non_finite_vector_value");
}

#[test]
fn test_20_feature_commit_rejects_unnormalized_vectors() {
    let (config, _tmp) = create_temp_env();
    seed_node(
        &config,
        "node-norm-val",
        "tok-norm-val",
        "linux",
        None,
        0,
        "accepting",
        1,
        1,
    );
    seed_task(&config, "vid-norm-val", "feature", 10);

    let claim = claim_task(
        &config,
        "Bearer tok-norm-val",
        ClaimTaskRequest {
            node_id: Some("node-norm-val".to_string()),
            stage: "feature".to_string(),
            lease_duration_s: Some(60.0),
        },
    )
    .unwrap()
    .expect("task leased");

    // Unnormalized vector: all zeros -> norm 0.0
    let vec_zero: Vec<f64> = vec![0.0; 512];
    let res = commit_task_result(
        &config,
        "Bearer tok-norm-val",
        CommitTaskResultRequest {
            node_id: Some("node-norm-val".to_string()),
            attempt_id: Some(claim.attempt_id),
            vid: Some("vid-norm-val".to_string()),
            coordinator_generation: Some(json!(claim.coordinator_generation)),
            stage: Some("feature".to_string()),
            detections: None,
            vectors: Some(vec![json!(vec_zero)]),
            text_snippets: None,
            execution_proof: None,
            status: Some("completed".to_string()),
            error_type: None,
            error_message: None,
        },
    );

    assert!(res.is_err());
    let err = res.unwrap_err();
    assert_eq!(err.status, 422);
    assert_eq!(err.code, "invalid_vector_norm");
}

#[test]
fn test_21_ocr_claim_with_roi_box() {
    let (config, _tmp) = create_temp_env();
    seed_node(
        &config,
        "node-ocr-roi",
        "tok-ocr-roi",
        "darwin",
        None,
        0,
        "accepting",
        1,
        1,
    );
    seed_task(&config, "vid-ocr-roi-1", "ocr", 10);

    let claim = claim_task(
        &config,
        "Bearer tok-ocr-roi",
        ClaimTaskRequest {
            node_id: Some("node-ocr-roi".to_string()),
            stage: "ocr".to_string(),
            lease_duration_s: Some(60.0),
        },
    )
    .unwrap()
    .expect("task leased");

    assert_eq!(claim.stage, "ocr");
    assert!(!claim.frames.is_empty());
    assert!(claim.frames[0].roi_box.is_some(), "ROI box should be present for OCR stage");
}

#[test]
fn test_22_ocr_unambiguous_rule_match_skips_face() {
    let (config, _tmp) = create_temp_env();
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    conn.execute(
        "INSERT INTO account_rules (id, account_name, creator_name, alias_list, action)
         VALUES ('r1', 'creator_alice', 'AliceCreator', '[\"alice_studio\"]', 'skip_face')",
        [],
    ).unwrap();

    seed_node(
        &config,
        "node-ocr-match",
        "tok-ocr-match",
        "linux",
        None,
        0,
        "accepting",
        1,
        1,
    );
    seed_task(&config, "vid-ocr-match", "ocr", 10);

    let claim = claim_task(
        &config,
        "Bearer tok-ocr-match",
        ClaimTaskRequest {
            node_id: Some("node-ocr-match".to_string()),
            stage: "ocr".to_string(),
            lease_duration_s: Some(60.0),
        },
    )
    .unwrap()
    .expect("task leased");

    let res = commit_task_result(
        &config,
        "Bearer tok-ocr-match",
        CommitTaskResultRequest {
            node_id: Some("node-ocr-match".to_string()),
            attempt_id: Some(claim.attempt_id),
            vid: Some("vid-ocr-match".to_string()),
            coordinator_generation: Some(json!(claim.coordinator_generation)),
            stage: Some("ocr".to_string()),
            detections: None,
            vectors: None,
            text_snippets: Some(vec![json!("关注抖音：creator_alice 谢谢支持")]),
            execution_proof: None,
            status: Some("completed".to_string()),
            error_type: None,
            error_message: None,
        },
    ).unwrap();

    assert!(res.ok);
    assert_eq!(res.next_stage.as_deref(), Some("done"));
    assert_eq!(res.completed, Some(true));

    let (stage, data_raw): (String, String) = conn
        .query_row("SELECT stage, data FROM tasks WHERE vid = 'vid-ocr-match'", [], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap();
    assert_eq!(stage, "done");
    let d: serde_json::Value = serde_json::from_str(&data_raw).unwrap();
    assert_eq!(d["face_skipped"], json!(true));
    assert_eq!(d["creator"], json!("AliceCreator"));
}

#[test]
fn test_23_ocr_ambiguous_rule_match_does_not_skip_face() {
    let (config, _tmp) = create_temp_env();
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    conn.execute(
        "INSERT INTO account_rules (id, account_name, creator_name, alias_list, action)
         VALUES ('r1', 'creator_alice', 'AliceCreator', '[]', 'skip_face'),
                ('r2', 'creator_bob', 'BobCreator', '[]', 'skip_face')",
        [],
    ).unwrap();

    seed_node(
        &config,
        "node-ocr-ambig",
        "tok-ocr-ambig",
        "linux",
        None,
        0,
        "accepting",
        1,
        1,
    );
    seed_task(&config, "vid-ocr-ambig", "ocr", 10);

    let claim = claim_task(
        &config,
        "Bearer tok-ocr-ambig",
        ClaimTaskRequest {
            node_id: Some("node-ocr-ambig".to_string()),
            stage: "ocr".to_string(),
            lease_duration_s: Some(60.0),
        },
    )
    .unwrap()
    .expect("task leased");

    // Matches both Alice and Bob -> Ambiguous conflict!
    let res = commit_task_result(
        &config,
        "Bearer tok-ocr-ambig",
        CommitTaskResultRequest {
            node_id: Some("node-ocr-ambig".to_string()),
            attempt_id: Some(claim.attempt_id),
            vid: Some("vid-ocr-ambig".to_string()),
            coordinator_generation: Some(json!(claim.coordinator_generation)),
            stage: Some("ocr".to_string()),
            detections: None,
            vectors: None,
            text_snippets: Some(vec![json!("合作视频：creator_alice x creator_bob")]),
            execution_proof: None,
            status: Some("completed".to_string()),
            error_type: None,
            error_message: None,
        },
    ).unwrap();

    assert!(res.ok);
    assert_eq!(res.next_stage.as_deref(), Some("detect"));
    assert_eq!(res.completed, Some(false));

    let (stage, data_raw): (String, String) = conn
        .query_row("SELECT stage, data FROM tasks WHERE vid = 'vid-ocr-ambig'", [], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap();
    assert_eq!(stage, "detect");
    let d: serde_json::Value = serde_json::from_str(&data_raw).unwrap();
    assert_eq!(d["face_skipped"], json!(false));
}

#[test]
fn test_24_ocr_no_text_found_advances_to_detect() {
    let (config, _tmp) = create_temp_env();
    seed_node(
        &config,
        "node-ocr-none",
        "tok-ocr-none",
        "linux",
        None,
        0,
        "accepting",
        1,
        1,
    );
    seed_task(&config, "vid-ocr-notext", "ocr", 10);

    let claim = claim_task(
        &config,
        "Bearer tok-ocr-none",
        ClaimTaskRequest {
            node_id: Some("node-ocr-none".to_string()),
            stage: "ocr".to_string(),
            lease_duration_s: Some(60.0),
        },
    )
    .unwrap()
    .expect("task leased");

    let res = commit_task_result(
        &config,
        "Bearer tok-ocr-none",
        CommitTaskResultRequest {
            node_id: Some("node-ocr-none".to_string()),
            attempt_id: Some(claim.attempt_id),
            vid: Some("vid-ocr-notext".to_string()),
            coordinator_generation: Some(json!(claim.coordinator_generation)),
            stage: Some("ocr".to_string()),
            detections: None,
            vectors: None,
            text_snippets: Some(vec![]),
            execution_proof: None,
            status: Some("completed".to_string()),
            error_type: Some("no_text_found".to_string()),
            error_message: None,
        },
    ).unwrap();

    assert!(res.ok);
    assert_eq!(res.next_stage.as_deref(), Some("detect"));
    assert_eq!(res.completed, Some(false));

    let conn = processing_scheduler::open_or_init(&config).unwrap();
    let (stage, error, data_raw): (String, Option<String>, String) = conn
        .query_row("SELECT stage, error, data FROM tasks WHERE vid = 'vid-ocr-notext'", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap();
    assert_eq!(stage, "detect");
    assert!(error.is_none(), "Clean OCR with no text is NOT an error");
    let d: serde_json::Value = serde_json::from_str(&data_raw).unwrap();
    assert_eq!(d["ocr_result"], json!("no_text_found"));
}

#[test]
fn test_25_ocr_parse_failure_records_error_and_retries() {
    let (config, _tmp) = create_temp_env();
    seed_node(
        &config,
        "node-ocr-fail",
        "tok-ocr-fail",
        "linux",
        None,
        0,
        "accepting",
        1,
        1,
    );
    seed_task(&config, "vid-ocr-fail-1", "ocr", 10);

    let claim = claim_task(
        &config,
        "Bearer tok-ocr-fail",
        ClaimTaskRequest {
            node_id: Some("node-ocr-fail".to_string()),
            stage: "ocr".to_string(),
            lease_duration_s: Some(60.0),
        },
    )
    .unwrap()
    .expect("task leased");

    let res = commit_task_result(
        &config,
        "Bearer tok-ocr-fail",
        CommitTaskResultRequest {
            node_id: Some("node-ocr-fail".to_string()),
            attempt_id: Some(claim.attempt_id.clone()),
            vid: Some("vid-ocr-fail-1".to_string()),
            coordinator_generation: Some(json!(claim.coordinator_generation)),
            stage: Some("ocr".to_string()),
            detections: None,
            vectors: None,
            text_snippets: None,
            execution_proof: None,
            status: Some("failed".to_string()),
            error_type: Some("parse_failure".to_string()),
            error_message: Some("corrupt_jpeg_header".to_string()),
        },
    ).unwrap();

    assert!(res.ok);
    assert_eq!(res.next_stage.as_deref(), Some("retry"));
    assert_eq!(res.completed, Some(false));

    let conn = processing_scheduler::open_or_init(&config).unwrap();
    let (stage, retry_stage, error, text_attempts): (String, String, Option<String>, i64) = conn
        .query_row("SELECT stage, retry_stage, error, text_attempts FROM tasks WHERE vid = 'vid-ocr-fail-1'", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap();
    assert_eq!(stage, "retry");
    assert_eq!(retry_stage, "ocr");
    assert_eq!(error.as_deref(), Some("corrupt_jpeg_header"));
    assert_eq!(text_attempts, 1);
}

#[test]
fn test_26_scheduler_reaps_expired_ocr_and_feature_leases() {
    let (config, _tmp) = create_temp_env();
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    conn.execute("UPDATE settings SET value = '10' WHERE key = 'concurrency'", []).unwrap();

    seed_node(
        &config,
        "node-reap",
        "tok-reap",
        "linux",
        None,
        0,
        "accepting",
        1,
        1,
    );
    seed_task(&config, "vid-reap-ocr", "ocr", 1);
    seed_task(&config, "vid-reap-feat", "feature", 1);

    let _claim_ocr = claim_task(
        &config,
        "Bearer tok-reap",
        ClaimTaskRequest {
            node_id: Some("node-reap".to_string()),
            stage: "ocr".to_string(),
            lease_duration_s: Some(10.0),
        },
    )
    .unwrap()
    .expect("ocr task leased");

    let _claim_feat = claim_task(
        &config,
        "Bearer tok-reap",
        ClaimTaskRequest {
            node_id: Some("node-reap".to_string()),
            stage: "feature".to_string(),
            lease_duration_s: Some(10.0),
        },
    )
    .unwrap()
    .expect("feat task leased");

    let conn = processing_scheduler::open_or_init(&config).unwrap();
    let now = agent_dispatcher::now_sec();

    // Expire leases
    conn.execute(
        "UPDATE tasks SET lease_expires_at = ?1 WHERE vid IN ('vid-reap-ocr', 'vid-reap-feat')",
        params![now - 20.0],
    ).unwrap();

    // Reap leases
    let reap_res = processing_scheduler::reap_expired_leases(&conn, now).unwrap();
    assert_eq!(reap_res.count, 2, "Both expired OCR and Feature leases must be reaped");

    let ocr_stage: String = conn.query_row("SELECT stage FROM tasks WHERE vid = 'vid-reap-ocr'", [], |r| r.get(0)).unwrap();
    let feat_stage: String = conn.query_row("SELECT stage FROM tasks WHERE vid = 'vid-reap-feat'", [], |r| r.get(0)).unwrap();
    assert_eq!(ocr_stage, "retry");
    assert_eq!(feat_stage, "retry");
}
