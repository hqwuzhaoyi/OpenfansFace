use face_file_operations::agent_deployment::{
    deploy_package, list_deployments, register_package, rollback_deployment,
    submit_deploy_receipt, DeployPackageRequest, DeployReceiptRequest, RegisterPackageRequest,
    RollbackRequest, SelfTestResult, DRIFT_TOLERANCE,
};
use face_file_operations::agent_registry::{
    create_enrollment_token, register_node, EnrollmentTokenRequest, GpuSpecs, RegisterRequest,
};
use face_file_operations::config::Config;
use face_file_operations::processing_scheduler;
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;

fn temp_fixture_config() -> (Config, PathBuf) {
    let tmp_dir = std::env::temp_dir().join(format!("face-challenger-m2-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&tmp_dir).unwrap();
    let media = tmp_dir.join("media");
    fs::create_dir_all(&media).unwrap();
    let ledger = tmp_dir.join("ledger.db");

    let config = Config {
        base: tmp_dir.clone(),
        media,
        ledger,
        bind: "127.0.0.1:0".to_string(),
        token: Some("test-admin-secret".to_string()),
        fixture: None,
        parity_root: None,
    };

    (config, tmp_dir)
}

fn register_test_node(config: &Config, hostname: &str, os: &str, arch: &str) -> String {
    let token_resp = create_enrollment_token(
        config,
        EnrollmentTokenRequest {
            note: Some("Challenger test node".to_string()),
            ttl_seconds: Some(3600.0),
        },
    )
    .expect("enrollment token creation");

    let reg_req = RegisterRequest {
        enrollment_token: token_resp.token,
        hostname: hostname.to_string(),
        os: os.to_string(),
        arch: arch.to_string(),
        gpu: Some(GpuSpecs {
            model: "NVIDIA GeForce RTX 4070 Ti".to_string(),
            vram_bytes: 12884901888,
            device_uuid: format!("GPU-uuid-{}", hostname),
        }),
        ram_bytes: 34359738368,
        cpu_cores: 16,
        agent_version: "0.1.0".to_string(),
        max_concurrent_tasks: None,
    };

    let reg_resp = register_node(config, reg_req).expect("node registration");
    reg_resp.node_id
}

/// Helper to register a test package
fn register_test_package(config: &Config, pkg_id: &str, version: &str) {
    register_package(
        config,
        RegisterPackageRequest {
            package_id: pkg_id.to_string(),
            manifest_sha256: format!("manifest-sha-{}", pkg_id),
            model_sha256: format!("model-sha-{}", pkg_id),
            execution_provider: "CUDAExecutionProvider".to_string(),
            version: version.to_string(),
            is_pinned: 1,
        },
    )
    .expect("register package");
}

// =========================================================================
// OBJECTIVE 1: DEPLOY RECEIPT VALIDATION STRESS
// =========================================================================

/// Test drift validation stress at fine-grained boundaries:
/// - drift = 0.049 -> ACCEPT (status = 'ready', model_ready = 1)
/// - drift = 0.050 -> ACCEPT (status = 'ready', exact tolerance bound)
/// - drift = 0.05000000001 -> REJECT with 422 (status = 'failed')
/// - drift = 0.051 -> REJECT with 422 (status = 'failed')
/// - drift = 0.100 -> REJECT with 422 (status = 'failed')
#[test]
fn challenge_receipt_drift_boundary_stress() {
    let (config, _dir) = temp_fixture_config();
    let node_id = register_test_node(&config, "wsl-drift-test", "linux", "x86_64");
    register_test_package(&config, "pkg-v1", "1.0.0");

    // 1. drift = 0.049 (Must accept and transition to ready)
    let dep1 = deploy_package(
        &config,
        DeployPackageRequest {
            node_id: node_id.clone(),
            package_id: "pkg-v1".to_string(),
        },
    )
    .expect("deploy dep1");

    let resp1 = submit_deploy_receipt(
        &config,
        DeployReceiptRequest {
            deployment_id: dep1.deployment_id.clone(),
            package_id: "pkg-v1".to_string(),
            status: "ready".to_string(),
            stage: Some("ready".to_string()),
            startup_proof: Some(json!({"kernel_events": 141})),
            self_test: Some(SelfTestResult {
                passed: true,
                drift: 0.049,
            }),
            duration_ms: Some(1000),
            error: None,
        },
    )
    .expect("drift = 0.049 must be accepted");
    assert!(resp1.ok);

    // Verify deployment status is ready and node is model_ready = 1
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    let (status1, stage1, drift1): (String, String, f64) = conn
        .query_row(
            "SELECT status, stage, self_test_drift FROM agent_deployments WHERE deployment_id = ?",
            [&dep1.deployment_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(status1, "ready");
    assert_eq!(stage1, "ready");
    assert!((drift1 - 0.049).abs() < 1e-6);

    let (model_ready1, node_status1): (i32, String) = conn
        .query_row(
            "SELECT model_ready, status FROM agent_nodes WHERE id = ?",
            [&node_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(model_ready1, 1);
    assert_eq!(node_status1, "accepting");

    // 2. drift = 0.050 (Exact tolerance bound DRIFT_TOLERANCE: must accept)
    register_test_package(&config, "pkg-v2", "2.0.0");
    let dep2 = deploy_package(
        &config,
        DeployPackageRequest {
            node_id: node_id.clone(),
            package_id: "pkg-v2".to_string(),
        },
    )
    .expect("deploy dep2");

    let resp2 = submit_deploy_receipt(
        &config,
        DeployReceiptRequest {
            deployment_id: dep2.deployment_id.clone(),
            package_id: "pkg-v2".to_string(),
            status: "ready".to_string(),
            stage: Some("ready".to_string()),
            startup_proof: Some(json!({"kernel_events": 141})),
            self_test: Some(SelfTestResult {
                passed: true,
                drift: DRIFT_TOLERANCE,
            }),
            duration_ms: Some(1100),
            error: None,
        },
    )
    .expect("drift = 0.050 exact bound must be accepted");
    assert!(resp2.ok);

    // 3. drift = 0.05000000001 (Microscopic exceed: must reject with 422)
    register_test_package(&config, "pkg-v3", "3.0.0");
    let dep3 = deploy_package(
        &config,
        DeployPackageRequest {
            node_id: node_id.clone(),
            package_id: "pkg-v3".to_string(),
        },
    )
    .expect("deploy dep3");

    let err3 = submit_deploy_receipt(
        &config,
        DeployReceiptRequest {
            deployment_id: dep3.deployment_id.clone(),
            package_id: "pkg-v3".to_string(),
            status: "ready".to_string(),
            stage: Some("ready".to_string()),
            startup_proof: Some(json!({"kernel_events": 141})),
            self_test: Some(SelfTestResult {
                passed: true,
                drift: 0.05000000001,
            }),
            duration_ms: Some(1100),
            error: None,
        },
    )
    .unwrap_err();
    assert_eq!(err3.status, 422, "drift > 0.05 must return HTTP 422");
    assert_eq!(err3.code, "self_test_drift_exceeded");

    // Verify dep3 status is failed in database
    let (status3, stage3, err_msg3): (String, String, Option<String>) = conn
        .query_row(
            "SELECT status, stage, error FROM agent_deployments WHERE deployment_id = ?",
            [&dep3.deployment_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(status3, "failed");
    assert_eq!(stage3, "self_test");
    assert_eq!(err_msg3, Some("self_test_drift_exceeded".to_string()));

    // 4. drift = 0.051 (Must reject with 422)
    register_test_package(&config, "pkg-v4", "4.0.0");
    let dep4 = deploy_package(
        &config,
        DeployPackageRequest {
            node_id: node_id.clone(),
            package_id: "pkg-v4".to_string(),
        },
    )
    .expect("deploy dep4");

    let err4 = submit_deploy_receipt(
        &config,
        DeployReceiptRequest {
            deployment_id: dep4.deployment_id.clone(),
            package_id: "pkg-v4".to_string(),
            status: "ready".to_string(),
            stage: Some("ready".to_string()),
            startup_proof: Some(json!({"kernel_events": 141})),
            self_test: Some(SelfTestResult {
                passed: true,
                drift: 0.051,
            }),
            duration_ms: Some(1200),
            error: None,
        },
    )
    .unwrap_err();
    assert_eq!(err4.status, 422);
    assert_eq!(err4.code, "self_test_drift_exceeded");

    // 5. drift = 0.100 (Gross exceed: must reject with 422)
    register_test_package(&config, "pkg-v5", "5.0.0");
    let dep5 = deploy_package(
        &config,
        DeployPackageRequest {
            node_id: node_id.clone(),
            package_id: "pkg-v5".to_string(),
        },
    )
    .expect("deploy dep5");

    let err5 = submit_deploy_receipt(
        &config,
        DeployReceiptRequest {
            deployment_id: dep5.deployment_id.clone(),
            package_id: "pkg-v5".to_string(),
            status: "ready".to_string(),
            stage: Some("ready".to_string()),
            startup_proof: Some(json!({"kernel_events": 141})),
            self_test: Some(SelfTestResult {
                passed: true,
                drift: 0.1,
            }),
            duration_ms: Some(1200),
            error: None,
        },
    )
    .unwrap_err();
    assert_eq!(err5.status, 422);
    assert_eq!(err5.code, "self_test_drift_exceeded");
}

/// Test self_test.passed = false handling under both 'ready' and 'failed' status
#[test]
fn challenge_self_test_passed_false_stress() {
    let (config, _dir) = temp_fixture_config();
    let node_id = register_test_node(&config, "wsl-selftest-fail", "linux", "x86_64");
    register_test_package(&config, "pkg-v1", "1.0.0");

    let conn = processing_scheduler::open_or_init(&config).unwrap();

    // Scenario A: Agent deceitfully submits status = 'ready', but self_test.passed = false
    let dep_a = deploy_package(
        &config,
        DeployPackageRequest {
            node_id: node_id.clone(),
            package_id: "pkg-v1".to_string(),
        },
    )
    .expect("deploy dep_a");

    let err_a = submit_deploy_receipt(
        &config,
        DeployReceiptRequest {
            deployment_id: dep_a.deployment_id.clone(),
            package_id: "pkg-v1".to_string(),
            status: "ready".to_string(),
            stage: Some("ready".to_string()),
            startup_proof: Some(json!({"kernel_events": 141})),
            self_test: Some(SelfTestResult {
                passed: false, // FALSE!
                drift: 0.001,  // small drift, but passed is false
            }),
            duration_ms: Some(1000),
            error: None,
        },
    )
    .unwrap_err();

    assert_eq!(err_a.status, 422, "status=ready with passed=false must be rejected with 422");
    assert_eq!(err_a.code, "self_test_drift_exceeded");

    // Verify deployment is marked failed
    let (status_a, passed_a): (String, Option<i32>) = conn
        .query_row(
            "SELECT status, self_test_passed FROM agent_deployments WHERE deployment_id = ?",
            [&dep_a.deployment_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(status_a, "failed");
    assert_eq!(passed_a, Some(0));

    // Verify node is NOT accepting or model_ready
    let model_ready_a: i32 = conn
        .query_row(
            "SELECT model_ready FROM agent_nodes WHERE id = ?",
            [&node_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(model_ready_a, 0);

    // Scenario B: Agent honestly reports status = 'failed' due to self-test failure
    let dep_b = deploy_package(
        &config,
        DeployPackageRequest {
            node_id: node_id.clone(),
            package_id: "pkg-v1".to_string(),
        },
    )
    .expect("deploy dep_b");

    let resp_b = submit_deploy_receipt(
        &config,
        DeployReceiptRequest {
            deployment_id: dep_b.deployment_id.clone(),
            package_id: "pkg-v1".to_string(),
            status: "failed".to_string(),
            stage: Some("self_test".to_string()),
            startup_proof: None,
            self_test: Some(SelfTestResult {
                passed: false,
                drift: 0.095,
            }),
            duration_ms: Some(800),
            error: Some("landmark_drift_exceeded_floor".to_string()),
        },
    )
    .expect("honestly reported failure must be acknowledged");

    assert!(resp_b.ok);
    assert_eq!(resp_b.marked_failed, Some(true));

    // Verify node model_ready is explicitly set to 0
    let model_ready_b: i32 = conn
        .query_row(
            "SELECT model_ready FROM agent_nodes WHERE id = ?",
            [&node_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(model_ready_b, 0);

    // Scenario C: Agent submits status = 'ready' but omits self_test object entirely
    let dep_c = deploy_package(
        &config,
        DeployPackageRequest {
            node_id: node_id.clone(),
            package_id: "pkg-v1".to_string(),
        },
    )
    .expect("deploy dep_c");

    let err_c = submit_deploy_receipt(
        &config,
        DeployReceiptRequest {
            deployment_id: dep_c.deployment_id.clone(),
            package_id: "pkg-v1".to_string(),
            status: "ready".to_string(),
            stage: Some("ready".to_string()),
            startup_proof: Some(json!({"kernel_events": 141})),
            self_test: None, // Missing!
            duration_ms: Some(1000),
            error: None,
        },
    )
    .unwrap_err();

    assert_eq!(err_c.status, 422, "Missing self_test must be rejected with 422");
}

/// Test receipt submission for non-existent or invalid deployment_id
#[test]
fn challenge_receipt_non_existent_deployment_id() {
    let (config, _dir) = temp_fixture_config();

    // 1. Completely random non-existent deployment_id -> 404
    let err1 = submit_deploy_receipt(
        &config,
        DeployReceiptRequest {
            deployment_id: "dep-non-existent-999999".to_string(),
            package_id: "pkg-v1".to_string(),
            status: "ready".to_string(),
            stage: Some("ready".to_string()),
            startup_proof: None,
            self_test: Some(SelfTestResult {
                passed: true,
                drift: 0.001,
            }),
            duration_ms: Some(100),
            error: None,
        },
    )
    .unwrap_err();

    assert_eq!(err1.status, 404);
    assert_eq!(err1.code, "deployment_not_found");

    // 2. Empty deployment_id -> 400
    let err2 = submit_deploy_receipt(
        &config,
        DeployReceiptRequest {
            deployment_id: "".to_string(),
            package_id: "pkg-v1".to_string(),
            status: "ready".to_string(),
            stage: Some("ready".to_string()),
            startup_proof: None,
            self_test: None,
            duration_ms: None,
            error: None,
        },
    )
    .unwrap_err();

    assert_eq!(err2.status, 400);
    assert_eq!(err2.code, "invalid_request");

    // 3. Whitespace-only deployment_id -> 400
    let err3 = submit_deploy_receipt(
        &config,
        DeployReceiptRequest {
            deployment_id: "   \t\n".to_string(),
            package_id: "pkg-v1".to_string(),
            status: "ready".to_string(),
            stage: None,
            startup_proof: None,
            self_test: None,
            duration_ms: None,
            error: None,
        },
    )
    .unwrap_err();

    assert_eq!(err3.status, 400);
}

// =========================================================================
// OBJECTIVE 2: DEPLOYMENT DISPATCH RACE & ERROR CONDITIONS
// =========================================================================

/// Deploy to offline node must return 409
#[test]
fn challenge_deploy_to_offline_node_stress() {
    let (config, _dir) = temp_fixture_config();
    let node_id = register_test_node(&config, "wsl-offline-node", "linux", "x86_64");
    register_test_package(&config, "pkg-v1", "1.0.0");

    let conn = processing_scheduler::open_or_init(&config).unwrap();

    // 1. Explicit status = 'offline' -> 409
    conn.execute(
        "UPDATE agent_nodes SET status = 'offline' WHERE id = ?",
        [&node_id],
    )
    .unwrap();

    let err1 = deploy_package(
        &config,
        DeployPackageRequest {
            node_id: node_id.clone(),
            package_id: "pkg-v1".to_string(),
        },
    )
    .unwrap_err();
    assert_eq!(err1.status, 409);
    assert_eq!(err1.code, "node_offline");

    // 2. Implicit offline: status = 'online', but last_heartbeat_at is 150 seconds ago (> 90s) -> 409
    let now = face_file_operations::agent_deployment::now_sec();
    conn.execute(
        "UPDATE agent_nodes SET status = 'online', last_heartbeat_at = ? WHERE id = ?",
        rusqlite::params![now - 150.0, node_id],
    )
    .unwrap();

    let err2 = deploy_package(
        &config,
        DeployPackageRequest {
            node_id: node_id.clone(),
            package_id: "pkg-v1".to_string(),
        },
    )
    .unwrap_err();
    assert_eq!(err2.status, 409);
    assert_eq!(err2.code, "node_offline");

    // 3. Online with fresh heartbeat (10s ago) -> SUCCESS
    conn.execute(
        "UPDATE agent_nodes SET status = 'online', last_heartbeat_at = ? WHERE id = ?",
        rusqlite::params![now - 10.0, node_id],
    )
    .unwrap();

    let ok_resp = deploy_package(
        &config,
        DeployPackageRequest {
            node_id: node_id.clone(),
            package_id: "pkg-v1".to_string(),
        },
    )
    .expect("freshly online node should accept deploy");
    assert_eq!(ok_resp.status, "accepted");
}

/// Deploy unregistered package_id must return 404
#[test]
fn challenge_deploy_unregistered_package_id_stress() {
    let (config, _dir) = temp_fixture_config();
    let node_id = register_test_node(&config, "wsl-unreg-node", "linux", "x86_64");

    // 1. Unregistered package -> 404
    let err1 = deploy_package(
        &config,
        DeployPackageRequest {
            node_id: node_id.clone(),
            package_id: "pkg-ghost-unregistered".to_string(),
        },
    )
    .unwrap_err();
    assert_eq!(err1.status, 404);
    assert_eq!(err1.code, "package_not_found");

    // 2. Empty package_id -> 400
    let err2 = deploy_package(
        &config,
        DeployPackageRequest {
            node_id: node_id.clone(),
            package_id: "".to_string(),
        },
    )
    .unwrap_err();
    assert_eq!(err2.status, 400);

    // 3. Ghost node_id -> 404 node_not_found
    register_test_package(&config, "pkg-v1", "1.0.0");
    let err3 = deploy_package(
        &config,
        DeployPackageRequest {
            node_id: "node-ghost-nonexistent".to_string(),
            package_id: "pkg-v1".to_string(),
        },
    )
    .unwrap_err();
    assert_eq!(err3.status, 404);
    assert_eq!(err3.code, "node_not_found");
}

/// Concurrent deployment requests to the same node from multiple threads.
/// Tests database locking, transactional isolation, and consistency.
#[test]
fn challenge_concurrent_deployment_dispatch_to_same_node() {
    let (config, _dir) = temp_fixture_config();
    let node_id = register_test_node(&config, "wsl-concurrent-node", "linux", "x86_64");

    // Register 10 packages
    for i in 1..=10 {
        register_test_package(&config, &format!("pkg-v{}", i), &format!("{}.0.0", i));
    }

    let config_arc = Arc::new(config.clone());
    let node_id_arc = Arc::new(node_id.clone());

    // Launch 10 concurrent threads dispatching different packages to the same node
    let mut handles = Vec::new();
    for i in 1..=10 {
        let cfg = Arc::clone(&config_arc);
        let nid = Arc::clone(&node_id_arc);
        let pkg = format!("pkg-v{}", i);

        handles.push(thread::spawn(move || {
            deploy_package(
                &cfg,
                DeployPackageRequest {
                    node_id: (*nid).clone(),
                    package_id: pkg,
                },
            )
        }));
    }

    let mut successful_deployments = Vec::new();
    let mut errors = Vec::new();

    for h in handles {
        match h.join().expect("thread join") {
            Ok(resp) => successful_deployments.push(resp),
            Err(e) => errors.push(e),
        }
    }

    println!(
        "Concurrent dispatch results: {} succeeded, {} errors (busy/retry)",
        successful_deployments.len(),
        errors.len()
    );

    // In SQLite WAL mode with Immediate transactions, either all succeed sequentially or some get busy
    // In our test, all 10 dispatches should succeed cleanly
    assert_eq!(successful_deployments.len(), 10, "All 10 concurrent dispatches should succeed");

    // Check SQLite database integrity
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    let dep_count: i64 = conn
        .query_row(
            "SELECT count(*) FROM agent_deployments WHERE node_id = ?",
            [&node_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(dep_count, 10, "10 deployment records must exist");

    // Node's desired_version must be one of the 10 dispatched versions
    let desired_version: String = conn
        .query_row(
            "SELECT desired_version FROM agent_nodes WHERE id = ?",
            [&node_id],
            |r| r.get(0),
        )
        .unwrap();
    let valid_versions: Vec<String> = (1..=10).map(|i| format!("{}.0.0", i)).collect();
    assert!(
        valid_versions.contains(&desired_version),
        "desired_version '{}' must be one of the dispatched versions",
        desired_version
    );

    // Verify audit events
    let event_count: i64 = conn
        .query_row(
            "SELECT count(*) FROM events WHERE kind = 'deployment_dispatched'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(event_count, 10, "10 deployment_dispatched events must be recorded");
}

/// Out-of-order and stale receipt submission challenge:
/// Dispatches v1, then dispatches v2. v2 finishes and becomes ready.
/// Then v1's delayed receipt arrives. What is the behavior?
#[test]
fn challenge_stale_receipt_and_version_transition_integrity() {
    let (config, _dir) = temp_fixture_config();
    let node_id = register_test_node(&config, "wsl-version-transition", "linux", "x86_64");

    register_test_package(&config, "pkg-v1", "1.0.0");
    register_test_package(&config, "pkg-v2", "2.0.0");

    // 1. Dispatch v1
    let dep1 = deploy_package(
        &config,
        DeployPackageRequest {
            node_id: node_id.clone(),
            package_id: "pkg-v1".to_string(),
        },
    )
    .unwrap();

    // 2. Dispatch v2 (superseding v1 desired_version)
    let dep2 = deploy_package(
        &config,
        DeployPackageRequest {
            node_id: node_id.clone(),
            package_id: "pkg-v2".to_string(),
        },
    )
    .unwrap();

    let conn = processing_scheduler::open_or_init(&config).unwrap();
    let desired_ver: String = conn
        .query_row("SELECT desired_version FROM agent_nodes WHERE id = ?", [&node_id], |r| r.get(0))
        .unwrap();
    assert_eq!(desired_ver, "2.0.0");

    // 3. v2 succeeds first and submits receipt
    let resp2 = submit_deploy_receipt(
        &config,
        DeployReceiptRequest {
            deployment_id: dep2.deployment_id.clone(),
            package_id: "pkg-v2".to_string(),
            status: "ready".to_string(),
            stage: Some("ready".to_string()),
            startup_proof: Some(json!({"kernel_events": 141})),
            self_test: Some(SelfTestResult { passed: true, drift: 0.002 }),
            duration_ms: Some(950),
            error: None,
        },
    )
    .unwrap();
    assert!(resp2.ok);

    let (current_ver2, model_ready2): (String, i32) = conn
        .query_row(
            "SELECT current_version, model_ready FROM agent_nodes WHERE id = ?",
            [&node_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(current_ver2, "2.0.0");
    assert_eq!(model_ready2, 1);

    // 4. Now v1's stale receipt arrives!
    // Let's observe what happens when v1 receipt is submitted:
    let resp1 = submit_deploy_receipt(
        &config,
        DeployReceiptRequest {
            deployment_id: dep1.deployment_id.clone(),
            package_id: "pkg-v1".to_string(),
            status: "ready".to_string(),
            stage: Some("ready".to_string()),
            startup_proof: Some(json!({"kernel_events": 141})),
            self_test: Some(SelfTestResult { passed: true, drift: 0.001 }),
            duration_ms: Some(1500),
            error: None,
        },
    );

    println!("Stale receipt submission response: {:?}", resp1);
    // In dep1 record, actual_version = 1.0.0
    let dep1_status: (String, Option<String>) = conn
        .query_row(
            "SELECT status, actual_version FROM agent_deployments WHERE deployment_id = ?",
            [&dep1.deployment_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(dep1_status.0, "ready");
    assert_eq!(dep1_status.1, Some("1.0.0".to_string()));

    let current_ver_after_stale: String = conn
        .query_row(
            "SELECT current_version FROM agent_nodes WHERE id = ?",
            [&node_id],
            |r| r.get(0),
        )
        .unwrap();
    println!("Node current_version after stale receipt: {}", current_ver_after_stale);
}

/// Challenge rollback boundary and error conditions
#[test]
fn challenge_rollback_boundaries_and_error_handling() {
    let (config, _dir) = temp_fixture_config();
    let node_id = register_test_node(&config, "wsl-rb-test", "linux", "x86_64");

    // 1. Empty node_id -> 400
    let err1 = rollback_deployment(
        &config,
        RollbackRequest {
            node_id: "".to_string(),
            target_version: None,
        },
    )
    .unwrap_err();
    assert_eq!(err1.status, 400);

    // 2. Non-existent node_id -> 404
    let err2 = rollback_deployment(
        &config,
        RollbackRequest {
            node_id: "node-ghost".to_string(),
            target_version: None,
        },
    )
    .unwrap_err();
    assert_eq!(err2.status, 404);
    assert_eq!(err2.code, "node_not_found");

    // 3. Target version does not exist -> 404
    let err3 = rollback_deployment(
        &config,
        RollbackRequest {
            node_id: node_id.clone(),
            target_version: Some("non-existent-version-9.9.9".to_string()),
        },
    )
    .unwrap_err();
    assert_eq!(err3.status, 404);
    assert_eq!(err3.code, "target_package_not_found");

    // 4. Verify list_deployments output structure
    let list_res = list_deployments(&config).expect("list deployments");
    assert!(list_res.get("deployments").is_some());
    assert!(list_res.get("total").is_some());
}

