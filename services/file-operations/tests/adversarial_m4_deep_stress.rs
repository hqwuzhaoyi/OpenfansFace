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
    let tmp_dir = std::env::temp_dir().join(format!("face-m4-adv-stress-{}", uuid::Uuid::new_v4()));
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

fn seed_node(
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
            ?1, ?2, 'inst-m4-adv', 'host-m4-adv', ?3, ?4, '1.0.0',
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

fn seed_task(config: &Config, vid: &str, stage: &str) {
    let conn = processing_scheduler::open_or_init(config).unwrap();
    let now = now_sec();

    conn.execute(
        "INSERT INTO tasks (
            vid, path, name, size, mtime_ns, stage, priority, batch_id,
            created_at, updated_at, data
        ) VALUES (
            ?1, ?2, ?3, 1048576, '1700000000000000', ?4, 10, 'batch-m4-adv',
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
// Feature 12 & 13: Empirical Adversarial Tests
// =============================================================================

#[test]
fn stress_test_vector_dimension_boundaries() {
    let (config, _tmp) = create_temp_env();
    seed_node(&config, "node-dim-tester", "tok-dim", "darwin", "arm64", None, 0);

    let test_lengths = [0, 1, 511, 513, 1024];

    for len in test_lengths {
        let vid = format!("vid-dim-{}", len);
        seed_task(&config, &vid, "feature");

        let claim = claim_task(
            &config,
            "Bearer tok-dim",
            ClaimTaskRequest {
                node_id: Some("node-dim-tester".to_string()),
                stage: "feature".to_string(),
                lease_duration_s: Some(60.0),
            },
        ).unwrap().expect("claimed task for dimension test");

        let bad_vec: Vec<f64> = vec![0.044; len];
        let res = commit_task_result(
            &config,
            "Bearer tok-dim",
            CommitTaskResultRequest {
                node_id: Some("node-dim-tester".to_string()),
                attempt_id: Some(claim.attempt_id),
                vid: Some(vid.clone()),
                coordinator_generation: Some(json!(claim.coordinator_generation)),
                stage: Some("feature".to_string()),
                detections: None,
                vectors: Some(vec![json!(bad_vec)]),
                text_snippets: None,
                execution_proof: None,
                status: Some("completed".to_string()),
                error_type: None,
                error_message: None,
            },
        );

        assert!(res.is_err(), "Vector length {} must be rejected", len);
        let err = res.unwrap_err();
        assert_eq!(err.status, 422, "Length {} must yield HTTP 422", len);
        assert_eq!(err.code, "invalid_vector_dimension", "Length {} error code mismatch", len);
    }
}

#[test]
fn stress_test_float_finiteness_rejection() {
    let (config, _tmp) = create_temp_env();
    seed_node(&config, "node-finite-tester", "tok-finite", "darwin", "arm64", None, 0);

    let test_cases = [
        ("nan-value", serde_json::Value::Null), // JSON serialization of NaN
        ("pos-inf-value", serde_json::Value::Null), // JSON serialization of +Inf
        ("str-nan", json!("NaN")),
        ("str-inf", json!("+Infinity")),
        ("str-neg-inf", json!("-Infinity")),
    ];

    for (case_name, non_finite_val) in test_cases {
        let vid = format!("vid-finite-{}", case_name);
        seed_task(&config, &vid, "feature");

        let claim = claim_task(
            &config,
            "Bearer tok-finite",
            ClaimTaskRequest {
                node_id: Some("node-finite-tester".to_string()),
                stage: "feature".to_string(),
                lease_duration_s: Some(60.0),
            },
        ).unwrap().expect("claimed task for finiteness test");

        let mut bad_vec: Vec<serde_json::Value> = vec![json!(0.044); 512];
        bad_vec[42] = non_finite_val;

        let res = commit_task_result(
            &config,
            "Bearer tok-finite",
            CommitTaskResultRequest {
                node_id: Some("node-finite-tester".to_string()),
                attempt_id: Some(claim.attempt_id),
                vid: Some(vid.clone()),
                coordinator_generation: Some(json!(claim.coordinator_generation)),
                stage: Some("feature".to_string()),
                detections: None,
                vectors: Some(vec![json!(bad_vec)]),
                text_snippets: None,
                execution_proof: None,
                status: Some("completed".to_string()),
                error_type: None,
                error_message: None,
            },
        );

        assert!(res.is_err(), "Non-finite case {} must be rejected", case_name);
        let err = res.unwrap_err();
        assert_eq!(err.status, 422, "Case {} must yield HTTP 422", case_name);
        assert_eq!(err.code, "non_finite_vector_value", "Case {} error code mismatch", case_name);
    }
}

#[test]
fn stress_test_l2_unit_norm_boundaries() {
    let (config, _tmp) = create_temp_env();
    seed_node(&config, "node-norm-tester", "tok-norm", "linux", "x86_64", None, 0);

    // Unnormalized vectors that MUST be rejected (HTTP 422 invalid_vector_norm)
    let rejection_norms = [0.0f64, 0.5f64, 2.0f64, 100.0f64, 0.9499f64, 1.0501f64];

    for target_norm in rejection_norms {
        let vid = format!("vid-norm-reject-{:.4}", target_norm);
        seed_task(&config, &vid, "feature");

        let claim = claim_task(
            &config,
            "Bearer tok-norm",
            ClaimTaskRequest {
                node_id: Some("node-norm-tester".to_string()),
                stage: "feature".to_string(),
                lease_duration_s: Some(60.0),
            },
        ).unwrap().expect("claimed task for norm test");

        let vec_val: Vec<f64> = if target_norm == 0.0 {
            vec![0.0; 512]
        } else {
            let elem = target_norm / (512.0f64).sqrt();
            vec![elem; 512]
        };

        let res = commit_task_result(
            &config,
            "Bearer tok-norm",
            CommitTaskResultRequest {
                node_id: Some("node-norm-tester".to_string()),
                attempt_id: Some(claim.attempt_id),
                vid: Some(vid.clone()),
                coordinator_generation: Some(json!(claim.coordinator_generation)),
                stage: Some("feature".to_string()),
                detections: None,
                vectors: Some(vec![json!(vec_val)]),
                text_snippets: None,
                execution_proof: None,
                status: Some("completed".to_string()),
                error_type: None,
                error_message: None,
            },
        );

        assert!(res.is_err(), "Target norm {} must be rejected", target_norm);
        let err = res.unwrap_err();
        assert_eq!(err.status, 422, "Target norm {} must yield HTTP 422", target_norm);
        assert_eq!(err.code, "invalid_vector_norm", "Target norm {} code mismatch", target_norm);
    }

    // Vectors inside tolerance 1.0 +/- 0.05 that MUST pass
    let acceptance_norms = [0.9501f64, 1.0f64, 1.0499f64];
    for target_norm in acceptance_norms {
        let vid = format!("vid-norm-accept-{:.4}", target_norm);
        seed_task(&config, &vid, "feature");

        let claim = claim_task(
            &config,
            "Bearer tok-norm",
            ClaimTaskRequest {
                node_id: Some("node-norm-tester".to_string()),
                stage: "feature".to_string(),
                lease_duration_s: Some(60.0),
            },
        ).unwrap().expect("claimed task for norm test");

        let elem = target_norm / (512.0f64).sqrt();
        let vec_val: Vec<f64> = vec![elem; 512];

        let res = commit_task_result(
            &config,
            "Bearer tok-norm",
            CommitTaskResultRequest {
                node_id: Some("node-norm-tester".to_string()),
                attempt_id: Some(claim.attempt_id),
                vid: Some(vid.clone()),
                coordinator_generation: Some(json!(claim.coordinator_generation)),
                stage: Some("feature".to_string()),
                detections: None,
                vectors: Some(vec![json!(vec_val)]),
                text_snippets: None,
                execution_proof: None,
                status: Some("completed".to_string()),
                error_type: None,
                error_message: None,
            },
        );

        assert!(res.is_ok(), "Target norm {} within 0.05 must be accepted: {:?}", target_norm, res.err());
    }
}

fn construct_vector_with_cosine(u: &[f32], target_cos: f64) -> Vec<f32> {
    // Generate an independent vector and orthogonalize against u via Gram-Schmidt
    let v = generate_unit_vector(u.len(), 9.8765);
    let mut dot_uv: f64 = 0.0;
    for (&x, &y) in u.iter().zip(v.iter()) {
        dot_uv += (x as f64) * (y as f64);
    }
    let mut u_perp = Vec::with_capacity(u.len());
    let mut norm_sq: f64 = 0.0;
    for (&x, &y) in u.iter().zip(v.iter()) {
        let val = (y as f64) - dot_uv * (x as f64);
        u_perp.push(val);
        norm_sq += val * val;
    }
    let norm = norm_sq.sqrt();
    let sin_val = (1.0 - target_cos * target_cos).max(0.0).sqrt();

    let mut w = Vec::with_capacity(u.len());
    for (&ux, &perpx) in u.iter().zip(u_perp.iter()) {
        let val = target_cos * (ux as f64) + sin_val * (perpx / norm);
        w.push(val as f32);
    }
    w
}

#[test]
fn stress_test_cosine_similarity_drift_and_degeneracy() {
    let v_base = generate_unit_vector(512, 3.14159);

    // 1. Exact match: cos = 1.0 (pass >= 0.9999)
    let cos_exact = cosine_similarity(&v_base, &v_base);
    assert!((cos_exact - 1.0).abs() < 1e-6);
    assert!(cos_exact >= 0.9999);

    // 2. Micro-FP variance: cos = 0.99995 (pass >= 0.9999)
    let v_micro = construct_vector_with_cosine(&v_base, 0.99995);
    let cos_micro = cosine_similarity(&v_base, &v_micro);
    assert!(cos_micro >= 0.9999, "cos {} must be >= 0.9999", cos_micro);
    assert!((cos_micro - 0.99995).abs() < 1e-5);

    // 3. Exact threshold: cos = 0.9999000 (pass >= 0.9999)
    let v_thresh = construct_vector_with_cosine(&v_base, 0.9999000);
    let cos_thresh = cosine_similarity(&v_base, &v_thresh);
    assert!(cos_thresh >= 0.9999000, "cos {} must satisfy >= 0.9999", cos_thresh);
    assert!((cos_thresh - 0.9999000).abs() < 1e-5);

    // 4. Below threshold: cos = 0.9998999 and 0.95 (must fail/reject)
    let v_below = construct_vector_with_cosine(&v_base, 0.9998999);
    let cos_below = cosine_similarity(&v_base, &v_below);
    assert!(cos_below < 0.9999000, "cos {} must be < 0.9999", cos_below);

    let v_95 = construct_vector_with_cosine(&v_base, 0.95);
    let cos_95 = cosine_similarity(&v_base, &v_95);
    assert!(cos_95 < 0.9999, "cos {} must be < 0.9999", cos_95);
    assert!((cos_95 - 0.95).abs() < 1e-5);

    // 5. Degenerate vectors: 0-length or all zeros return 0.0 without panic
    let empty: [f32; 0] = [];
    assert_eq!(cosine_similarity(&empty, &empty), 0.0);
    assert_eq!(cosine_similarity(&empty, &v_base), 0.0);
    assert_eq!(cosine_similarity(&v_base, &empty), 0.0);

    let zeros = vec![0.0f32; 512];
    assert_eq!(cosine_similarity(&zeros, &zeros), 0.0);
    assert_eq!(cosine_similarity(&zeros, &v_base), 0.0);
    assert_eq!(cosine_similarity(&v_base, &zeros), 0.0);

    let nan_vec = vec![f32::NAN; 512];
    assert_eq!(cosine_similarity(&nan_vec, &v_base), 0.0);
    assert_eq!(cosine_similarity(&v_base, &nan_vec), 0.0);
}

// =============================================================================
// Feature 14, 15, 16: Empirical Adversarial Tests
// =============================================================================

#[test]
fn stress_test_unambiguous_creator_match_skips_face_and_increments_completed() {
    let (config, _tmp) = create_temp_env();
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    conn.execute(
        "INSERT INTO account_rules (id, account_name, creator_name, alias_list, action)
         VALUES ('rule-alpha', 'official_alice', 'AliceCreator', '[\"alice_vlog\", \"alice_real\"]', 'skip_face')",
        [],
    ).unwrap();

    seed_node(&config, "node-ocr-alice", "tok-alice", "darwin", "arm64", None, 0);
    seed_task(&config, "vid-alice-001", "ocr");

    let initial_completed: i64 = conn
        .query_row("SELECT value FROM settings WHERE key = 'completed_count'", [], |r| r.get::<_, String>(0))
        .unwrap()
        .parse()
        .unwrap();

    let claim = claim_task(
        &config,
        "Bearer tok-alice",
        ClaimTaskRequest {
            node_id: Some("node-ocr-alice".to_string()),
            stage: "ocr".to_string(),
            lease_duration_s: Some(60.0),
        },
    ).unwrap().expect("claimed ocr task");

    let res = commit_task_result(
        &config,
        "Bearer tok-alice",
        CommitTaskResultRequest {
            node_id: Some("node-ocr-alice".to_string()),
            attempt_id: Some(claim.attempt_id),
            vid: Some("vid-alice-001".to_string()),
            coordinator_generation: Some(json!(claim.coordinator_generation)),
            stage: Some("ocr".to_string()),
            detections: None,
            vectors: None,
            text_snippets: Some(vec![json!("本期视频来自作者: @alice_vlog")]),
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
        .query_row("SELECT stage, data FROM tasks WHERE vid = 'vid-alice-001'", [], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap();
    assert_eq!(stage, "done");
    let d: serde_json::Value = serde_json::from_str(&data_raw).unwrap();
    assert_eq!(d["face_skipped"], json!(true));
    assert_eq!(d["creator"], json!("AliceCreator"));
    assert_eq!(d["skip_reason"], json!("account_rule_match"));

    let new_completed: i64 = conn
        .query_row("SELECT value FROM settings WHERE key = 'completed_count'", [], |r| r.get::<_, String>(0))
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(new_completed, initial_completed + 1, "Completed count must increment by exactly 1");
}

#[test]
fn stress_test_ambiguous_conflict_does_not_skip_face_and_does_not_increment_completed() {
    let (config, _tmp) = create_temp_env();
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    conn.execute(
        "INSERT INTO account_rules (id, account_name, creator_name, alias_list, action)
         VALUES ('rule-a', 'creator_a', 'CreatorA', '[]', 'skip_face'),
                ('rule-b', 'creator_b', 'CreatorB', '[]', 'skip_face')",
        [],
    ).unwrap();

    seed_node(&config, "node-ocr-amb", "tok-amb", "linux", "x86_64", None, 0);
    seed_task(&config, "vid-amb-001", "ocr");

    let initial_completed: i64 = conn
        .query_row("SELECT value FROM settings WHERE key = 'completed_count'", [], |r| r.get::<_, String>(0))
        .unwrap()
        .parse()
        .unwrap();

    let claim = claim_task(
        &config,
        "Bearer tok-amb",
        ClaimTaskRequest {
            node_id: Some("node-ocr-amb".to_string()),
            stage: "ocr".to_string(),
            lease_duration_s: Some(60.0),
        },
    ).unwrap().expect("claimed ocr task");

    let res = commit_task_result(
        &config,
        "Bearer tok-amb",
        CommitTaskResultRequest {
            node_id: Some("node-ocr-amb".to_string()),
            attempt_id: Some(claim.attempt_id),
            vid: Some("vid-amb-001".to_string()),
            coordinator_generation: Some(json!(claim.coordinator_generation)),
            stage: Some("ocr".to_string()),
            detections: None,
            vectors: None,
            text_snippets: Some(vec![json!("合作：creator_a 与 creator_b 联合呈现")]),
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
        .query_row("SELECT stage, data FROM tasks WHERE vid = 'vid-amb-001'", [], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap();
    assert_eq!(stage, "detect");
    let d: serde_json::Value = serde_json::from_str(&data_raw).unwrap();
    assert_eq!(d["face_skipped"], json!(false));

    let new_completed: i64 = conn
        .query_row("SELECT value FROM settings WHERE key = 'completed_count'", [], |r| r.get::<_, String>(0))
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(new_completed, initial_completed, "Ambiguous conflict must NOT increment completed count");
}

#[test]
fn stress_test_clean_execution_with_zero_text_advances_to_detect() {
    let (config, _tmp) = create_temp_env();
    seed_node(&config, "node-ocr-notext", "tok-notext", "darwin", "arm64", None, 0);
    seed_task(&config, "vid-notext-001", "ocr");

    let conn = processing_scheduler::open_or_init(&config).unwrap();
    let initial_completed: i64 = conn
        .query_row("SELECT value FROM settings WHERE key = 'completed_count'", [], |r| r.get::<_, String>(0))
        .unwrap()
        .parse()
        .unwrap();

    let claim = claim_task(
        &config,
        "Bearer tok-notext",
        ClaimTaskRequest {
            node_id: Some("node-ocr-notext".to_string()),
            stage: "ocr".to_string(),
            lease_duration_s: Some(60.0),
        },
    ).unwrap().expect("claimed ocr task");

    let res = commit_task_result(
        &config,
        "Bearer tok-notext",
        CommitTaskResultRequest {
            node_id: Some("node-ocr-notext".to_string()),
            attempt_id: Some(claim.attempt_id),
            vid: Some("vid-notext-001".to_string()),
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

    let (stage, data_raw): (String, String) = conn
        .query_row("SELECT stage, data FROM tasks WHERE vid = 'vid-notext-001'", [], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap();
    assert_eq!(stage, "detect");
    let d: serde_json::Value = serde_json::from_str(&data_raw).unwrap();
    assert_eq!(d["ocr_result"], json!("no_text_found"));
    assert_eq!(d["face_skipped"], json!(false));

    let new_completed: i64 = conn
        .query_row("SELECT value FROM settings WHERE key = 'completed_count'", [], |r| r.get::<_, String>(0))
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(new_completed, initial_completed, "Clean no_text_found must NOT increment completed count");
}

#[test]
fn stress_test_explicit_failure_distinction_never_advances_to_detect_or_done() {
    let (config, _tmp) = create_temp_env();
    seed_node(&config, "node-ocr-failures", "tok-fail", "linux", "x86_64", None, 0);

    let error_cases = [
        ("parse_failure", "corrupt_json_payload"),
        ("model_error", "paddleocr_cuda_oom"),
        ("operator_crash", "sigkill_received"),
    ];

    for (err_type, err_msg) in error_cases {
        let vid = format!("vid-fail-{}", err_type);
        seed_task(&config, &vid, "ocr");

        let claim = claim_task(
            &config,
            "Bearer tok-fail",
            ClaimTaskRequest {
                node_id: Some("node-ocr-failures".to_string()),
                stage: "ocr".to_string(),
                lease_duration_s: Some(60.0),
            },
        ).unwrap().expect("claimed task");

        let res = commit_task_result(
            &config,
            "Bearer tok-fail",
            CommitTaskResultRequest {
                node_id: Some("node-ocr-failures".to_string()),
                attempt_id: Some(claim.attempt_id.clone()),
                vid: Some(vid.clone()),
                coordinator_generation: Some(json!(claim.coordinator_generation)),
                stage: Some("ocr".to_string()),
                detections: None,
                vectors: None,
                text_snippets: None,
                execution_proof: None,
                status: Some("failed".to_string()),
                error_type: Some(err_type.to_string()),
                error_message: Some(err_msg.to_string()),
            },
        ).unwrap();

        assert!(res.ok);
        assert_ne!(res.next_stage.as_deref(), Some("detect"), "Failure must NEVER advance to detect");
        assert_ne!(res.next_stage.as_deref(), Some("done"), "Failure must NEVER advance to done");
        assert_eq!(res.completed, Some(false));

        let conn = processing_scheduler::open_or_init(&config).unwrap();
        let (stage, error, attempts): (String, Option<String>, i64) = conn
            .query_row("SELECT stage, error, text_attempts FROM tasks WHERE vid = ?", params![vid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap();
        assert_eq!(stage, "retry", "Must enter retry stage");
        assert_eq!(error.as_deref(), Some(err_msg));
        assert_eq!(attempts, 1, "Must increment text_attempts");

        // Receipt must be recorded in agent_receipts
        let receipt_exists: bool = conn
            .query_row(
                "SELECT count(*) FROM agent_receipts WHERE node_id = 'node-ocr-failures' AND attempt_id = ?",
                params![claim.attempt_id],
                |r| r.get::<_, i64>(0),
            )
            .map(|cnt| cnt > 0)
            .unwrap();
        assert!(receipt_exists, "Failure receipt must be recorded in agent_receipts");
    }
}

#[test]
fn stress_test_expired_lease_commit_rejected_with_http_409() {
    let (config, _tmp) = create_temp_env();
    seed_node(&config, "node-lease-tester", "tok-lease", "darwin", "arm64", None, 0);
    seed_task(&config, "vid-expired-lease", "ocr");

    let claim = claim_task(
        &config,
        "Bearer tok-lease",
        ClaimTaskRequest {
            node_id: Some("node-lease-tester".to_string()),
            stage: "ocr".to_string(),
            lease_duration_s: Some(60.0),
        },
    ).unwrap().expect("claimed task");

    // Manually force lease expiration in SQLite
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    conn.execute(
        "UPDATE tasks SET lease_expires_at = ?1 WHERE vid = 'vid-expired-lease'",
        params![now_sec() - 10.0],
    ).unwrap();

    let res = commit_task_result(
        &config,
        "Bearer tok-lease",
        CommitTaskResultRequest {
            node_id: Some("node-lease-tester".to_string()),
            attempt_id: Some(claim.attempt_id),
            vid: Some("vid-expired-lease".to_string()),
            coordinator_generation: Some(json!(claim.coordinator_generation)),
            stage: Some("ocr".to_string()),
            detections: None,
            vectors: None,
            text_snippets: Some(vec![json!("some text")]),
            execution_proof: None,
            status: Some("completed".to_string()),
            error_type: None,
            error_message: None,
        },
    );

    assert!(res.is_err(), "Post-lease submission must be rejected");
    let err = res.unwrap_err();
    assert_eq!(err.status, 409, "Must return HTTP 409 for expired lease");
    assert_eq!(err.code, "lease_expired");
}

#[test]
fn stress_test_scheduler_lease_reaping_across_all_stages() {
    let (config, _tmp) = create_temp_env();
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    let now = now_sec();

    seed_node(&config, "node-reap-target", "tok-reap", "linux", "x86_64", None, 0);
    // Artificially set running_tasks to 3
    conn.execute("UPDATE agent_nodes SET running_tasks = 3 WHERE id = 'node-reap-target'", []).unwrap();

    let stages = ["ocr", "detect", "feature", "text", "face"];
    for stg in &stages {
        let vid = format!("vid-reap-{}", stg);
        conn.execute(
            "INSERT INTO tasks (
                vid, path, name, size, mtime_ns, stage, priority, batch_id,
                created_at, updated_at, run_id, attempt_id, assigned_node_id,
                lease_expires_at, manual, data
            ) VALUES (
                ?1, ?2, ?3, 1024, '1700000000000000', ?4, 10, 'batch-reap',
                ?5, ?5, ?6, ?6, 'node-reap-target', ?7, 0, '{}'
            )",
            params![
                vid,
                format!("/isolated/media/{}.mp4", vid),
                format!("{}.mp4", vid),
                stg,
                now - 100.0,
                format!("run-{}", stg),
                now - 10.0, // expired 10s ago
            ],
        ).unwrap();
    }

    // Also seed an unexpired task
    conn.execute(
        "INSERT INTO tasks (
            vid, path, name, size, mtime_ns, stage, priority, batch_id,
            created_at, updated_at, run_id, attempt_id, assigned_node_id,
            lease_expires_at, manual, data
        ) VALUES (
            'vid-unexpired', '/isolated/media/unexpired.mp4', 'unexpired.mp4', 1024, '1700000000000000',
            'ocr', 10, 'batch-reap', ?1, ?1, 'run-unexpired', 'run-unexpired', 'node-reap-target',
            ?2, 0, '{}'
        )",
        params![now, now + 60.0],
    ).unwrap();

    // Also seed a manual takeover task (manual = 1)
    conn.execute(
        "INSERT INTO tasks (
            vid, path, name, size, mtime_ns, stage, priority, batch_id,
            created_at, updated_at, run_id, attempt_id, assigned_node_id,
            lease_expires_at, manual, data
        ) VALUES (
            'vid-manual', '/isolated/media/manual.mp4', 'manual.mp4', 1024, '1700000000000000',
            'detect', 10, 'batch-reap', ?1, ?1, 'run-manual', 'run-manual', 'node-reap-target',
            ?2, 1, '{}'
        )",
        params![now - 100.0, now - 10.0],
    ).unwrap();

    let reaped_res = processing_scheduler::reap_expired_leases(&conn, now).unwrap();
    assert_eq!(reaped_res.count, 5, "Exactly 5 expired tasks must be reaped across ocr, detect, feature, text, face");

    // Verify all 5 are in 'retry' stage with cleared lease
    for stg in &stages {
        let vid = format!("vid-reap-{}", stg);
        let (stage, lease_exp, run_id, retry_stage): (String, f64, String, Option<String>) = conn
            .query_row(
                "SELECT stage, lease_expires_at, run_id, retry_stage FROM tasks WHERE vid = ?",
                params![vid],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(stage, "retry", "Reaped task {} must transition to retry", vid);
        assert_eq!(lease_exp, 0.0, "Lease expiration must be reset to 0");
        assert_eq!(run_id, "", "run_id must be cleared");
        assert_eq!(retry_stage.as_deref(), Some(*stg), "retry_stage must record original stage");
    }

    // Verify unexpired task is untouched
    let unexp_stage: String = conn
        .query_row("SELECT stage FROM tasks WHERE vid = 'vid-unexpired'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(unexp_stage, "ocr", "Unexpired task must remain in ocr stage");

    // Verify manual task is untouched
    let manual_stage: String = conn
        .query_row("SELECT stage FROM tasks WHERE vid = 'vid-manual'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(manual_stage, "detect", "Manual takeover task must not be reaped");
}
