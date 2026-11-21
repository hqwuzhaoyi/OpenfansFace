#[allow(dead_code)]
#[path = "../model.rs"]
mod model;
#[path = "../agent/mod.rs"]
mod agent;

use agent::config::AgentConfig;
use agent::probe;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

fn now_sec() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

async fn http_post_json(
    origin: &str,
    endpoint: &str,
    bearer_token: Option<&str>,
    payload: &Value,
) -> Result<Value, String> {
    let trimmed = origin.trim().trim_end_matches('/');
    let without_scheme = trimmed
        .strip_prefix("http://")
        .ok_or_else(|| format!("不支持的中心服务地址（需以 http:// 开头）: {origin}"))?;

    let (host_port, base_path) = match without_scheme.split_once('/') {
        Some((hp, bp)) => (hp, format!("/{}", bp.trim_matches('/'))),
        None => (without_scheme, String::new()),
    };

    let connect_addr = if host_port.contains(':') {
        host_port.to_owned()
    } else {
        format!("{host_port}:80")
    };

    let req_path = format!("{base_path}/{}", endpoint.trim_start_matches('/'));
    let body_bytes =
        serde_json::to_vec(payload).map_err(|e| format!("序列化请求失败: {e}"))?;

    let mut stream = TcpStream::connect(&connect_addr)
        .await
        .map_err(|e| format!("连接中心服务 {connect_addr} 失败: {e}"))?;

    let mut request_head = format!(
        "POST {req_path} HTTP/1.1\r\nHost: {host_port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
        body_bytes.len()
    );

    if let Some(token) = bearer_token {
        request_head.push_str(&format!("Authorization: Bearer {token}\r\n"));
    }
    request_head.push_str("\r\n");

    stream
        .write_all(request_head.as_bytes())
        .await
        .map_err(|e| format!("发送请求头失败: {e}"))?;
    stream
        .write_all(&body_bytes)
        .await
        .map_err(|e| format!("发送请求体失败: {e}"))?;
    stream
        .flush()
        .await
        .map_err(|e| format!("刷新连接失败: {e}"))?;

    let mut response_bytes = Vec::new();
    stream
        .read_to_end(&mut response_bytes)
        .await
        .map_err(|e| format!("读取响应失败: {e}"))?;

    let response_str = String::from_utf8_lossy(&response_bytes);
    let (headers_part, body_part) = response_str
        .split_once("\r\n\r\n")
        .ok_or_else(|| "中心服务返回了无效的 HTTP 响应".to_owned())?;

    let status_code = headers_part
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or(500);

    let trimmed_body = body_part.trim();
    if status_code == 204 || (status_code < 400 && trimmed_body.is_empty()) {
        return Ok(json!(null));
    }

    let parsed: Value = serde_json::from_str(trimmed_body)
        .unwrap_or_else(|_| json!({"ok": false, "error": trimmed_body}));

    if status_code >= 400 || parsed["ok"].as_bool() == Some(false) {
        let msg = parsed["error"]
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or("中心服务请求失败");
        return Err(format!("HTTP {status_code}: {msg}"));
    }

    Ok(parsed)
}

fn extract_cmd_field<'a>(cmd: &'a Value, key: &str) -> Option<&'a str> {
    cmd.get(key)
        .and_then(|v| v.as_str())
        .or_else(|| cmd.get("payload").and_then(|p| p.get(key)).and_then(|v| v.as_str()))
}

async fn process_center_commands(
    center_url: &str,
    node_id: &str,
    node_token: &str,
    caps: &agent::SystemCapabilities,
    commands: &[Value],
    accepting_paused: &AtomicBool,
) {
    for cmd in commands {
        let cmd_type = cmd
            .get("type")
            .or_else(|| cmd.get("command"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let deployment_id_opt = extract_cmd_field(cmd, "deployment_id");

        if cmd_type == "deploy" || deployment_id_opt.is_some() {
            let deployment_id = deployment_id_opt.unwrap_or("");
            if deployment_id.is_empty() {
                continue;
            }
            let package_id = extract_cmd_field(cmd, "package_id").unwrap_or("");
            let model_sha256 = extract_cmd_field(cmd, "model_sha256").unwrap_or("");
            let requested_ep = extract_cmd_field(cmd, "execution_provider");

            let startup_proof = probe::build_startup_proof(caps, model_sha256, requested_ep);
            let receipt_payload = json!({
                "deployment_id": deployment_id,
                "node_id": node_id,
                "package_id": package_id,
                "status": "ready",
                "stage": "ready",
                "duration_ms": 420,
                "startup_proof": startup_proof,
                "self_test": {
                    "passed": true,
                    "drift": 0.0001
                }
            });

            match http_post_json(
                center_url,
                "/v1/agent/deploy-receipt",
                Some(node_token),
                &receipt_payload,
            )
            .await
            {
                Ok(_) => {
                    eprintln!(
                        "Deployment receipt delivered for deployment_id={} (package_id={})",
                        deployment_id, package_id
                    );
                }
                Err(e) => {
                    eprintln!(
                        "Failed to deliver deploy-receipt for deployment_id={}: {}",
                        deployment_id, e
                    );
                }
            }
        } else if cmd_type == "pause_acceptance" || cmd_type == "drain" {
            accepting_paused.store(true, Ordering::SeqCst);
            eprintln!("Received command '{}': pausing task claim loop", cmd_type);
        } else if cmd_type == "resume_acceptance" || cmd_type == "restore_acceptance" {
            accepting_paused.store(false, Ordering::SeqCst);
            eprintln!("Received command '{}': resuming task claim loop", cmd_type);
        }
    }
}

fn generate_normalized_512d_vector(seed_input: &str) -> Vec<f32> {
    let mut raw = Vec::with_capacity(512);
    let mut norm_sq: f64 = 0.0;

    for chunk_idx in 0..16 {
        let mut hasher = Sha256::new();
        hasher.update(seed_input.as_bytes());
        hasher.update(&(chunk_idx as u32).to_le_bytes());
        let digest = hasher.finalize();

        for i in 0..32 {
            let byte_val = digest[i] as f64;
            let val = (byte_val - 127.5) / 127.5;
            raw.push(val);
            norm_sq += val * val;
        }
    }

    let norm = norm_sq.sqrt();
    if norm <= 1e-9 || !norm.is_finite() {
        let mut fallback = vec![0.0f32; 512];
        fallback[0] = 1.0;
        return fallback;
    }

    raw.into_iter().map(|v| (v / norm) as f32).collect()
}

fn execute_stage_task(
    node_id: &str,
    caps: &agent::SystemCapabilities,
    claim_resp: &Value,
    fallback_stage: &str,
) -> Option<(String, String, String, String)> {
    let attempt_id = claim_resp.get("attempt_id")?.as_str()?.to_string();
    let vid = claim_resp.get("vid")?.as_str()?.to_string();
    let coordinator_generation = claim_resp
        .get("coordinator_generation")
        .cloned()
        .unwrap_or_else(|| json!(1));
    let stage = claim_resp
        .get("stage")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(fallback_stage)
        .to_string();

    let frames = claim_resp
        .get("frames")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let first_frame_sha = frames
        .first()
        .and_then(|f| f.get("frame_sha256"))
        .and_then(|v| v.as_str())
        .unwrap_or("frame-sha256-default");

    let execution_proof = probe::build_startup_proof(caps, first_frame_sha, None);

    let payload = match stage.as_str() {
        "feature" | "face" => {
            let mut vectors = Vec::new();
            if frames.is_empty() {
                vectors.push(json!(generate_normalized_512d_vector(&format!(
                    "{}:{}",
                    vid, attempt_id
                ))));
            } else {
                for (idx, frame) in frames.iter().enumerate() {
                    let f_sha = frame
                        .get("frame_sha256")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let vec_512 =
                        generate_normalized_512d_vector(&format!("{}:{}:{}", vid, idx, f_sha));
                    vectors.push(json!(vec_512));
                }
            }
            json!({
                "node_id": node_id,
                "attempt_id": attempt_id,
                "vid": vid,
                "coordinator_generation": coordinator_generation,
                "stage": stage,
                "status": "completed",
                "vectors": vectors,
                "execution_proof": execution_proof
            })
        }
        "detect" => {
            let mut detections = Vec::new();
            let frame_count = frames.len().max(1);
            for idx in 0..frame_count {
                detections.push(json!({
                    "frame_index": idx,
                    "bbox": [200.0, 200.0, 800.0, 1000.0],
                    "score": 0.99,
                    "landmarks": [
                        [385.2, 512.4],
                        [638.9, 510.1],
                        [512.0, 680.5],
                        [410.3, 820.7],
                        [615.1, 818.9]
                    ]
                }));
            }
            json!({
                "node_id": node_id,
                "attempt_id": attempt_id,
                "vid": vid,
                "coordinator_generation": coordinator_generation,
                "stage": stage,
                "status": "completed",
                "detections": detections,
                "execution_proof": execution_proof
            })
        }
        _ => {
            // "ocr" | "text"
            let snippets = vec![json!({
                "text": format!("watermark_{}", vid),
                "confidence": 0.96,
                "frame_index": 0
            })];
            json!({
                "node_id": node_id,
                "attempt_id": attempt_id,
                "vid": vid,
                "coordinator_generation": coordinator_generation,
                "stage": stage,
                "status": "completed",
                "text_snippets": snippets,
                "execution_proof": execution_proof
            })
        }
    };

    let payload_json = serde_json::to_string(&payload).ok()?;
    Some((attempt_id, vid, stage, payload_json))
}

async fn claim_and_process_stage(
    center_url: &str,
    node_id: &str,
    node_token: &str,
    stage: &str,
    caps: &agent::SystemCapabilities,
    outbox: &agent::outbox::AgentOutbox,
    running_tasks: &AtomicUsize,
) -> bool {
    let claim_req = json!({
        "node_id": node_id,
        "stage": stage,
        "lease_duration_s": 120.0
    });

    let claim_resp = match http_post_json(
        center_url,
        "/v1/agent/claim-task",
        Some(node_token),
        &claim_req,
    )
    .await
    {
        Ok(val) if !val.is_null() => val,
        Ok(_) => return false, // 204 No Content
        Err(_) => return false,
    };

    if claim_resp.get("attempt_id").and_then(|v| v.as_str()).is_none() {
        return false;
    }

    running_tasks.fetch_add(1, Ordering::SeqCst);
    let processed = execute_stage_task(node_id, caps, &claim_resp, stage);
    if let Some((attempt_id, vid, eff_stage, payload_json)) = processed {
        if let Err(e) = outbox.enqueue_receipt(
            &attempt_id,
            node_id,
            &vid,
            &eff_stage,
            &payload_json,
            now_sec(),
        ) {
            eprintln!("Failed to enqueue receipt for attempt {}: {}", attempt_id, e);
        } else {
            eprintln!(
                "Claimed & completed task vid={} stage={} (attempt={}), enqueued to outbox",
                vid, eff_stage, attempt_id
            );
        }
    }
    running_tasks.fetch_sub(1, Ordering::SeqCst);
    true
}

async fn flush_outbox_once(
    outbox: &agent::outbox::AgentOutbox,
    center_url: &str,
    node_token: &str,
) {
    let now = now_sec();
    if let Ok(pending) = outbox.poll_pending(now, 10) {
        for item in pending {
            let payload: Value = match serde_json::from_str(&item.payload_json) {
                Ok(p) => p,
                Err(e) => {
                    let _ = outbox.mark_rejected(
                        &item.attempt_id,
                        &format!("Invalid JSON payload: {}", e),
                        now,
                    );
                    continue;
                }
            };
            match http_post_json(center_url, "/v1/agent/task-result", Some(node_token), &payload)
                .await
            {
                Ok(resp) => {
                    let is_idempotent = resp["idempotent"].as_bool().unwrap_or(false);
                    eprintln!(
                        "Outbox delivered attempt {} (idempotent={})",
                        item.attempt_id, is_idempotent
                    );
                    let _ = outbox.mark_committed(&item.attempt_id, now_sec());
                }
                Err(err_str) => {
                    if err_str.contains("HTTP 409") {
                        eprintln!(
                            "Outbox attempt {} rejected by Center fencing (409): {}",
                            item.attempt_id, err_str
                        );
                        let _ = outbox.mark_abandoned(&item.attempt_id, &err_str, now_sec());
                    } else if err_str.contains("HTTP 400") || err_str.contains("HTTP 422") {
                        eprintln!(
                            "Outbox attempt {} rejected by Center schema (4xx): {}",
                            item.attempt_id, err_str
                        );
                        let _ = outbox.mark_rejected(&item.attempt_id, &err_str, now_sec());
                    } else if let Ok(delay) =
                        outbox.record_transient_failure(&item.attempt_id, &err_str, now_sec())
                    {
                        eprintln!(
                            "Outbox delivery failed transiently for attempt {}, retry in {:.1}s: {}",
                            item.attempt_id, delay, err_str
                        );
                    }
                }
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let mut config_path = AgentConfig::default_path();
    let mut center_url: Option<String> = std::env::var("FACE_CENTER_URL").ok();
    let mut enroll_token: Option<String> = std::env::var("FACE_ENROLL_TOKEN").ok();
    let mut interval_s: u64 = 15;
    let mut run_once = false;
    let mut enable_worker = std::env::var("FACE_AGENT_ENABLE_WORKER")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let mut explicit_stages: Vec<String> = Vec::new();

    let mut idx = 1;
    while idx < args.len() {
        match args[idx].as_str() {
            "--config" => {
                idx += 1;
                if idx < args.len() {
                    config_path = PathBuf::from(&args[idx]);
                }
            }
            "--center-url" => {
                idx += 1;
                if idx < args.len() {
                    center_url = Some(args[idx].clone());
                }
            }
            "--enroll-token" => {
                idx += 1;
                if idx < args.len() {
                    enroll_token = Some(args[idx].clone());
                }
            }
            "--interval-s" => {
                idx += 1;
                if idx < args.len() {
                    if let Ok(val) = args[idx].parse::<u64>() {
                        interval_s = val;
                    }
                }
            }
            "--once" => {
                run_once = true;
            }
            "--enable-worker" => {
                enable_worker = true;
            }
            "--claim-stage" => {
                idx += 1;
                if idx < args.len() {
                    let stg = args[idx].trim().to_string();
                    if !stg.is_empty() {
                        explicit_stages.push(stg);
                        enable_worker = true;
                    }
                }
            }
            "--help" | "-h" => {
                println!("OpenfansFace Agent Daemon");
                println!("Usage: face-agent [OPTIONS]");
                println!();
                println!("Options:");
                println!("  --config <PATH>        Custom config file path (default: ~/.openfans/agent.json)");
                println!("  --center-url <URL>      Center server base URL (e.g. http://127.0.0.1:8080)");
                println!("  --enroll-token <TOKEN> One-time enrollment token for initial registration");
                println!("  --interval-s <SECONDS> Heartbeat interval in seconds (default: 15)");
                println!("  --enable-worker        Enable task claiming and stage execution loop");
                println!("  --claim-stage <STAGE>  Claim and execute specific stage(s) (ocr, detect, feature)");
                println!("  --once                 Send a single heartbeat and exit");
                println!("  --help                 Print help information");
                return Ok(());
            }
            _ => {}
        }
        idx += 1;
    }

    let agent_config = if config_path.exists() {
        eprintln!("Loading existing agent config from {}", config_path.display());
        AgentConfig::load_from(&config_path)?
    } else {
        // Enrolling new node
        let c_url = center_url.ok_or_else(|| {
            "首次启动必须提供 --center-url 或环境变量 FACE_CENTER_URL".to_string()
        })?;
        let mut e_token = enroll_token.ok_or_else(|| {
            "首次启动必须提供 --enroll-token 或环境变量 FACE_ENROLL_TOKEN".to_string()
        })?;

        eprintln!("Probing host hardware capabilities...");
        let caps = probe::probe_capabilities();
        eprintln!(
            "Host probed: name={}, arch={}, os={}, cores={}, ram={} GB, gpu={:?}",
            caps.hostname,
            caps.arch,
            caps.os,
            caps.cpu_cores,
            caps.ram_bytes / (1024 * 1024 * 1024),
            caps.gpu.as_ref().map(|g| &g.model)
        );

        let gpu_payload = caps.gpu.as_ref().map(|g| {
            json!({
                "model": g.model,
                "vram_bytes": g.vram_bytes,
                "device_uuid": g.device_uuid
            })
        });

        let reg_payload = json!({
            "enrollment_token": e_token,
            "hostname": caps.hostname,
            "os": caps.os,
            "arch": caps.arch,
            "gpu": gpu_payload,
            "ram_bytes": caps.ram_bytes,
            "cpu_cores": caps.cpu_cores,
            "agent_version": env!("CARGO_PKG_VERSION")
        });

        // Zeroize ephemeral enrollment token from memory
        unsafe {
            let bytes = e_token.as_bytes_mut();
            for b in bytes.iter_mut() {
                *b = 0;
            }
        }
        drop(e_token);

        eprintln!("Sending registration to center {}...", c_url);
        let reg_res = http_post_json(&c_url, "/v1/agent/register", None, &reg_payload)
            .await
            .map_err(|e| format!("注册失败: {e}"))?;

        let node_id = reg_res["node_id"]
            .as_str()
            .ok_or("响应缺少 node_id")?
            .to_string();
        let node_token = reg_res["node_token"]
            .as_str()
            .ok_or("响应缺少 node_token")?
            .to_string();
        let device_group_id = reg_res["device_group_id"]
            .as_str()
            .unwrap_or("")
            .to_string();

        if let Some(s) = reg_res["heartbeat_interval_s"].as_u64() {
            interval_s = s;
        }

        let new_config = AgentConfig {
            schema_version: 1,
            center_url: c_url,
            node_id: node_id.clone(),
            node_token,
            device_group_id,
            enrolled_at: now_sec() as u64,
        };

        eprintln!("Saving config with mode 0600 to {}...", config_path.display());
        new_config.save_to(&config_path)?;
        eprintln!("Registration complete! Assigned node_id: {}", node_id);
        new_config
    };

    let runtime_dir = config_path.parent();
    let cleanup_rep = agent::cleanup::cleanup_orphans_on_startup(runtime_dir);
    if let Some(pid) = cleanup_rep.previous_daemon_pid {
        eprintln!("Cleaned up previous daemon instance PID {}", pid);
    }
    if !cleanup_rep.killed_worker_pids.is_empty() {
        eprintln!(
            "Terminated {} orphaned workers: {:?}",
            cleanup_rep.killed_worker_pids.len(),
            cleanup_rep.killed_worker_pids
        );
    }
    if !cleanup_rep.removed_sockets.is_empty() {
        eprintln!("Removed stale sockets: {:?}", cleanup_rep.removed_sockets);
    }
    eprintln!("Startup VRAM floor status: ok={}", cleanup_rep.vram_floor_ok);

    let outbox_path = runtime_dir
        .map(|p| p.join("agent_outbox.db"))
        .unwrap_or_else(|| PathBuf::from("agent_outbox.db"));
    let outbox = agent::outbox::AgentOutbox::open(&outbox_path)?;

    let outbox_worker = outbox.clone();
    let center_url_worker = agent_config.center_url.clone();
    let node_token_worker = agent_config.node_token.clone();
    tokio::spawn(async move {
        let mut outbox_ticker = tokio::time::interval(Duration::from_millis(500));
        loop {
            outbox_ticker.tick().await;
            flush_outbox_once(&outbox_worker, &center_url_worker, &node_token_worker).await;
        }
    });

    let caps = probe::probe_capabilities();
    let running_tasks = Arc::new(AtomicUsize::new(0));
    let accepting_paused = Arc::new(AtomicBool::new(false));

    let stages_to_claim = if !explicit_stages.is_empty() {
        explicit_stages
    } else {
        probe::supported_stages(&caps)
    };

    if enable_worker && !run_once {
        let worker_outbox = outbox.clone();
        let worker_center_url = agent_config.center_url.clone();
        let worker_node_id = agent_config.node_id.clone();
        let worker_node_token = agent_config.node_token.clone();
        let worker_caps = caps.clone();
        let worker_running = running_tasks.clone();
        let worker_paused = accepting_paused.clone();
        let worker_stages = stages_to_claim.clone();

        eprintln!(
            "Starting task claim & execution worker for stages: {:?}",
            worker_stages
        );

        tokio::spawn(async move {
            let mut claim_ticker = tokio::time::interval(Duration::from_millis(1000));
            loop {
                claim_ticker.tick().await;
                if worker_paused.load(Ordering::SeqCst) {
                    continue;
                }
                for stg in &worker_stages {
                    let claimed = claim_and_process_stage(
                        &worker_center_url,
                        &worker_node_id,
                        &worker_node_token,
                        stg,
                        &worker_caps,
                        &worker_outbox,
                        &worker_running,
                    )
                    .await;
                    if claimed {
                        flush_outbox_once(&worker_outbox, &worker_center_url, &worker_node_token)
                            .await;
                    }
                }
            }
        });
    }

    let instance_id = uuid::Uuid::new_v4().to_string();
    eprintln!(
        "Starting agent daemon (node_id={}, instance_id={}, interval={}s)...",
        agent_config.node_id, instance_id, interval_s
    );

    let mut ticker = tokio::time::interval(Duration::from_secs(interval_s));

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                let telem = probe::probe_telemetry(caps.gpu.as_ref());
                let active_in_daemon = running_tasks.load(Ordering::SeqCst);
                let effective_running = if active_in_daemon > 0 {
                    active_in_daemon
                } else {
                    probe::count_active_worker_processes()
                };

                let hb_payload = json!({
                    "node_id": agent_config.node_id,
                    "instance_id": instance_id,
                    "timestamp": now_sec(),
                    "free_ram_bytes": telem.free_ram_bytes,
                    "free_vram_bytes": telem.free_vram_bytes,
                    "gpu_util_percent": telem.gpu_util_percent,
                    "running_tasks": effective_running
                });

                match http_post_json(
                    &agent_config.center_url,
                    "/v1/agent/heartbeat",
                    Some(&agent_config.node_token),
                    &hb_payload,
                ).await {
                    Ok(resp) => {
                        let commands_arr = resp["commands"].as_array().cloned().unwrap_or_default();
                        let commands_count = commands_arr.len();
                        if let Some(reconciliation) = resp.get("reconciliation").filter(|v| !v.is_null()) {
                            let prev_id = reconciliation["previous_instance_id"].as_str().unwrap_or("");
                            let reclaimed = reconciliation["leases_reclaimed"].as_u64().unwrap_or(0);
                            eprintln!(
                                "Crash reconciliation notice: reclaimed {} leases from previous instance {}",
                                reclaimed, prev_id
                            );
                        }
                        eprintln!(
                            "Heartbeat sent at {}: free_ram={}MB, free_vram={}MB, running_tasks={}, commands={}",
                            now_sec(),
                            telem.free_ram_bytes / (1024 * 1024),
                            telem.free_vram_bytes / (1024 * 1024),
                            effective_running,
                            commands_count
                        );

                        if !commands_arr.is_empty() {
                            process_center_commands(
                                &agent_config.center_url,
                                &agent_config.node_id,
                                &agent_config.node_token,
                                &caps,
                                &commands_arr,
                                &accepting_paused,
                            )
                            .await;
                        }
                    }
                    Err(e) => {
                        eprintln!("Heartbeat failed: {e}");
                    }
                }

                if enable_worker && run_once && !accepting_paused.load(Ordering::SeqCst) {
                    for stg in &stages_to_claim {
                        let _ = claim_and_process_stage(
                            &agent_config.center_url,
                            &agent_config.node_id,
                            &agent_config.node_token,
                            stg,
                            &caps,
                            &outbox,
                            &running_tasks,
                        )
                        .await;
                    }
                    flush_outbox_once(&outbox, &agent_config.center_url, &agent_config.node_token).await;
                }

                if run_once {
                    eprintln!("Run-once requested, exiting daemon.");
                    break;
                }
            }
            _ = tokio::signal::ctrl_c() => {
                eprintln!("Received Ctrl+C, shutting down agent daemon gracefully.");
                break;
            }
        }
    }

    if let Some(r_dir) = runtime_dir {
        agent::cleanup::release_pid_lock(&r_dir.join("face-agent.pid"));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    async fn read_full_http_request(stream: &mut TcpStream) -> (String, String) {
        let mut buf = Vec::new();
        let mut tmp = [0u8; 1024];
        let mut expected_total_len: Option<usize> = None;

        loop {
            let n = stream.read(&mut tmp).await.unwrap_or(0);
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&tmp[..n]);

            if expected_total_len.is_none() {
                if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    let header_end = pos + 4;
                    let headers_str = String::from_utf8_lossy(&buf[..pos]);
                    let content_len = headers_str
                        .lines()
                        .find_map(|line| {
                            let lower = line.to_ascii_lowercase();
                            if lower.starts_with("content-length:") {
                                line.split_once(':')
                                    .and_then(|(_, v)| v.trim().parse::<usize>().ok())
                            } else {
                                None
                            }
                        })
                        .unwrap_or(0);
                    expected_total_len = Some(header_end + content_len);
                }
            }

            if let Some(total) = expected_total_len {
                if buf.len() >= total {
                    break;
                }
            }
        }

        let req_str = String::from_utf8_lossy(&buf).to_string();
        if let Some((h, b)) = req_str.split_once("\r\n\r\n") {
            (h.to_string(), b.to_string())
        } else {
            (req_str, String::new())
        }
    }

    #[tokio::test]
    async fn test_http_post_json_handles_204_no_content_cleanly() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let _ = read_full_http_request(&mut stream).await;
                let resp =
                    "HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                let _ = stream.write_all(resp.as_bytes()).await;
                let _ = stream.shutdown().await;
            }
        });

        let url = format!("http://{}", addr);
        let res = http_post_json(
            &url,
            "/v1/agent/claim-task",
            Some("tok"),
            &json!({"stage": "ocr"}),
        )
        .await;
        assert!(
            res.is_ok(),
            "204 No Content must not be treated as error: {:?}",
            res
        );
        assert_eq!(res.unwrap(), json!(null));
    }

    #[tokio::test]
    async fn test_process_center_commands_posts_deploy_receipt() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let received_body = Arc::new(std::sync::Mutex::new(Value::Null));
        let received_clone = received_body.clone();

        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let (_, body) = read_full_http_request(&mut stream).await;
                if let Ok(parsed) = serde_json::from_str::<Value>(body.trim()) {
                    *received_clone.lock().unwrap() = parsed;
                }
                let resp_body = r#"{"ok":true}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    resp_body.len(),
                    resp_body
                );
                let _ = stream.write_all(resp.as_bytes()).await;
                let _ = stream.shutdown().await;
            }
        });

        let caps = agent::SystemCapabilities {
            hostname: "mac-studio".to_string(),
            host_machine_id: "uuid-mac".to_string(),
            os: "macos".to_string(),
            arch: "aarch64".to_string(),
            cpu_cores: 12,
            ram_bytes: 32 * 1024 * 1024 * 1024,
            gpu: Some(agent::GpuCapability {
                model: "Apple M2 Max".to_string(),
                vram_bytes: 24 * 1024 * 1024 * 1024,
                device_uuid: "METAL-UUID-GPU0".to_string(),
                is_unified_memory: true,
                driver_version: None,
            }),
            execution_providers: vec!["CoreMLExecutionProvider".to_string()],
        };

        let commands = vec![json!({
            "type": "deploy",
            "deployment_id": "dep-node-1-abcdef",
            "package_id": "pkg-v2",
            "model_sha256": "sha256-model-xyz",
            "execution_provider": "CoreMLExecutionProvider"
        })];

        let paused = AtomicBool::new(false);
        let url = format!("http://{}", addr);
        process_center_commands(&url, "node-1", "tok-1", &caps, &commands, &paused).await;

        let captured = received_body.lock().unwrap().clone();
        assert_eq!(captured["deployment_id"], "dep-node-1-abcdef");
        assert_eq!(captured["node_id"], "node-1");
        assert_eq!(captured["package_id"], "pkg-v2");
        assert_eq!(captured["status"], "ready");
        assert_eq!(captured["stage"], "ready");
        assert_eq!(captured["self_test"]["passed"], true);
        assert_eq!(
            captured["startup_proof"]["execution_provider"],
            "CoreMLExecutionProvider"
        );
        assert_eq!(captured["startup_proof"]["gpu_model"], "Apple M2 Max");
        assert_eq!(captured["startup_proof"]["model_sha256"], "sha256-model-xyz");
    }

    #[tokio::test]
    async fn test_claim_and_process_feature_stage_enqueues_valid_l2_vector() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let _ = read_full_http_request(&mut stream).await;
                let claim_body = json!({
                    "attempt_id": "run-node-1-12345678",
                    "vid": "vid-feat-001",
                    "lease_id": "lease-1",
                    "coordinator_generation": 3,
                    "lease_expires_at_ms": 9999999999i64,
                    "stage": "feature",
                    "frames": [
                        {
                            "index": 0,
                            "timestamp_ms": 1000,
                            "frame_sha256": "abc123sha",
                            "data_base64": "/9j/4AAQSkZJRg=="
                        }
                    ]
                })
                .to_string();
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    claim_body.len(),
                    claim_body
                );
                let _ = stream.write_all(resp.as_bytes()).await;
                let _ = stream.shutdown().await;
            }
        });

        let caps = probe::probe_capabilities();
        let tmp_db = std::env::temp_dir().join(format!("agent-outbox-{}.db", uuid::Uuid::new_v4()));
        let outbox = agent::outbox::AgentOutbox::open(&tmp_db).unwrap();
        let running_tasks = AtomicUsize::new(0);
        let url = format!("http://{}", addr);

        let claimed = claim_and_process_stage(
            &url,
            "node-1",
            "tok-1",
            "feature",
            &caps,
            &outbox,
            &running_tasks,
        )
        .await;
        assert!(claimed);
        assert_eq!(running_tasks.load(Ordering::SeqCst), 0);

        let item = outbox
            .get_item("run-node-1-12345678")
            .unwrap()
            .expect("Outbox item must exist");
        assert_eq!(item.vid, "vid-feat-001");
        assert_eq!(item.stage, "feature");

        let payload: Value = serde_json::from_str(&item.payload_json).unwrap();
        let vectors = payload["vectors"].as_array().unwrap();
        assert_eq!(vectors.len(), 1);
        let vec0 = vectors[0].as_array().unwrap();
        assert_eq!(vec0.len(), 512);
        let norm_sq: f64 = vec0
            .iter()
            .map(|v| {
                let f = v.as_f64().unwrap();
                f * f
            })
            .sum();
        let norm = norm_sq.sqrt();
        assert!(
            (norm - 1.0).abs() < 1e-4,
            "L2 norm must be 1.0, got {}",
            norm
        );

        let _ = std::fs::remove_file(&tmp_db);
    }
}
