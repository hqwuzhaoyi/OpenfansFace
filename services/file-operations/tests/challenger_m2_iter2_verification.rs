use face_file_operations::agent_deployment::{
    deploy_package, list_deployments, register_package, submit_deploy_receipt,
    DeployPackageRequest, DeployReceiptRequest, RegisterPackageRequest, SelfTestResult,
    DRIFT_TOLERANCE,
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

/// Creates an isolated temporary directory and ledger for testing.
/// Strictly isolated from production media libraries and real DBs.
fn temp_isolated_config() -> (Config, PathBuf) {
    let tmp_dir = std::env::temp_dir().join(format!("face-challenger-m2-iter2-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&tmp_dir).expect("create temp dir");
    let media = tmp_dir.join("media");
    fs::create_dir_all(&media).expect("create temp media dir");
    let ledger = tmp_dir.join("ledger.db");

    let config = Config {
        base: tmp_dir.clone(),
        media,
        ledger,
        bind: "127.0.0.1:0".to_string(),
        token: Some("challenger-iter2-token".to_string()),
        fixture: None,
        parity_root: None,
    };

    (config, tmp_dir)
}

/// Helper to register a test node with custom GPU specs
fn register_custom_node(
    config: &Config,
    hostname: &str,
    os: &str,
    arch: &str,
    gpu: Option<GpuSpecs>,
) -> String {
    let token_resp = create_enrollment_token(
        config,
        EnrollmentTokenRequest {
            note: Some("Challenger node".to_string()),
            ttl_seconds: Some(3600.0),
        },
    )
    .expect("enrollment token");

    let reg_req = RegisterRequest {
        enrollment_token: token_resp.token,
        hostname: hostname.to_string(),
        os: os.to_string(),
        arch: arch.to_string(),
        gpu,
        ram_bytes: 34359738368,
        cpu_cores: 16,
        agent_version: "0.2.0".to_string(),
        max_concurrent_tasks: None,
    };

    let reg_resp = register_node(config, reg_req).expect("register node");
    reg_resp.node_id
}

/// Helper to register an immutable package manifest
fn register_package_manifest(config: &Config, pkg_id: &str, version: &str, provider: &str) {
    register_package(
        config,
        RegisterPackageRequest {
            package_id: pkg_id.to_string(),
            manifest_sha256: format!("sha256-manifest-{}", pkg_id),
            model_sha256: format!("sha256-model-{}", pkg_id),
            execution_provider: provider.to_string(),
            version: version.to_string(),
            is_pinned: 1,
        },
    )
    .expect("register package");
}

// =========================================================================
// OBJECTIVE 1: IEEE 754 FLOAT DRIFT EDGE CASES STRESS TEST
// =========================================================================

#[test]
fn challenge_ieee754_drift_rejections_and_db_safety() {
    let (config, _dir) = temp_isolated_config();
    let node_id = register_custom_node(
        &config,
        "wsl-edge-cases",
        "linux",
        "x86_64",
        Some(GpuSpecs {
            model: "NVIDIA GeForce RTX 4070 Ti".to_string(),
            vram_bytes: 12884901888,
            device_uuid: "GPU-uuid-edge".to_string(),
        }),
    );
    register_package_manifest(&config, "pkg-edge", "1.0.0", "CUDAExecutionProvider");

    // Subnormal positive float (smallest non-zero f64: ~4.94e-324)
    let subnormal_pos = f64::from_bits(1);
    // Subnormal negative float (-~4.94e-324)
    let subnormal_neg = -f64::from_bits(1);

    // List of adversarial drift values that MUST be rejected with HTTP 422
    let rejected_test_cases: Vec<(&str, f64)> = vec![
        ("NaN", f64::NAN),
        ("+Infinity", f64::INFINITY),
        ("-Infinity", f64::NEG_INFINITY),
        ("Negative tiny: -0.0001", -0.0001),
        ("Negative epsilon: -1e-15", -1e-15),
        ("Negative subnormal", subnormal_neg),
        ("Negative huge: -1e30", -1e30),
        ("Negative f64::MIN", f64::MIN),
        ("Boundary exceeding: 0.050001", 0.050001),
        ("Boundary fine excess: 0.0500000000000001", 0.0500000000000001),
        ("Huge positive: 1e30", 1e30),
        ("Positive f64::MAX", f64::MAX),
    ];

    for (label, drift_val) in rejected_test_cases {
        let dep = deploy_package(
            &config,
            DeployPackageRequest {
                node_id: node_id.clone(),
                package_id: "pkg-edge".to_string(),
            },
        )
        .expect("deploy package dispatch");

        let err = submit_deploy_receipt(
            &config,
            DeployReceiptRequest {
                deployment_id: dep.deployment_id.clone(),
                package_id: "pkg-edge".to_string(),
                status: "ready".to_string(),
                stage: Some("ready".to_string()),
                startup_proof: Some(json!({"kernel_events": 141})),
                self_test: Some(SelfTestResult {
                    passed: true,
                    drift: drift_val,
                }),
                duration_ms: Some(1000),
                error: None,
            },
        )
        .expect_err(&format!("Drift case '{}' must be rejected", label));

        assert_eq!(err.status, 422, "Case '{}' status should be 422", label);
        assert_eq!(
            err.code, "self_test_drift_exceeded",
            "Case '{}' code should be self_test_drift_exceeded",
            label
        );

        // Verify SQLite state: deployment marked failed, stage='self_test'
        let conn = processing_scheduler::open_or_init(&config).unwrap();
        let (dep_status, dep_stage, dep_drift, dep_err): (String, String, Option<f64>, Option<String>) = conn
            .query_row(
                "SELECT status, stage, self_test_drift, error FROM agent_deployments WHERE deployment_id = ?",
                [&dep.deployment_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();

        assert_eq!(dep_status, "failed", "Case '{}' deployment status in DB", label);
        assert_eq!(dep_stage, "self_test", "Case '{}' stage in DB", label);
        assert_eq!(dep_err.as_deref(), Some("self_test_drift_exceeded"), "Case '{}' error in DB", label);

        // CRITICAL: Non-finite floats (NaN, +Inf, -Inf) MUST be stored as NULL in SQLite!
        if !drift_val.is_finite() {
            assert!(
                dep_drift.is_none(),
                "Case '{}' non-finite drift must be stored as NULL in SQLite",
                label
            );
        } else {
            // Finite numbers are safely recorded
            assert!(dep_drift.is_some(), "Case '{}' finite drift should be stored in SQLite", label);
        }

        // Verify node is NOT marked model_ready
        let (model_ready, node_status): (i32, String) = conn
            .query_row(
                "SELECT model_ready, status FROM agent_nodes WHERE id = ?",
                [&node_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(model_ready, 0, "Node must not be model_ready after case '{}'", label);
        assert_ne!(node_status, "accepting", "Node must not be accepting after case '{}'", label);
    }

    // Now verify ACCEPTED cases:
    // 1. Exact boundary: 0.0500000000000000
    // 2. Positive subnormal: subnormal_pos (~4.94e-324, which is finite, > 0.0, and <= 0.05)
    // 3. Perfect zero: 0.0
    // 4. IEEE negative zero: -0.0 (in IEEE 754: -0.0 == 0.0)
    let accepted_test_cases: Vec<(&str, f64)> = vec![
        ("Exact boundary 0.05", DRIFT_TOLERANCE),
        ("Positive subnormal", subnormal_pos),
        ("Zero drift: 0.0", 0.0),
        ("IEEE negative zero: -0.0", -0.0),
        ("Typical good drift: 0.0012", 0.0012),
    ];

    for (label, drift_val) in accepted_test_cases {
        let dep = deploy_package(
            &config,
            DeployPackageRequest {
                node_id: node_id.clone(),
                package_id: "pkg-edge".to_string(),
            },
        )
        .expect("deploy package dispatch");

        let resp = submit_deploy_receipt(
            &config,
            DeployReceiptRequest {
                deployment_id: dep.deployment_id.clone(),
                package_id: "pkg-edge".to_string(),
                status: "ready".to_string(),
                stage: Some("ready".to_string()),
                startup_proof: Some(json!({"kernel_events": 141})),
                self_test: Some(SelfTestResult {
                    passed: true,
                    drift: drift_val,
                }),
                duration_ms: Some(850),
                error: None,
            },
        )
        .expect(&format!("Valid drift case '{}' should be accepted", label));

        assert!(resp.ok, "Valid drift case '{}' should return ok=true", label);

        let conn = processing_scheduler::open_or_init(&config).unwrap();
        let (dep_status, dep_drift): (String, Option<f64>) = conn
            .query_row(
                "SELECT status, self_test_drift FROM agent_deployments WHERE deployment_id = ?",
                [&dep.deployment_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();

        assert_eq!(dep_status, "ready", "Case '{}' status in DB", label);
        assert!(dep_drift.is_some(), "Case '{}' drift recorded in DB", label);

        // Reset node readiness for next iteration
        conn.execute(
            "UPDATE agent_nodes SET model_ready = 0, status = 'registered' WHERE id = ?",
            [&node_id],
        )
        .unwrap();
    }

    // Verify SQLite integrity after all edge-case tests
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    let integrity: String = conn
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .unwrap();
    assert_eq!(integrity, "ok", "SQLite database must pass integrity check");
}

// =========================================================================
// OBJECTIVE 2: JSON DESERIALIZATION AND PAYLOAD FUZZING STRESS TEST
// =========================================================================

#[test]
fn challenge_malformed_json_and_payload_fuzzing() {
    let (config, _dir) = temp_isolated_config();
    let node_id = register_custom_node(
        &config,
        "wsl-json-fuzz",
        "linux",
        "x86_64",
        Some(GpuSpecs {
            model: "NVIDIA GeForce RTX 4070 Ti".to_string(),
            vram_bytes: 12884901888,
            device_uuid: "GPU-uuid-fuzz".to_string(),
        }),
    );
    register_package_manifest(&config, "pkg-fuzz", "1.0.0", "CUDAExecutionProvider");

    let dep = deploy_package(
        &config,
        DeployPackageRequest {
            node_id: node_id.clone(),
            package_id: "pkg-fuzz".to_string(),
        },
    )
    .unwrap();

    // 1. JSON with string "not-a-number" for drift -> serde deserialization error
    let raw_invalid_drift_str = format!(
        r#"{{"deployment_id": "{}", "status": "ready", "self_test": {{"passed": true, "drift": "NaN"}}}}"#,
        dep.deployment_id
    );
    let parse_err = serde_json::from_str::<DeployReceiptRequest>(&raw_invalid_drift_str);
    assert!(parse_err.is_err(), "String 'NaN' must fail standard f64 JSON parsing");

    // 2. JSON with null for drift -> serde deserialization error
    let raw_null_drift = format!(
        r#"{{"deployment_id": "{}", "status": "ready", "self_test": {{"passed": true, "drift": null}}}}"#,
        dep.deployment_id
    );
    let parse_null_err = serde_json::from_str::<DeployReceiptRequest>(&raw_null_drift);
    assert!(parse_null_err.is_err(), "null drift must fail f64 JSON deserialization");

    // 3. JSON with missing drift field -> defaults to 1.0 -> rejected by tolerance gate (drift > 0.05)
    let raw_omitted_drift = format!(
        r#"{{"deployment_id": "{}", "status": "ready", "self_test": {{"passed": true}}}}"#,
        dep.deployment_id
    );
    let parsed_omitted: DeployReceiptRequest = serde_json::from_str(&raw_omitted_drift).expect("deserialization with default drift");
    assert_eq!(parsed_omitted.self_test.as_ref().unwrap().drift, 1.0, "Omitted drift should default to 1.0");

    let err_omitted = submit_deploy_receipt(&config, parsed_omitted).unwrap_err();
    assert_eq!(err_omitted.status, 422, "Omitted drift (default 1.0) must be rejected with 422");

    // 4. JSON with syntax error
    let raw_broken_json = format!(
        r#"{{"deployment_id": "{}", "status": "ready", "self_test": {{broken"#,
        dep.deployment_id
    );
    assert!(serde_json::from_str::<DeployReceiptRequest>(&raw_broken_json).is_err());

    // 5. Empty deployment_id -> rejected with 400
    let empty_dep_req = DeployReceiptRequest {
        deployment_id: "   ".to_string(),
        package_id: "pkg-fuzz".to_string(),
        status: "ready".to_string(),
        stage: None,
        startup_proof: None,
        self_test: None,
        duration_ms: None,
        error: None,
    };
    let err_empty = submit_deploy_receipt(&config, empty_dep_req).unwrap_err();
    assert_eq!(err_empty.status, 400);

    // 6. Non-existent deployment_id -> rejected with 404
    let ghost_dep_req = DeployReceiptRequest {
        deployment_id: "dep-nonexistent-999999".to_string(),
        package_id: "pkg-fuzz".to_string(),
        status: "ready".to_string(),
        stage: None,
        startup_proof: None,
        self_test: None,
        duration_ms: None,
        error: None,
    };
    let err_ghost = submit_deploy_receipt(&config, ghost_dep_req).unwrap_err();
    assert_eq!(err_ghost.status, 404);

    // 7. Malformed startup_proof in JSON (e.g. nested array with 50 levels)
    let mut deeply_nested = json!("innermost");
    for _ in 0..30 {
        deeply_nested = json!([deeply_nested]);
    }
    let dep2 = deploy_package(
        &config,
        DeployPackageRequest {
            node_id: node_id.clone(),
            package_id: "pkg-fuzz".to_string(),
        },
    )
    .unwrap();

    let resp_nested = submit_deploy_receipt(
        &config,
        DeployReceiptRequest {
            deployment_id: dep2.deployment_id.clone(),
            package_id: "pkg-fuzz".to_string(),
            status: "ready".to_string(),
            stage: Some("ready".to_string()),
            startup_proof: Some(deeply_nested),
            self_test: Some(SelfTestResult { passed: true, drift: 0.01 }),
            duration_ms: Some(100),
            error: None,
        },
    )
    .expect("deeply nested startup_proof should be handled without panic");
    assert!(resp_nested.ok);

    // Verify DB integrity
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    let integrity: String = conn
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .unwrap();
    assert_eq!(integrity, "ok");
}

// =========================================================================
// OBJECTIVE 3: CONCURRENCY & VERSION FENCING STRESS TEST
// =========================================================================

#[test]
fn challenge_concurrency_and_version_fencing_interleaved_receipts() {
    let (config, _dir) = temp_isolated_config();
    let node_id = register_custom_node(
        &config,
        "wsl-interleave-node",
        "linux",
        "x86_64",
        Some(GpuSpecs {
            model: "NVIDIA GeForce RTX 4070 Ti".to_string(),
            vram_bytes: 12884901888,
            device_uuid: "GPU-uuid-interleave".to_string(),
        }),
    );

    // Register versions 1.0.0, 2.0.0, and 3.0.0
    for ver in ["1.0.0", "2.0.0", "3.0.0"] {
        register_package_manifest(&config, &format!("pkg-v{}", ver), ver, "CUDAExecutionProvider");
    }

    // Step 1: Dispatch deployment v1.0.0
    let dep_v1 = deploy_package(
        &config,
        DeployPackageRequest {
            node_id: node_id.clone(),
            package_id: "pkg-v1.0.0".to_string(),
        },
    )
    .unwrap();

    // Verify desired_version is 1.0.0
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    let node_desired: String = conn
        .query_row("SELECT desired_version FROM agent_nodes WHERE id = ?", [&node_id], |r| r.get(0))
        .unwrap();
    assert_eq!(node_desired, "1.0.0");

    // Step 2: Before v1 completes, Center dispatches deployment v2.0.0 (upgrade initiated)
    let dep_v2 = deploy_package(
        &config,
        DeployPackageRequest {
            node_id: node_id.clone(),
            package_id: "pkg-v2.0.0".to_string(),
        },
    )
    .unwrap();

    // Verify desired_version is now bumped to 2.0.0
    let node_desired: String = conn
        .query_row("SELECT desired_version FROM agent_nodes WHERE id = ?", [&node_id], |r| r.get(0))
        .unwrap();
    assert_eq!(node_desired, "2.0.0");

    // Step 3: Deployment v2.0.0 completes successfully first
    let resp_v2 = submit_deploy_receipt(
        &config,
        DeployReceiptRequest {
            deployment_id: dep_v2.deployment_id.clone(),
            package_id: "pkg-v2.0.0".to_string(),
            status: "ready".to_string(),
            stage: Some("ready".to_string()),
            startup_proof: Some(json!({"kernel_events": 141})),
            self_test: Some(SelfTestResult { passed: true, drift: 0.008 }),
            duration_ms: Some(1100),
            error: None,
        },
    )
    .expect("v2 receipt submission");
    assert!(resp_v2.ok);

    // Verify node is now current_version = 2.0.0 and model_ready = 1
    let (cur_ver, model_ready, status): (String, i32, String) = conn
        .query_row(
            "SELECT current_version, model_ready, status FROM agent_nodes WHERE id = ?",
            [&node_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(cur_ver, "2.0.0", "Node current_version must be 2.0.0");
    assert_eq!(model_ready, 1, "Node model_ready must be 1");
    assert_eq!(status, "accepting", "Node status must be accepting");

    // Step 4: Now, delayed v1 receipt arrives reporting "ready"
    let resp_v1_stale = submit_deploy_receipt(
        &config,
        DeployReceiptRequest {
            deployment_id: dep_v1.deployment_id.clone(),
            package_id: "pkg-v1.0.0".to_string(),
            status: "ready".to_string(),
            stage: Some("ready".to_string()),
            startup_proof: Some(json!({"kernel_events": 141})),
            self_test: Some(SelfTestResult { passed: true, drift: 0.005 }),
            duration_ms: Some(3500),
            error: None,
        },
    )
    .expect("v1 stale receipt acknowledged");
    assert!(resp_v1_stale.ok);

    // CRITICAL ASSERTION:
    // v1 deployment itself should record ready, BUT node current_version must REMAIN 2.0.0!
    let (cur_ver_after_v1, model_ready_after_v1): (String, i32) = conn
        .query_row(
            "SELECT current_version, model_ready FROM agent_nodes WHERE id = ?",
            [&node_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        cur_ver_after_v1, "2.0.0",
        "Delayed v1 receipt MUST NOT regress agent_nodes.current_version from 2.0.0 to 1.0.0!"
    );
    assert_eq!(
        model_ready_after_v1, 1,
        "Delayed v1 receipt must not alter model_ready=1 of current v2 node"
    );

    // Verify audit event 'deployment_stale_receipt_ignored' was logged
    let stale_event_count: i64 = conn
        .query_row(
            "SELECT count(*) FROM events WHERE kind = 'deployment_stale_receipt_ignored'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(stale_event_count >= 1, "Must log deployment_stale_receipt_ignored audit event");

    // Step 5: Delayed v1 duplicate arrives reporting FAILURE (e.g. worker timed out or cleaned up v1)
    let resp_v1_fail = submit_deploy_receipt(
        &config,
        DeployReceiptRequest {
            deployment_id: dep_v1.deployment_id.clone(),
            package_id: "pkg-v1.0.0".to_string(),
            status: "failed".to_string(),
            stage: Some("execute".to_string()),
            startup_proof: None,
            self_test: None,
            duration_ms: Some(500),
            error: Some("delayed_oom_or_process_killed".to_string()),
        },
    )
    .expect("v1 failure receipt acknowledged");
    assert!(resp_v1_fail.ok);

    // CRITICAL ASSERTION:
    // Failure of superseded deployment v1 MUST NEVER knock down a healthy v2 node's model_ready!
    let (cur_ver_after_v1_fail, model_ready_after_v1_fail, status_after_v1_fail): (String, i32, String) = conn
        .query_row(
            "SELECT current_version, model_ready, status FROM agent_nodes WHERE id = ?",
            [&node_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        cur_ver_after_v1_fail, "2.0.0",
        "Superseded v1 failure must not modify current_version"
    );
    assert_eq!(
        model_ready_after_v1_fail, 1,
        "Superseded v1 failure MUST NOT knock down model_ready=1 of healthy v2 node!"
    );
    assert_eq!(
        status_after_v1_fail, "accepting",
        "Superseded v1 failure must not mark healthy v2 node unaccepting"
    );
}

#[test]
fn challenge_v1_failure_while_v2_in_flight_preserves_target_intent() {
    let (config, _dir) = temp_isolated_config();
    let node_id = register_custom_node(
        &config,
        "wsl-inflight-node",
        "linux",
        "x86_64",
        Some(GpuSpecs {
            model: "NVIDIA GeForce RTX 4070 Ti".to_string(),
            vram_bytes: 12884901888,
            device_uuid: "GPU-uuid-inflight".to_string(),
        }),
    );

    register_package_manifest(&config, "pkg-v1", "1.0.0", "CUDAExecutionProvider");
    register_package_manifest(&config, "pkg-v2", "2.0.0", "CUDAExecutionProvider");

    // 1. Dispatch v1
    let dep1 = deploy_package(&config, DeployPackageRequest { node_id: node_id.clone(), package_id: "pkg-v1".to_string() }).unwrap();

    // 2. Dispatch v2 before v1 finishes
    let dep2 = deploy_package(&config, DeployPackageRequest { node_id: node_id.clone(), package_id: "pkg-v2".to_string() }).unwrap();

    // 3. v1 now fails while v2 is downloading
    let resp1 = submit_deploy_receipt(
        &config,
        DeployReceiptRequest {
            deployment_id: dep1.deployment_id,
            package_id: "pkg-v1".to_string(),
            status: "failed".to_string(),
            stage: Some("download".to_string()),
            startup_proof: None,
            self_test: None,
            duration_ms: Some(300),
            error: Some("connection_reset".to_string()),
        },
    )
    .unwrap();
    assert!(resp1.ok);

    // Verify that desired_version is still 2.0.0 (not reset or corrupted by v1 failure)
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    let desired: String = conn
        .query_row("SELECT desired_version FROM agent_nodes WHERE id = ?", [&node_id], |r| r.get(0))
        .unwrap();
    assert_eq!(desired, "2.0.0", "v1 failure must not reset desired_version 2.0.0");

    // 4. v2 now succeeds
    let resp2 = submit_deploy_receipt(
        &config,
        DeployReceiptRequest {
            deployment_id: dep2.deployment_id,
            package_id: "pkg-v2".to_string(),
            status: "ready".to_string(),
            stage: Some("ready".to_string()),
            startup_proof: Some(json!({"kernel_events": 141})),
            self_test: Some(SelfTestResult { passed: true, drift: 0.001 }),
            duration_ms: Some(1200),
            error: None,
        },
    )
    .unwrap();
    assert!(resp2.ok);

    // Verify node is successfully running v2
    let (cur_ver, model_ready): (String, i32) = conn
        .query_row(
            "SELECT current_version, model_ready FROM agent_nodes WHERE id = ?",
            [&node_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(cur_ver, "2.0.0");
    assert_eq!(model_ready, 1);
}

// =========================================================================
// OBJECTIVE 4: NULL GPU_MODEL & NON-STANDARD GPU STRINGS QUERY RESILIENCE
// =========================================================================

#[test]
fn challenge_list_deployments_null_and_nonstandard_gpu_strings() {
    let (config, _dir) = temp_isolated_config();

    // Node 1: CPU node (Mac ARM64 / VM) without GPU -> gpu_model is NULL in DB
    let cpu_node_id = register_custom_node(
        &config,
        "mac-arm64-cpu",
        "darwin",
        "aarch64",
        None, // gpu is None -> SQLite gpu_model is NULL
    );

    // Node 2: Node with empty string gpu_model
    let empty_gpu_node_id = register_custom_node(
        &config,
        "empty-gpu-node",
        "linux",
        "x86_64",
        Some(GpuSpecs {
            model: "".to_string(),
            vram_bytes: 0,
            device_uuid: "GPU-uuid-empty".to_string(),
        }),
    );

    // Node 3: Node with non-standard Unicode GPU model
    let unicode_gpu_node_id = register_custom_node(
        &config,
        "domestic-gpu-node",
        "linux",
        "x86_64",
        Some(GpuSpecs {
            model: "摩尔线程 MTT S80 (16GB GDDR6 / 4096 MUSA 核心)".to_string(),
            vram_bytes: 17179869184,
            device_uuid: "GPU-uuid-mtt".to_string(),
        }),
    );

    // Node 4: Node with SQL injection and special character strings in gpu_model
    let injection_gpu_str = "NVIDIA RTX 4090'; DROP TABLE agent_nodes; -- \"'\\//<>\0\n\t";
    let injection_gpu_node_id = register_custom_node(
        &config,
        "injection-gpu-node",
        "linux",
        "x86_64",
        Some(GpuSpecs {
            model: injection_gpu_str.to_string(),
            vram_bytes: 25769803776,
            device_uuid: "GPU-uuid-injection".to_string(),
        }),
    );

    // Register packages for CPU and GPU
    register_package_manifest(&config, "pkg-cpu", "1.0.0", "CPUExecutionProvider");
    register_package_manifest(&config, "pkg-cuda", "1.0.0", "CUDAExecutionProvider");

    // Deploy to each node
    let dep_cpu = deploy_package(&config, DeployPackageRequest { node_id: cpu_node_id.clone(), package_id: "pkg-cpu".to_string() }).unwrap();
    let dep_empty = deploy_package(&config, DeployPackageRequest { node_id: empty_gpu_node_id.clone(), package_id: "pkg-cuda".to_string() }).unwrap();
    let dep_uni = deploy_package(&config, DeployPackageRequest { node_id: unicode_gpu_node_id.clone(), package_id: "pkg-cuda".to_string() }).unwrap();
    let dep_inj = deploy_package(&config, DeployPackageRequest { node_id: injection_gpu_node_id.clone(), package_id: "pkg-cuda".to_string() }).unwrap();

    // Query list_deployments
    let list_val = list_deployments(&config).expect("list_deployments must succeed without panic");
    assert_eq!(list_val["total"].as_i64().unwrap(), 4, "Total deployments count must be 4");

    let deployments = list_val["deployments"].as_array().expect("deployments array");

    // Find and verify Node 1 (CPU / NULL gpu_model)
    let cpu_dep = deployments.iter().find(|d| d["deployment_id"] == dep_cpu.deployment_id).expect("find cpu deployment");
    assert!(
        cpu_dep["gpu_model"].is_null(),
        "CPU node must project genuine JSON null for gpu_model, never synthetic strings"
    );
    assert_eq!(cpu_dep["execution_provider"], "CPUExecutionProvider");

    // Find and verify Node 2 (empty string gpu_model)
    let empty_dep = deployments.iter().find(|d| d["deployment_id"] == dep_empty.deployment_id).expect("find empty gpu deployment");
    assert_eq!(empty_dep["gpu_model"], "");

    // Find and verify Node 3 (Unicode gpu_model)
    let uni_dep = deployments.iter().find(|d| d["deployment_id"] == dep_uni.deployment_id).expect("find unicode gpu deployment");
    assert_eq!(uni_dep["gpu_model"], "摩尔线程 MTT S80 (16GB GDDR6 / 4096 MUSA 核心)");

    // Find and verify Node 4 (Injection string gpu_model)
    let inj_dep = deployments.iter().find(|d| d["deployment_id"] == dep_inj.deployment_id).expect("find injection gpu deployment");
    assert_eq!(inj_dep["gpu_model"], injection_gpu_str);

    // Verify DB integrity is intact (no SQL injection executed)
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    let table_count: i64 = conn
        .query_row("SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'agent_nodes'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(table_count, 1, "agent_nodes table must exist and not be dropped");
}

// =========================================================================
// OBJECTIVE 5: MULTI-THREADED CONCURRENT RECEIPT BOMBARDMENT
// =========================================================================

#[test]
fn challenge_multithreaded_concurrent_receipt_bombardment() {
    let (config, _dir) = temp_isolated_config();
    let node_id = register_custom_node(
        &config,
        "wsl-concurrent-bombard",
        "linux",
        "x86_64",
        Some(GpuSpecs {
            model: "NVIDIA GeForce RTX 4070 Ti".to_string(),
            vram_bytes: 12884901888,
            device_uuid: "GPU-uuid-bombard".to_string(),
        }),
    );

    register_package_manifest(&config, "pkg-stress", "1.0.0", "CUDAExecutionProvider");

    // Pre-create 12 distinct deployments for the node
    let mut deployment_ids = Vec::new();
    for _ in 0..12 {
        let dep = deploy_package(
            &config,
            DeployPackageRequest {
                node_id: node_id.clone(),
                package_id: "pkg-stress".to_string(),
            },
        )
        .unwrap();
        deployment_ids.push(dep.deployment_id);
    }

    let config_arc = Arc::new(config);
    let mut handles = Vec::new();

    // Spawn 12 concurrent threads submitting receipts simultaneously
    for (i, dep_id) in deployment_ids.iter().cloned().enumerate() {
        let cfg = Arc::clone(&config_arc);
        let handle = thread::spawn(move || {
            let drift = if i % 2 == 0 { 0.001 * (i as f64 + 1.0) } else { 0.08 }; // alternating valid / invalid drift
            let res = submit_deploy_receipt(
                &cfg,
                DeployReceiptRequest {
                    deployment_id: dep_id.clone(),
                    package_id: "pkg-stress".to_string(),
                    status: "ready".to_string(),
                    stage: Some("ready".to_string()),
                    startup_proof: Some(json!({"kernel_events": 141})),
                    self_test: Some(SelfTestResult {
                        passed: true,
                        drift,
                    }),
                    duration_ms: Some(1000 + i as i64 * 10),
                    error: None,
                },
            );

            if drift <= DRIFT_TOLERANCE {
                assert!(res.is_ok(), "Thread {} valid drift should succeed", i);
            } else {
                let err = res.unwrap_err();
                assert_eq!(err.status, 422, "Thread {} invalid drift must return 422", i);
            }
        });
        handles.push(handle);
    }

    for h in handles {
        h.join().expect("thread join must not panic");
    }

    // Verify DB integrity and consistency
    let conn = processing_scheduler::open_or_init(&config_arc).unwrap();
    let integrity: String = conn
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .unwrap();
    assert_eq!(integrity, "ok", "Database must remain healthy under concurrent load");

    let dep_count: i64 = conn
        .query_row("SELECT count(*) FROM agent_deployments WHERE node_id = ?", [&node_id], |r| r.get(0))
        .unwrap();
    assert_eq!(dep_count, 12, "All 12 deployments must be recorded");
}
