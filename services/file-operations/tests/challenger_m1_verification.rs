use face_file_operations::agent::probe::{probe_capabilities, probe_telemetry};
use face_file_operations::agent_registry::{
    self, control_node, create_enrollment_token, handle_heartbeat, list_nodes, register_node,
    EnrollmentTokenRequest, GpuSpecs, HeartbeatRequest, NodeControlRequest, RegisterRequest,
};
use face_file_operations::config::Config;
use face_file_operations::processing_scheduler;
use std::fs;
use std::path::PathBuf;

fn temp_fixture_config() -> (Config, PathBuf) {
    let tmp_dir = std::env::temp_dir().join(format!("face-challenger-{}", uuid::Uuid::new_v4()));
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

/// CHALLENGE 1: Empirical Probe Diagnosis — Demonstrating Bug in probe.rs on macOS
/// Verification Objective 2 states:
/// "Verify Metal GPU probe on macOS does not crash and correctly sets CoreMLExecutionProvider (never fake CUDA)"
///
/// EMPIRICAL FINDING:
/// probe.rs line 224 checks: `if os == "darwin"`.
/// However, in Rust, `std::env::consts::OS` returns `"macos"`, NOT `"darwin"`!
/// As a consequence, `probe_gpu()` fails to enter `probe_macos_metal`,
/// returning `None` for GPU and omitting `CoreMLExecutionProvider`.
#[test]
fn challenge_probe_macos_metal_empirical_diagnosis() {
    let caps = probe_capabilities();

    println!("Detected hostname: {}", caps.hostname);
    println!("Detected OS: {}, Arch: {}", caps.os, caps.arch);
    println!("Detected CPU cores: {}", caps.cpu_cores);
    println!("Detected RAM: {} bytes", caps.ram_bytes);
    println!("Detected Providers: {:?}", caps.execution_providers);

    assert!(caps.cpu_cores > 0, "CPU cores must be > 0");
    assert!(caps.ram_bytes > 0, "RAM bytes must be > 0");

    #[cfg(target_os = "macos")]
    {
        // 1. Rust OS constant on macOS is "macos", NOT "darwin"
        assert_eq!(caps.os, "macos", "In Rust, std::env::consts::OS is 'macos'");

        // 2. Proving that CUDA is never faked on macOS
        assert!(
            !caps.execution_providers.iter().any(|p| p.contains("CUDA")),
            "macOS must NEVER report CUDAExecutionProvider"
        );

        // 3. Verify Metal GPU & CoreMLExecutionProvider are registered on Apple Silicon
        assert!(
            caps.execution_providers.contains(&"CoreMLExecutionProvider".to_string()),
            "Apple Silicon macOS host must register CoreMLExecutionProvider in execution_providers, got {:?}",
            caps.execution_providers
        );
        assert!(
            caps.gpu.is_some(),
            "Apple Silicon macOS host must detect GPU capability"
        );
        let gpu = caps.gpu.as_ref().unwrap();
        assert!(
            gpu.is_unified_memory,
            "Apple Silicon GPU must be unified memory"
        );
        assert!(
            gpu.device_uuid.starts_with("METAL-"),
            "Metal GPU device_uuid must start with METAL-, got {}",
            gpu.device_uuid
        );
        assert!(
            gpu.vram_bytes > 0,
            "UMA VRAM must be > 0 bytes, got {}",
            gpu.vram_bytes
        );

        // 4. Verify system_profiler displays data is actually available and parses cleanly
        let output = std::process::Command::new("system_profiler")
            .args(["SPDisplaysDataType", "-json"])
            .output();
        assert!(output.is_ok(), "system_profiler must be callable on macOS");
        let out = output.unwrap();
        assert!(out.status.success(), "system_profiler must exit successfully");
        let json_val: serde_json::Value = serde_json::from_slice(&out.stdout).expect("Valid JSON from system_profiler");
        let displays = json_val.get("SPDisplaysDataType").and_then(|v| v.as_array());
        assert!(displays.is_some() && !displays.unwrap().is_empty(), "SPDisplaysDataType must contain displays on Apple Silicon");

        // 5. Test probe telemetry does not crash and handles None or UMA
        let telemetry = probe_telemetry(caps.gpu.as_ref());
        println!("Sampled telemetry: {:?}", telemetry);
        assert!(telemetry.free_ram_bytes > 0, "Free RAM must be sampled via vm_stat");
    }
}

/// CHALLENGE 2: Truth-in-UI Invariant — Missing & Offline Telemetry MUST be Null, NEVER 0
#[test]
fn challenge_truth_in_ui_metrics_never_zero_when_missing_or_offline() {
    let (config, _dir) = temp_fixture_config();

    // 1. Freshly registered node with no heartbeat
    let t1 = create_enrollment_token(&config, EnrollmentTokenRequest { note: None, ttl_seconds: None }).unwrap();
    let reg1 = register_node(
        &config,
        RegisterRequest {
            enrollment_token: t1.token,
            hostname: "uncontacted-agent".to_string(),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            gpu: Some(GpuSpecs {
                model: "RTX 4070 Ti".to_string(),
                vram_bytes: 12884901888,
                device_uuid: "GPU-TRUTH-1".to_string(),
            }),
            ram_bytes: 34359738368,
            cpu_cores: 16,
            agent_version: "0.1.0".to_string(),
            max_concurrent_tasks: None,
        },
    ).unwrap();

    let list = list_nodes(&config).unwrap();
    let node1 = list["nodes"].as_array().unwrap().iter().find(|n| n["node_id"] == reg1.node_id).unwrap();

    assert_eq!(node1["status"], "registered");
    // CRITICAL: telemetry fields must be JSON null, NEVER 0!
    assert!(node1["telemetry"]["last_heartbeat_at"].is_null(), "Uncontacted node last_heartbeat_at must be null");
    assert!(node1["telemetry"]["free_ram_bytes"].is_null(), "Uncontacted node free_ram_bytes must be null");
    assert!(node1["telemetry"]["gpu"]["free_vram_bytes"].is_null(), "Uncontacted node free_vram_bytes must be null");
    assert!(node1["telemetry"]["gpu"]["gpu_util_percent"].is_null(), "Uncontacted node gpu_util_percent must be null");

    // 2. Heartbeat makes it online with real values
    handle_heartbeat(
        &config,
        &reg1.node_token,
        HeartbeatRequest {
            node_id: reg1.node_id.clone(),
            instance_id: "inst-truth".to_string(),
            timestamp: agent_registry::now_sec(),
            uptime_s: None,
            free_ram_bytes: 25000000000,
            free_vram_bytes: 10000000000,
            gpu_util_percent: 42.5,
            running_tasks: 2,
        },
    ).unwrap();

    let list_online = list_nodes(&config).unwrap();
    let node1_online = list_online["nodes"].as_array().unwrap().iter().find(|n| n["node_id"] == reg1.node_id).unwrap();
    assert_eq!(node1_online["status"], "online");
    assert_eq!(node1_online["telemetry"]["free_ram_bytes"], 25000000000i64);
    assert_eq!(node1_online["telemetry"]["gpu"]["free_vram_bytes"], 10000000000i64);
    assert_eq!(node1_online["telemetry"]["gpu"]["gpu_util_percent"], 42.5);

    // 3. Age node past 90 seconds (offline threshold)
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    let past = agent_registry::now_sec() - 95.0;
    conn.execute(
        "UPDATE agent_nodes SET last_heartbeat_at = ? WHERE id = ?",
        rusqlite::params![past, reg1.node_id],
    ).unwrap();

    let list_offline = list_nodes(&config).unwrap();
    let node1_offline = list_offline["nodes"].as_array().unwrap().iter().find(|n| n["node_id"] == reg1.node_id).unwrap();

    assert_eq!(node1_offline["status"], "offline", "Node aged >90s must be offline");
    // Telemetry MUST be wiped to null to guarantee truth-in-UI ("未知", NEVER 0)
    assert!(node1_offline["telemetry"]["free_ram_bytes"].is_null(), "Offline node free_ram_bytes must be null");
    assert!(node1_offline["telemetry"]["gpu"]["free_vram_bytes"].is_null(), "Offline node free_vram_bytes must be null");
    assert!(node1_offline["telemetry"]["gpu"]["gpu_util_percent"].is_null(), "Offline node gpu_util_percent must be null");
}

/// CHALLENGE 3: Node Control Boundary & Error Cases
#[test]
fn challenge_node_control_boundaries_and_error_cases() {
    let (config, _dir) = temp_fixture_config();

    // 1. Control on non-existent node returns 404
    let err_not_found = control_node(
        &config,
        NodeControlRequest {
            node_id: "node-nonexistent-999".to_string(),
            action: "revoke_acceptance".to_string(),
        },
    ).expect_err("Control on non-existent node must return error");
    assert_eq!(err_not_found.status, 404);

    // 2. Control with invalid action returns 400
    let t = create_enrollment_token(&config, EnrollmentTokenRequest { note: None, ttl_seconds: None }).unwrap();
    let reg = register_node(
        &config,
        RegisterRequest {
            enrollment_token: t.token,
            hostname: "boundary-host".to_string(),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            gpu: None,
            ram_bytes: 8000000000,
            cpu_cores: 4,
            agent_version: "0.1.0".to_string(),
            max_concurrent_tasks: None,
        },
    ).unwrap();

    let err_invalid_action = control_node(
        &config,
        NodeControlRequest {
            node_id: reg.node_id.clone(),
            action: "unsupported_action_hack".to_string(),
        },
    ).expect_err("Control with invalid action must return 400");
    assert_eq!(err_invalid_action.status, 400);

    // 3. Valid actions: revoke_acceptance, drain, restore_acceptance
    let res_drain = control_node(
        &config,
        NodeControlRequest {
            node_id: reg.node_id.clone(),
            action: "drain".to_string(),
        },
    ).expect("drain action must succeed");
    assert!(!res_drain.accepting);

    let res_restore = control_node(
        &config,
        NodeControlRequest {
            node_id: reg.node_id.clone(),
            action: "restore_acceptance".to_string(),
        },
    ).expect("restore_acceptance action must succeed");
    assert!(res_restore.accepting);
}

/// CHALLENGE 4: Physical Device Pooling Isolation & Capacity Grouping
#[test]
fn challenge_physical_device_pooling_isolation() {
    let (config, _dir) = temp_fixture_config();

    // Group 1: 3 agents sharing GPU-ALPHA
    let gpu_alpha = "GPU-PHYSICAL-ALPHA-12345678";
    for i in 1..=3 {
        let t = create_enrollment_token(&config, EnrollmentTokenRequest { note: None, ttl_seconds: None }).unwrap();
        let reg = register_node(
            &config,
            RegisterRequest {
                enrollment_token: t.token,
                hostname: format!("host-alpha-agent{}", i),
                os: "linux".to_string(),
                arch: "x86_64".to_string(),
                gpu: Some(GpuSpecs {
                    model: "NVIDIA RTX 4070 Ti".to_string(),
                    vram_bytes: 12884901888,
                    device_uuid: gpu_alpha.to_string(),
                }),
                ram_bytes: 34359738368,
                cpu_cores: 16,
                agent_version: "0.1.0".to_string(),
                max_concurrent_tasks: None,
            },
        ).unwrap();
        assert!(reg.device_group_id.starts_with("grp-gpu-"));
    }

    // Group 2: 2 agents sharing GPU-BETA
    let gpu_beta = "GPU-PHYSICAL-BETA-87654321";
    for i in 1..=2 {
        let t = create_enrollment_token(&config, EnrollmentTokenRequest { note: None, ttl_seconds: None }).unwrap();
        let reg = register_node(
            &config,
            RegisterRequest {
                enrollment_token: t.token,
                hostname: format!("host-beta-agent{}", i),
                os: "linux".to_string(),
                arch: "x86_64".to_string(),
                gpu: Some(GpuSpecs {
                    model: "NVIDIA RTX 4090".to_string(),
                    vram_bytes: 25769803776,
                    device_uuid: gpu_beta.to_string(),
                }),
                ram_bytes: 68719476736,
                cpu_cores: 32,
                agent_version: "0.1.0".to_string(),
                max_concurrent_tasks: None,
            },
        ).unwrap();
        assert!(reg.device_group_id.starts_with("grp-gpu-"));
    }

    // Group 3: 1 CPU-only Mac
    let t_mac = create_enrollment_token(&config, EnrollmentTokenRequest { note: None, ttl_seconds: None }).unwrap();
    let reg_mac = register_node(
        &config,
        RegisterRequest {
            enrollment_token: t_mac.token,
            hostname: "mac-studio-cpu".to_string(),
            os: "darwin".to_string(),
            arch: "aarch64".to_string(),
            gpu: None,
            ram_bytes: 68719476736,
            cpu_cores: 12,
            agent_version: "0.1.0".to_string(),
            max_concurrent_tasks: None,
        },
    ).unwrap();
    assert!(reg_mac.device_group_id.starts_with("grp-host-"));

    // Verify cluster aggregation
    let list = list_nodes(&config).unwrap();
    assert_eq!(list["cluster"]["total_nodes"], 6, "Total agents must be 6");
    assert_eq!(list["cluster"]["device_groups_count"], 3, "Total physical device groups must be 3");
}
