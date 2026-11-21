use face_file_operations::agent_registry::{
    self, control_node, create_enrollment_token, handle_heartbeat, list_nodes, register_node,
    EnrollmentTokenRequest, GpuSpecs, HeartbeatRequest, NodeControlRequest, RegisterRequest,
};
use face_file_operations::config::Config;
use face_file_operations::processing_scheduler;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Barrier};
use std::thread;

fn temp_fixture_config() -> (Config, PathBuf) {
    let tmp_dir = std::env::temp_dir().join(format!("face-adv-test-{}", uuid::Uuid::new_v4()));
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

// ============================================================================
// Objective 1: Stress test token enrollment
// ============================================================================

#[test]
fn test_adversarial_token_replay_and_expiration() {
    let (config, _dir) = temp_fixture_config();

    // 1. Expired token in the past (negative TTL)
    let exp_resp = create_enrollment_token(
        &config,
        EnrollmentTokenRequest {
            note: Some("Expired token".to_string()),
            ttl_seconds: Some(-10.0),
        },
    )
    .unwrap();

    let err_exp = register_node(
        &config,
        RegisterRequest {
            enrollment_token: exp_resp.token,
            hostname: "exp-host".to_string(),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            gpu: None,
            ram_bytes: 8 * 1024 * 1024 * 1024,
            cpu_cores: 4,
            agent_version: "0.1.0".to_string(),
            max_concurrent_tasks: None,
        },
    )
    .expect_err("Expired token must fail registration");
    assert_eq!(err_exp.status, 401);

    // 2. Token replay attack: valid token used once, then replayed 50 times
    let valid_resp = create_enrollment_token(
        &config,
        EnrollmentTokenRequest {
            note: Some("Single use token".to_string()),
            ttl_seconds: Some(3600.0),
        },
    )
    .unwrap();

    let reg_ok = register_node(
        &config,
        RegisterRequest {
            enrollment_token: valid_resp.token.clone(),
            hostname: "legit-host".to_string(),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            gpu: None,
            ram_bytes: 16 * 1024 * 1024 * 1024,
            cpu_cores: 8,
            agent_version: "0.1.0".to_string(),
            max_concurrent_tasks: None,
        },
    )
    .expect("First registration must succeed");
    assert!(!reg_ok.node_id.is_empty());

    // 50 sequential replays of the exact same token
    for i in 0..50 {
        let replay_err = register_node(
            &config,
            RegisterRequest {
                enrollment_token: valid_resp.token.clone(),
                hostname: format!("replay-host-{}", i),
                os: "linux".to_string(),
                arch: "x86_64".to_string(),
                gpu: None,
                ram_bytes: 8 * 1024 * 1024 * 1024,
                cpu_cores: 4,
                agent_version: "0.1.0".to_string(),
            max_concurrent_tasks: None,
            },
        )
        .expect_err("Token replay must be rejected with 401");
        assert_eq!(replay_err.status, 401, "Replay attempt {} did not return 401", i);
    }

    // 3. Malformed and adversarial injection tokens
    let long_token = "A".repeat(8192);
    let adversarial_tokens = vec![
        "",                                         // empty
        "   ",                                      // whitespace only
        "\t\r\n",                                  // tab and newlines
        "et_non_existent_token_1234567890",         // non-existent random token
        "' OR '1'='1",                              // classic SQLi
        "'; DROP TABLE agent_nodes; --",            // table drop attack
        "\" UNION SELECT 1, 2, 3, 4 --",            // union injection
        "\x00nullbyte_token",                       // null byte injection
        &long_token,                                // 8KB buffer overflow attempt
    ];

    for bad_token in adversarial_tokens {
        let bad_err = register_node(
            &config,
            RegisterRequest {
                enrollment_token: bad_token.to_string(),
                hostname: "fuzz-host".to_string(),
                os: "linux".to_string(),
                arch: "x86_64".to_string(),
                gpu: None,
                ram_bytes: 8 * 1024 * 1024 * 1024,
                cpu_cores: 4,
                agent_version: "0.1.0".to_string(),
            max_concurrent_tasks: None,
            },
        )
        .expect_err(&format!("Malformed token '{}' must fail with 401", bad_token));
        assert_eq!(bad_err.status, 401);
    }

    // Verify agent_nodes table was not dropped or corrupted by SQLi
    let list = list_nodes(&config).expect("list_nodes must succeed after SQLi fuzzing");
    assert_eq!(list["cluster"]["total_nodes"], 1);
}

#[test]
fn test_adversarial_concurrent_token_registration_race() {
    let (config, _dir) = temp_fixture_config();

    // Create exactly ONE single-use enrollment token
    let single_token_resp = create_enrollment_token(
        &config,
        EnrollmentTokenRequest {
            note: Some("Race condition test token".to_string()),
            ttl_seconds: Some(60.0),
        },
    )
    .unwrap();

    let shared_token = single_token_resp.token;
    let concurrency = 20;
    let barrier = Arc::new(Barrier::new(concurrency));
    let mut handles = Vec::new();

    // Spawn 20 threads racing simultaneously to consume the exact same token
    for i in 0..concurrency {
        let b = Arc::clone(&barrier);
        let token = shared_token.clone();
        let cfg = config.clone();

        handles.push(thread::spawn(move || {
            // Synchronize all threads so they hit register_node at the exact same millisecond
            b.wait();

            let req = RegisterRequest {
                enrollment_token: token,
                hostname: format!("race-host-{:02}", i),
                os: "linux".to_string(),
                arch: "x86_64".to_string(),
                gpu: None,
                ram_bytes: 8 * 1024 * 1024 * 1024,
                cpu_cores: 4,
                agent_version: "0.1.0".to_string(),
            max_concurrent_tasks: None,
            };

            register_node(&cfg, req)
        }));
    }

    let mut success_count = 0;
    let mut failure_count = 0;
    let mut winning_node_id = String::new();

    for h in handles {
        let res = h.join().expect("Thread must not panic");
        match res {
            Ok(resp) => {
                success_count += 1;
                winning_node_id = resp.node_id;
            }
            Err(e) => {
                failure_count += 1;
                assert_eq!(e.status, 401, "Failed registration must return 401");
            }
        }
    }

    // STRICT INVARIANT: Exactly 1 succeeds, exactly 19 fail
    assert_eq!(
        success_count, 1,
        "Race condition violation: exactly 1 registration must succeed, got {}",
        success_count
    );
    assert_eq!(
        failure_count,
        concurrency - 1,
        "Race condition violation: exactly {} registrations must fail, got {}",
        concurrency - 1,
        failure_count
    );

    // Verify database state: only 1 node registered and token marked used by winning node
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    let total_nodes: i64 = conn
        .query_row("SELECT count(*) FROM agent_nodes", [], |r| r.get(0))
        .unwrap();
    assert_eq!(total_nodes, 1, "Only 1 node should exist in agent_nodes table");

    let (used_at, used_by): (Option<f64>, Option<String>) = conn
        .query_row(
            "SELECT used_at, used_by_node_id FROM agent_enrollment_tokens WHERE token = ?",
            [&shared_token],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();

    assert!(used_at.is_some(), "Token used_at must be populated");
    assert_eq!(
        used_by.as_deref(),
        Some(winning_node_id.as_str()),
        "Token used_by_node_id must match the winning node"
    );
}

// ============================================================================
// Objective 2: Stress test heartbeat & offline transition
// ============================================================================

#[test]
fn test_adversarial_heartbeat_bearer_token_attacks() {
    let (config, _dir) = temp_fixture_config();

    // Register Node A
    let t_a = create_enrollment_token(&config, EnrollmentTokenRequest { note: None, ttl_seconds: None }).unwrap();
    let reg_a = register_node(
        &config,
        RegisterRequest {
            enrollment_token: t_a.token,
            hostname: "node-alpha".to_string(),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            gpu: None,
            ram_bytes: 16 * 1024 * 1024 * 1024,
            cpu_cores: 8,
            agent_version: "0.1.0".to_string(),
            max_concurrent_tasks: None,
        },
    )
    .unwrap();

    // Register Node B
    let t_b = create_enrollment_token(&config, EnrollmentTokenRequest { note: None, ttl_seconds: None }).unwrap();
    let reg_b = register_node(
        &config,
        RegisterRequest {
            enrollment_token: t_b.token,
            hostname: "node-bravo".to_string(),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            gpu: None,
            ram_bytes: 16 * 1024 * 1024 * 1024,
            cpu_cores: 8,
            agent_version: "0.1.0".to_string(),
            max_concurrent_tasks: None,
        },
    )
    .unwrap();

    let make_hb = |node_id: String| HeartbeatRequest {
        node_id,
        instance_id: "inst-test".to_string(),
        timestamp: agent_registry::now_sec(),
        uptime_s: None,
        free_ram_bytes: 8000000000,
        free_vram_bytes: 0,
        gpu_util_percent: 0.0,
        running_tasks: 0,
    };

    // 1. Empty bearer token -> 401
    let err1 = handle_heartbeat(&config, "", make_hb(reg_a.node_id.clone())).expect_err("Empty token must fail");
    assert_eq!(err1.status, 401);

    // 2. Corrupted token -> 401
    let err2 = handle_heartbeat(
        &config,
        "agt_completely_bogus_token",
        make_hb(reg_a.node_id.clone()),
    )
    .expect_err("Bogus token must fail");
    assert_eq!(err2.status, 401);

    // 3. Cross-node token impersonation: Node B's token used with Node A's node_id -> 401
    let err3 = handle_heartbeat(
        &config,
        &reg_b.node_token, // Node B's token
        make_hb(reg_a.node_id.clone()), // But Node A's id!
    )
    .expect_err("Cross-node token impersonation must fail");
    assert_eq!(err3.status, 401);

    // 4. Non-existent node_id with Node A's valid token -> 401
    let err4 = handle_heartbeat(
        &config,
        &reg_a.node_token,
        make_hb("node-nonexistent-999".to_string()),
    )
    .expect_err("Non-existent node_id must fail");
    assert_eq!(err4.status, 401);
}

#[test]
fn test_adversarial_heartbeat_timestamp_fuzzing_and_drift() {
    let (config, _dir) = temp_fixture_config();

    let t = create_enrollment_token(&config, EnrollmentTokenRequest { note: None, ttl_seconds: None }).unwrap();
    let reg = register_node(
        &config,
        RegisterRequest {
            enrollment_token: t.token,
            hostname: "drift-node".to_string(),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            gpu: None,
            ram_bytes: 16 * 1024 * 1024 * 1024,
            cpu_cores: 8,
            agent_version: "0.1.0".to_string(),
            max_concurrent_tasks: None,
        },
    )
    .unwrap();

    let t_start = agent_registry::now_sec();

    // Adversarial Case 1: Client clock is 10 years in the future (timestamp = now + 315_360_000s)
    let hb_future = handle_heartbeat(
        &config,
        &reg.node_token,
        HeartbeatRequest {
            node_id: reg.node_id.clone(),
            instance_id: "inst-future".to_string(),
            timestamp: t_start + 315_360_000.0, // Future client timestamp!
            uptime_s: None,
            free_ram_bytes: 8000000000,
            free_vram_bytes: 0,
            gpu_util_percent: 0.0,
            running_tasks: 0,
        },
    )
    .expect("Heartbeat with future timestamp must be handled safely");
    assert!(hb_future.ok);

    // Verify that the server recorded authoritative server time, NOT the client's future timestamp!
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    let stored_hb_at: f64 = conn
        .query_row(
            "SELECT last_heartbeat_at FROM agent_nodes WHERE id = ?",
            [&reg.node_id],
            |r| r.get(0),
        )
        .unwrap();

    let t_now = agent_registry::now_sec();
    assert!(
        (stored_hb_at - t_now).abs() < 5.0,
        "Server must record server now_sec(), not poisoned future timestamp! stored={}, now={}",
        stored_hb_at,
        t_now
    );

    // Adversarial Case 2: Client clock is far in the past (timestamp = -999999.0)
    let hb_past = handle_heartbeat(
        &config,
        &reg.node_token,
        HeartbeatRequest {
            node_id: reg.node_id.clone(),
            instance_id: "inst-past".to_string(),
            timestamp: -999999.0, // Past client timestamp!
            uptime_s: None,
            free_ram_bytes: 8000000000,
            free_vram_bytes: 0,
            gpu_util_percent: 0.0,
            running_tasks: 0,
        },
    )
    .expect("Heartbeat with past timestamp must be handled safely");
    assert!(hb_past.ok);

    let stored_hb_at_2: f64 = conn
        .query_row(
            "SELECT last_heartbeat_at FROM agent_nodes WHERE id = ?",
            [&reg.node_id],
            |r| r.get(0),
        )
        .unwrap();

    let t_now_2 = agent_registry::now_sec();
    assert!(
        (stored_hb_at_2 - t_now_2).abs() < 5.0,
        "Server must record server now_sec(), not poisoned past timestamp! stored={}, now={}",
        stored_hb_at_2,
        t_now_2
    );
}

#[test]
fn test_adversarial_offline_strict_boundary_and_truth_in_ui() {
    let (config, _dir) = temp_fixture_config();

    // 1. Freshly registered node with NO heartbeats
    let t1 = create_enrollment_token(&config, EnrollmentTokenRequest { note: None, ttl_seconds: None }).unwrap();
    let reg1 = register_node(
        &config,
        RegisterRequest {
            enrollment_token: t1.token,
            hostname: "unheard-node".to_string(),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            gpu: Some(GpuSpecs {
                model: "RTX 4070 Ti".to_string(),
                vram_bytes: 12000000000,
                device_uuid: "GPU-UNHEARD-01".to_string(),
            }),
            ram_bytes: 32000000000,
            cpu_cores: 16,
            agent_version: "0.1.0".to_string(),
            max_concurrent_tasks: None,
        },
    )
    .unwrap();

    // Verify initial state: status is "registered", telemetry is Null ("未知")
    let list0 = list_nodes(&config).unwrap();
    let node0 = list0["nodes"].as_array().unwrap().iter().find(|n| n["node_id"] == reg1.node_id).unwrap();
    assert_eq!(node0["status"], "registered");
    assert!(node0["telemetry"]["free_ram_bytes"].is_null(), "Registered-only node must have null free_ram");
    assert!(node0["telemetry"]["gpu"]["free_vram_bytes"].is_null(), "Registered-only node must have null free_vram");
    assert!(node0["telemetry"]["gpu"]["gpu_util_percent"].is_null(), "Registered-only node must have null gpu_util");

    // 2. Node sends heartbeat -> becomes "online" with real numbers
    handle_heartbeat(
        &config,
        &reg1.node_token,
        HeartbeatRequest {
            node_id: reg1.node_id.clone(),
            instance_id: "inst-heartbeat-1".to_string(),
            timestamp: agent_registry::now_sec(),
            uptime_s: Some(10.0),
            free_ram_bytes: 20000000000,
            free_vram_bytes: 10000000000,
            gpu_util_percent: 42.5,
            running_tasks: 0,
        },
    )
    .unwrap();

    let list1 = list_nodes(&config).unwrap();
    let node1 = list1["nodes"].as_array().unwrap().iter().find(|n| n["node_id"] == reg1.node_id).unwrap();
    assert_eq!(node1["status"], "online");
    assert_eq!(node1["telemetry"]["free_ram_bytes"], 20000000000i64);
    assert_eq!(node1["telemetry"]["gpu"]["free_vram_bytes"], 10000000000i64);
    assert_eq!(node1["telemetry"]["gpu"]["gpu_util_percent"], 42.5);

    let conn = processing_scheduler::open_or_init(&config).unwrap();

    // 3. Boundary test: age to 89.0s (under 90s boundary)
    let now = agent_registry::now_sec();
    conn.execute(
        "UPDATE agent_nodes SET last_heartbeat_at = ? WHERE id = ?",
        rusqlite::params![now - 89.0, reg1.node_id],
    )
    .unwrap();

    let list_89 = list_nodes(&config).unwrap();
    let node_89 = list_89["nodes"].as_array().unwrap().iter().find(|n| n["node_id"] == reg1.node_id).unwrap();
    assert_eq!(
        node_89["status"], "online",
        "At 89s (<90s), node must still be online"
    );
    assert!(!node_89["telemetry"]["free_ram_bytes"].is_null());

    // 4. Boundary test: age to 90.5s (>90s boundary)
    conn.execute(
        "UPDATE agent_nodes SET last_heartbeat_at = ? WHERE id = ?",
        rusqlite::params![now - 90.5, reg1.node_id],
    )
    .unwrap();

    let list_91 = list_nodes(&config).unwrap();
    let node_91 = list_91["nodes"].as_array().unwrap().iter().find(|n| n["node_id"] == reg1.node_id).unwrap();
    assert_eq!(
        node_91["status"], "offline",
        "At >90s, node status strictly must be 'offline'"
    );
    // TRUTH IN UI EMPIRICAL VERIFICATION:
    // Telemetry MUST BE NULL so frontend displays "未知", NEVER 0!
    assert!(
        node_91["telemetry"]["free_ram_bytes"].is_null(),
        "Offline node free_ram_bytes must be Null (Truth in UI)"
    );
    assert!(
        node_91["telemetry"]["gpu"]["free_vram_bytes"].is_null(),
        "Offline node free_vram_bytes must be Null (Truth in UI)"
    );
    assert!(
        node_91["telemetry"]["gpu"]["gpu_util_percent"].is_null(),
        "Offline node gpu_util_percent must be Null (Truth in UI)"
    );

    // 5. Cluster-wide offline reap triggered by another active node's heartbeat
    // Register Node 2 and send heartbeat; this should reap Node 1 in the database table
    let t2 = create_enrollment_token(&config, EnrollmentTokenRequest { note: None, ttl_seconds: None }).unwrap();
    let reg2 = register_node(
        &config,
        RegisterRequest {
            enrollment_token: t2.token,
            hostname: "active-node".to_string(),
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

    handle_heartbeat(
        &config,
        &reg2.node_token,
        HeartbeatRequest {
            node_id: reg2.node_id.clone(),
            instance_id: "inst-reaper".to_string(),
            timestamp: agent_registry::now_sec(),
            uptime_s: None,
            free_ram_bytes: 8000000000,
            free_vram_bytes: 0,
            gpu_util_percent: 0.0,
            running_tasks: 0,
        },
    )
    .unwrap();

    // Verify Node 1's persisted status column in SQLite was reaped to 'offline'
    let n1_persisted_status: String = conn
        .query_row(
            "SELECT status FROM agent_nodes WHERE id = ?",
            [&reg1.node_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        n1_persisted_status, "offline",
        "Heartbeat handler must reap stale nodes to 'offline' in agent_nodes table"
    );

    // 6. Node recovery: Node 1 sends heartbeat again -> transitions back to 'online'
    handle_heartbeat(
        &config,
        &reg1.node_token,
        HeartbeatRequest {
            node_id: reg1.node_id.clone(),
            instance_id: "inst-heartbeat-recovered".to_string(),
            timestamp: agent_registry::now_sec(),
            uptime_s: None,
            free_ram_bytes: 22000000000,
            free_vram_bytes: 11000000000,
            gpu_util_percent: 10.0,
            running_tasks: 0,
        },
    )
    .unwrap();

    let list_rec = list_nodes(&config).unwrap();
    let node_rec = list_rec["nodes"].as_array().unwrap().iter().find(|n| n["node_id"] == reg1.node_id).unwrap();
    assert_eq!(node_rec["status"], "online", "Recovered node must be online");
    assert_eq!(node_rec["telemetry"]["free_ram_bytes"], 22000000000i64);
}

// ============================================================================
// Objective 3: Physical device pooling verification
// ============================================================================

#[test]
fn test_adversarial_device_group_pooling_and_collision_resistance() {
    let (config, _dir) = temp_fixture_config();

    // 1. Two agents on different hosts sharing the SAME physical GPU (e.g. multi-agent container / WSL sharing)
    let t1 = create_enrollment_token(&config, EnrollmentTokenRequest { note: None, ttl_seconds: None }).unwrap();
    let reg1 = register_node(
        &config,
        RegisterRequest {
            enrollment_token: t1.token,
            hostname: "wsl-container-1".to_string(),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            gpu: Some(GpuSpecs {
                model: "RTX 4070 Ti".to_string(),
                vram_bytes: 12884901888,
                device_uuid: "GPU-PHYSICAL-UUID-SHARED-12345".to_string(),
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
            hostname: "wsl-container-2".to_string(),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            gpu: Some(GpuSpecs {
                model: "RTX 4070 Ti".to_string(),
                vram_bytes: 12884901888,
                device_uuid: "  GPU-PHYSICAL-UUID-SHARED-12345  ".to_string(), // Whitespace padded!
            }),
            ram_bytes: 34359738368,
            cpu_cores: 16,
            agent_version: "0.1.0".to_string(),
            max_concurrent_tasks: None,
        },
    )
    .unwrap();

    // STRICT INVARIANT: reg1 and reg2 share identical device_group_id despite different hostnames and padded whitespace!
    assert_eq!(
        reg1.device_group_id, reg2.device_group_id,
        "Agents sharing GPU UUID must be grouped into identical device_group_id"
    );
    assert!(reg1.device_group_id.starts_with("grp-gpu-"));

    // 2. Third agent with a DISTINCT physical GPU UUID
    let t3 = create_enrollment_token(&config, EnrollmentTokenRequest { note: None, ttl_seconds: None }).unwrap();
    let reg3 = register_node(
        &config,
        RegisterRequest {
            enrollment_token: t3.token,
            hostname: "pc-server-rtx4090".to_string(),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            gpu: Some(GpuSpecs {
                model: "RTX 4090".to_string(),
                vram_bytes: 25769803776,
                device_uuid: "GPU-PHYSICAL-UUID-DISTINCT-67890".to_string(),
            }),
            ram_bytes: 68719476736,
            cpu_cores: 32,
            agent_version: "0.1.0".to_string(),
            max_concurrent_tasks: None,
        },
    )
    .unwrap();

    assert_ne!(
        reg1.device_group_id, reg3.device_group_id,
        "Distinct GPU UUIDs must never map to the same device_group_id"
    );

    // 3. Collision resistance: Can a CPU hostname collide with a GPU UUID?
    // Let an adversarial node register with hostname = "GPU-PHYSICAL-UUID-SHARED-12345"
    let t4 = create_enrollment_token(&config, EnrollmentTokenRequest { note: None, ttl_seconds: None }).unwrap();
    let reg4 = register_node(
        &config,
        RegisterRequest {
            enrollment_token: t4.token,
            hostname: "GPU-PHYSICAL-UUID-SHARED-12345".to_string(), // Hostname mimicking GPU UUID!
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            gpu: None, // CPU-only
            ram_bytes: 16000000000,
            cpu_cores: 4,
            agent_version: "0.1.0".to_string(),
            max_concurrent_tasks: None,
        },
    )
    .unwrap();

    assert_ne!(
        reg1.device_group_id, reg4.device_group_id,
        "Collision attack: Hostname matching GPU UUID must NOT collide with GPU pool"
    );
    assert!(
        reg4.device_group_id.starts_with("grp-host-"),
        "CPU group must start with grp-host-"
    );

    // 4. Multiple CPU-only agents on the SAME host
    let t5 = create_enrollment_token(&config, EnrollmentTokenRequest { note: None, ttl_seconds: None }).unwrap();
    let reg5 = register_node(
        &config,
        RegisterRequest {
            enrollment_token: t5.token,
            hostname: "mac-studio".to_string(),
            os: "darwin".to_string(),
            arch: "aarch64".to_string(),
            gpu: None,
            ram_bytes: 68719476736,
            cpu_cores: 12,
            agent_version: "0.1.0".to_string(),
            max_concurrent_tasks: None,
        },
    )
    .unwrap();

    let t6 = create_enrollment_token(&config, EnrollmentTokenRequest { note: None, ttl_seconds: None }).unwrap();
    let reg6 = register_node(
        &config,
        RegisterRequest {
            enrollment_token: t6.token,
            hostname: "mac-studio".to_string(), // Same host
            os: "darwin".to_string(),
            arch: "aarch64".to_string(),
            gpu: None,
            ram_bytes: 68719476736,
            cpu_cores: 12,
            agent_version: "0.1.0".to_string(),
            max_concurrent_tasks: None,
        },
    )
    .unwrap();

    assert_eq!(
        reg5.device_group_id, reg6.device_group_id,
        "Two CPU-only agents on the same host must share device_group_id"
    );

    // 5. Cluster inventory aggregation check
    // Total nodes = 6 (reg1..reg6)
    // Device pools:
    // Pool 1: GPU-PHYSICAL-UUID-SHARED-12345 (reg1, reg2)
    // Pool 2: GPU-PHYSICAL-UUID-DISTINCT-67890 (reg3)
    // Pool 3: host "GPU-PHYSICAL-UUID-SHARED-12345" (reg4)
    // Pool 4: host "mac-studio" (reg5, reg6)
    // Total distinct device groups = 4!
    let list = list_nodes(&config).unwrap();
    assert_eq!(list["cluster"]["total_nodes"], 6);
    assert_eq!(
        list["cluster"]["device_groups_count"], 4,
        "Cluster must aggregate to 4 distinct physical device pools, not 6"
    );
}


#[test]
fn test_adversarial_high_concurrency_multi_token_race() {
    let (config, _dir) = temp_fixture_config();

    // 1. Extreme race: 50 concurrent threads racing for ONE token
    let single_token = create_enrollment_token(
        &config,
        EnrollmentTokenRequest {
            note: Some("50-thread race".to_string()),
            ttl_seconds: Some(60.0),
        },
    )
    .unwrap()
    .token;

    let barrier_50 = Arc::new(Barrier::new(50));
    let mut handles_50 = Vec::new();

    for i in 0..50 {
        let b = Arc::clone(&barrier_50);
        let token = single_token.clone();
        let cfg = config.clone();

        handles_50.push(thread::spawn(move || {
            b.wait();
            register_node(
                &cfg,
                RegisterRequest {
                    enrollment_token: token,
                    hostname: format!("extreme-race-{:02}", i),
                    os: "linux".to_string(),
                    arch: "x86_64".to_string(),
                    gpu: None,
                    ram_bytes: 8 * 1024 * 1024 * 1024,
                    cpu_cores: 4,
                    agent_version: "0.1.0".to_string(),
            max_concurrent_tasks: None,
                },
            )
        }));
    }

    let mut ok_50 = 0;
    let mut err_50 = 0;
    for h in handles_50 {
        match h.join().unwrap() {
            Ok(_) => ok_50 += 1,
            Err(e) => {
                err_50 += 1;
                assert_eq!(e.status, 401);
            }
        }
    }
    assert_eq!(ok_50, 1, "Exactly 1 thread out of 50 must win the race");
    assert_eq!(err_50, 49, "Exactly 49 threads out of 50 must receive 401");

    // 2. Multi-token concurrent throughput: 10 distinct valid tokens, 10 threads running simultaneously
    let mut distinct_tokens = Vec::new();
    for i in 0..10 {
        distinct_tokens.push(
            create_enrollment_token(
                &config,
                EnrollmentTokenRequest {
                    note: Some(format!("token-{}", i)),
                    ttl_seconds: Some(60.0),
                },
            )
            .unwrap()
            .token,
        );
    }

    let barrier_10 = Arc::new(Barrier::new(10));
    let mut handles_10 = Vec::new();

    for (i, token) in distinct_tokens.into_iter().enumerate() {
        let b = Arc::clone(&barrier_10);
        let cfg = config.clone();

        handles_10.push(thread::spawn(move || {
            b.wait();
            register_node(
                &cfg,
                RegisterRequest {
                    enrollment_token: token,
                    hostname: format!("concurrent-node-{:02}", i),
                    os: "darwin".to_string(),
                    arch: "aarch64".to_string(),
                    gpu: None,
                    ram_bytes: 16 * 1024 * 1024 * 1024,
                    cpu_cores: 8,
                    agent_version: "0.1.0".to_string(),
            max_concurrent_tasks: None,
                },
            )
        }));
    }

    let mut ok_10 = 0;
    for h in handles_10 {
        if h.join().unwrap().is_ok() {
            ok_10 += 1;
        }
    }
    assert_eq!(ok_10, 10, "All 10 distinct tokens must succeed concurrently without lock starvation");

    // Cluster should now have 1 (from first test) + 10 = 11 nodes
    let list = list_nodes(&config).unwrap();
    assert_eq!(list["cluster"]["total_nodes"], 11);
}

#[test]
fn test_adversarial_hostname_and_spec_fuzzing() {
    let (config, _dir) = temp_fixture_config();

    // 1. Extreme hostname with emojis, punctuation, Chinese characters, and 16KB length
    let huge_hostname = format!("🚀-主机名-测试_{}-!@#$%^&*()_+", "X".repeat(16384));
    let t1 = create_enrollment_token(&config, EnrollmentTokenRequest { note: None, ttl_seconds: None }).unwrap();
    let reg1 = register_node(
        &config,
        RegisterRequest {
            enrollment_token: t1.token,
            hostname: huge_hostname,
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            gpu: None,
            ram_bytes: 8 * 1024 * 1024 * 1024,
            cpu_cores: 4,
            agent_version: "0.1.0".to_string(),
            max_concurrent_tasks: None,
        },
    )
    .expect("Fuzzed hostname must be handled safely");

    // Ensure node_id was constructed and is queryable
    assert!(!reg1.node_id.is_empty());
    let list = list_nodes(&config).unwrap();
    let node1 = list["nodes"].as_array().unwrap().iter().find(|n| n["node_id"] == reg1.node_id).unwrap();
    assert_eq!(node1["status"], "registered");

    // 2. Negative numbers or zero for specs
    let t2 = create_enrollment_token(&config, EnrollmentTokenRequest { note: None, ttl_seconds: None }).unwrap();
    let reg2 = register_node(
        &config,
        RegisterRequest {
            enrollment_token: t2.token,
            hostname: "zero-spec-host".to_string(),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            gpu: None,
            ram_bytes: 0,
            cpu_cores: 0,
            agent_version: "0.1.0".to_string(),
            max_concurrent_tasks: None,
        },
    )
    .expect("Zero specs must not panic");
    assert!(!reg2.node_id.is_empty());
}

#[test]
fn test_adversarial_revoked_node_offline_and_command_delivery() {
    let (config, _dir) = temp_fixture_config();

    let t = create_enrollment_token(&config, EnrollmentTokenRequest { note: None, ttl_seconds: None }).unwrap();
    let reg = register_node(
        &config,
        RegisterRequest {
            enrollment_token: t.token,
            hostname: "pause-test-node".to_string(),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            gpu: None,
            ram_bytes: 16000000000,
            cpu_cores: 8,
            agent_version: "0.1.0".to_string(),
            max_concurrent_tasks: None,
        },
    )
    .unwrap();

    // 1. Send initial heartbeat to get online
    handle_heartbeat(
        &config,
        &reg.node_token,
        HeartbeatRequest {
            node_id: reg.node_id.clone(),
            instance_id: "inst-pause-1".to_string(),
            timestamp: agent_registry::now_sec(),
            uptime_s: None,
            free_ram_bytes: 8000000000,
            free_vram_bytes: 0,
            gpu_util_percent: 0.0,
            running_tasks: 0,
        },
    )
    .unwrap();

    // 2. Age node to 120s (offline)
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    let past = agent_registry::now_sec() - 120.0;
    conn.execute("UPDATE agent_nodes SET last_heartbeat_at = ? WHERE id = ?", rusqlite::params![past, reg.node_id]).unwrap();

    // 3. Admin revokes acceptance while node is offline
    let rev_res = control_node(
        &config,
        NodeControlRequest {
            node_id: reg.node_id.clone(),
            action: "revoke_acceptance".to_string(),
        },
    )
    .unwrap();

    // Must be offline AND accepting = false
    assert_eq!(rev_res.status, "offline");
    assert_eq!(rev_res.accepting, false);

    // 4. Node recovers and heartbeats
    let hb_rec = handle_heartbeat(
        &config,
        &reg.node_token,
        HeartbeatRequest {
            node_id: reg.node_id.clone(),
            instance_id: "inst-pause-2".to_string(),
            timestamp: agent_registry::now_sec(),
            uptime_s: None,
            free_ram_bytes: 8000000000,
            free_vram_bytes: 0,
            gpu_util_percent: 0.0,
            running_tasks: 0,
        },
    )
    .unwrap();

    // Node is now online, but MUST receive pause_acceptance command because accepting == 0!
    assert!(
        hb_rec.commands.iter().any(|c| c.command == "pause_acceptance"),
        "Revoked node must receive pause_acceptance command in heartbeat response"
    );

    let list = list_nodes(&config).unwrap();
    let n = list["nodes"].as_array().unwrap().iter().find(|n| n["node_id"] == reg.node_id).unwrap();
    assert_eq!(n["status"], "online");
    assert_eq!(n["accepting_tasks"], false);
}
