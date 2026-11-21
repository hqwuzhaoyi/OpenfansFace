use face_file_operations::agent::config::AgentConfig;
use face_file_operations::agent_registry::{
    self, control_node, create_enrollment_token, handle_heartbeat, list_nodes, register_node,
    EnrollmentTokenRequest, GpuSpecs, HeartbeatRequest, NodeControlRequest, RegisterRequest,
};
use face_file_operations::config::Config;
use face_file_operations::processing_scheduler;
use std::fs;
use std::path::PathBuf;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

fn temp_fixture_config() -> (Config, PathBuf) {
    let tmp_dir = std::env::temp_dir().join(format!("face-test-{}", uuid::Uuid::new_v4()));
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

#[test]
fn test_enrollment_token_lifecycle_and_single_use() {
    let (config, _dir) = temp_fixture_config();

    // 1. Admin generates one-time enrollment token
    let token_resp = create_enrollment_token(
        &config,
        EnrollmentTokenRequest {
            note: Some("Test node enrollment".to_string()),
            ttl_seconds: Some(3600.0),
        },
    )
    .expect("Failed to create enrollment token");

    assert!(!token_resp.token.is_empty());
    assert!(token_resp.expires_at > agent_registry::now_sec());

    // 2. Register node with valid token -> SUCCESS
    let reg_req = RegisterRequest {
        enrollment_token: token_resp.token.clone(),
        hostname: "wsl-node-1".to_string(),
        os: "linux".to_string(),
        arch: "x86_64".to_string(),
        gpu: Some(GpuSpecs {
            model: "NVIDIA GeForce RTX 4070 Ti".to_string(),
            vram_bytes: 12884901888,
            device_uuid: "GPU-7a2e85b1-3cf9-42b7-84e1-2a1c0d4e9f80".to_string(),
        }),
        ram_bytes: 34359738368,
        cpu_cores: 16,
        agent_version: "0.1.0".to_string(),
        max_concurrent_tasks: None,
    };

    let reg_resp = register_node(&config, reg_req).expect("First registration should succeed");
    assert!(reg_resp.node_id.starts_with("node-wsl-node-1-"));
    assert!(reg_resp.node_token.starts_with("agt_"));
    assert_eq!(reg_resp.heartbeat_interval_s, 15);
    assert!(reg_resp.device_group_id.starts_with("grp-gpu-"));

    // 3. Register AGAIN with the same token -> FAILS (401 single-use consumption)
    let dup_req = RegisterRequest {
        enrollment_token: token_resp.token.clone(),
        hostname: "wsl-node-2".to_string(),
        os: "linux".to_string(),
        arch: "x86_64".to_string(),
        gpu: None,
        ram_bytes: 17179869184,
        cpu_cores: 8,
        agent_version: "0.1.0".to_string(),
        max_concurrent_tasks: None,
    };

    let dup_err = register_node(&config, dup_req).expect_err("Duplicate registration must fail");
    assert_eq!(dup_err.status, 401);
}

#[test]
fn test_invalid_enrollment_token_rejected() {
    let (config, _dir) = temp_fixture_config();

    let invalid_req = RegisterRequest {
        enrollment_token: "invalid-token-12345".to_string(),
        hostname: "rogue-agent".to_string(),
        os: "linux".to_string(),
        arch: "x86_64".to_string(),
        gpu: None,
        ram_bytes: 8589934592,
        cpu_cores: 4,
        agent_version: "0.1.0".to_string(),
        max_concurrent_tasks: None,
    };

    let err = register_node(&config, invalid_req).expect_err("Invalid token must fail");
    assert_eq!(err.status, 401);
}

#[test]
fn test_heartbeat_telemetry_and_authentication() {
    let (config, _dir) = temp_fixture_config();

    let token_resp = create_enrollment_token(
        &config,
        EnrollmentTokenRequest {
            note: None,
            ttl_seconds: Some(600.0),
        },
    )
    .unwrap();

    let reg = register_node(
        &config,
        RegisterRequest {
            enrollment_token: token_resp.token,
            hostname: "worker-pc".to_string(),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            gpu: Some(GpuSpecs {
                model: "RTX 4070 Ti".to_string(),
                vram_bytes: 12884901888,
                device_uuid: "GPU-uuid-111".to_string(),
            }),
            ram_bytes: 34359738368,
            cpu_cores: 12,
            agent_version: "0.1.0".to_string(),
            max_concurrent_tasks: None,
        },
    )
    .unwrap();

    // 1. Send heartbeat with valid token -> SUCCESS
    let hb_resp = handle_heartbeat(
        &config,
        &reg.node_token,
        HeartbeatRequest {
            node_id: reg.node_id.clone(),
            instance_id: "inst-1".to_string(),
            timestamp: agent_registry::now_sec(),
            uptime_s: Some(10.0),
            free_ram_bytes: 20000000000,
            free_vram_bytes: 10000000000,
            gpu_util_percent: 15.5,
            running_tasks: 0,
        },
    )
    .expect("Heartbeat with valid token should succeed");

    assert!(hb_resp.ok);

    // Verify node status in list_nodes is now "online"
    let list = list_nodes(&config).unwrap();
    let node_entry = list["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["node_id"] == reg.node_id)
        .unwrap();
    assert_eq!(node_entry["status"], "online");
    assert_eq!(
        node_entry["telemetry"]["free_ram_bytes"],
        20000000000i64
    );
    assert_eq!(
        node_entry["telemetry"]["gpu"]["free_vram_bytes"],
        10000000000i64
    );
    assert_eq!(
        node_entry["telemetry"]["gpu"]["gpu_util_percent"],
        15.5
    );

    // 2. Send heartbeat with INVALID token -> FAILS (401)
    let bad_hb_err = handle_heartbeat(
        &config,
        "agt_wrong_secret_token",
        HeartbeatRequest {
            node_id: reg.node_id.clone(),
            instance_id: "inst-1".to_string(),
            timestamp: agent_registry::now_sec(),
            uptime_s: None,
            free_ram_bytes: 20000000000,
            free_vram_bytes: 10000000000,
            gpu_util_percent: 15.5,
            running_tasks: 0,
        },
    )
    .expect_err("Heartbeat with wrong token must fail");
    assert_eq!(bad_hb_err.status, 401);

    // 3. Crash detection: heartbeat with new instance_id
    let restart_hb = handle_heartbeat(
        &config,
        &reg.node_token,
        HeartbeatRequest {
            node_id: reg.node_id.clone(),
            instance_id: "inst-2-restarted".to_string(),
            timestamp: agent_registry::now_sec(),
            uptime_s: Some(1.0),
            free_ram_bytes: 20000000000,
            free_vram_bytes: 10000000000,
            gpu_util_percent: 0.0,
            running_tasks: 0,
        },
    )
    .expect("Heartbeat with restarted instance should succeed");
    assert!(restart_hb.ok);

    let conn = processing_scheduler::open_or_init(&config).unwrap();
    let restart_event: String = conn
        .query_row(
            "SELECT message FROM events WHERE kind = 'agent_restarted' ORDER BY at DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .expect("agent_restarted event must be recorded");
    assert!(restart_event.contains("restarted with new instance_id inst-2-restarted"));
}

#[test]
fn test_heartbeat_offline_timeout_and_truth_in_ui() {
    let (config, _dir) = temp_fixture_config();

    let t1 = create_enrollment_token(&config, EnrollmentTokenRequest { note: None, ttl_seconds: None }).unwrap();
    let reg = register_node(
        &config,
        RegisterRequest {
            enrollment_token: t1.token,
            hostname: "aging-node".to_string(),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            gpu: Some(GpuSpecs {
                model: "RTX 4070 Ti".to_string(),
                vram_bytes: 12000000000,
                device_uuid: "GPU-uuid-age".to_string(),
            }),
            ram_bytes: 32000000000,
            cpu_cores: 8,
            agent_version: "0.1.0".to_string(),
            max_concurrent_tasks: None,
        },
    )
    .unwrap();

    // Simulate an initial heartbeat
    handle_heartbeat(
        &config,
        &reg.node_token,
        HeartbeatRequest {
            node_id: reg.node_id.clone(),
            instance_id: "inst-age".to_string(),
            timestamp: agent_registry::now_sec(),
            uptime_s: None,
            free_ram_bytes: 16000000000,
            free_vram_bytes: 8000000000,
            gpu_util_percent: 25.0,
            running_tasks: 0,
        },
    )
    .unwrap();

    // Now artificially age the last_heartbeat_at to 100 seconds ago (>90s)
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    let past = agent_registry::now_sec() - 100.0;
    conn.execute(
        "UPDATE agent_nodes SET last_heartbeat_at = ? WHERE id = ?",
        rusqlite::params![past, reg.node_id],
    ).unwrap();

    // List nodes: dynamic evaluation must classify it as "offline"
    // And its dynamic free telemetry must be Null (Truth in UI: "未知", NEVER 0)
    let list = list_nodes(&config).unwrap();
    let node_entry = list["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["node_id"] == reg.node_id)
        .unwrap();

    assert_eq!(node_entry["status"], "offline");
    assert!(node_entry["telemetry"]["free_ram_bytes"].is_null());
    assert!(node_entry["telemetry"]["gpu"]["free_vram_bytes"].is_null());
    assert!(node_entry["telemetry"]["gpu"]["gpu_util_percent"].is_null());
}

#[test]
fn test_physical_device_pooling_groups_matching_gpu_uuids() {
    let (config, _dir) = temp_fixture_config();

    // Register 2 agents with the SAME GPU UUID (e.g. 2 instances on same physical machine)
    let t1 = create_enrollment_token(&config, EnrollmentTokenRequest { note: None, ttl_seconds: None }).unwrap();
    let reg1 = register_node(
        &config,
        RegisterRequest {
            enrollment_token: t1.token,
            hostname: "desktop-wsl-agent1".to_string(),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            gpu: Some(GpuSpecs {
                model: "RTX 4070 Ti".to_string(),
                vram_bytes: 12884901888,
                device_uuid: "GPU-SHARED-UUID-9999".to_string(),
            }),
            ram_bytes: 34359738368,
            cpu_cores: 16,
            agent_version: "0.1.0".to_string(),
            max_concurrent_tasks: None,
        },
    )
    .unwrap();

    let t2 = create_enrollment_token(&config, EnrollmentTokenRequest { note: None, ttl_seconds: None }).unwrap();
    let reg2 = register_node(
        &config,
        RegisterRequest {
            enrollment_token: t2.token,
            hostname: "desktop-wsl-agent2".to_string(),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            gpu: Some(GpuSpecs {
                model: "RTX 4070 Ti".to_string(),
                vram_bytes: 12884901888,
                device_uuid: "GPU-SHARED-UUID-9999".to_string(),
            }),
            ram_bytes: 34359738368,
            cpu_cores: 16,
            agent_version: "0.1.0".to_string(),
            max_concurrent_tasks: None,
        },
    )
    .unwrap();

    // Register a 3rd agent with CPU-only on a distinct host
    let t3 = create_enrollment_token(&config, EnrollmentTokenRequest { note: None, ttl_seconds: None }).unwrap();
    let reg3 = register_node(
        &config,
        RegisterRequest {
            enrollment_token: t3.token,
            hostname: "mac-mini".to_string(),
            os: "darwin".to_string(),
            arch: "aarch64".to_string(),
            gpu: None,
            ram_bytes: 68719476736,
            cpu_cores: 10,
            agent_version: "0.1.0".to_string(),
            max_concurrent_tasks: None,
        },
    )
    .unwrap();

    // Invariant: reg1 and reg2 share the EXACT same device_group_id
    assert_eq!(reg1.device_group_id, reg2.device_group_id);
    assert!(reg1.device_group_id.starts_with("grp-gpu-"));

    // Invariant: reg3 has host-based device_group_id
    assert_ne!(reg1.device_group_id, reg3.device_group_id);
    assert!(reg3.device_group_id.starts_with("grp-host-"));

    // Total physical device pools in cluster must be 2, not 3!
    let list = list_nodes(&config).unwrap();
    assert_eq!(list["cluster"]["total_nodes"], 3);
    assert_eq!(list["cluster"]["device_groups_count"], 2);
}

#[test]
fn test_node_control_revoke_and_restore_acceptance() {
    let (config, _dir) = temp_fixture_config();

    let t = create_enrollment_token(&config, EnrollmentTokenRequest { note: None, ttl_seconds: None }).unwrap();
    let reg = register_node(
        &config,
        RegisterRequest {
            enrollment_token: t.token,
            hostname: "controlled-node".to_string(),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            gpu: None,
            ram_bytes: 16000000000,
            cpu_cores: 4,
            agent_version: "0.1.0".to_string(),
            max_concurrent_tasks: None,
        },
    )
    .unwrap();

    // Heartbeat to make it online
    handle_heartbeat(
        &config,
        &reg.node_token,
        HeartbeatRequest {
            node_id: reg.node_id.clone(),
            instance_id: "inst-ctrl".to_string(),
            timestamp: agent_registry::now_sec(),
            uptime_s: None,
            free_ram_bytes: 8000000000,
            free_vram_bytes: 0,
            gpu_util_percent: 0.0,
            running_tasks: 0,
        },
    )
    .unwrap();

    // 1. Revoke acceptance
    let rev_res = control_node(
        &config,
        NodeControlRequest {
            node_id: reg.node_id.clone(),
            action: "revoke_acceptance".to_string(),
        },
    )
    .unwrap();
    assert!(!rev_res.accepting);

    let list1 = list_nodes(&config).unwrap();
    let n1 = list1["nodes"].as_array().unwrap().iter().find(|n| n["node_id"] == reg.node_id).unwrap();
    assert_eq!(n1["accepting_tasks"], false);

    // 2. Next heartbeat will return command to pause acceptance
    let hb_after_revoke = handle_heartbeat(
        &config,
        &reg.node_token,
        HeartbeatRequest {
            node_id: reg.node_id.clone(),
            instance_id: "inst-ctrl".to_string(),
            timestamp: agent_registry::now_sec(),
            uptime_s: None,
            free_ram_bytes: 8000000000,
            free_vram_bytes: 0,
            gpu_util_percent: 0.0,
            running_tasks: 0,
        },
    )
    .unwrap();
    assert!(hb_after_revoke.commands.iter().any(|c| c.command == "pause_acceptance"));

    // 3. Restore acceptance
    let rest_res = control_node(
        &config,
        NodeControlRequest {
            node_id: reg.node_id.clone(),
            action: "restore_acceptance".to_string(),
        },
    )
    .unwrap();
    assert!(rest_res.accepting);

    let list2 = list_nodes(&config).unwrap();
    let n2 = list2["nodes"].as_array().unwrap().iter().find(|n| n["node_id"] == reg.node_id).unwrap();
    assert_eq!(n2["accepting_tasks"], true);
}

#[test]
fn test_agent_config_permissions_and_security() {
    let tmp_dir = std::env::temp_dir().join(format!("agent-cfg-test-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&tmp_dir).unwrap();
    let cfg_path = tmp_dir.join("agent.json");

    let cfg = AgentConfig {
        schema_version: 1,
        center_url: "http://127.0.0.1:8080".to_string(),
        node_id: "node-test-01".to_string(),
        node_token: "agt_secret_123456".to_string(),
        device_group_id: "grp-host-123".to_string(),
        enrolled_at: agent_registry::now_sec() as u64,
    };

    // 1. Save config: verifies 0600 mode
    cfg.save_to(&cfg_path).expect("Saving agent config must succeed");

    #[cfg(unix)]
    {
        let mode = fs::metadata(&cfg_path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "Config file mode must be 0600 (-rw-------)");
    }

    // 2. Load config: succeeds with 0600
    let loaded = AgentConfig::load_from(&cfg_path).expect("Loading config must succeed");
    assert_eq!(loaded.node_id, "node-test-01");
    assert_eq!(loaded.node_token, "agt_secret_123456");

    // 3. Deliberately alter permissions to 0644 (-rw-r--r--)
    #[cfg(unix)]
    {
        fs::set_permissions(&cfg_path, fs::Permissions::from_mode(0o644)).unwrap();
        let insecure_err = AgentConfig::load_from(&cfg_path).expect_err("Loading insecure config must fail");
        assert_eq!(insecure_err.code, "insecure_config_permissions");
    }

    let _ = fs::remove_dir_all(&tmp_dir);
}
