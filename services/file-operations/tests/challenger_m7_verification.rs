#![allow(dead_code, unused_imports)]

use face_file_operations::agent_deployment::{
    deploy_package, list_deployments, register_package, rollback_deployment,
    submit_deploy_receipt, DeployPackageRequest, DeployReceiptRequest, RegisterPackageRequest,
    RollbackRequest, SelfTestResult, DRIFT_TOLERANCE,
};
use face_file_operations::agent_registry::{
    control_node, NodeControlRequest,
};
use face_file_operations::config::Config;
use face_file_operations::processing_scheduler;
use rusqlite::params;
use serde_json::json;
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Barrier, Mutex};
use std::thread;

pub use face_file_operations::agent_deployment;
pub use face_file_operations::agent_registry;
pub use face_file_operations::config;
pub use face_file_operations::model;

#[path = "../src/agent_dispatcher.rs"]
mod agent_dispatcher;

use agent_dispatcher::{
    claim_task, commit_task_result, init_tables, now_sec, sha256_hex,
    ClaimTaskRequest, CommitTaskResultRequest,
};

fn create_temp_env() -> (Config, PathBuf) {
    let tmp_dir = std::env::temp_dir().join(format!("face-m7-chal-stress-{}", uuid::Uuid::new_v4()));
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
    conn.execute("UPDATE settings SET value = '16' WHERE key = 'concurrency'", []).unwrap();

    (config, tmp_dir)
}

fn enroll_custom_node(
    config: &Config,
    node_id: &str,
    hostname: &str,
    gpu_uuid: Option<&str>,
    device_group_id: Option<&str>,
    max_concurrent_tasks: i64,
    cpu_cores: i64,
    free_ram_bytes: i64,
) -> String {
    let conn = processing_scheduler::open_or_init(config).unwrap();
    init_tables(&conn).unwrap();

    let now = now_sec();
    let token = format!("token_{}_{}", node_id, uuid::Uuid::new_v4().simple());
    let token_hash = sha256_hex(&token);

    let dev_grp = device_group_id.unwrap_or("");
    let gpu_model = if gpu_uuid.is_some() {
        Some("NVIDIA GeForce RTX 4070 Ti".to_string())
    } else {
        None
    };

    conn.execute(
        "INSERT OR REPLACE INTO agent_nodes (
            id, token_hash, instance_id, hostname, os, arch, agent_version,
            cpu_cores, ram_bytes, free_ram_bytes, gpu_model, gpu_vram_bytes, free_vram_bytes,
            gpu_device_uuid, device_group_id, max_concurrent_tasks,
            status, accepting, model_ready, running_tasks, last_heartbeat_at,
            registered_at, updated_at
        ) VALUES (
            ?1, ?2, 'inst-m7-chal', ?3, 'linux', 'x86_64', '0.1.0',
            ?4, 34359738368, ?5, ?6, 12884901888, 8589934592,
            ?7, ?8, ?9,
            'accepting', 1, 1, 0, ?10,
            ?10, ?10
        )",
        params![
            node_id,
            token_hash,
            hostname,
            cpu_cores,
            free_ram_bytes,
            gpu_model,
            gpu_uuid,
            dev_grp,
            max_concurrent_tasks,
            now
        ],
    ).unwrap();

    token
}

fn seed_task(config: &Config, vid: &str, stage: &str, priority: i64) {
    let conn = processing_scheduler::open_or_init(config).unwrap();
    let now = now_sec();
    conn.execute(
        "INSERT INTO tasks (vid, path, name, size, duration, mtime_ns, stage, priority, batch_id, created_at, updated_at)
         VALUES (?1, ?2, ?3, 1048576, 60.0, '0', ?4, ?5, 'batch-chal-m7', ?6, ?6)",
        params![vid, format!("/tmp/{}.mp4", vid), format!("{}.mp4", vid), stage, priority, now],
    ).unwrap();
}

// =========================================================================
// CHALLENGE 1: SIMULTANEOUS CLAIMS ARRIVING DURING DRAIN RACE
// =========================================================================

#[test]
fn challenge_simultaneous_claims_arriving_during_drain_race() {
    let (config, _tmp) = create_temp_env();
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    conn.execute("UPDATE settings SET value = '32' WHERE key = 'concurrency'", []).unwrap();

    let canary_id = "node-canary-drain-race";
    let canary_tok = enroll_custom_node(
        &config,
        canary_id,
        "drain-race-host",
        Some("GPU-DRAIN-RACE-1"),
        Some("grp-drain-race"),
        16,
        16,
        16 * 1024 * 1024 * 1024,
    );

    let peer_id = "node-peer-unaffected";
    let peer_tok = enroll_custom_node(
        &config,
        peer_id,
        "peer-host",
        Some("GPU-PEER-1"),
        Some("grp-peer"),
        16,
        16,
        16 * 1024 * 1024 * 1024,
    );

    // Seed 40 tasks in detect stage
    for i in 0..40 {
        seed_task(&config, &format!("vid-drain-race-{}", i), "detect", 100 - i as i64);
    }

    // Launch 16 worker claim threads + 1 drain trigger thread synchronized via Barrier(17)
    let barrier = Arc::new(Barrier::new(17));
    let claims = Arc::new(Mutex::new(Vec::new()));
    let rejections = Arc::new(Mutex::new(Vec::new()));
    let mut handles = Vec::new();

    for t_idx in 0..16 {
        let b = Arc::clone(&barrier);
        let c = Arc::clone(&claims);
        let r = Arc::clone(&rejections);
        let cfg = config.clone();
        let tok = canary_tok.clone();
        let nid = canary_id.to_string();

        handles.push(thread::spawn(move || {
            b.wait();
            let res = claim_task(
                &cfg,
                &tok,
                ClaimTaskRequest {
                    node_id: Some(nid),
                    stage: "detect".to_string(),
                    lease_duration_s: Some(60.0),
                },
            );
            match res {
                Ok(Some(claim)) => {
                    c.lock().unwrap().push((t_idx, claim));
                }
                Ok(None) => {}
                Err(err) => {
                    r.lock().unwrap().push((t_idx, err.status, err.code));
                }
            }
        }));
    }

    // Drain thread
    let b_drain = Arc::clone(&barrier);
    let cfg_drain = config.clone();
    let nid_drain = canary_id.to_string();
    let drain_handle = thread::spawn(move || {
        b_drain.wait();
        control_node(
            &cfg_drain,
            NodeControlRequest {
                node_id: nid_drain,
                action: "drain".to_string(),
            },
        )
    });

    for h in handles {
        h.join().unwrap();
    }
    let drain_res = drain_handle.join().unwrap().expect("Drain control succeeds");
    assert_eq!(drain_res.status, "model_ready");
    assert_eq!(drain_res.accepting, false);

    let claimed_tasks = claims.lock().unwrap().clone();
    let rejected_tasks = rejections.lock().unwrap().clone();

    // Invariant A: Total attempts = 16
    assert_eq!(claimed_tasks.len() + rejected_tasks.len(), 16, "All 16 claim attempts accounted for");

    // Invariant B: Rejections must all be either HTTP 409 node_not_accepting (after drain) or HTTP 503 (quota/gpu busy before drain)
    for (_idx, status, code) in &rejected_tasks {
        assert!(
            (*status == 409 && code == "node_not_accepting") || (*status == 503 && (code == "gpu_busy" || code == "node_busy" || code == "cluster_busy")),
            "Unexpected rejection: status={}, code={}", status, code
        );
    }

    // Invariant C: All claimed tasks are distinct
    let mut claimed_vids = HashSet::new();
    for (_idx, claim) in &claimed_tasks {
        assert!(claimed_vids.insert(claim.vid.clone()), "Duplicate claim for vid: {}", claim.vid);
    }

    // Invariant D: All tasks claimed before drain completed must commit cleanly even while drained
    for (_idx, claim) in &claimed_tasks {
        let commit_res = commit_task_result(
            &config,
            &canary_tok,
            CommitTaskResultRequest {
                node_id: Some(canary_id.to_string()),
                attempt_id: Some(claim.attempt_id.clone()),
                vid: Some(claim.vid.clone()),
                coordinator_generation: Some(json!(claim.coordinator_generation)),
                stage: Some("detect".to_string()),
                status: Some("success".to_string()),
                detections: Some(vec![]),
                error_type: None,
                error_message: None,
                text_snippets: None,
                vectors: None,
                execution_proof: None,
            },
        ).expect("In-flight task commit must succeed even while drained");
        assert!(commit_res.ok);
    }

    // Invariant E: running_tasks drops to strictly 0
    let (running_after, accepting_after): (i64, i32) = conn.query_row(
        "SELECT running_tasks, accepting FROM agent_nodes WHERE id = ?",
        params![canary_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    ).unwrap();
    assert_eq!(running_after, 0, "Canary running_tasks must drop to 0 after all in-flight tasks finish");
    assert_eq!(accepting_after, 0, "Canary node must remain non-accepting");

    // Invariant F: Subsequent claims on canary node strictly fail with 409
    let post_drain_err = claim_task(
        &config,
        &canary_tok,
        ClaimTaskRequest {
            node_id: Some(canary_id.to_string()),
            stage: "detect".to_string(),
            lease_duration_s: Some(60.0),
        },
    ).unwrap_err();
    assert_eq!(post_drain_err.status, 409);
    assert_eq!(post_drain_err.code, "node_not_accepting");

    // Invariant G: Peer node continues processing remaining tasks without starvation
    let peer_claim = claim_task(
        &config,
        &peer_tok,
        ClaimTaskRequest {
            node_id: Some(peer_id.to_string()),
            stage: "detect".to_string(),
            lease_duration_s: Some(60.0),
        },
    ).unwrap().expect("Peer node can claim unassigned tasks");
    assert!(!claimed_vids.contains(&peer_claim.vid), "Peer node must claim a fresh task");
}

// =========================================================================
// CHALLENGE 2: CONCURRENT ROLLBACK TRIGGERS UNDER HIGH THREAD CONTENTION
// =========================================================================

#[test]
fn challenge_concurrent_rollback_triggers_high_thread_contention() {
    let (config, _tmp) = create_temp_env();
    let conn = processing_scheduler::open_or_init(&config).unwrap();

    let node_id = "node-rb-contention";
    let _token = enroll_custom_node(
        &config,
        node_id,
        "contention-host",
        Some("GPU-RB-CONT-1"),
        Some("grp-rb-cont"),
        4,
        16,
        16 * 1024 * 1024 * 1024,
    );

    // Register v1 (stable 1.0.0), v2 (stable 2.0.0), v3 (faulty 3.0.0)
    register_package(&config, RegisterPackageRequest {
        package_id: "pkg-cont-v1".to_string(),
        manifest_sha256: "m1".to_string(),
        model_sha256: "w1".to_string(),
        execution_provider: "CUDAExecutionProvider".to_string(),
        version: "1.0.0".to_string(),
        is_pinned: 1,
    }).unwrap();

    register_package(&config, RegisterPackageRequest {
        package_id: "pkg-cont-v2".to_string(),
        manifest_sha256: "m2".to_string(),
        model_sha256: "w2".to_string(),
        execution_provider: "CUDAExecutionProvider".to_string(),
        version: "2.0.0".to_string(),
        is_pinned: 1,
    }).unwrap();

    register_package(&config, RegisterPackageRequest {
        package_id: "pkg-cont-v3".to_string(),
        manifest_sha256: "m3".to_string(),
        model_sha256: "w3".to_string(),
        execution_provider: "CUDAExecutionProvider".to_string(),
        version: "3.0.0-rc1".to_string(),
        is_pinned: 0,
    }).unwrap();

    // Deploy v1 -> ready
    let d1 = deploy_package(&config, DeployPackageRequest { node_id: node_id.to_string(), package_id: "pkg-cont-v1".to_string() }).unwrap();
    submit_deploy_receipt(&config, DeployReceiptRequest {
        deployment_id: d1.deployment_id,
        package_id: "pkg-cont-v1".to_string(),
        status: "ready".to_string(),
        stage: Some("ready".to_string()),
        startup_proof: Some(json!({"kernel_events": 100})),
        self_test: Some(SelfTestResult { passed: true, drift: 0.001 }),
        duration_ms: Some(200),
        error: None,
    }).unwrap();

    // Deploy v2 -> ready
    let d2 = deploy_package(&config, DeployPackageRequest { node_id: node_id.to_string(), package_id: "pkg-cont-v2".to_string() }).unwrap();
    submit_deploy_receipt(&config, DeployReceiptRequest {
        deployment_id: d2.deployment_id,
        package_id: "pkg-cont-v2".to_string(),
        status: "ready".to_string(),
        stage: Some("ready".to_string()),
        startup_proof: Some(json!({"kernel_events": 100})),
        self_test: Some(SelfTestResult { passed: true, drift: 0.001 }),
        duration_ms: Some(200),
        error: None,
    }).unwrap();

    // Deploy v3 -> failed
    let d3 = deploy_package(&config, DeployPackageRequest { node_id: node_id.to_string(), package_id: "pkg-cont-v3".to_string() }).unwrap();
    submit_deploy_receipt(&config, DeployReceiptRequest {
        deployment_id: d3.deployment_id.clone(),
        package_id: "pkg-cont-v3".to_string(),
        status: "failed".to_string(),
        stage: Some("start".to_string()),
        startup_proof: None,
        self_test: None,
        duration_ms: Some(100),
        error: Some("vram_allocation_failed".to_string()),
    }).unwrap();

    // Launch 32 concurrent threads all requesting target_version: None
    const NUM_THREADS: usize = 32;
    let barrier = Arc::new(Barrier::new(NUM_THREADS));
    let results = Arc::new(Mutex::new(Vec::new()));
    let mut handles = Vec::new();

    for _i in 0..NUM_THREADS {
        let b = Arc::clone(&barrier);
        let r = Arc::clone(&results);
        let cfg = config.clone();
        let nid = node_id.to_string();

        handles.push(thread::spawn(move || {
            b.wait();
            let res = rollback_deployment(&cfg, RollbackRequest {
                node_id: nid,
                target_version: None,
            });
            r.lock().unwrap().push(res);
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    let all_res = results.lock().unwrap();
    assert_eq!(all_res.len(), NUM_THREADS);

    let mut pkg_ids = HashSet::new();
    let mut dep_ids = HashSet::new();
    for (idx, res) in all_res.iter().enumerate() {
        assert!(res.is_ok(), "Thread {} failed: {:?}", idx, res);
        let dep = res.as_ref().unwrap();
        pkg_ids.insert(dep.package_id.clone());
        dep_ids.insert(dep.deployment_id.clone());
    }

    let active_deployments: Vec<(String, String, String)> = conn
        .prepare("SELECT deployment_id, package_id, status FROM agent_deployments WHERE node_id = ? AND status NOT IN ('ready', 'failed', 'rolled_back')")
        .unwrap()
        .query_map(params![node_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();

    let desired_ver_in_db: String = conn.query_row(
        "SELECT desired_version FROM agent_nodes WHERE id = ?",
        params![node_id],
        |r| r.get(0),
    ).unwrap();

    println!("=== EMPIRICAL CHALLENGE FINDING ===");
    println!("Unique packages resolved: {:?}", pkg_ids);
    println!("Unique deployment IDs returned: {:?}", dep_ids);
    println!("Active deployments in DB: {:?}", active_deployments);
    println!("Final desired_version in DB: {}", desired_ver_in_db);

    // Invariant: Rollback must be strictly idempotent under high thread contention
    assert_eq!(
        pkg_ids.len(),
        1,
        "Idempotency violation: rollback resolved to multiple distinct packages: {:?}",
        pkg_ids
    );
    assert_eq!(
        dep_ids.len(),
        1,
        "Idempotency violation: rollback generated multiple deployment records: {:?}",
        dep_ids
    );
    assert_eq!(
        active_deployments.len(),
        1,
        "Database bloat violation: multiple active deployments exist simultaneously for node {}: {:?}",
        node_id, active_deployments
    );
    assert_eq!(
        desired_ver_in_db,
        "2.0.0",
        "Target resolution corruption: node desired_version downgraded to {} instead of 2.0.0",
        desired_ver_in_db
    );
}

// =========================================================================
// CHALLENGE 3: DRIFT TOLERANCE THRESHOLD BOUNDARY PRECISION (0.050 vs 0.051)
// =========================================================================

#[test]
fn challenge_drift_tolerance_threshold_boundary_precision() {
    let (config, _tmp) = create_temp_env();
    let conn = processing_scheduler::open_or_init(&config).unwrap();

    let node_id = "node-drift-precision";
    let _token = enroll_custom_node(
        &config,
        node_id,
        "drift-precision-host",
        Some("GPU-DRIFT-PREC"),
        Some("grp-drift-prec"),
        4,
        16,
        16 * 1024 * 1024 * 1024,
    );

    // Register test packages for each boundary case
    let test_cases = vec![
        // (case_name, drift, passed_flag, should_succeed, expected_error_code)
        ("case-exact-0.050", 0.050, true, true, None),
        ("case-just-below-0.049999", 0.04999999999999999, true, true, None),
        ("case-zero-drift", 0.0, true, true, None),
        ("case-just-above-0.0500001", 0.0500001, true, false, Some("self_test_drift_exceeded")),
        ("case-0.051", 0.051, true, false, Some("self_test_drift_exceeded")),
        ("case-large-drift-0.10", 0.10, true, false, Some("self_test_drift_exceeded")),
        ("case-negative-drift", -0.0001, true, false, Some("self_test_drift_exceeded")),
        ("case-passed-false-with-low-drift", 0.001, false, false, Some("self_test_drift_exceeded")),
        ("case-nan-drift", f64::NAN, true, false, Some("self_test_drift_exceeded")),
        ("case-inf-drift", f64::INFINITY, true, false, Some("self_test_drift_exceeded")),
        ("case-neg-inf-drift", f64::NEG_INFINITY, true, false, Some("self_test_drift_exceeded")),
    ];

    for (idx, (case_name, drift, passed_flag, should_succeed, expected_err)) in test_cases.into_iter().enumerate() {
        let pkg_id = format!("pkg-drift-{}", idx);
        let version = format!("1.{}.0", idx);

        register_package(&config, RegisterPackageRequest {
            package_id: pkg_id.clone(),
            manifest_sha256: format!("m-{}", idx),
            model_sha256: format!("w-{}", idx),
            execution_provider: "CUDAExecutionProvider".to_string(),
            version: version.clone(),
            is_pinned: 0,
        }).unwrap();

        let dep = deploy_package(&config, DeployPackageRequest {
            node_id: node_id.to_string(),
            package_id: pkg_id.clone(),
        }).unwrap();

        let receipt_res = submit_deploy_receipt(&config, DeployReceiptRequest {
            deployment_id: dep.deployment_id.clone(),
            package_id: pkg_id.clone(),
            status: "ready".to_string(),
            stage: Some("self_test".to_string()),
            startup_proof: Some(json!({"kernel_events": 128})),
            self_test: Some(SelfTestResult {
                passed: passed_flag,
                drift,
            }),
            duration_ms: Some(200),
            error: None,
        });

        if should_succeed {
            assert!(receipt_res.is_ok(), "{}: expected OK, got {:?}", case_name, receipt_res);
            let (status, stage, self_test_passed, self_test_drift): (String, String, Option<i32>, Option<f64>) = conn.query_row(
                "SELECT status, stage, self_test_passed, self_test_drift FROM agent_deployments WHERE deployment_id = ?",
                params![dep.deployment_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            ).unwrap();
            assert_eq!(status, "ready", "{}: status must be ready", case_name);
            assert_eq!(stage, "ready", "{}: stage must be ready", case_name);
            assert_eq!(self_test_passed, Some(1), "{}: self_test_passed must be 1", case_name);
            assert_eq!(self_test_drift, Some(drift), "{}: self_test_drift must match", case_name);

            // Node current_version must be promoted
            let cur_ver: String = conn.query_row(
                "SELECT current_version FROM agent_nodes WHERE id = ?",
                params![node_id],
                |r| r.get(0),
            ).unwrap();
            assert_eq!(cur_ver, version, "{}: current_version must be promoted to {}", case_name, version);
        } else {
            let err = receipt_res.unwrap_err();
            assert_eq!(err.status, 422, "{}: expected HTTP 422, got {}", case_name, err.status);
            assert_eq!(err.code, expected_err.unwrap(), "{}: code mismatch", case_name);

            let (status, stage, self_test_passed): (String, String, Option<i32>) = conn.query_row(
                "SELECT status, stage, self_test_passed FROM agent_deployments WHERE deployment_id = ?",
                params![dep.deployment_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            ).unwrap();
            assert_eq!(status, "failed", "{}: status must be failed", case_name);
            assert_eq!(stage, "self_test", "{}: stage must be self_test", case_name);
            assert_eq!(self_test_passed, Some(0), "{}: self_test_passed must be 0", case_name);
        }
    }
}

// =========================================================================
// CHALLENGE 4: MISSING SELF-TEST STRUCT REJECTION
// =========================================================================

#[test]
fn challenge_missing_self_test_struct_rejection() {
    let (config, _tmp) = create_temp_env();
    let conn = processing_scheduler::open_or_init(&config).unwrap();

    let node_id = "node-missing-st";
    enroll_custom_node(&config, node_id, "host-st", Some("GPU-ST"), Some("grp-st"), 4, 16, 16 * 1024 * 1024 * 1024);

    register_package(&config, RegisterPackageRequest {
        package_id: "pkg-missing-st".to_string(),
        manifest_sha256: "m-st".to_string(),
        model_sha256: "w-st".to_string(),
        execution_provider: "CUDAExecutionProvider".to_string(),
        version: "1.0.0".to_string(),
        is_pinned: 1,
    }).unwrap();

    let dep = deploy_package(&config, DeployPackageRequest {
        node_id: node_id.to_string(),
        package_id: "pkg-missing-st".to_string(),
    }).unwrap();

    // Submit receipt with status = 'ready' but self_test = None
    let receipt_err = submit_deploy_receipt(&config, DeployReceiptRequest {
        deployment_id: dep.deployment_id.clone(),
        package_id: "pkg-missing-st".to_string(),
        status: "ready".to_string(),
        stage: Some("ready".to_string()),
        startup_proof: Some(json!({"kernel_events": 100})),
        self_test: None,
        duration_ms: Some(100),
        error: None,
    }).unwrap_err();

    assert_eq!(receipt_err.status, 422);
    assert_eq!(receipt_err.code, "self_test_drift_exceeded");

    // Current version must NOT be promoted
    let cur_ver: String = conn.query_row(
        "SELECT current_version FROM agent_nodes WHERE id = ?",
        params![node_id],
        |r| r.get(0),
    ).unwrap();
    assert_eq!(cur_ver, "");
}
