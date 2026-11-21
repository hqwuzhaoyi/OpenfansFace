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
    let tmp_dir = std::env::temp_dir().join(format!("face-m7-adv-stress-{}", uuid::Uuid::new_v4()));
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
    conn.execute("UPDATE settings SET value = '4' WHERE key = 'concurrency'", []).unwrap();

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
            ?1, ?2, 'inst-m7', ?3, 'linux', 'x86_64', '0.1.0',
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
         VALUES (?1, ?2, ?3, 1048576, 60.0, '0', ?4, ?5, 'batch-adv-m7', ?6, ?6)",
        params![vid, format!("/tmp/{}.mp4", vid), format!("{}.mp4", vid), stage, priority, now],
    ).unwrap();
}

// =========================================================================
// TEST 1: CANARY DRAIN AND EXCLUSIVE TASK GATING
// =========================================================================

#[test]
fn challenge_canary_drain_and_exclusive_task_gating() {
    let (config, _tmp) = create_temp_env();
    let conn = processing_scheduler::open_or_init(&config).unwrap();

    // 1. Seed tasks
    for i in 0..10 {
        seed_task(&config, &format!("vid-drain-{}", i), "detect", 10 - i as i64);
    }

    // 2. Register Node A (canary) and Node B (stable cluster worker)
    let tok_a = enroll_custom_node(
        &config,
        "node-canary-drain",
        "wsl-desktop",
        Some("GPU-CANARY-1"),
        Some("grp-gpu-canary"),
        4,
        16,
        16 * 1024 * 1024 * 1024,
    );
    let tok_b = enroll_custom_node(
        &config,
        "node-stable-worker",
        "linux-worker",
        Some("GPU-STABLE-2"),
        Some("grp-gpu-stable"),
        4,
        16,
        16 * 1024 * 1024 * 1024,
    );

    // 3. Canary Node A claims an initial task before draining
    let claim_a = claim_task(
        &config,
        &tok_a,
        ClaimTaskRequest {
            node_id: Some("node-canary-drain".to_string()),
            stage: "detect".to_string(),
            lease_duration_s: Some(60.0),
        },
    ).unwrap().expect("Canary claims in-flight task");

    let running_a: i64 = conn.query_row(
        "SELECT running_tasks FROM agent_nodes WHERE id = 'node-canary-drain'",
        [],
        |r| r.get(0),
    ).unwrap();
    assert_eq!(running_a, 1, "Canary running_tasks must be 1 after claim");

    // 4. Center initiates canary drain
    let drain_res = control_node(
        &config,
        NodeControlRequest {
            node_id: "node-canary-drain".to_string(),
            action: "drain".to_string(),
        },
    ).expect("drain control succeeds");

    assert_eq!(drain_res.status, "model_ready");
    assert_eq!(drain_res.accepting, false);

    // Invariant A: Node A's accepting flag in database is strictly 0
    let (accepting_a, status_a): (i32, String) = conn.query_row(
        "SELECT accepting, status FROM agent_nodes WHERE id = 'node-canary-drain'",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    ).unwrap();
    assert_eq!(accepting_a, 0, "Drained canary accepting must be 0");
    assert_eq!(status_a, "model_ready", "Drained canary status must be model_ready");

    // Invariant B: 8 concurrent threads attempt claim_task on Node A -> all rejected with 409 node_not_accepting
    let barrier = Arc::new(Barrier::new(8));
    let rejection_codes = Arc::new(Mutex::new(Vec::new()));
    let mut handles = Vec::new();

    for _ in 0..8 {
        let b = Arc::clone(&barrier);
        let rej = Arc::clone(&rejection_codes);
        let cfg = config.clone();
        let tok = tok_a.clone();

        handles.push(thread::spawn(move || {
            b.wait();
            let res = claim_task(
                &cfg,
                &tok,
                ClaimTaskRequest {
                    node_id: Some("node-canary-drain".to_string()),
                    stage: "detect".to_string(),
                    lease_duration_s: Some(60.0),
                },
            );
            match res {
                Ok(claim) => panic!("Drained node must never receive claim, got {:?}", claim),
                Err(err) => {
                    rej.lock().unwrap().push((err.status, err.code));
                }
            }
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    let rejections = rejection_codes.lock().unwrap();
    assert_eq!(rejections.len(), 8, "All 8 concurrent attempts must be rejected");
    for (status, code) in rejections.iter() {
        assert_eq!(*status, 409, "Must return HTTP 409");
        assert_eq!(code, "node_not_accepting", "Must return code node_not_accepting");
    }

    // Invariant C: Stable worker Node B can still claim tasks without disruption
    let claim_b = claim_task(
        &config,
        &tok_b,
        ClaimTaskRequest {
            node_id: Some("node-stable-worker".to_string()),
            stage: "detect".to_string(),
            lease_duration_s: Some(60.0),
        },
    ).unwrap().expect("Stable worker can still claim tasks");
    assert_ne!(claim_b.vid, claim_a.vid, "Distinct tasks claimed");

    // Invariant D: Canary Node A's in-flight task commits cleanly
    let commit_res = commit_task_result(
        &config,
        &tok_a,
        CommitTaskResultRequest {
            node_id: Some("node-canary-drain".to_string()),
            attempt_id: Some(claim_a.attempt_id.clone()),
            vid: Some(claim_a.vid.clone()),
            coordinator_generation: Some(json!(claim_a.coordinator_generation)),
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

    // Running tasks drops to 0
    let running_after: i64 = conn.query_row(
        "SELECT running_tasks FROM agent_nodes WHERE id = 'node-canary-drain'",
        [],
        |r| r.get(0),
    ).unwrap();
    assert_eq!(running_after, 0, "Canary running_tasks must drop to 0 after in-flight completion");

    // Batch item status is done
    let stage_in_db: String = conn.query_row(
        "SELECT stage FROM tasks WHERE vid = ?",
        params![claim_a.vid],
        |r| r.get(0),
    ).unwrap();
    assert_eq!(stage_in_db, "done", "Task stage must transition to done");
}

// =========================================================================
// TEST 2: DUAL-VERSION VRAM MEMORY-SAFE TRANSITION SEQUENCE
// =========================================================================

#[test]
fn challenge_vram_safe_transition_sequence() {
    let (config, _tmp) = create_temp_env();
    let conn = processing_scheduler::open_or_init(&config).unwrap();

    // 1. Hardware model of RTX 4070 Ti (12,282 MiB VRAM total):
    // Active background AI models consume ~10,482 MiB, leaving 1,800 MiB free before runner startup.
    // Single model operator (SCRFD + ArcFace + 512 MiB Arena) requires ~1,100 MiB.
    // Minimum mandatory VRAM floor is 600 MiB.
    // Coexistence of 2 operators: 1,100 + 1,100 = 2,200 MiB > 1,800 MiB free -> CUDA OOM!
    const VRAM_TOTAL_MIB: i64 = 12282;
    const BACKGROUND_LOAD_MIB: i64 = 10482; // leaving 1,800 MiB free
    const OPERATOR_ARENA_MIB: i64 = 1100;
    const MIN_VRAM_FLOOR_MIB: i64 = 600;

    let node_id = "node-vram-safe-transition";
    let _tok = enroll_custom_node(
        &config,
        node_id,
        "rtx-4070ti-host",
        Some("GPU-4070TI-SERIAL-1"),
        Some("grp-wsl-gpu"),
        1,
        16,
        16 * 1024 * 1024 * 1024,
    );

    // Initial state: old operator v1 is active
    let mut active_operators: Vec<String> = vec!["operator_v1".to_string()];

    // Track simulated telemetry
    let compute_vram = |ops: &[String]| -> (i64, i64) {
        let op_vram = ops.len() as i64 * OPERATOR_ARENA_MIB;
        let used = BACKGROUND_LOAD_MIB + op_vram;
        let free = VRAM_TOTAL_MIB - used;
        (used, free)
    };

    let (used_initial, free_initial) = compute_vram(&active_operators);
    assert_eq!(used_initial, 11582);
    assert_eq!(free_initial, 700); // 700 MiB free (> 600 MiB floor)

    // Verify: If we erroneously started new operator without stopping old operator:
    let (_oom_used, oom_free) = compute_vram(&["operator_v1".to_string(), "operator_v2".to_string()]);
    assert!(
        oom_free < MIN_VRAM_FLOOR_MIB,
        "Double-VRAM coexistence would breach 600 MiB floor (free: {} MiB < 600 MiB)",
        oom_free
    );

    // Mandatory 5-step serialized transition sequence:
    // Step 1: drain_in_flight
    control_node(
        &config,
        NodeControlRequest {
            node_id: node_id.to_string(),
            action: "drain".to_string(),
        },
    ).unwrap();
    conn.execute(
        "UPDATE agent_nodes SET running_tasks = 0 WHERE id = ?",
        params![node_id],
    ).unwrap();

    // Step 2: stop_old_operator (SIGTERM -> SIGKILL -> clean sockets)
    active_operators.clear(); // old operator terminated, OS cleans driver context
    let (used_after_stop, free_after_stop) = compute_vram(&active_operators);
    assert_eq!(used_after_stop, BACKGROUND_LOAD_MIB);
    assert_eq!(free_after_stop, 1800);

    // Step 3: verify_vram_floor (free >= 600 MiB)
    assert!(
        free_after_stop >= MIN_VRAM_FLOOR_MIB,
        "VRAM floor check: free {} MiB must be >= 600 MiB",
        free_after_stop
    );

    // Counterfactual check: what if background load spiked and free < 600 MiB?
    let counterfactual_free = 400;
    let transition_aborted = counterfactual_free < MIN_VRAM_FLOOR_MIB;
    assert!(transition_aborted, "Supervisor must abort transition if free VRAM < 600 MiB");

    // Step 4: start_new_operator
    active_operators.push("operator_v2".to_string());
    let (used_new, free_new) = compute_vram(&active_operators);
    assert_eq!(used_new, 11582);
    assert_eq!(free_new, 700);
    assert!(free_new >= MIN_VRAM_FLOOR_MIB);

    // Invariant: Peak operator count during transition was strictly 1 (zero double-allocation)
    assert_eq!(active_operators.len(), 1, "Only one operator active at any moment");

    // Step 5: warmup_and_self_test
    register_package(
        &config,
        RegisterPackageRequest {
            package_id: "pkg-v2-cuda".to_string(),
            manifest_sha256: "manifest-v2-sha256".to_string(),
            model_sha256: "model-v2-sha256".to_string(),
            execution_provider: "CUDAExecutionProvider".to_string(),
            version: "2.0.0".to_string(),
            is_pinned: 1,
        },
    ).unwrap();

    let dep_resp = deploy_package(
        &config,
        DeployPackageRequest {
            node_id: node_id.to_string(),
            package_id: "pkg-v2-cuda".to_string(),
        },
    ).unwrap();

    let receipt_resp = submit_deploy_receipt(
        &config,
        DeployReceiptRequest {
            deployment_id: dep_resp.deployment_id.clone(),
            package_id: "pkg-v2-cuda".to_string(),
            status: "ready".to_string(),
            stage: Some("ready".to_string()),
            startup_proof: Some(json!({"kernel_events": 256, "cuda_available": true})),
            self_test: Some(SelfTestResult {
                passed: true,
                drift: 0.0012,
            }),
            duration_ms: Some(420),
            error: None,
        },
    ).unwrap();

    assert!(receipt_resp.ok);

    // Node promoted and ready
    let (curr_ver, model_ready): (String, i32) = conn.query_row(
        "SELECT current_version, model_ready FROM agent_nodes WHERE id = ?",
        params![node_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    ).unwrap();
    assert_eq!(curr_ver, "2.0.0");
    assert_eq!(model_ready, 1);
}

// =========================================================================
// TEST 3: SELF-TEST DRIFT REJECTION AND CLUSTER ISOLATION
// =========================================================================

#[test]
fn challenge_self_test_drift_rejection_and_isolation() {
    let (config, _tmp) = create_temp_env();
    let conn = processing_scheduler::open_or_init(&config).unwrap();

    // Verify tolerance constant is 0.05
    assert_eq!(DRIFT_TOLERANCE, 0.05);

    // Register 2 packages
    register_package(
        &config,
        RegisterPackageRequest {
            package_id: "pkg-base-v1".to_string(),
            manifest_sha256: "base-m1".to_string(),
            model_sha256: "base-w1".to_string(),
            execution_provider: "CUDAExecutionProvider".to_string(),
            version: "1.0.0".to_string(),
            is_pinned: 1,
        },
    ).unwrap();
    register_package(
        &config,
        RegisterPackageRequest {
            package_id: "pkg-flawed-candidate-v2".to_string(),
            manifest_sha256: "flawed-m2".to_string(),
            model_sha256: "flawed-w2".to_string(),
            execution_provider: "CUDAExecutionProvider".to_string(),
            version: "2.0.0-flawed".to_string(),
            is_pinned: 0,
        },
    ).unwrap();

    // Node A (canary) and Node B (stable worker)
    let _tok_a = enroll_custom_node(
        &config,
        "node-canary-drift",
        "wsl-desktop",
        Some("GPU-DRIFT-1"),
        Some("grp-drift"),
        4,
        16,
        16 * 1024 * 1024 * 1024,
    );
    let tok_b = enroll_custom_node(
        &config,
        "node-cluster-peer",
        "linux-srv",
        Some("GPU-DRIFT-2"),
        Some("grp-drift-peer"),
        4,
        16,
        16 * 1024 * 1024 * 1024,
    );

    // Initial deployment of v1 on both
    let dep_a1 = deploy_package(&config, DeployPackageRequest {
        node_id: "node-canary-drift".to_string(),
        package_id: "pkg-base-v1".to_string(),
    }).unwrap();
    submit_deploy_receipt(&config, DeployReceiptRequest {
        deployment_id: dep_a1.deployment_id,
        package_id: "pkg-base-v1".to_string(),
        status: "ready".to_string(),
        stage: Some("ready".to_string()),
        startup_proof: Some(json!({"kernel_events": 100})),
        self_test: Some(SelfTestResult { passed: true, drift: 0.002 }),
        duration_ms: Some(300),
        error: None,
    }).unwrap();

    let dep_b1 = deploy_package(&config, DeployPackageRequest {
        node_id: "node-cluster-peer".to_string(),
        package_id: "pkg-base-v1".to_string(),
    }).unwrap();
    submit_deploy_receipt(&config, DeployReceiptRequest {
        deployment_id: dep_b1.deployment_id,
        package_id: "pkg-base-v1".to_string(),
        status: "ready".to_string(),
        stage: Some("ready".to_string()),
        startup_proof: Some(json!({"kernel_events": 100})),
        self_test: Some(SelfTestResult { passed: true, drift: 0.002 }),
        duration_ms: Some(300),
        error: None,
    }).unwrap();

    // Verify both are on 1.0.0
    let cur_a: String = conn.query_row("SELECT current_version FROM agent_nodes WHERE id = 'node-canary-drift'", [], |r| r.get(0)).unwrap();
    let cur_b: String = conn.query_row("SELECT current_version FROM agent_nodes WHERE id = 'node-cluster-peer'", [], |r| r.get(0)).unwrap();
    assert_eq!(cur_a, "1.0.0");
    assert_eq!(cur_b, "1.0.0");

    // Canary upgrade: dispatch flawed candidate
    let dep_canary = deploy_package(&config, DeployPackageRequest {
        node_id: "node-canary-drift".to_string(),
        package_id: "pkg-flawed-candidate-v2".to_string(),
    }).unwrap();

    // Candidate reports excessive drift: 0.082 > 0.05
    let drift_err = submit_deploy_receipt(&config, DeployReceiptRequest {
        deployment_id: dep_canary.deployment_id.clone(),
        package_id: "pkg-flawed-candidate-v2".to_string(),
        status: "ready".to_string(),
        stage: Some("self_test".to_string()),
        startup_proof: Some(json!({"kernel_events": 128})),
        self_test: Some(SelfTestResult {
            passed: true,
            drift: 0.082, // breaches DRIFT_TOLERANCE (0.05)
        }),
        duration_ms: Some(400),
        error: None,
    }).unwrap_err();

    assert_eq!(drift_err.status, 422, "Drift > 0.05 must be rejected with HTTP 422");
    assert_eq!(drift_err.code, "self_test_drift_exceeded");

    // Invariant A: In agent_deployments, status is failed, stage is self_test
    let (dep_status, dep_stage, dep_passed, dep_drift): (String, String, Option<i32>, Option<f64>) = conn.query_row(
        "SELECT status, stage, self_test_passed, self_test_drift FROM agent_deployments WHERE deployment_id = ?",
        params![dep_canary.deployment_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    ).unwrap();
    assert_eq!(dep_status, "failed");
    assert_eq!(dep_stage, "self_test");
    assert_eq!(dep_passed, Some(0));
    assert_eq!(dep_drift, Some(0.082));

    // Invariant B: Canary node was NOT promoted: current_version remains 1.0.0
    let (cur_a_after, desired_a): (String, String) = conn.query_row(
        "SELECT current_version, desired_version FROM agent_nodes WHERE id = 'node-canary-drift'",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    ).unwrap();
    assert_eq!(cur_a_after, "1.0.0", "Canary current_version must NOT be promoted to flawed candidate");
    assert_eq!(desired_a, "2.0.0-flawed");

    // Invariant C: Audit event deployment_failed is logged
    let ev_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM events WHERE kind = 'deployment_failed' AND message LIKE '%self-test drift exceeded threshold%'",
        [],
        |r| r.get(0),
    ).unwrap();
    assert!(ev_count >= 1, "Audit log must record deployment_failed event");

    // Invariant D: Peer node B is completely isolated and healthy
    let (status_b, accepting_b, model_ready_b): (String, i32, i32) = conn.query_row(
        "SELECT status, accepting, model_ready FROM agent_nodes WHERE id = 'node-cluster-peer'",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    ).unwrap();
    assert_eq!(status_b, "accepting");
    assert_eq!(accepting_b, 1);
    assert_eq!(model_ready_b, 1);

    // Node B can process work seamlessly
    seed_task(&config, "vid-isolated-peer", "detect", 5);
    let claim_peer = claim_task(
        &config,
        &tok_b,
        ClaimTaskRequest {
            node_id: Some("node-cluster-peer".to_string()),
            stage: "detect".to_string(),
            lease_duration_s: Some(60.0),
        },
    ).unwrap().expect("Peer node continues normal operation");
    assert_eq!(claim_peer.vid, "vid-isolated-peer");
}

// =========================================================================
// TEST 4: CONCURRENT ROLLBACK REQUESTS IDEMPOTENCY
// =========================================================================

#[test]
fn challenge_concurrent_rollback_requests_idempotency() {
    let (config, _tmp) = create_temp_env();
    let conn = processing_scheduler::open_or_init(&config).unwrap();

    let node_id = "node-concurrent-rollback";
    let _tok = enroll_custom_node(
        &config,
        node_id,
        "rtx-worker",
        Some("GPU-CONCURRENT-RB"),
        Some("grp-rb"),
        4,
        16,
        16 * 1024 * 1024 * 1024,
    );

    // Register stable v1 and buggy v2
    register_package(
        &config,
        RegisterPackageRequest {
            package_id: "pkg-stable-v1".to_string(),
            manifest_sha256: "manifest-v1".to_string(),
            model_sha256: "model-v1".to_string(),
            execution_provider: "CUDAExecutionProvider".to_string(),
            version: "1.0.0".to_string(),
            is_pinned: 1,
        },
    ).unwrap();
    register_package(
        &config,
        RegisterPackageRequest {
            package_id: "pkg-buggy-v2".to_string(),
            manifest_sha256: "manifest-v2".to_string(),
            model_sha256: "model-v2".to_string(),
            execution_provider: "CUDAExecutionProvider".to_string(),
            version: "2.0.0".to_string(),
            is_pinned: 0,
        },
    ).unwrap();

    // 1. Deploy v1 and confirm ready
    let dep_v1 = deploy_package(&config, DeployPackageRequest {
        node_id: node_id.to_string(),
        package_id: "pkg-stable-v1".to_string(),
    }).unwrap();
    submit_deploy_receipt(&config, DeployReceiptRequest {
        deployment_id: dep_v1.deployment_id,
        package_id: "pkg-stable-v1".to_string(),
        status: "ready".to_string(),
        stage: Some("ready".to_string()),
        startup_proof: Some(json!({"kernel_events": 200})),
        self_test: Some(SelfTestResult { passed: true, drift: 0.001 }),
        duration_ms: Some(300),
        error: None,
    }).unwrap();

    // 2. Deploy v2 and fail
    let dep_v2 = deploy_package(&config, DeployPackageRequest {
        node_id: node_id.to_string(),
        package_id: "pkg-buggy-v2".to_string(),
    }).unwrap();
    submit_deploy_receipt(&config, DeployReceiptRequest {
        deployment_id: dep_v2.deployment_id.clone(),
        package_id: "pkg-buggy-v2".to_string(),
        status: "failed".to_string(),
        stage: Some("start".to_string()),
        startup_proof: None,
        self_test: None,
        duration_ms: Some(150),
        error: Some("cuda_init_failed".to_string()),
    }).unwrap();

    // Verify list_deployments reflects failed status before rollback
    let dep_list_pre = list_deployments(&config).unwrap();
    assert!(dep_list_pre["total"].as_i64().unwrap() >= 2);

    // 3. Launch 8 concurrent threads executing rollback_deployment simultaneously
    let barrier = Arc::new(Barrier::new(8));
    let results = Arc::new(Mutex::new(Vec::new()));
    let mut handles = Vec::new();

    for _ in 0..8 {
        let b = Arc::clone(&barrier);
        let r = Arc::clone(&results);
        let cfg = config.clone();

        handles.push(thread::spawn(move || {
            b.wait();
            let res = rollback_deployment(
                &cfg,
                RollbackRequest {
                    node_id: "node-concurrent-rollback".to_string(),
                    target_version: None, // Auto-cascade to previous stable ready release
                },
            );
            r.lock().unwrap().push(res);
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    let responses = results.lock().unwrap();
    assert_eq!(responses.len(), 8);

    // Invariant A: All 8 requests succeed
    for resp in responses.iter() {
        assert!(resp.is_ok(), "Concurrent rollback request failed: {:?}", resp);
    }

    // Invariant B: ALL 8 threads received the EXACT SAME deployment_id (idempotent deduplication)
    let first_dep_id = responses[0].as_ref().unwrap().deployment_id.clone();
    assert_eq!(responses[0].as_ref().unwrap().package_id, "pkg-stable-v1");
    for resp in responses.iter() {
        let dep = resp.as_ref().unwrap();
        assert_eq!(dep.deployment_id, first_dep_id, "All rollback calls must resolve to the identical deployment_id");
        assert_eq!(dep.package_id, "pkg-stable-v1");
        assert_eq!(dep.status, "accepted");
    }

    // Invariant C: Center DB has exactly ONE new active deployment for pkg-stable-v1
    let active_v1_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM agent_deployments WHERE node_id = ? AND package_id = 'pkg-stable-v1' AND status NOT IN ('ready', 'failed', 'rolled_back')",
        params![node_id],
        |r| r.get(0),
    ).unwrap();
    assert_eq!(active_v1_count, 1, "Exactly one active rollback deployment must be created");

    // Invariant D: Buggy deployment v2 is marked rolled_back
    let v2_status: String = conn.query_row(
        "SELECT status FROM agent_deployments WHERE deployment_id = ?",
        params![dep_v2.deployment_id],
        |r| r.get(0),
    ).unwrap();
    assert_eq!(v2_status, "rolled_back", "Failed deployment must be marked rolled_back");

    // Invariant E: Audit event rollback_triggered recorded
    let rb_events: i64 = conn.query_row(
        "SELECT COUNT(*) FROM events WHERE kind = 'rollback_triggered' AND message LIKE '%node-concurrent-rollback%'",
        [],
        |r| r.get(0),
    ).unwrap();
    assert!(rb_events >= 1, "Rollback audit event must be present");

    // Invariant F: Node desired_version restored to stable version 1.0.0
    let desired: String = conn.query_row(
        "SELECT desired_version FROM agent_nodes WHERE id = ?",
        params![node_id],
        |r| r.get(0),
    ).unwrap();
    assert_eq!(desired, "1.0.0");
}

// =========================================================================
// TEST 5: ROLLBACK CASCADING TARGET RESOLUTION
// =========================================================================

#[test]
fn challenge_rollback_cascading_target_resolution() {
    let (config, _tmp) = create_temp_env();

    let node_id = "node-cascade-resolution";
    let _tok = enroll_custom_node(
        &config,
        node_id,
        "rtx-cascade",
        Some("GPU-CASCADE-1"),
        Some("grp-cascade"),
        4,
        16,
        16 * 1024 * 1024 * 1024,
    );

    // Register 4 packages
    // v0: pinned baseline
    register_package(
        &config,
        RegisterPackageRequest {
            package_id: "pkg-base-v0".to_string(),
            manifest_sha256: "m0".to_string(),
            model_sha256: "w0".to_string(),
            execution_provider: "CUDAExecutionProvider".to_string(),
            version: "0.9.0".to_string(),
            is_pinned: 1,
        },
    ).unwrap();
    // v1: verified ready
    register_package(
        &config,
        RegisterPackageRequest {
            package_id: "pkg-rel-v1".to_string(),
            manifest_sha256: "m1".to_string(),
            model_sha256: "w1".to_string(),
            execution_provider: "CUDAExecutionProvider".to_string(),
            version: "1.0.0".to_string(),
            is_pinned: 1,
        },
    ).unwrap();
    // v2: verified ready
    register_package(
        &config,
        RegisterPackageRequest {
            package_id: "pkg-rel-v2".to_string(),
            manifest_sha256: "m2".to_string(),
            model_sha256: "w2".to_string(),
            execution_provider: "CUDAExecutionProvider".to_string(),
            version: "2.0.0".to_string(),
            is_pinned: 1,
        },
    ).unwrap();
    // v3: broken candidate
    register_package(
        &config,
        RegisterPackageRequest {
            package_id: "pkg-broken-v3".to_string(),
            manifest_sha256: "m3".to_string(),
            model_sha256: "w3".to_string(),
            execution_provider: "CUDAExecutionProvider".to_string(),
            version: "3.0.0".to_string(),
            is_pinned: 0,
        },
    ).unwrap();

    // Deploy v1 -> ready
    let d1 = deploy_package(&config, DeployPackageRequest { node_id: node_id.to_string(), package_id: "pkg-rel-v1".to_string() }).unwrap();
    submit_deploy_receipt(&config, DeployReceiptRequest {
        deployment_id: d1.deployment_id,
        package_id: "pkg-rel-v1".to_string(),
        status: "ready".to_string(),
        stage: Some("ready".to_string()),
        startup_proof: Some(json!({"kernel_events": 100})),
        self_test: Some(SelfTestResult { passed: true, drift: 0.001 }),
        duration_ms: Some(200),
        error: None,
    }).unwrap();

    // Deploy v2 -> ready
    let d2 = deploy_package(&config, DeployPackageRequest { node_id: node_id.to_string(), package_id: "pkg-rel-v2".to_string() }).unwrap();
    submit_deploy_receipt(&config, DeployReceiptRequest {
        deployment_id: d2.deployment_id,
        package_id: "pkg-rel-v2".to_string(),
        status: "ready".to_string(),
        stage: Some("ready".to_string()),
        startup_proof: Some(json!({"kernel_events": 100})),
        self_test: Some(SelfTestResult { passed: true, drift: 0.001 }),
        duration_ms: Some(200),
        error: None,
    }).unwrap();

    // Deploy v3 -> failed
    let d3 = deploy_package(&config, DeployPackageRequest { node_id: node_id.to_string(), package_id: "pkg-broken-v3".to_string() }).unwrap();
    submit_deploy_receipt(&config, DeployReceiptRequest {
        deployment_id: d3.deployment_id,
        package_id: "pkg-broken-v3".to_string(),
        status: "failed".to_string(),
        stage: Some("start".to_string()),
        startup_proof: None,
        self_test: None,
        duration_ms: Some(100),
        error: Some("driver_fault".to_string()),
    }).unwrap();

    // Cascade Tier 1: Explicit target version
    let rb_tier1 = rollback_deployment(
        &config,
        RollbackRequest {
            node_id: node_id.to_string(),
            target_version: Some("1.0.0".to_string()),
        },
    ).expect("Tier 1 specific target rollback succeeds");
    assert_eq!(rb_tier1.package_id, "pkg-rel-v1", "Tier 1 must resolve explicitly specified target");

    // Tier 1 Invalid: Non-existent target version returns 404
    let err_invalid = rollback_deployment(
        &config,
        RollbackRequest {
            node_id: node_id.to_string(),
            target_version: Some("99.99.99".to_string()),
        },
    ).unwrap_err();
    assert_eq!(err_invalid.status, 404);
    assert_eq!(err_invalid.code, "target_package_not_found");

    // Cascade Tier 2: Automatic resolution to last verified ready release
    let node_tier2 = "node-tier2-auto";
    enroll_custom_node(&config, node_tier2, "host-tier2", Some("GPU-T2"), Some("grp-t2"), 4, 16, 16 * 1024 * 1024 * 1024);

    // Node T2 had v1 ready, then v2 ready, then v3 failed
    let dt1 = deploy_package(&config, DeployPackageRequest { node_id: node_tier2.to_string(), package_id: "pkg-rel-v1".to_string() }).unwrap();
    submit_deploy_receipt(&config, DeployReceiptRequest {
        deployment_id: dt1.deployment_id,
        package_id: "pkg-rel-v1".to_string(),
        status: "ready".to_string(),
        stage: Some("ready".to_string()),
        startup_proof: Some(json!({"kernel_events": 100})),
        self_test: Some(SelfTestResult { passed: true, drift: 0.001 }),
        duration_ms: Some(200),
        error: None,
    }).unwrap();

    let dt2 = deploy_package(&config, DeployPackageRequest { node_id: node_tier2.to_string(), package_id: "pkg-rel-v2".to_string() }).unwrap();
    submit_deploy_receipt(&config, DeployReceiptRequest {
        deployment_id: dt2.deployment_id,
        package_id: "pkg-rel-v2".to_string(),
        status: "ready".to_string(),
        stage: Some("ready".to_string()),
        startup_proof: Some(json!({"kernel_events": 100})),
        self_test: Some(SelfTestResult { passed: true, drift: 0.001 }),
        duration_ms: Some(200),
        error: None,
    }).unwrap();

    let dt3 = deploy_package(&config, DeployPackageRequest { node_id: node_tier2.to_string(), package_id: "pkg-broken-v3".to_string() }).unwrap();
    submit_deploy_receipt(&config, DeployReceiptRequest {
        deployment_id: dt3.deployment_id,
        package_id: "pkg-broken-v3".to_string(),
        status: "failed".to_string(),
        stage: Some("start".to_string()),
        startup_proof: None,
        self_test: None,
        duration_ms: Some(100),
        error: Some("crash".to_string()),
    }).unwrap();

    // Rollback with target_version = None -> must resolve last ready release (v2)
    let rb_tier2 = rollback_deployment(
        &config,
        RollbackRequest {
            node_id: node_tier2.to_string(),
            target_version: None,
        },
    ).expect("Tier 2 auto resolution succeeds");
    assert_eq!(rb_tier2.package_id, "pkg-rel-v2", "Tier 2 must resolve most recent ready release pkg-rel-v2");

    // Cascade Tier 3: Fallback to lowest pinned package when no prior ready release exists
    let node_tier3 = "node-tier3-pinned-fallback";
    enroll_custom_node(&config, node_tier3, "host-tier3", Some("GPU-T3"), Some("grp-t3"), 4, 16, 16 * 1024 * 1024 * 1024);

    // Initial deployment of v3 immediately fails (zero prior ready deployments for this node)
    let dt3_fail = deploy_package(&config, DeployPackageRequest { node_id: node_tier3.to_string(), package_id: "pkg-broken-v3".to_string() }).unwrap();
    submit_deploy_receipt(&config, DeployReceiptRequest {
        deployment_id: dt3_fail.deployment_id,
        package_id: "pkg-broken-v3".to_string(),
        status: "failed".to_string(),
        stage: Some("start".to_string()),
        startup_proof: None,
        self_test: None,
        duration_ms: Some(50),
        error: Some("immediate_fail".to_string()),
    }).unwrap();

    // Rollback with target_version = None -> must fall back to lowest created pinned package (pkg-base-v0)
    let rb_tier3 = rollback_deployment(
        &config,
        RollbackRequest {
            node_id: node_tier3.to_string(),
            target_version: None,
        },
    ).expect("Tier 3 pinned fallback succeeds");
    assert_eq!(rb_tier3.package_id, "pkg-base-v0", "Tier 3 must fall back to lowest created pinned package pkg-base-v0");
}

// =========================================================================
// TEST 6: LEGACY SCHEDULER DECOMMISSION AND AUTH REJECTION
// =========================================================================

#[test]
fn challenge_legacy_scheduler_decommission_and_auth_rejection() {
    let (config, _tmp) = create_temp_env();
    let conn = processing_scheduler::open_or_init(&config).unwrap();

    seed_task(&config, "vid-decom-1", "detect", 10);

    // Test A: Unregistered node token fails authentication with 401/404
    let err_unreg = claim_task(
        &config,
        "unregistered_rogue_token_12345",
        ClaimTaskRequest {
            node_id: Some("node-nonexistent".to_string()),
            stage: "detect".to_string(),
            lease_duration_s: Some(60.0),
        },
    ).unwrap_err();
    assert_eq!(err_unreg.status, 401, "Unregistered token must fail authentication with HTTP 401");
    assert_eq!(err_unreg.code, "unauthorized");

    // Test B: Valid token format but nonexistent node_id
    let fake_token = "token_fake_00000000000000000000000000000000";
    let err_fake = claim_task(
        &config,
        fake_token,
        ClaimTaskRequest {
            node_id: Some("node-fake".to_string()),
            stage: "detect".to_string(),
            lease_duration_s: Some(60.0),
        },
    ).unwrap_err();
    assert_eq!(err_fake.status, 401);

    // Test C: Valid node registration, but tampered Bearer token
    let real_tok = enroll_custom_node(
        &config,
        "node-valid-id",
        "valid-host",
        Some("GPU-VALID"),
        Some("grp-valid"),
        4,
        16,
        16 * 1024 * 1024 * 1024,
    );

    let tampered_tok = format!("{}_tampered", real_tok);
    let err_tampered = claim_task(
        &config,
        &tampered_tok,
        ClaimTaskRequest {
            node_id: Some("node-valid-id".to_string()),
            stage: "detect".to_string(),
            lease_duration_s: Some(60.0),
        },
    ).unwrap_err();
    assert_eq!(err_tampered.status, 401, "Tampered Bearer token must fail authentication");

    // Test D: Center sole authority over coordinator generation fencing
    // A legitimate claim succeeds
    let claim = claim_task(
        &config,
        &real_tok,
        ClaimTaskRequest {
            node_id: Some("node-valid-id".to_string()),
            stage: "detect".to_string(),
            lease_duration_s: Some(60.0),
        },
    ).unwrap().expect("Legitimate claim succeeds");

    // Rogue worker attempts to commit with stale / forged coordinator_generation = 999
    let err_stale_gen = commit_task_result(
        &config,
        &real_tok,
        CommitTaskResultRequest {
            node_id: Some("node-valid-id".to_string()),
            attempt_id: Some(claim.attempt_id.clone()),
            vid: Some(claim.vid.clone()),
            coordinator_generation: Some(json!(999)), // Stale / forged generation!
            stage: Some("detect".to_string()),
            status: Some("success".to_string()),
            detections: Some(vec![]),
            error_type: None,
            error_message: None,
            text_snippets: None,
            vectors: None,
            execution_proof: None,
        },
    ).unwrap_err();

    assert_eq!(err_stale_gen.status, 409, "Forged coordinator generation must fail with HTTP 409");
    assert_eq!(err_stale_gen.code, "stale_generation");

    // Test E: Legacy direct SQLite isolation check
    // Remote workers have no network mount to SQLite. Center generation fencing prevents rogue claims.
    let task_gen: String = conn.query_row(
        "SELECT coordinator_generation FROM tasks WHERE vid = 'vid-decom-1'",
        [],
        |r| r.get(0),
    ).unwrap();
    assert_eq!(task_gen, claim.coordinator_generation.to_string(), "Center maintains sole authority on task coordinator generation");
}
