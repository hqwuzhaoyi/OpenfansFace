use face_file_operations::agent_deployment::{
    deploy_package, list_deployments, register_package, rollback_deployment,
    submit_deploy_receipt, DeployPackageRequest, DeployReceiptRequest, RegisterPackageRequest,
    RollbackRequest, SelfTestResult,
};
use face_file_operations::agent_registry::{
    create_enrollment_token, register_node, EnrollmentTokenRequest, GpuSpecs, RegisterRequest,
};
use face_file_operations::config::Config;
use face_file_operations::processing_scheduler;
use serde_json::json;
use std::fs;
use std::path::PathBuf;

fn temp_fixture_config() -> (Config, PathBuf) {
    let tmp_dir = std::env::temp_dir().join(format!("face-deploy-test-{}", uuid::Uuid::new_v4()));
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

fn register_test_node(config: &Config, hostname: &str, os: &str, arch: &str) -> String {
    let token_resp = create_enrollment_token(
        config,
        EnrollmentTokenRequest {
            note: Some("Test node".to_string()),
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

#[test]
fn test_package_manifest_registration_and_pinning() {
    let (config, _dir) = temp_fixture_config();

    // 1. Register pinned package manifest
    let pkg_req = RegisterPackageRequest {
        package_id: "pkg-scrfd-cuda-v2.4.0".to_string(),
        manifest_sha256: "4a5b6c7d8e9f0123456789abcdef0123456789abcdef0123456789abcdef0123".to_string(),
        model_sha256: "670bfee8dbe1d2e45ad352161c8179d6e29096c3cb730861fda5f96f78cd05af".to_string(),
        execution_provider: "CUDAExecutionProvider".to_string(),
        version: "v2.4.0-cuda".to_string(),
        is_pinned: 1,
    };

    let reg_resp = register_package(&config, pkg_req).expect("register package should succeed");
    assert!(reg_resp.ok);
    assert_eq!(reg_resp.package_id, "pkg-scrfd-cuda-v2.4.0");

    // Verify row in SQLite
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    let row: (String, String, String, String, i32) = conn
        .query_row(
            "SELECT manifest_sha256, model_sha256, execution_provider, version, is_pinned FROM agent_packages WHERE package_id = ?",
            ["pkg-scrfd-cuda-v2.4.0"],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap();

    assert_eq!(row.0, "4a5b6c7d8e9f0123456789abcdef0123456789abcdef0123456789abcdef0123");
    assert_eq!(row.1, "670bfee8dbe1d2e45ad352161c8179d6e29096c3cb730861fda5f96f78cd05af");
    assert_eq!(row.2, "CUDAExecutionProvider");
    assert_eq!(row.3, "v2.4.0-cuda");
    assert_eq!(row.4, 1);

    // Validation: empty package_id fails
    let invalid_req = RegisterPackageRequest {
        package_id: "".to_string(),
        manifest_sha256: "hash".to_string(),
        model_sha256: "hash".to_string(),
        execution_provider: "CUDAExecutionProvider".to_string(),
        version: "1.0.0".to_string(),
        is_pinned: 1,
    };
    let err = register_package(&config, invalid_req).unwrap_err();
    assert_eq!(err.status, 400);
}

#[test]
fn test_deploy_dispatch_happy_path_and_state_transition() {
    let (config, _dir) = temp_fixture_config();
    let node_id = register_test_node(&config, "wsl-rtx4070ti", "linux", "x86_64");

    // Register package
    register_package(
        &config,
        RegisterPackageRequest {
            package_id: "pkg-v1".to_string(),
            manifest_sha256: "m-hash-1".to_string(),
            model_sha256: "w-hash-1".to_string(),
            execution_provider: "CUDAExecutionProvider".to_string(),
            version: "1.0.0".to_string(),
            is_pinned: 1,
        },
    )
    .unwrap();

    // Dispatch deploy
    let deploy_resp = deploy_package(
        &config,
        DeployPackageRequest {
            node_id: node_id.clone(),
            package_id: "pkg-v1".to_string(),
        },
    )
    .expect("deploy dispatch should succeed");

    assert_eq!(deploy_resp.status, "accepted");
    assert!(deploy_resp.deployment_id.starts_with(&format!("dep-{}-", node_id)));
    assert_eq!(deploy_resp.manifest_sha256, "m-hash-1");
    assert_eq!(deploy_resp.model_sha256, "w-hash-1");
    assert_eq!(deploy_resp.execution_provider, "CUDAExecutionProvider");

    // Check deployment record in SQLite
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    let dep_row: (String, String, String, Option<String>) = conn
        .query_row(
            "SELECT status, desired_version, stage, actual_version FROM agent_deployments WHERE deployment_id = ?",
            [&deploy_resp.deployment_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();

    assert_eq!(dep_row.0, "downloading");
    assert_eq!(dep_row.1, "1.0.0");
    assert_eq!(dep_row.2, "download");
    assert_eq!(dep_row.3, None);

    // Check node desired_version updated
    let node_desired: String = conn
        .query_row(
            "SELECT desired_version FROM agent_nodes WHERE id = ?",
            [&node_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(node_desired, "1.0.0");
}

#[test]
fn test_deploy_error_handling_missing_node_package_and_offline() {
    let (config, _dir) = temp_fixture_config();
    let node_id = register_test_node(&config, "wsl-node", "linux", "x86_64");

    // 1. Missing package -> 404
    let err1 = deploy_package(
        &config,
        DeployPackageRequest {
            node_id: node_id.clone(),
            package_id: "pkg-non-existent".to_string(),
        },
    )
    .unwrap_err();
    assert_eq!(err1.status, 404);
    assert_eq!(err1.error, "package_not_found");

    // 2. Missing node -> 404
    let err2 = deploy_package(
        &config,
        DeployPackageRequest {
            node_id: "node-ghost".to_string(),
            package_id: "pkg-v1".to_string(),
        },
    )
    .unwrap_err();
    assert_eq!(err2.status, 404);
    assert_eq!(err2.error, "node_not_found");

    // 3. Offline node -> 409
    register_package(
        &config,
        RegisterPackageRequest {
            package_id: "pkg-v1".to_string(),
            manifest_sha256: "m".to_string(),
            model_sha256: "w".to_string(),
            execution_provider: "CUDAExecutionProvider".to_string(),
            version: "1.0.0".to_string(),
            is_pinned: 1,
        },
    )
    .unwrap();

    let conn = processing_scheduler::open_or_init(&config).unwrap();
    conn.execute(
        "UPDATE agent_nodes SET status = 'offline' WHERE id = ?",
        [&node_id],
    )
    .unwrap();

    let err3 = deploy_package(
        &config,
        DeployPackageRequest {
            node_id: node_id.clone(),
            package_id: "pkg-v1".to_string(),
        },
    )
    .unwrap_err();
    assert_eq!(err3.status, 409);
    assert_eq!(err3.error, "node_offline");
}

#[test]
fn test_deploy_receipt_self_test_pass_and_node_accepting() {
    let (config, _dir) = temp_fixture_config();
    let node_id = register_test_node(&config, "wsl-node", "linux", "x86_64");

    register_package(
        &config,
        RegisterPackageRequest {
            package_id: "pkg-v1".to_string(),
            manifest_sha256: "m1".to_string(),
            model_sha256: "w1".to_string(),
            execution_provider: "CUDAExecutionProvider".to_string(),
            version: "1.0.0".to_string(),
            is_pinned: 1,
        },
    )
    .unwrap();

    let dep = deploy_package(
        &config,
        DeployPackageRequest {
            node_id: node_id.clone(),
            package_id: "pkg-v1".to_string(),
        },
    )
    .unwrap();

    // Submit receipt with valid self-test (drift = 0.0012 <= 0.05)
    let receipt_resp = submit_deploy_receipt(
        &config,
        DeployReceiptRequest {
            deployment_id: dep.deployment_id.clone(),
            package_id: "pkg-v1".to_string(),
            status: "ready".to_string(),
            stage: Some("ready".to_string()),
            startup_proof: Some(json!({
                "cuda_initialized": true,
                "kernel_events": 141,
                "cpu_kernel_events": 12,
                "arena_allocated_bytes": 134217728
            })),
            self_test: Some(SelfTestResult {
                passed: true,
                drift: 0.0012,
            }),
            duration_ms: Some(1450),
            error: None,
        },
    )
    .expect("receipt should be accepted");

    assert!(receipt_resp.ok);

    // Verify deployment status updated to ready
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    let dep_row: (String, String, Option<String>, Option<f64>, Option<i32>) = conn
        .query_row(
            "SELECT status, stage, actual_version, self_test_drift, self_test_passed FROM agent_deployments WHERE deployment_id = ?",
            [&dep.deployment_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap();

    assert_eq!(dep_row.0, "ready");
    assert_eq!(dep_row.1, "ready");
    assert_eq!(dep_row.2, Some("1.0.0".to_string()));
    assert_eq!(dep_row.3, Some(0.0012));
    assert_eq!(dep_row.4, Some(1));

    // Verify node status transitioned to accepting and model_ready = 1
    let node_row: (i32, String, String) = conn
        .query_row(
            "SELECT model_ready, current_version, status FROM agent_nodes WHERE id = ?",
            [&node_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();

    assert_eq!(node_row.0, 1);
    assert_eq!(node_row.1, "1.0.0");
    assert_eq!(node_row.2, "accepting");
}

#[test]
fn test_deploy_receipt_self_test_drift_exceeded_gating() {
    let (config, _dir) = temp_fixture_config();
    let node_id = register_test_node(&config, "wsl-node", "linux", "x86_64");

    register_package(
        &config,
        RegisterPackageRequest {
            package_id: "pkg-v1".to_string(),
            manifest_sha256: "m1".to_string(),
            model_sha256: "w1".to_string(),
            execution_provider: "CUDAExecutionProvider".to_string(),
            version: "1.0.0".to_string(),
            is_pinned: 1,
        },
    )
    .unwrap();

    let dep = deploy_package(
        &config,
        DeployPackageRequest {
            node_id: node_id.clone(),
            package_id: "pkg-v1".to_string(),
        },
    )
    .unwrap();

    // Submit receipt with drift > 0.05 (0.082)
    let err = submit_deploy_receipt(
        &config,
        DeployReceiptRequest {
            deployment_id: dep.deployment_id.clone(),
            package_id: "pkg-v1".to_string(),
            status: "ready".to_string(),
            stage: Some("ready".to_string()),
            startup_proof: Some(json!({"cuda": true})),
            self_test: Some(SelfTestResult {
                passed: true,
                drift: 0.082,
            }),
            duration_ms: Some(1200),
            error: None,
        },
    )
    .unwrap_err();

    assert_eq!(err.status, 422);
    assert!(err.error.contains("drift > 0.05"));

    // Verify deployment status is failed
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    let dep_row: (String, String, Option<f64>, Option<String>) = conn
        .query_row(
            "SELECT status, stage, self_test_drift, error FROM agent_deployments WHERE deployment_id = ?",
            [&dep.deployment_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();

    assert_eq!(dep_row.0, "failed");
    assert_eq!(dep_row.1, "self_test");
    assert_eq!(dep_row.2, Some(0.082));
    assert_eq!(dep_row.3, Some("self_test_drift_exceeded".to_string()));

    // Verify node is NOT accepting
    let node_status: String = conn
        .query_row(
            "SELECT status FROM agent_nodes WHERE id = ?",
            [&node_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_ne!(node_status, "accepting");
}

#[test]
fn test_deploy_receipt_verify_failure_tampered_model() {
    let (config, _dir) = temp_fixture_config();
    let node_id = register_test_node(&config, "wsl-node", "linux", "x86_64");

    register_package(
        &config,
        RegisterPackageRequest {
            package_id: "pkg-tampered".to_string(),
            manifest_sha256: "m".to_string(),
            model_sha256: "w".to_string(),
            execution_provider: "CUDAExecutionProvider".to_string(),
            version: "1.0.0".to_string(),
            is_pinned: 1,
        },
    )
    .unwrap();

    let dep = deploy_package(
        &config,
        DeployPackageRequest {
            node_id: node_id.clone(),
            package_id: "pkg-tampered".to_string(),
        },
    )
    .unwrap();

    let receipt_resp = submit_deploy_receipt(
        &config,
        DeployReceiptRequest {
            deployment_id: dep.deployment_id.clone(),
            package_id: "pkg-tampered".to_string(),
            status: "failed".to_string(),
            stage: Some("verify".to_string()),
            startup_proof: None,
            self_test: None,
            duration_ms: Some(150),
            error: Some("sha256_mismatch".to_string()),
        },
    )
    .expect("failure receipt is acknowledged");

    assert!(receipt_resp.ok);
    assert_eq!(receipt_resp.marked_failed, Some(true));

    let conn = processing_scheduler::open_or_init(&config).unwrap();
    let dep_row: (String, String, Option<String>) = conn
        .query_row(
            "SELECT status, stage, error FROM agent_deployments WHERE deployment_id = ?",
            [&dep.deployment_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();

    assert_eq!(dep_row.0, "failed");
    assert_eq!(dep_row.1, "verify");
    assert_eq!(dep_row.2, Some("sha256_mismatch".to_string()));
}

#[test]
fn test_deployments_list_and_tri_view_metadata() {
    let (config, _dir) = temp_fixture_config();
    let n1 = register_test_node(&config, "wsl-node-1", "linux", "x86_64");
    let n2 = register_test_node(&config, "mac-node-2", "darwin", "arm64");

    register_package(
        &config,
        RegisterPackageRequest {
            package_id: "pkg-cuda".to_string(),
            manifest_sha256: "m1".to_string(),
            model_sha256: "w1".to_string(),
            execution_provider: "CUDAExecutionProvider".to_string(),
            version: "1.0.0".to_string(),
            is_pinned: 1,
        },
    )
    .unwrap();

    register_package(
        &config,
        RegisterPackageRequest {
            package_id: "pkg-cpu".to_string(),
            manifest_sha256: "m2".to_string(),
            model_sha256: "w2".to_string(),
            execution_provider: "CPUExecutionProvider".to_string(),
            version: "1.0.0".to_string(),
            is_pinned: 1,
        },
    )
    .unwrap();

    let d1 = deploy_package(&config, DeployPackageRequest { node_id: n1, package_id: "pkg-cuda".to_string() }).unwrap();
    let _d2 = deploy_package(&config, DeployPackageRequest { node_id: n2, package_id: "pkg-cpu".to_string() }).unwrap();

    // Transition d1 to ready
    submit_deploy_receipt(
        &config,
        DeployReceiptRequest {
            deployment_id: d1.deployment_id,
            package_id: "pkg-cuda".to_string(),
            status: "ready".to_string(),
            stage: Some("ready".to_string()),
            startup_proof: Some(json!({"kernel_events": 141})),
            self_test: Some(SelfTestResult { passed: true, drift: 0.0005 }),
            duration_ms: Some(890),
            error: None,
        },
    )
    .unwrap();

    let list_val = list_deployments(&config).unwrap();
    let deployments = list_val["deployments"].as_array().unwrap();
    assert_eq!(deployments.len(), 2);

    let dep_ready = deployments.iter().find(|d| d["package_id"] == "pkg-cuda").unwrap();
    assert_eq!(dep_ready["status"], "ready");
    assert_eq!(dep_ready["stage"], "ready");
    assert_eq!(dep_ready["desired_version"], "1.0.0");
    assert_eq!(dep_ready["actual_version"], "1.0.0");
    assert_eq!(dep_ready["self_test_drift"], 0.0005);
    assert_eq!(dep_ready["startup_proof"]["kernel_events"], 141);

    let dep_pending = deployments.iter().find(|d| d["package_id"] == "pkg-cpu").unwrap();
    assert_eq!(dep_pending["status"], "downloading");
    assert_eq!(dep_pending["stage"], "download");
    assert!(dep_pending["actual_version"].is_null());
}

#[test]
fn test_safe_rollback_reverts_to_stable_release() {
    let (config, _dir) = temp_fixture_config();
    let node_id = register_test_node(&config, "wsl-rollback", "linux", "x86_64");

    register_package(
        &config,
        RegisterPackageRequest {
            package_id: "pkg-v1".to_string(),
            manifest_sha256: "m1".to_string(),
            model_sha256: "w1".to_string(),
            execution_provider: "CUDAExecutionProvider".to_string(),
            version: "1.0.0".to_string(),
            is_pinned: 1,
        },
    )
    .unwrap();

    register_package(
        &config,
        RegisterPackageRequest {
            package_id: "pkg-v2".to_string(),
            manifest_sha256: "m2".to_string(),
            model_sha256: "w2".to_string(),
            execution_provider: "CUDAExecutionProvider".to_string(),
            version: "2.0.0".to_string(),
            is_pinned: 1,
        },
    )
    .unwrap();

    // 1. Deploy v1 and ready
    let d1 = deploy_package(&config, DeployPackageRequest { node_id: node_id.clone(), package_id: "pkg-v1".to_string() }).unwrap();
    submit_deploy_receipt(&config, DeployReceiptRequest {
        deployment_id: d1.deployment_id,
        package_id: "pkg-v1".to_string(),
        status: "ready".to_string(),
        stage: Some("ready".to_string()),
        startup_proof: Some(json!({"cuda": true})),
        self_test: Some(SelfTestResult { passed: true, drift: 0.001 }),
        duration_ms: Some(1000),
        error: None,
    }).unwrap();

    // 2. Deploy v2 and fail
    let d2 = deploy_package(&config, DeployPackageRequest { node_id: node_id.clone(), package_id: "pkg-v2".to_string() }).unwrap();
    submit_deploy_receipt(&config, DeployReceiptRequest {
        deployment_id: d2.deployment_id,
        package_id: "pkg-v2".to_string(),
        status: "failed".to_string(),
        stage: Some("start".to_string()),
        startup_proof: None,
        self_test: None,
        duration_ms: Some(200),
        error: Some("cuda_init_failed".to_string()),
    }).unwrap();

    // 3. Rollback to v1
    let rb_resp = rollback_deployment(&config, RollbackRequest {
        node_id: node_id.clone(),
        target_version: Some("1.0.0".to_string()),
    }).expect("rollback should succeed");

    assert_eq!(rb_resp.package_id, "pkg-v1");

    // Verify rollback audit event
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    let ev: (f64, String) = conn
        .query_row(
            "SELECT at, message FROM events WHERE kind = 'rollback_triggered' ORDER BY at DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();

    assert!(ev.1.contains(&node_id));
}

#[test]
fn test_drift_tolerance_nan_infinity_and_negative_rejected() {
    let (config, _dir) = temp_fixture_config();
    let node_id = register_test_node(&config, "wsl-nan-test", "linux", "x86_64");

    register_package(
        &config,
        RegisterPackageRequest {
            package_id: "pkg-drift-nan".to_string(),
            manifest_sha256: "manifest-nan".to_string(),
            model_sha256: "model-nan".to_string(),
            execution_provider: "CUDAExecutionProvider".to_string(),
            version: "1.0.0".to_string(),
            is_pinned: 1,
        },
    )
    .unwrap();

    // 1. Test NaN drift
    let dep_nan = deploy_package(
        &config,
        DeployPackageRequest {
            node_id: node_id.clone(),
            package_id: "pkg-drift-nan".to_string(),
        },
    )
    .unwrap();

    let err_nan = submit_deploy_receipt(
        &config,
        DeployReceiptRequest {
            deployment_id: dep_nan.deployment_id.clone(),
            package_id: "pkg-drift-nan".to_string(),
            status: "ready".to_string(),
            stage: Some("ready".to_string()),
            startup_proof: Some(json!({"kernel_events": 141})),
            self_test: Some(SelfTestResult {
                passed: true,
                drift: f64::NAN,
            }),
            duration_ms: Some(1000),
            error: None,
        },
    )
    .unwrap_err();

    assert_eq!(err_nan.status, 422, "NaN drift must be rejected with HTTP 422");
    assert_eq!(err_nan.code, "self_test_drift_exceeded");

    // Verify deployment is marked failed in DB and self_test_drift is NULL
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    let (status_nan, drift_in_db): (String, Option<f64>) = conn
        .query_row(
            "SELECT status, self_test_drift FROM agent_deployments WHERE deployment_id = ?",
            [&dep_nan.deployment_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(status_nan, "failed");
    assert!(drift_in_db.is_none(), "NaN drift should be recorded as NULL");

    // Verify node is NOT marked model_ready
    let model_ready: i32 = conn
        .query_row("SELECT model_ready FROM agent_nodes WHERE id = ?", [&node_id], |r| r.get(0))
        .unwrap();
    assert_eq!(model_ready, 0, "Node must not be model_ready after NaN rejection");

    // 2. Test negative drift (-0.5)
    let dep_neg = deploy_package(
        &config,
        DeployPackageRequest {
            node_id: node_id.clone(),
            package_id: "pkg-drift-nan".to_string(),
        },
    )
    .unwrap();

    let err_neg = submit_deploy_receipt(
        &config,
        DeployReceiptRequest {
            deployment_id: dep_neg.deployment_id.clone(),
            package_id: "pkg-drift-nan".to_string(),
            status: "ready".to_string(),
            stage: Some("ready".to_string()),
            startup_proof: Some(json!({"kernel_events": 141})),
            self_test: Some(SelfTestResult {
                passed: true,
                drift: -0.5,
            }),
            duration_ms: Some(1000),
            error: None,
        },
    )
    .unwrap_err();

    assert_eq!(err_neg.status, 422, "Negative drift must be rejected with HTTP 422");
    assert_eq!(err_neg.code, "self_test_drift_exceeded");

    // 3. Test -infinity drift
    let dep_ninf = deploy_package(
        &config,
        DeployPackageRequest {
            node_id: node_id.clone(),
            package_id: "pkg-drift-nan".to_string(),
        },
    )
    .unwrap();

    let err_ninf = submit_deploy_receipt(
        &config,
        DeployReceiptRequest {
            deployment_id: dep_ninf.deployment_id.clone(),
            package_id: "pkg-drift-nan".to_string(),
            status: "ready".to_string(),
            stage: Some("ready".to_string()),
            startup_proof: Some(json!({"kernel_events": 141})),
            self_test: Some(SelfTestResult {
                passed: true,
                drift: f64::NEG_INFINITY,
            }),
            duration_ms: Some(1000),
            error: None,
        },
    )
    .unwrap_err();

    assert_eq!(err_ninf.status, 422, "-Infinity drift must be rejected with HTTP 422");
}

#[test]
fn test_stale_receipt_fencing_prevents_version_regression() {
    let (config, _dir) = temp_fixture_config();
    let node_id = register_test_node(&config, "wsl-fencing-test", "linux", "x86_64");

    for ver in ["1.0.0", "2.0.0"] {
        register_package(
            &config,
            RegisterPackageRequest {
                package_id: format!("pkg-v{}", ver),
                manifest_sha256: format!("manifest-{}", ver),
                model_sha256: format!("model-{}", ver),
                execution_provider: "CUDAExecutionProvider".to_string(),
                version: ver.to_string(),
                is_pinned: 1,
            },
        )
        .unwrap();
    }

    // 1. Dispatch v1
    let dep1 = deploy_package(
        &config,
        DeployPackageRequest {
            node_id: node_id.clone(),
            package_id: "pkg-v1.0.0".to_string(),
        },
    )
    .unwrap();

    // 2. Dispatch v2 (desired_version becomes 2.0.0)
    let dep2 = deploy_package(
        &config,
        DeployPackageRequest {
            node_id: node_id.clone(),
            package_id: "pkg-v2.0.0".to_string(),
        },
    )
    .unwrap();

    // 3. v2 succeeds first
    let resp2 = submit_deploy_receipt(
        &config,
        DeployReceiptRequest {
            deployment_id: dep2.deployment_id.clone(),
            package_id: "pkg-v2.0.0".to_string(),
            status: "ready".to_string(),
            stage: Some("ready".to_string()),
            startup_proof: Some(json!({"kernel_events": 141})),
            self_test: Some(SelfTestResult { passed: true, drift: 0.002 }),
            duration_ms: Some(900),
            error: None,
        },
    )
    .unwrap();
    assert!(resp2.ok);

    let conn = processing_scheduler::open_or_init(&config).unwrap();
    let current_ver: String = conn
        .query_row("SELECT current_version FROM agent_nodes WHERE id = ?", [&node_id], |r| r.get(0))
        .unwrap();
    assert_eq!(current_ver, "2.0.0");

    // 4. Delayed v1 receipt arrives
    let resp1 = submit_deploy_receipt(
        &config,
        DeployReceiptRequest {
            deployment_id: dep1.deployment_id.clone(),
            package_id: "pkg-v1.0.0".to_string(),
            status: "ready".to_string(),
            stage: Some("ready".to_string()),
            startup_proof: Some(json!({"kernel_events": 141})),
            self_test: Some(SelfTestResult { passed: true, drift: 0.001 }),
            duration_ms: Some(1500),
            error: None,
        },
    )
    .unwrap();
    assert!(resp1.ok);

    // CRITICAL ASSERTION: agent_nodes.current_version MUST NOT regress to 1.0.0!
    let current_ver_after_stale: String = conn
        .query_row("SELECT current_version FROM agent_nodes WHERE id = ?", [&node_id], |r| r.get(0))
        .unwrap();
    assert_eq!(
        current_ver_after_stale, "2.0.0",
        "Stale receipt must NOT regress agent_nodes.current_version to 1.0.0"
    );

    // Verify audit event was logged
    let stale_events: i64 = conn
        .query_row(
            "SELECT count(*) FROM events WHERE kind = 'deployment_stale_receipt_ignored'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stale_events, 1, "Expected stale receipt audit event");
}

#[test]
fn test_list_deployments_populates_gpu_model() {
    let (config, _dir) = temp_fixture_config();
    let gpu_node_id = register_test_node(&config, "wsl-gpu-node", "linux", "x86_64");

    register_package(
        &config,
        RegisterPackageRequest {
            package_id: "pkg-list-test".to_string(),
            manifest_sha256: "manifest-test".to_string(),
            model_sha256: "model-test".to_string(),
            execution_provider: "CUDAExecutionProvider".to_string(),
            version: "1.0.0".to_string(),
            is_pinned: 1,
        },
    )
    .unwrap();

    let _dep = deploy_package(
        &config,
        DeployPackageRequest {
            node_id: gpu_node_id.clone(),
            package_id: "pkg-list-test".to_string(),
        },
    )
    .unwrap();

    let list_val = list_deployments(&config).unwrap();
    let deps = list_val["deployments"].as_array().expect("deployments array");
    let match_dep = deps
        .iter()
        .find(|d| d["node_id"] == gpu_node_id)
        .expect("should find deployed item");

    assert_eq!(
        match_dep["gpu_model"],
        "NVIDIA GeForce RTX 4070 Ti",
        "gpu_model must be projected in list_deployments"
    );
}
