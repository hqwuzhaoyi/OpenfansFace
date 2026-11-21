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
    claim_task, commit_task_result, cosine_similarity, init_tables, now_sec, sha256_hex,
    ClaimTaskRequest, CommitTaskResultRequest,
};

fn create_temp_env() -> (Config, PathBuf) {
    let tmp_dir = std::env::temp_dir().join(format!("face-m4-chal-{}", uuid::Uuid::new_v4()));
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

fn seed_node_m4(
    config: &Config,
    node_id: &str,
    token: &str,
    os: &str,
    arch: &str,
    gpu_model: Option<&str>,
    vram_bytes: i64,
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
            ?1, ?2, 'inst-m4', 'host-m4', ?3, ?4, '1.0.0',
            8, 34359738368, ?5, ?6, ?6,
            'accepting', 1, 1, 0, ?7,
            ?7, ?7
        )",
        params![
            node_id,
            token_hash,
            os,
            arch,
            gpu_model,
            vram_bytes,
            now,
        ],
    ).unwrap();
}

fn seed_task_m4(config: &Config, vid: &str, stage: &str) {
    let conn = processing_scheduler::open_or_init(config).unwrap();
    let now = now_sec();

    conn.execute(
        "INSERT INTO tasks (
            vid, path, name, size, mtime_ns, stage, priority, batch_id,
            created_at, updated_at, data
        ) VALUES (
            ?1, ?2, ?3, 1048576, '1700000000000000', ?4, 10, 'batch-m4',
            ?5, ?5, '{}'
        )",
        params![
            vid,
            format!("/isolated/media/{}.mp4", vid),
            format!("{}.mp4", vid),
            stage,
            now,
        ],
    ).unwrap();
}

// =============================================================================
// Feature 13: Cosine Similarity Tolerance Drift Invariant (cos >= 0.9999)
// =============================================================================

#[test]
fn challenge_cosine_similarity_identical_vectors_pass() {
    let v = generate_unit_vector(512, 0.42);
    let sim = cosine_similarity(&v, &v);
    assert!((sim - 1.0).abs() < 1e-6, "Self cosine similarity must equal 1.0, got: {}", sim);
    assert!(sim >= 0.9999, "Self cosine similarity must exceed 0.9999 threshold");
}

#[test]
fn challenge_cosine_similarity_cross_platform_fp_perturbation_pass() {
    let v_arm = generate_unit_vector(512, 1.23);
    // Simulate ARM64 NEON vs x86_64 AVX2 micro floating point variance (<= 1e-6)
    let mut v_x86 = Vec::with_capacity(512);
    let mut norm_sq = 0.0f32;
    for &x in &v_arm {
        let perturbed = x + 1e-6;
        v_x86.push(perturbed);
        norm_sq += perturbed * perturbed;
    }
    let norm = norm_sq.sqrt();
    for x in v_x86.iter_mut() {
        *x /= norm;
    }

    let sim = cosine_similarity(&v_arm, &v_x86);
    assert!(sim >= 0.9999, "Micro FP rounding difference must satisfy cos >= 0.9999, got: {}", sim);
}

#[test]
fn challenge_cosine_similarity_excessive_drift_rejection() {
    let v_base = generate_unit_vector(512, 2.5);
    // Perturbation of 0.02 causes cosine drift below 0.9999
    let mut v_drifted = Vec::with_capacity(512);
    let mut norm_sq = 0.0f32;
    for &x in &v_base {
        let val = x + 0.02;
        v_drifted.push(val);
        norm_sq += val * val;
    }
    let norm = norm_sq.sqrt();
    for x in v_drifted.iter_mut() {
        *x /= norm;
    }

    let sim = cosine_similarity(&v_base, &v_drifted);
    assert!(sim < 0.9999, "Drifted vector must not meet 0.9999 threshold, got: {}", sim);
}

#[test]
fn challenge_cosine_similarity_zero_and_mismatched_dimension_safety() {
    let v_zero = vec![0.0f32; 512];
    let v_normal = generate_unit_vector(512, 0.5);
    let sim_zero = cosine_similarity(&v_zero, &v_normal);
    assert_eq!(sim_zero, 0.0, "Zero vector cosine similarity must be 0.0");

    let v_short = generate_unit_vector(256, 0.5);
    let sim_mismatch = cosine_similarity(&v_short, &v_normal);
    assert_eq!(sim_mismatch, 0.0, "Mismatched dimension vector similarity must return 0.0");
}

// =============================================================================
// Feature 12: Remote CPU Feature Extraction & Vector Validation
// =============================================================================

#[test]
fn challenge_feature_extraction_claim_and_validation() {
    let (config, _tmp) = create_temp_env();
    seed_node_m4(&config, "node-mac-m4", "tok-mac-m4", "darwin", "arm64", None, 0);
    seed_task_m4(&config, "vid-feat-e2e", "feature");

    let claim = claim_task(
        &config,
        "Bearer tok-mac-m4",
        ClaimTaskRequest {
            node_id: Some("node-mac-m4".to_string()),
            stage: "feature".to_string(),
            lease_duration_s: Some(60.0),
        },
    ).unwrap().expect("feature task leased");

    assert_eq!(claim.stage, "feature");
    assert!(!claim.frames.is_empty());
    assert!(claim.frames[0].landmarks.is_some(), "Landmarks must be present for CPU face recognition");
    assert_eq!(claim.frames[0].landmarks.as_ref().unwrap().len(), 5);

    // 1. Commit invalid dimension (510 instead of 512)
    let bad_dim_vec: Vec<f64> = vec![0.044; 510];
    let res_bad_dim = commit_task_result(
        &config,
        "Bearer tok-mac-m4",
        CommitTaskResultRequest {
            node_id: Some("node-mac-m4".to_string()),
            attempt_id: Some(claim.attempt_id.clone()),
            vid: Some("vid-feat-e2e".to_string()),
            coordinator_generation: Some(json!(claim.coordinator_generation)),
            stage: Some("feature".to_string()),
            detections: None,
            vectors: Some(vec![json!(bad_dim_vec)]),
            text_snippets: None,
            execution_proof: None,
            status: Some("completed".to_string()),
            error_type: None,
            error_message: None,
        },
    );
    assert!(res_bad_dim.is_err());
    let err = res_bad_dim.unwrap_err();
    assert_eq!(err.status, 422);
    assert_eq!(err.code, "invalid_vector_dimension");

    // 2. Commit unnormalized vector (all 1.0, norm = sqrt(512) ~ 22.6)
    let unnorm_vec: Vec<f64> = vec![1.0; 512];
    let res_unnorm = commit_task_result(
        &config,
        "Bearer tok-mac-m4",
        CommitTaskResultRequest {
            node_id: Some("node-mac-m4".to_string()),
            attempt_id: Some(claim.attempt_id.clone()),
            vid: Some("vid-feat-e2e".to_string()),
            coordinator_generation: Some(json!(claim.coordinator_generation)),
            stage: Some("feature".to_string()),
            detections: None,
            vectors: Some(vec![json!(unnorm_vec)]),
            text_snippets: None,
            execution_proof: None,
            status: Some("completed".to_string()),
            error_type: None,
            error_message: None,
        },
    );
    assert!(res_unnorm.is_err());
    let err = res_unnorm.unwrap_err();
    assert_eq!(err.status, 422);
    assert_eq!(err.code, "invalid_vector_norm");

    // 3. Commit valid 512-dim normalized vector
    let valid_vec = generate_unit_vector(512, 0.88);
    let res_valid = commit_task_result(
        &config,
        "Bearer tok-mac-m4",
        CommitTaskResultRequest {
            node_id: Some("node-mac-m4".to_string()),
            attempt_id: Some(claim.attempt_id.clone()),
            vid: Some("vid-feat-e2e".to_string()),
            coordinator_generation: Some(json!(claim.coordinator_generation)),
            stage: Some("feature".to_string()),
            detections: None,
            vectors: Some(vec![json!(valid_vec)]),
            text_snippets: None,
            execution_proof: Some(json!({"provider": "CPUExecutionProvider", "arch": "arm64"})),
            status: Some("completed".to_string()),
            error_type: None,
            error_message: None,
        },
    ).unwrap();

    assert!(res_valid.ok);
    assert_eq!(res_valid.next_stage.as_deref(), Some("done"));
    assert_eq!(res_valid.completed, Some(true));

    // Verify task database persistence
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    let (stage, data_raw): (String, String) = conn
        .query_row("SELECT stage, data FROM tasks WHERE vid = 'vid-feat-e2e'", [], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap();
    assert_eq!(stage, "done");
    let d: serde_json::Value = serde_json::from_str(&data_raw).unwrap();
    assert_eq!(d["vectors_count"], json!(1));
    assert!(d["vectors"].is_array());
}

// =============================================================================
// Feature 14, 15, 16: Remote OCR, Account Rules & Explicit Error Distinction
// =============================================================================

#[test]
fn challenge_remote_ocr_roi_dispatch_and_unambiguous_match_skips_face() {
    let (config, _tmp) = create_temp_env();
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    conn.execute(
        "INSERT INTO account_rules (id, account_name, creator_name, alias_list, action)
         VALUES ('rule-zhang', 'creator_zhang', '张三', '[\"zs_official\"]', 'skip_face')",
        [],
    ).unwrap();

    seed_node_m4(&config, "node-ocr-worker", "tok-ocr-w", "linux", "x86_64", None, 0);
    seed_task_m4(&config, "vid-skip-zhang", "ocr");

    let claim = claim_task(
        &config,
        "Bearer tok-ocr-w",
        ClaimTaskRequest {
            node_id: Some("node-ocr-worker".to_string()),
            stage: "ocr".to_string(),
            lease_duration_s: Some(60.0),
        },
    ).unwrap().expect("ocr task leased");

    // Verify ROI crop dispatch contract
    assert_eq!(claim.stage, "ocr");
    assert!(claim.frames[0].roi_box.is_some(), "ROI box required in claim frame");

    // Commit unambiguous creator watermark match
    let res = commit_task_result(
        &config,
        "Bearer tok-ocr-w",
        CommitTaskResultRequest {
            node_id: Some("node-ocr-worker".to_string()),
            attempt_id: Some(claim.attempt_id),
            vid: Some("vid-skip-zhang".to_string()),
            coordinator_generation: Some(json!(claim.coordinator_generation)),
            stage: Some("ocr".to_string()),
            detections: None,
            vectors: None,
            text_snippets: Some(vec![json!("关注快手官方：zs_official 感谢支持")]),
            execution_proof: Some(json!({"engine": "PaddleOCR-mobile"})),
            status: Some("completed".to_string()),
            error_type: None,
            error_message: None,
        },
    ).unwrap();

    assert!(res.ok);
    assert_eq!(res.next_stage.as_deref(), Some("done"));
    assert_eq!(res.completed, Some(true));

    let (stage, data_raw): (String, String) = conn
        .query_row("SELECT stage, data FROM tasks WHERE vid = 'vid-skip-zhang'", [], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap();
    assert_eq!(stage, "done");
    let d: serde_json::Value = serde_json::from_str(&data_raw).unwrap();
    assert_eq!(d["face_skipped"], json!(true));
    assert_eq!(d["creator"], json!("张三"));
    assert_eq!(d["skip_reason"], json!("account_rule_match"));
}

#[test]
fn challenge_ocr_ambiguous_conflict_does_not_skip_face() {
    let (config, _tmp) = create_temp_env();
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    conn.execute(
        "INSERT INTO account_rules (id, account_name, creator_name, alias_list, action)
         VALUES ('r-a', 'creator_zhang', '张三', '[]', 'skip_face'),
                ('r-b', 'creator_li', '李四', '[]', 'skip_face')",
        [],
    ).unwrap();

    seed_node_m4(&config, "node-ocr-amb", "tok-ocr-amb", "darwin", "arm64", None, 0);
    seed_task_m4(&config, "vid-amb-conflict", "ocr");

    let claim = claim_task(
        &config,
        "Bearer tok-ocr-amb",
        ClaimTaskRequest {
            node_id: Some("node-ocr-amb".to_string()),
            stage: "ocr".to_string(),
            lease_duration_s: Some(60.0),
        },
    ).unwrap().expect("ocr task leased");

    // Text matches both Zhang and Li
    let res = commit_task_result(
        &config,
        "Bearer tok-ocr-amb",
        CommitTaskResultRequest {
            node_id: Some("node-ocr-amb".to_string()),
            attempt_id: Some(claim.attempt_id),
            vid: Some("vid-amb-conflict".to_string()),
            coordinator_generation: Some(json!(claim.coordinator_generation)),
            stage: Some("ocr".to_string()),
            detections: None,
            vectors: None,
            text_snippets: Some(vec![json!("联合投稿：creator_zhang 和 creator_li")]),
            execution_proof: None,
            status: Some("completed".to_string()),
            error_type: None,
            error_message: None,
        },
    ).unwrap();

    assert!(res.ok);
    assert_eq!(res.next_stage.as_deref(), Some("detect"), "Ambiguous match must cascade to detect!");
    assert_eq!(res.completed, Some(false));

    let (stage, data_raw): (String, String) = conn
        .query_row("SELECT stage, data FROM tasks WHERE vid = 'vid-amb-conflict'", [], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap();
    assert_eq!(stage, "detect");
    let d: serde_json::Value = serde_json::from_str(&data_raw).unwrap();
    assert_eq!(d["face_skipped"], json!(false));
}

#[test]
fn challenge_ocr_parse_failure_and_model_error_explicit_error_distinction() {
    let (config, _tmp) = create_temp_env();
    seed_node_m4(&config, "node-ocr-err", "tok-ocr-err", "linux", "x86_64", None, 0);
    seed_task_m4(&config, "vid-parse-err", "ocr");
    seed_task_m4(&config, "vid-model-err", "ocr");

    // Case 1: parse_failure
    let claim1 = claim_task(
        &config,
        "Bearer tok-ocr-err",
        ClaimTaskRequest {
            node_id: Some("node-ocr-err".to_string()),
            stage: "ocr".to_string(),
            lease_duration_s: Some(60.0),
        },
    ).unwrap().expect("task 1 leased");

    let res1 = commit_task_result(
        &config,
        "Bearer tok-ocr-err",
        CommitTaskResultRequest {
            node_id: Some("node-ocr-err".to_string()),
            attempt_id: Some(claim1.attempt_id),
            vid: Some("vid-parse-err".to_string()),
            coordinator_generation: Some(json!(claim1.coordinator_generation)),
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

    assert!(res1.ok);
    assert_eq!(res1.next_stage.as_deref(), Some("retry"));
    assert_eq!(res1.completed, Some(false));

    let conn = processing_scheduler::open_or_init(&config).unwrap();
    let (stage1, error1, attempts1): (String, Option<String>, i64) = conn
        .query_row("SELECT stage, error, text_attempts FROM tasks WHERE vid = 'vid-parse-err'", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap();
    assert_eq!(stage1, "retry");
    assert_eq!(error1.as_deref(), Some("corrupt_jpeg_header"));
    assert_eq!(attempts1, 1);

    // Case 2: model_error
    let claim2 = claim_task(
        &config,
        "Bearer tok-ocr-err",
        ClaimTaskRequest {
            node_id: Some("node-ocr-err".to_string()),
            stage: "ocr".to_string(),
            lease_duration_s: Some(60.0),
        },
    ).unwrap().expect("task 2 leased");

    let res2 = commit_task_result(
        &config,
        "Bearer tok-ocr-err",
        CommitTaskResultRequest {
            node_id: Some("node-ocr-err".to_string()),
            attempt_id: Some(claim2.attempt_id),
            vid: Some("vid-model-err".to_string()),
            coordinator_generation: Some(json!(claim2.coordinator_generation)),
            stage: Some("ocr".to_string()),
            detections: None,
            vectors: None,
            text_snippets: None,
            execution_proof: None,
            status: Some("failed".to_string()),
            error_type: Some("model_error".to_string()),
            error_message: Some("paddleocr_oom".to_string()),
        },
    ).unwrap();

    assert!(res2.ok);
    assert_eq!(res2.next_stage.as_deref(), Some("retry"));
    assert_eq!(res2.completed, Some(false));

    let (stage2, error2): (String, Option<String>) = conn
        .query_row("SELECT stage, error FROM tasks WHERE vid = 'vid-model-err'", [], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap();
    assert_eq!(stage2, "retry");
    assert_eq!(error2.as_deref(), Some("paddleocr_oom"));
}
