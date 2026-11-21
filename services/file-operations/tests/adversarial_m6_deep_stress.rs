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
    claim_task, commit_task_result, get_coordinator_generation, init_tables, list_agent_tasks,
    now_sec, sha256_hex, ClaimTaskRequest, CommitTaskResultRequest, ListTasksQuery,
};

fn create_temp_env() -> (Config, PathBuf) {
    let tmp_dir = std::env::temp_dir().join(format!("face-m6-adv-stress-{}", uuid::Uuid::new_v4()));
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
            ?1, ?2, 'inst-1', ?3, 'linux', 'x86_64', '0.1.0',
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
         VALUES (?1, ?2, ?3, 1048576, 60.0, '0', ?4, ?5, 'batch-adv-m6', ?6, ?6)",
        params![vid, format!("/tmp/{}.mp4", vid), format!("{}.mp4", vid), stage, priority, now],
    ).unwrap();
}

// =========================================================================
// TEST 1: 16-THREAD CLAIM RACES UNDER CONCURRENCY CEILING = 4
// =========================================================================

#[test]
fn challenge_16_thread_claim_race_and_strict_concurrency_ceiling() {
    let (config, _tmp) = create_temp_env();
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    init_tables(&conn).unwrap();

    // 1. Configure concurrency = 4
    conn.execute("UPDATE settings SET value = '4' WHERE key = 'concurrency'", []).unwrap();

    // 2. Seed 25 tasks
    for i in 0..25 {
        seed_task(&config, &format!("vid-adv-race-{}", i), "detect", i as i64);
    }

    // 3. Register 16 distinct nodes (each with own GPU UUID so GPU pool does not gate)
    let mut tokens = Vec::new();
    for i in 0..16 {
        let node_id = format!("node-race-{}", i);
        let gpu_uuid = format!("GPU-RACE-UUID-{}", i);
        let grp_id = format!("grp-gpu-{}", i);
        let tok = enroll_custom_node(
            &config,
            &node_id,
            &format!("host-{}", i),
            Some(&gpu_uuid),
            Some(&grp_id),
            4,
            8,
            8 * 1024 * 1024 * 1024,
        );
        tokens.push((node_id, tok));
    }

    // 4. Concurrently launch 16 threads releasing at the exact same millisecond via Barrier
    let barrier = Arc::new(Barrier::new(16));
    let claims = Arc::new(Mutex::new(Vec::new()));
    let busy_errors = Arc::new(Mutex::new(Vec::new()));
    let mut handles = Vec::new();

    for (node_id, tok) in tokens {
        let b = Arc::clone(&barrier);
        let c = Arc::clone(&claims);
        let errs = Arc::clone(&busy_errors);
        let cfg = config.clone();

        handles.push(thread::spawn(move || {
            b.wait(); // synchronize start
            let res = claim_task(
                &cfg,
                &tok,
                ClaimTaskRequest {
                    node_id: Some(node_id),
                    stage: "detect".to_string(),
                    lease_duration_s: Some(60.0),
                },
            );
            match res {
                Ok(Some(claim)) => {
                    c.lock().unwrap().push(claim);
                }
                Ok(None) => {}
                Err(e) => {
                    errs.lock().unwrap().push(e);
                }
            }
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    let claimed = claims.lock().unwrap();
    let errors = busy_errors.lock().unwrap();

    // Invariant A: Granted leases MUST BE EXACTLY 4 (strictly bounded by concurrency ceiling = 4)
    assert_eq!(claimed.len(), 4, "Granted leases {} exceeded concurrency limit 4", claimed.len());

    // Invariant B: Exactly 12 threads must have received 503 cluster_busy error
    assert_eq!(errors.len(), 12, "Expected 12 cluster_busy rejections, got {}", errors.len());
    for err in errors.iter() {
        assert_eq!(err.status, 503);
        assert_eq!(err.code, "cluster_busy");
    }

    // Invariant C: All claimed VIDs are strictly disjoint (zero duplicate tasks leased)
    let mut vids: Vec<String> = claimed.iter().map(|c| c.vid.clone()).collect();
    let original_vid_count = vids.len();
    vids.sort();
    vids.dedup();
    assert_eq!(vids.len(), original_vid_count, "Duplicate task leased to multiple nodes!");

    // Invariant D: Unique attempt_ids and lease_ids
    let mut attempt_ids: Vec<String> = claimed.iter().map(|c| c.attempt_id.clone()).collect();
    let attempt_count = attempt_ids.len();
    attempt_ids.sort();
    attempt_ids.dedup();
    assert_eq!(attempt_ids.len(), attempt_count, "Duplicate attempt_ids generated!");

    // Invariant E: Center DB confirms active unexpired tasks count is exactly 4
    let now = now_sec();
    let active_in_db: i64 = conn.query_row(
        "SELECT COUNT(*) FROM tasks WHERE lease_expires_at > ?",
        params![now],
        |r| r.get(0),
    ).unwrap();
    assert_eq!(active_in_db, 4, "Database active unexpired task count mismatch");
}

// =========================================================================
// TEST 2: PHYSICAL GPU POOL MUTUAL EXCLUSION & ISOLATION
// =========================================================================

#[test]
fn challenge_physical_gpu_pool_mutual_exclusion_and_isolation() {
    let (config, _tmp) = create_temp_env();
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    init_tables(&conn).unwrap();

    // Allow high global concurrency = 10 so global quota does not interfere
    conn.execute("UPDATE settings SET value = '10' WHERE key = 'concurrency'", []).unwrap();

    // Seed detect tasks and an ocr task
    for i in 0..5 {
        seed_task(&config, &format!("vid-gpu-{}", i), "detect", 10 - i as i64);
    }
    seed_task(&config, "vid-cpu-ocr", "ocr", 20);

    // Register Node 1 and Node 2 sharing the SAME physical GPU group
    let shared_group = "grp-gpu-rtx4070ti-phys1";
    let shared_uuid = "GPU-PHYSICAL-UUID-SHARED-1";

    let tok1 = enroll_custom_node(
        &config,
        "node-wsl-host",
        "wsl-desktop",
        Some(shared_uuid),
        Some(shared_group),
        4,
        8,
        8 * 1024 * 1024 * 1024,
    );

    let tok2 = enroll_custom_node(
        &config,
        "node-docker-container",
        "docker-runner",
        Some(shared_uuid),
        Some(shared_group),
        4,
        8,
        8 * 1024 * 1024 * 1024,
    );

    // Register Node 3 (CPU only node on another host)
    let tok3 = enroll_custom_node(
        &config,
        "node-mac-m2",
        "mac-mini",
        None,
        Some("grp-host-mac"),
        4,
        8,
        16 * 1024 * 1024 * 1024,
    );

    // Node 1 claims first detect task -> must succeed 200 OK
    let claim1 = claim_task(
        &config,
        &tok1,
        ClaimTaskRequest {
            node_id: Some("node-wsl-host".to_string()),
            stage: "detect".to_string(),
            lease_duration_s: Some(60.0),
        },
    ).unwrap().expect("Node 1 gets detect task");

    // Node 2 attempts to claim detect task concurrently -> must fail with 503 gpu_busy!
    let err2 = claim_task(
        &config,
        &tok2,
        ClaimTaskRequest {
            node_id: Some("node-docker-container".to_string()),
            stage: "detect".to_string(),
            lease_duration_s: Some(60.0),
        },
    ).expect_err("Node 2 must be blocked by physical GPU pool");

    assert_eq!(err2.status, 503);
    assert_eq!(err2.code, "gpu_busy");
    assert!(err2.error.contains("物理 GPU"));

    // CPU node claims OCR task -> must SUCCEED (CPU task is completely unblocked by GPU pool)
    let claim3 = claim_task(
        &config,
        &tok3,
        ClaimTaskRequest {
            node_id: Some("node-mac-m2".to_string()),
            stage: "ocr".to_string(),
            lease_duration_s: Some(60.0),
        },
    ).unwrap().expect("CPU node gets OCR task despite GPU pool being saturated");
    assert_eq!(claim3.vid, "vid-cpu-ocr");

    // Node 1 completes detect task
    let coord_gen = get_coordinator_generation(&config).unwrap();
    let commit_res = commit_task_result(
        &config,
        &tok1,
        CommitTaskResultRequest {
            node_id: Some("node-wsl-host".to_string()),
            attempt_id: Some(claim1.attempt_id.clone()),
            vid: Some(claim1.vid.clone()),
            coordinator_generation: Some(json!(coord_gen)),
            stage: Some("detect".to_string()),
            status: Some("success".to_string()),
            detections: Some(vec![]), // no faces -> done
            error_type: None,
            error_message: None,
            text_snippets: None,
            vectors: None,
            execution_proof: None,
        },
    ).unwrap();
    assert_eq!(commit_res.ok, true);

    // Now Node 2 attempts to claim again -> must SUCCEED immediately!
    let claim2 = claim_task(
        &config,
        &tok2,
        ClaimTaskRequest {
            node_id: Some("node-docker-container".to_string()),
            stage: "detect".to_string(),
            lease_duration_s: Some(60.0),
        },
    ).unwrap().expect("Node 2 claims detect task after Node 1 freed GPU pool");

    assert!(claim2.attempt_id.contains("node-docker-container"));

    // Now Node 1 is blocked because Node 2 holds the GPU pool
    let err1_again = claim_task(
        &config,
        &tok1,
        ClaimTaskRequest {
            node_id: Some("node-wsl-host".to_string()),
            stage: "detect".to_string(),
            lease_duration_s: Some(60.0),
        },
    ).expect_err("Node 1 must now be blocked while Node 2 holds GPU pool");
    assert_eq!(err1_again.status, 503);
    assert_eq!(err1_again.code, "gpu_busy");
}

// =========================================================================
// TEST 3: FALLBACK GPU UUID WHEN DEVICE GROUP ID IS EMPTY
// =========================================================================

#[test]
fn challenge_gpu_uuid_fallback_when_device_group_empty() {
    let (config, _tmp) = create_temp_env();
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    init_tables(&conn).unwrap();

    conn.execute("UPDATE settings SET value = '10' WHERE key = 'concurrency'", []).unwrap();
    seed_task(&config, "vid-fallback-1", "detect", 10);
    seed_task(&config, "vid-fallback-2", "detect", 5);

    // Register two nodes with EMPTY device_group_id, but IDENTICAL gpu_device_uuid
    let shared_uuid = "FALLBACK-GPU-UUID-SHARED-99";
    let tok_a = enroll_custom_node(
        &config,
        "node-fb-a",
        "fb-host-a",
        Some(shared_uuid),
        Some(""), // empty group
        4,
        8,
        8 * 1024 * 1024 * 1024,
    );
    let tok_b = enroll_custom_node(
        &config,
        "node-fb-b",
        "fb-host-b",
        Some(shared_uuid),
        Some(""), // empty group
        4,
        8,
        8 * 1024 * 1024 * 1024,
    );

    // Node A claims
    let _claim_a = claim_task(
        &config,
        &tok_a,
        ClaimTaskRequest {
            node_id: Some("node-fb-a".to_string()),
            stage: "detect".to_string(),
            lease_duration_s: Some(60.0),
        },
    ).unwrap().expect("Node A claims task");

    // Node B claims -> must be blocked by fallback gpu_device_uuid check
    let err_b = claim_task(
        &config,
        &tok_b,
        ClaimTaskRequest {
            node_id: Some("node-fb-b".to_string()),
            stage: "detect".to_string(),
            lease_duration_s: Some(60.0),
        },
    ).expect_err("Node B must be blocked by gpu_device_uuid pool");

    assert_eq!(err_b.status, 503);
    assert_eq!(err_b.code, "gpu_busy");
}

// =========================================================================
// TEST 4: DYNAMIC CONCURRENCY REDUCTION & EMERGENCY ZERO DRAIN
// =========================================================================

#[test]
fn challenge_dynamic_concurrency_reduction_and_emergency_zero_drain() {
    let (config, _tmp) = create_temp_env();
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    init_tables(&conn).unwrap();

    // 1. Initial concurrency = 6
    conn.execute("UPDATE settings SET value = '6' WHERE key = 'concurrency'", []).unwrap();

    // Register 6 distinct GPU nodes
    let mut node_tokens = Vec::new();
    for i in 0..6 {
        let nid = format!("node-drain-{}", i);
        let gpu_id = format!("GPU-DRAIN-UUID-{}", i);
        let grp_id = format!("grp-gpu-drain-{}", i);
        let tok = enroll_custom_node(
            &config,
            &nid,
            &format!("drain-host-{}", i),
            Some(&gpu_id),
            Some(&grp_id),
            4,
            8,
            8 * 1024 * 1024 * 1024,
        );
        node_tokens.push((nid, tok));
    }

    // Seed 10 detect tasks
    for i in 0..10 {
        seed_task(&config, &format!("vid-drain-flow-{}", i), "detect", 10 - i as i64);
    }

    // Claim 6 tasks into in-flight state
    let mut in_flight_claims = Vec::new();
    for i in 0..6 {
        let (nid, tok) = &node_tokens[i];
        let claim = claim_task(
            &config,
            tok,
            ClaimTaskRequest {
                node_id: Some(nid.clone()),
                stage: "detect".to_string(),
                lease_duration_s: Some(120.0),
            },
        ).unwrap().expect("Initial 6 claims succeed");
        in_flight_claims.push((nid.clone(), tok.clone(), claim));
    }

    let gen_before = get_coordinator_generation(&config).unwrap();

    // Operator throttles concurrency down from 6 to 2
    let sched_res = processing_scheduler::set_concurrency(&config, &json!({"concurrency": 2})).unwrap();
    assert_eq!(sched_res["effective"], 2);

    // Generation must NOT be modified by concurrency change
    let gen_after = get_coordinator_generation(&config).unwrap();
    assert_eq!(gen_before, gen_after);

    // New claim attempt must be immediately rejected with 503 cluster_busy (active 6 >= 2)
    let err_gated = claim_task(
        &config,
        &node_tokens[0].1,
        ClaimTaskRequest {
            node_id: Some(node_tokens[0].0.clone()),
            stage: "detect".to_string(),
            lease_duration_s: Some(60.0),
        },
    ).expect_err("New claims must be gated when active > concurrency");
    assert_eq!(err_gated.status, 503);
    assert_eq!(err_gated.code, "cluster_busy");

    // In-flight task 0 commits cleanly with 200 OK
    let (nid0, tok0, c0) = &in_flight_claims[0];
    let commit0 = commit_task_result(
        &config,
        tok0,
        CommitTaskResultRequest {
            node_id: Some(nid0.clone()),
            attempt_id: Some(c0.attempt_id.clone()),
            vid: Some(c0.vid.clone()),
            coordinator_generation: Some(json!(gen_before)),
            stage: Some("detect".to_string()),
            status: Some("success".to_string()),
            detections: Some(vec![]),
            error_type: None,
            error_message: None,
            text_snippets: None,
            vectors: None,
            execution_proof: None,
        },
    ).unwrap();
    assert_eq!(commit0.ok, true);

    // Active count is now 5. Claim must still be rejected (5 >= 2).
    let err_still_gated = claim_task(
        &config,
        tok0,
        ClaimTaskRequest {
            node_id: Some(nid0.clone()),
            stage: "detect".to_string(),
            lease_duration_s: Some(60.0),
        },
    ).expect_err("Still gated at 5 active");
    assert_eq!(err_still_gated.status, 503);

    // Commit tasks 1, 2, 3, 4 -> active count drops to 1 (< 2)!
    for i in 1..=4 {
        let (nid, tok, c) = &in_flight_claims[i];
        let com = commit_task_result(
            &config,
            tok,
            CommitTaskResultRequest {
                node_id: Some(nid.clone()),
                attempt_id: Some(c.attempt_id.clone()),
                vid: Some(c.vid.clone()),
                coordinator_generation: Some(json!(gen_before)),
                stage: Some("detect".to_string()),
                status: Some("success".to_string()),
                detections: Some(vec![]),
                error_type: None,
                error_message: None,
                text_snippets: None,
                vectors: None,
                execution_proof: None,
            },
        ).unwrap();
        assert_eq!(com.ok, true);
    }

    // Now active count is 1 (< 2). Node 0 can claim again!
    let claim_unblocked = claim_task(
        &config,
        tok0,
        ClaimTaskRequest {
            node_id: Some(nid0.clone()),
            stage: "detect".to_string(),
            lease_duration_s: Some(60.0),
        },
    ).unwrap().expect("Claim succeeds once active tasks fall below 2");

    assert!(claim_unblocked.attempt_id.contains(nid0.as_str()));

    // Emergency Drain: Concurrency set to 0!
    let drain_res = processing_scheduler::set_concurrency(&config, &json!({"concurrency": 0})).unwrap();
    assert_eq!(drain_res["effective"], 0);

    // Even if active count falls to 0, concurrency = 0 MUST gate ALL new claims with 503 cluster_busy!
    let (nid5, tok5, c5) = &in_flight_claims[5];
    commit_task_result(
        &config,
        tok5,
        CommitTaskResultRequest {
            node_id: Some(nid5.clone()),
            attempt_id: Some(c5.attempt_id.clone()),
            vid: Some(c5.vid.clone()),
            coordinator_generation: Some(json!(gen_before)),
            stage: Some("detect".to_string()),
            status: Some("success".to_string()),
            detections: Some(vec![]),
            error_type: None,
            error_message: None,
            text_snippets: None,
            vectors: None,
            execution_proof: None,
        },
    ).unwrap();

    let err_zero = claim_task(
        &config,
        tok0,
        ClaimTaskRequest {
            node_id: Some(nid0.clone()),
            stage: "detect".to_string(),
            lease_duration_s: Some(60.0),
        },
    ).expect_err("Concurrency 0 must reject all claims");
    assert_eq!(err_zero.status, 503);
    assert_eq!(err_zero.code, "cluster_busy");
}

// =========================================================================
// TEST 5: TIER 2 & TIER 4 HOST QUOTAS (SLOTS, CORES, RAM FLOOR)
// =========================================================================

#[test]
fn challenge_tier2_and_tier4_host_quotas() {
    let (config, _tmp) = create_temp_env();
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    init_tables(&conn).unwrap();

    conn.execute("UPDATE settings SET value = '10' WHERE key = 'concurrency'", []).unwrap();
    for i in 0..10 {
        seed_task(&config, &format!("vid-host-slot-{}", i), "ocr", 10 - i as i64);
    }

    // 1. Node with max_concurrent_tasks = 2 and cpu_cores = 8
    let tok_slots = enroll_custom_node(
        &config,
        "node-slots-2",
        "slot-host",
        None,
        Some("grp-host-slots"),
        2, // max_concurrent_tasks = 2
        8, // cpu_cores = 8
        8 * 1024 * 1024 * 1024,
    );

    let _c1 = claim_task(
        &config,
        &tok_slots,
        ClaimTaskRequest {
            node_id: Some("node-slots-2".to_string()),
            stage: "ocr".to_string(),
            lease_duration_s: Some(60.0),
        },
    ).unwrap().expect("Claim 1 succeeds");

    let _c2 = claim_task(
        &config,
        &tok_slots,
        ClaimTaskRequest {
            node_id: Some("node-slots-2".to_string()),
            stage: "ocr".to_string(),
            lease_duration_s: Some(60.0),
        },
    ).unwrap().expect("Claim 2 succeeds");

    // Claim 3 on the same node must fail with 503 node_busy
    let err_slots = claim_task(
        &config,
        &tok_slots,
        ClaimTaskRequest {
            node_id: Some("node-slots-2".to_string()),
            stage: "ocr".to_string(),
            lease_duration_s: Some(60.0),
        },
    ).expect_err("3rd claim on 2-slot node must fail");

    assert_eq!(err_slots.status, 503);
    assert_eq!(err_slots.code, "node_busy");
    assert!(err_slots.error.contains("节点任务配额上限"));

    // 2. RAM floor limit: free_ram_bytes < 256 MiB
    let tok_low_ram = enroll_custom_node(
        &config,
        "node-low-ram",
        "low-ram-host",
        None,
        Some("grp-host-ram"),
        4,
        8,
        128 * 1024 * 1024, // 128 MiB free RAM (< 256 MiB)
    );

    let err_ram = claim_task(
        &config,
        &tok_low_ram,
        ClaimTaskRequest {
            node_id: Some("node-low-ram".to_string()),
            stage: "ocr".to_string(),
            lease_duration_s: Some(60.0),
        },
    ).expect_err("Node with low RAM must fail claim");

    assert_eq!(err_ram.status, 503);
    assert_eq!(err_ram.code, "node_busy");
    assert!(err_ram.error.contains("256 MiB"));
}

// =========================================================================
// TEST 6: DYNAMIC WAITING REASON DERIVATION & TRUTH-IN-UI FILTERING
// =========================================================================

#[test]
fn challenge_dynamic_waiting_reasons_and_truth_in_ui() {
    let (config, _tmp) = create_temp_env();
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    init_tables(&conn).unwrap();

    let now = now_sec();

    // 1. Paused task: manual = 1
    seed_task(&config, "vid-t-paused", "detect", 1);
    conn.execute("UPDATE tasks SET manual = 1 WHERE vid = 'vid-t-paused'", []).unwrap();

    // 2. Rate limited task: retry with future backoff
    seed_task(&config, "vid-t-retry", "retry", 1);
    conn.execute("UPDATE tasks SET next_retry = ?1 WHERE vid = 'vid-t-retry'", params![now + 120.0]).unwrap();

    // 3. Quota exhausted: set concurrency = 1, seed 2 tasks, claim 1 task
    conn.execute("UPDATE settings SET value = '1' WHERE key = 'concurrency'", []).unwrap();
    let tok_norm = enroll_custom_node(
        &config,
        "node-truth-1",
        "host-truth",
        Some("GPU-TRUTH-1"),
        Some("grp-gpu-truth"),
        4,
        8,
        8 * 1024 * 1024 * 1024,
    );
    seed_task(&config, "vid-t-running", "detect", 10);
    seed_task(&config, "vid-t-quota-wait", "detect", 5);

    let claim = claim_task(
        &config,
        &tok_norm,
        ClaimTaskRequest {
            node_id: Some("node-truth-1".to_string()),
            stage: "detect".to_string(),
            lease_duration_s: Some(60.0),
        },
    ).unwrap().expect("Claim running task");

    // 4. GPU busy task: explicit note (under ample quota so active_tasks < concurrency)
    let (config_gpu, _tmp_gpu) = create_temp_env();
    let conn_gpu = processing_scheduler::open_or_init(&config_gpu).unwrap();
    init_tables(&conn_gpu).unwrap();
    conn_gpu.execute("UPDATE settings SET value = '10' WHERE key = 'concurrency'", []).unwrap();
    seed_task(&config_gpu, "vid-t-gpubusy", "detect", 1);
    conn_gpu.execute("UPDATE tasks SET note = 'waiting_for_vram' WHERE vid = 'vid-t-gpubusy'", []).unwrap();

    let res_gpu = list_agent_tasks(&config_gpu, ListTasksQuery::default()).unwrap();
    let tasks_arr_gpu = res_gpu["tasks"].as_array().expect("Tasks array");
    let t_gpubusy = tasks_arr_gpu.iter().find(|t| t["vid"] == "vid-t-gpubusy").unwrap();
    assert_eq!(t_gpubusy["waiting_reason"], "gpu_busy");

    // Query all tasks
    let res = list_agent_tasks(&config, ListTasksQuery::default()).unwrap();
    let tasks_arr = res["tasks"].as_array().expect("Tasks array");

    let t_paused = tasks_arr.iter().find(|t| t["vid"] == "vid-t-paused").unwrap();
    assert_eq!(t_paused["waiting_reason"], "paused");

    let t_retry = tasks_arr.iter().find(|t| t["vid"] == "vid-t-retry").unwrap();
    assert_eq!(t_retry["waiting_reason"], "rate_limited");

    let t_running = tasks_arr.iter().find(|t| t["vid"] == "vid-t-running").unwrap();
    assert_eq!(t_running["waiting_reason"], "none");
    assert_eq!(t_running["assigned_node_id"], "node-truth-1");
    assert_eq!(t_running["attempt_id"], claim.attempt_id);

    let t_quota = tasks_arr.iter().find(|t| t["vid"] == "vid-t-quota-wait").unwrap();
    assert_eq!(t_quota["waiting_reason"], "quota_exhausted");

    // Filter by node_id
    let res_node = list_agent_tasks(&config, ListTasksQuery {
        node_id: Some("node-truth-1".to_string()),
        ..Default::default()
    }).unwrap();
    assert_eq!(res_node["total"], 1);
    assert_eq!(res_node["tasks"][0]["vid"], "vid-t-running");

    // Filter by waiting_reason = "paused"
    let res_paused = list_agent_tasks(&config, ListTasksQuery {
        waiting_reason: Some("paused".to_string()),
        ..Default::default()
    }).unwrap();
    assert_eq!(res_paused["total"], 1);
    assert_eq!(res_paused["tasks"][0]["vid"], "vid-t-paused");

    // Filter by waiting_reason = "rate_limited"
    let res_rl = list_agent_tasks(&config, ListTasksQuery {
        waiting_reason: Some("rate_limited".to_string()),
        ..Default::default()
    }).unwrap();
    assert_eq!(res_rl["total"], 1);
    assert_eq!(res_rl["tasks"][0]["vid"], "vid-t-retry");

    // Verify Truth-in-UI: video_path is populated
    assert_eq!(t_paused["video_path"], "/tmp/vid-t-paused.mp4");
}

// =========================================================================
// TEST 7: EMPIRICAL DISCREPANCY DISCOVERY: WAITING REASON PRECEDENCE & GPU NODES
// =========================================================================

#[test]
fn challenge_discrepancy_waiting_reason_precedence_and_gpu_nodes() {
    let (config, _tmp) = create_temp_env();
    let conn = processing_scheduler::open_or_init(&config).unwrap();
    init_tables(&conn).unwrap();

    let _now = now_sec();

    // Condition 1: Zero GPU nodes registered. Concurrency = 4, active_tasks = 0.
    conn.execute("UPDATE settings SET value = '4' WHERE key = 'concurrency'", []).unwrap();
    seed_task(&config, "vid-disc-zero-gpu", "detect", 1);

    let res1 = list_agent_tasks(&config, ListTasksQuery::default()).unwrap();
    let tasks1 = res1["tasks"].as_array().expect("Tasks array");
    let task_zero_gpu = tasks1.iter().find(|t| t["vid"] == "vid-disc-zero-gpu").unwrap();
    let wr_zero_gpu = task_zero_gpu["waiting_reason"].as_str().unwrap();

    // Verify SQL filter matching for waiting_for_node under zero GPU nodes
    let res_filter_node1 = list_agent_tasks(&config, ListTasksQuery {
        waiting_reason: Some("waiting_for_node".to_string()),
        ..Default::default()
    }).unwrap();
    let tasks_node1 = res_filter_node1["tasks"].as_array().expect("Tasks array");
    assert!(tasks_node1.iter().any(|t| t["vid"] == "vid-disc-zero-gpu"), "vid-disc-zero-gpu must appear under waiting_for_node when 0 GPU nodes");

    // Condition 2: Cluster saturated (concurrency = 1, 1 active task).
    // Task with explicit note = 'waiting_for_vram'.
    let tok = enroll_custom_node(
        &config,
        "node-disc-1",
        "host-disc",
        Some("GPU-DISC-1"),
        Some("grp-gpu-disc"),
        4,
        8,
        8 * 1024 * 1024 * 1024,
    );
    seed_task(&config, "vid-disc-running", "detect", 10);
    seed_task(&config, "vid-disc-vram", "detect", 5);
    conn.execute("UPDATE tasks SET note = 'waiting_for_vram' WHERE vid = 'vid-disc-vram'", []).unwrap();
    conn.execute("UPDATE settings SET value = '1' WHERE key = 'concurrency'", []).unwrap();

    let _claim = claim_task(
        &config,
        &tok,
        ClaimTaskRequest {
            node_id: Some("node-disc-1".to_string()),
            stage: "detect".to_string(),
            lease_duration_s: Some(60.0),
        },
    ).unwrap().expect("Claim running task");

    let res2 = list_agent_tasks(&config, ListTasksQuery::default()).unwrap();
    let tasks2 = res2["tasks"].as_array().expect("Tasks array");
    let task_vram = tasks2.iter().find(|t| t["vid"] == "vid-disc-vram").unwrap();
    let wr_vram = task_vram["waiting_reason"].as_str().unwrap();

    eprintln!("[Empirical Evidence] zero GPU nodes detect waiting_reason: '{}'", wr_zero_gpu);
    eprintln!("[Empirical Evidence] saturated cluster with waiting_for_vram note waiting_reason: '{}'", wr_vram);

    // Verified fixed waiting reason assertions matching Python harness:
    assert_eq!(wr_zero_gpu, "waiting_for_node", "Zero GPU nodes with unleased detect task must derive 'waiting_for_node'");
    assert_eq!(wr_vram, "gpu_busy", "Saturated cluster with waiting_for_vram note must derive 'gpu_busy' over quota");

    // Verify SQL filter matching for gpu_busy
    let res_filter_gpu = list_agent_tasks(&config, ListTasksQuery {
        waiting_reason: Some("gpu_busy".to_string()),
        ..Default::default()
    }).unwrap();
    let tasks_gpu = res_filter_gpu["tasks"].as_array().expect("Tasks array");
    assert!(tasks_gpu.iter().any(|t| t["vid"] == "vid-disc-vram"), "vid-disc-vram must appear under gpu_busy");
    assert!(!tasks_gpu.iter().any(|t| t["vid"] == "vid-disc-zero-gpu"), "vid-disc-zero-gpu must NOT appear under gpu_busy");

    // Verify SQL filter matching for quota_exhausted
    let res_filter_quota = list_agent_tasks(&config, ListTasksQuery {
        waiting_reason: Some("quota_exhausted".to_string()),
        ..Default::default()
    }).unwrap();
    let tasks_quota = res_filter_quota["tasks"].as_array().expect("Tasks array");
    assert!(tasks_quota.iter().any(|t| t["vid"] == "vid-disc-zero-gpu"), "vid-disc-zero-gpu must appear under quota_exhausted when GPU node is online");
    assert!(!tasks_quota.iter().any(|t| t["vid"] == "vid-disc-vram"), "vid-disc-vram must NOT appear under quota_exhausted");

    // Verify SQL filter matching for waiting_for_node under saturated cluster with GPU node online
    let res_filter_node2 = list_agent_tasks(&config, ListTasksQuery {
        waiting_reason: Some("waiting_for_node".to_string()),
        ..Default::default()
    }).unwrap();
    let tasks_node2 = res_filter_node2["tasks"].as_array().expect("Tasks array");
    assert!(!tasks_node2.iter().any(|t| t["vid"] == "vid-disc-vram"), "vid-disc-vram must NOT appear under waiting_for_node");
}
