use crate::model::{Error, Result};
use crate::Config;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};

pub const AGENT_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS agent_enrollment_tokens (
    token TEXT PRIMARY KEY,
    created_at REAL NOT NULL,
    expires_at REAL NOT NULL,
    used_at REAL,
    used_by_node_id TEXT,
    note TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS idx_agent_enrollment_lookup ON agent_enrollment_tokens(token, used_at, expires_at);

CREATE TABLE IF NOT EXISTS agent_nodes (
    id TEXT PRIMARY KEY,
    token_hash TEXT NOT NULL,
    instance_id TEXT NOT NULL DEFAULT '',
    hostname TEXT NOT NULL,
    os TEXT NOT NULL,
    arch TEXT NOT NULL,
    agent_version TEXT NOT NULL,
    cpu_cores INTEGER NOT NULL DEFAULT 0,
    ram_bytes INTEGER NOT NULL DEFAULT 0,
    gpu_model TEXT,
    gpu_vram_bytes INTEGER NOT NULL DEFAULT 0,
    gpu_device_uuid TEXT,
    device_group_id TEXT NOT NULL DEFAULT '',
    max_concurrent_tasks INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL DEFAULT 'registered',
    accepting INTEGER NOT NULL DEFAULT 1,
    model_ready INTEGER NOT NULL DEFAULT 0,
    desired_version TEXT NOT NULL DEFAULT '',
    current_version TEXT NOT NULL DEFAULT '',
    free_ram_bytes INTEGER NOT NULL DEFAULT 0,
    free_vram_bytes INTEGER NOT NULL DEFAULT 0,
    gpu_util_percent REAL NOT NULL DEFAULT 0.0,
    running_tasks INTEGER NOT NULL DEFAULT 0,
    last_heartbeat_at REAL NOT NULL DEFAULT 0.0,
    registered_at REAL NOT NULL,
    updated_at REAL NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_agent_nodes_heartbeat ON agent_nodes(last_heartbeat_at);
CREATE INDEX IF NOT EXISTS idx_agent_nodes_device_group ON agent_nodes(device_group_id);
CREATE INDEX IF NOT EXISTS idx_agent_nodes_token_hash ON agent_nodes(token_hash);
CREATE INDEX IF NOT EXISTS idx_agent_nodes_status ON agent_nodes(status, accepting, model_ready);
"#;

pub fn now_sec() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

pub fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    format!("{:x}", hasher.finalize())
}

pub fn init_tables(conn: &Connection) -> Result<()> {
    if conn
        .prepare("SELECT max_concurrent_tasks FROM agent_nodes LIMIT 0")
        .is_ok()
        && conn
            .prepare("SELECT token FROM agent_enrollment_tokens LIMIT 0")
            .is_ok()
    {
        return Ok(());
    }
    conn.execute_batch(AGENT_SCHEMA)?;
    let has_max_tasks: bool = conn
        .prepare("SELECT max_concurrent_tasks FROM agent_nodes LIMIT 0")
        .is_ok();
    if !has_max_tasks {
        let _ = conn.execute(
            "ALTER TABLE agent_nodes ADD COLUMN max_concurrent_tasks INTEGER NOT NULL DEFAULT 0",
            [],
        );
    }
    Ok(())
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct GpuSpecs {
    pub model: String,
    pub vram_bytes: i64,
    pub device_uuid: String,
}

#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    pub enrollment_token: String,
    pub hostname: String,
    pub os: String,
    pub arch: String,
    #[serde(default)]
    pub gpu: Option<GpuSpecs>,
    pub ram_bytes: i64,
    pub cpu_cores: i32,
    pub agent_version: String,
    #[serde(default)]
    pub max_concurrent_tasks: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RegisterResponse {
    pub node_id: String,
    pub node_token: String,
    pub heartbeat_interval_s: u32,
    pub device_group_id: String,
}

#[derive(Debug, Deserialize)]
pub struct HeartbeatRequest {
    pub node_id: String,
    pub instance_id: String,
    #[allow(dead_code)]
    pub timestamp: f64,
    #[serde(default)]
    #[allow(dead_code)]
    pub uptime_s: Option<f64>,
    pub free_ram_bytes: i64,
    pub free_vram_bytes: i64,
    pub gpu_util_percent: f64,
    pub running_tasks: i32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AgentCommand {
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReconciliationSummary {
    pub previous_instance_id: String,
    pub leases_reclaimed: usize,
    pub reclaimed_vids: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HeartbeatResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub desired_version: Option<String>,
    #[serde(default)]
    pub commands: Vec<AgentCommand>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reconciliation: Option<ReconciliationSummary>,
}

#[derive(Debug, Deserialize)]
pub struct NodeControlRequest {
    pub node_id: String,
    pub action: String, // "revoke_acceptance" | "restore_acceptance" | "drain"
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NodeControlResponse {
    pub ok: bool,
    pub node_id: String,
    pub accepting: bool,
    pub status: String,
}

#[derive(Debug, Deserialize)]
pub struct EnrollmentTokenRequest {
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub ttl_seconds: Option<f64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct EnrollmentTokenResponse {
    pub token: String,
    pub expires_at: f64,
}

/// Create a new one-time enrollment token (admin endpoint).
pub fn create_enrollment_token(
    config: &Config,
    req: EnrollmentTokenRequest,
) -> Result<EnrollmentTokenResponse> {
    let conn = crate::processing_scheduler::open_or_init(config)?;
    let now = now_sec();
    let ttl = req.ttl_seconds.unwrap_or(86400.0); // default 24h
    let expires_at = now + ttl;
    let token = format!("et_{}", uuid::Uuid::new_v4().simple());
    let note = req.note.unwrap_or_default();

    conn.execute(
        "INSERT INTO agent_enrollment_tokens (token, created_at, expires_at, note) VALUES (?, ?, ?, ?)",
        params![token, now, expires_at, note],
    )?;

    Ok(EnrollmentTokenResponse { token, expires_at })
}

/// Atomically verify and consume an enrollment token.
/// Also allows matching against `AGENT_ENROLLMENT_SECRET` environment variable for bootstrapping.
pub fn consume_enrollment_token(
    conn: &Connection,
    token: &str,
    node_id: &str,
    now: f64,
) -> Result<bool> {
    let trimmed = token.trim();
    if trimmed.is_empty() {
        return Ok(false);
    }

    // Check environment bootstrap fallback
    if let Ok(secret) = std::env::var("AGENT_ENROLLMENT_SECRET") {
        if !secret.trim().is_empty() && secret.trim() == trimmed {
            return Ok(true);
        }
    }

    let rows_affected = conn.execute(
        "UPDATE agent_enrollment_tokens
         SET used_at = ?, used_by_node_id = ?
         WHERE token = ? AND used_at IS NULL AND expires_at > ?",
        params![now, node_id, trimmed, now],
    )?;

    Ok(rows_affected > 0)
}

/// Register a new agent node.
pub fn register_node(config: &Config, req: RegisterRequest) -> Result<RegisterResponse> {
    let mut conn = crate::processing_scheduler::open_or_init(config)?;
    let now = now_sec();

    // Sanitize hostname to make a readable node_id prefix
    let clean_hostname: String = req
        .hostname
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_lowercase();
    let prefix = if clean_hostname.is_empty() {
        "node"
    } else {
        &clean_hostname
    };
    let uuid_part = uuid::Uuid::new_v4().simple().to_string();
    let node_id = format!("node-{}-{}", prefix, &uuid_part[..8]);

    // Token creation
    let node_token = format!("agt_{}", uuid::Uuid::new_v4().simple());
    let token_hash = sha256_hex(&node_token);

    // Compute deterministic device_group_id
    let device_group_id = if let Some(gpu) = req.gpu.as_ref().filter(|g| !g.device_uuid.trim().is_empty()) {
        let hash = sha256_hex(gpu.device_uuid.trim());
        format!("grp-gpu-{}", &hash[..16])
    } else {
        let hash = sha256_hex(req.hostname.trim());
        format!("grp-host-{}", &hash[..16])
    };

    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;

    // Consume enrollment token
    let token_valid = consume_enrollment_token(&tx, &req.enrollment_token, &node_id, now)?;
    if !token_valid {
        return Err(Error::new(
            401,
            "unauthorized",
            "注册令牌无效、已过期或已被使用",
        ));
    }

    // Insert new node record
    let gpu_model = req.gpu.as_ref().map(|g| g.model.clone());
    let gpu_vram_bytes = req.gpu.as_ref().map(|g| g.vram_bytes).unwrap_or(0);
    let gpu_device_uuid = req.gpu.as_ref().map(|g| g.device_uuid.clone());

    tx.execute(
        "INSERT INTO agent_nodes (
            id, token_hash, instance_id, hostname, os, arch, agent_version,
            cpu_cores, ram_bytes, gpu_model, gpu_vram_bytes, gpu_device_uuid,
            device_group_id, status, accepting, model_ready, desired_version,
            current_version, free_ram_bytes, free_vram_bytes, gpu_util_percent,
            running_tasks, last_heartbeat_at, registered_at, updated_at,
            max_concurrent_tasks
        ) VALUES (
            ?, ?, '', ?, ?, ?, ?,
            ?, ?, ?, ?, ?,
            ?, 'registered', 1, 0, '',
            '', 0, 0, 0.0,
            0, 0.0, ?, ?,
            ?
        )",
        params![
            node_id,
            token_hash,
            req.hostname,
            req.os,
            req.arch,
            req.agent_version,
            req.cpu_cores,
            req.ram_bytes,
            gpu_model,
            gpu_vram_bytes,
            gpu_device_uuid,
            device_group_id,
            now,
            now,
            req.max_concurrent_tasks.unwrap_or(0),
        ],
    )?;

    // Record registration event
    let event_msg = format!(
        "Agent node {} registered (host={}, arch={}, os={})",
        node_id, req.hostname, req.arch, req.os
    );
    let _ = tx.execute(
        "INSERT INTO events (at, vid, kind, message) VALUES (?, NULL, 'agent_registered', ?)",
        params![now, event_msg],
    );

    tx.commit()?;

    Ok(RegisterResponse {
        node_id,
        node_token,
        heartbeat_interval_s: 15,
        device_group_id,
    })
}

/// Handle periodic heartbeat from agent daemon.
pub fn handle_heartbeat(
    config: &Config,
    raw_bearer_token: &str,
    req: HeartbeatRequest,
) -> Result<HeartbeatResponse> {
    let mut conn = crate::processing_scheduler::open_or_init(config)?;
    let now = now_sec();
    let token_hash = sha256_hex(raw_bearer_token.trim());

    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;

    // 1. Authenticate node
    let node_row: Option<(String, String, i32, i32, String)> = tx
        .query_row(
            "SELECT id, instance_id, accepting, model_ready, desired_version
             FROM agent_nodes
             WHERE id = ? AND token_hash = ?",
            params![req.node_id, token_hash],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .ok();

    let (node_id, stored_instance_id, accepting, model_ready, desired_version) = match node_row {
        Some(row) => row,
        None => {
            return Err(Error::new(
                401,
                "unauthorized",
                "节点令牌无效或节点编号不匹配",
            ));
        }
    };

    // 2. Crash/Restart detection & reconciliation
    let reconciliation = if !stored_instance_id.is_empty() && stored_instance_id != req.instance_id {
        let summary = reconcile_node_crash(&tx, &node_id, &stored_instance_id, now)?;
        let msg = format!(
            "Node {} restarted with new instance_id {} (was {}), reclaimed {} leases",
            node_id, req.instance_id, stored_instance_id, summary.leases_reclaimed
        );
        let _ = tx.execute(
            "INSERT INTO events (at, vid, kind, message) VALUES (?, NULL, 'agent_restarted', ?)",
            params![now, msg],
        );
        Some(summary)
    } else {
        None
    };

    // 3. Cluster-wide 90s offline reap & task reclamation
    let _ = check_liveness_timeouts(&tx, now);

    // 4. Determine new status for reporting node
    let new_status = if accepting == 1 && model_ready == 1 {
        "accepting"
    } else if model_ready == 1 {
        "model_ready"
    } else {
        "online"
    };

    // 5. Update reporting node's telemetry and heartbeat timestamp
    tx.execute(
        "UPDATE agent_nodes SET
            instance_id = ?,
            free_ram_bytes = ?,
            free_vram_bytes = ?,
            gpu_util_percent = ?,
            running_tasks = ?,
            last_heartbeat_at = ?,
            updated_at = ?,
            status = ?
         WHERE id = ?",
        params![
            req.instance_id,
            req.free_ram_bytes,
            req.free_vram_bytes,
            req.gpu_util_percent,
            req.running_tasks,
            now,
            now,
            new_status,
            node_id,
        ],
    )?;

    tx.commit()?;

    let mut commands = Vec::new();
    if accepting == 0 {
        commands.push(AgentCommand {
            command: "pause_acceptance".to_string(),
            payload: None,
        });
    }
    if let Ok(mut dep_stmt) = conn.prepare(
        "SELECT d.deployment_id, d.package_id, p.manifest_sha256, p.model_sha256, p.execution_provider, d.desired_version
         FROM agent_deployments d
         JOIN agent_packages p ON d.package_id = p.package_id
         WHERE d.node_id = ? AND d.status IN ('downloading', 'verifying', 'self_test')
         ORDER BY d.updated_at DESC LIMIT 1",
    ) {
        if let Ok((dep_id, pkg_id, man_sha, mod_sha, exec_prov, des_ver)) = dep_stmt.query_row(
            params![node_id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                ))
            },
        ) {
            commands.push(AgentCommand {
                command: "deploy".to_string(),
                payload: Some(json!({
                    "deployment_id": dep_id,
                    "package_id": pkg_id,
                    "manifest_sha256": man_sha,
                    "model_sha256": mod_sha,
                    "execution_provider": exec_prov,
                    "desired_version": des_ver,
                })),
            });
        }
    }

    Ok(HeartbeatResponse {
        ok: true,
        desired_version: if desired_version.is_empty() {
            None
        } else {
            Some(desired_version)
        },
        commands,
        reconciliation,
    })
}

pub fn reconcile_node_crash(
    tx: &Connection,
    node_id: &str,
    previous_instance_id: &str,
    now: f64,
) -> Result<ReconciliationSummary> {
    let id_col = if tx.prepare("SELECT node_id FROM agent_nodes LIMIT 1").is_ok() {
        "node_id"
    } else {
        "id"
    };

    let has_assigned_node_id = tx.prepare("SELECT assigned_node_id FROM tasks LIMIT 1").is_ok();
    if !has_assigned_node_id {
        tx.execute(
            &format!("UPDATE agent_nodes SET running_tasks = 0, updated_at = ? WHERE {} = ?", id_col),
            params![now, node_id],
        )?;
        return Ok(ReconciliationSummary {
            previous_instance_id: previous_instance_id.to_string(),
            leases_reclaimed: 0,
            reclaimed_vids: Vec::new(),
        });
    }

    let has_attempt_id = tx.prepare("SELECT attempt_id FROM tasks LIMIT 1").is_ok();
    let query_sql = if has_attempt_id {
        "SELECT vid, stage, retry_stage, text_attempts, face_attempts, run_id, attempt_id
         FROM tasks
         WHERE assigned_node_id = ? AND manual = 0"
    } else {
        "SELECT vid, stage, retry_stage, text_attempts, face_attempts, run_id, '' AS attempt_id
         FROM tasks
         WHERE assigned_node_id = ? AND manual = 0"
    };

    let mut stmt = tx.prepare(query_sql)?;

    struct InFlightTask {
        vid: String,
        stage: String,
        retry_stage: String,
        text_attempts: i64,
        face_attempts: i64,
        run_id: String,
        attempt_id: String,
    }

    let in_flight: Vec<InFlightTask> = stmt
        .query_map(params![node_id], |row| {
            Ok(InFlightTask {
                vid: row.get(0)?,
                stage: row.get(1)?,
                retry_stage: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                text_attempts: row.get::<_, Option<i64>>(3)?.unwrap_or(0),
                face_attempts: row.get::<_, Option<i64>>(4)?.unwrap_or(0),
                run_id: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
                attempt_id: row.get::<_, Option<String>>(6)?.unwrap_or_default(),
            })
        })?
        .filter_map(|r| r.ok())
        .collect();

    let mut reclaimed_vids = Vec::new();

    for task in in_flight {
        let attempts = if task.stage == "text" || task.stage == "ocr" || task.retry_stage == "text" || task.retry_stage == "ocr" {
            task.text_attempts
        } else {
            task.face_attempts
        };

        let next_stage = if attempts < 3 { "retry" } else { "error" };
        let orig_stage = if !task.retry_stage.is_empty() {
            task.retry_stage.clone()
        } else {
            task.stage.clone()
        };
        let msg = if next_stage == "error" {
            "Agent 异常重启超过最大重试次数"
        } else {
            "Agent 异常重启，中断租约已自动回收"
        };

        if has_attempt_id {
            tx.execute(
                "UPDATE tasks SET
                    stage = ?,
                    retry_stage = ?,
                    run_id = '',
                    attempt_id = '',
                    lease_id = '',
                    assigned_node_id = '',
                    lease_expires_at = 0,
                    coordinator_generation = '',
                    error = ?,
                    next_retry = 0,
                    updated_at = ?
                 WHERE vid = ? AND assigned_node_id = ?",
                params![next_stage, orig_stage, msg, now, task.vid, node_id],
            )?;
        } else {
            tx.execute(
                "UPDATE tasks SET
                    stage = ?,
                    retry_stage = ?,
                    run_id = '',
                    lease_expires_at = 0,
                    error = ?,
                    next_retry = 0,
                    updated_at = ?
                 WHERE vid = ? AND assigned_node_id = ?",
                params![next_stage, orig_stage, msg, now, task.vid, node_id],
            )?;
        }

        let effective_run = if !task.run_id.is_empty() {
            task.run_id
        } else {
            task.attempt_id
        };
        if !effective_run.is_empty() && tx.prepare("SELECT 1 FROM processing_runs LIMIT 1").is_ok() {
            let _ = tx.execute(
                "UPDATE processing_runs
                 SET status = 'interrupted', finished_at = ?, error = ?
                 WHERE id = ? AND status = 'running'",
                params![now, msg, effective_run],
            );
        }

        let evt_msg = format!(
            "Node {} crash/restart (was instance {}): reclaimed task {} to stage {}",
            node_id, previous_instance_id, task.vid, next_stage
        );
        let _ = tx.execute(
            "INSERT INTO events (at, vid, kind, message) VALUES (?, ?, 'agent_reconciled', ?)",
            params![now, task.vid, evt_msg],
        );

        reclaimed_vids.push(task.vid);
    }

    tx.execute(
        &format!("UPDATE agent_nodes SET running_tasks = 0, updated_at = ? WHERE {} = ?", id_col),
        params![now, node_id],
    )?;

    Ok(ReconciliationSummary {
        previous_instance_id: previous_instance_id.to_string(),
        leases_reclaimed: reclaimed_vids.len(),
        reclaimed_vids,
    })
}

pub fn check_liveness_timeouts(conn: &Connection, now: f64) -> Result<Vec<String>> {
    let cutoff = now - 90.0;
    let mut stmt = conn.prepare(
        "SELECT id FROM agent_nodes WHERE status != 'offline' AND status != 'revoked' AND last_heartbeat_at > 0.0 AND last_heartbeat_at < ?",
    )?;
    let offline_ids: Vec<String> = stmt
        .query_map(params![cutoff], |r| r.get(0))?
        .filter_map(|r| r.ok())
        .collect();

    let mut total_reclaimed = 0usize;
    let has_assigned_node_id = conn.prepare("SELECT assigned_node_id FROM tasks LIMIT 1").is_ok();
    let has_attempt_id = if has_assigned_node_id {
        conn.prepare("SELECT attempt_id FROM tasks LIMIT 1").is_ok()
    } else {
        false
    };

    for nid in &offline_ids {
        conn.execute(
            "UPDATE agent_nodes SET status = 'offline', running_tasks = 0, updated_at = ? WHERE id = ?",
            params![now, nid],
        )?;
        let _ = conn.execute(
            "INSERT INTO events (at, vid, kind, message) VALUES (?, NULL, 'node_offline', ?)",
            params![now, format!("Node {} timed out (>90s) and marked offline", nid)],
        );

        if !has_assigned_node_id {
            continue;
        }

        let task_query = if has_attempt_id {
            "SELECT vid, stage, retry_stage, text_attempts, face_attempts, run_id, attempt_id
             FROM tasks WHERE assigned_node_id = ? AND manual = 0"
        } else {
            "SELECT vid, stage, retry_stage, text_attempts, face_attempts, run_id, '' AS attempt_id
             FROM tasks WHERE assigned_node_id = ? AND manual = 0"
        };

        // Reclaim in-flight tasks assigned to this dead node
        let mut task_stmt = conn.prepare(task_query)?;
        struct TaskRow {
            vid: String,
            stage: String,
            retry_stage: String,
            text_attempts: i64,
            face_attempts: i64,
            run_id: String,
            attempt_id: String,
        }
        let in_flight: Vec<TaskRow> = task_stmt
            .query_map(params![nid], |r| {
                Ok(TaskRow {
                    vid: r.get(0)?,
                    stage: r.get(1)?,
                    retry_stage: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    text_attempts: r.get::<_, Option<i64>>(3)?.unwrap_or(0),
                    face_attempts: r.get::<_, Option<i64>>(4)?.unwrap_or(0),
                    run_id: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
                    attempt_id: r.get::<_, Option<String>>(6)?.unwrap_or_default(),
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        for t in in_flight {
            let attempts = if t.stage == "text" || t.stage == "ocr" || t.retry_stage == "text" || t.retry_stage == "ocr" {
                t.text_attempts
            } else {
                t.face_attempts
            };
            let next_stage = if attempts < 3 { "retry" } else { "error" };
            let orig_stage = if !t.retry_stage.is_empty() { t.retry_stage } else { t.stage };
            let msg = if next_stage == "error" {
                "节点心跳超时失联 (>90s) 且超过最大重试次数"
            } else {
                "节点心跳超时失联 (>90s)，任务已自动回收"
            };

            if has_attempt_id {
                conn.execute(
                    "UPDATE tasks SET
                        stage = ?,
                        retry_stage = ?,
                        run_id = '',
                        attempt_id = '',
                        lease_id = '',
                        assigned_node_id = '',
                        lease_expires_at = 0,
                        error = ?,
                        next_retry = 0,
                        updated_at = ?
                     WHERE vid = ? AND assigned_node_id = ?",
                    params![next_stage, orig_stage, msg, now, t.vid, nid],
                )?;
            } else {
                conn.execute(
                    "UPDATE tasks SET
                        stage = ?,
                        retry_stage = ?,
                        run_id = '',
                        lease_expires_at = 0,
                        error = ?,
                        next_retry = 0,
                        updated_at = ?
                     WHERE vid = ? AND assigned_node_id = ?",
                    params![next_stage, orig_stage, msg, now, t.vid, nid],
                )?;
            }

            let eff_run = if !t.run_id.is_empty() { t.run_id } else { t.attempt_id };
            if !eff_run.is_empty() && conn.prepare("SELECT 1 FROM processing_runs LIMIT 1").is_ok() {
                let _ = conn.execute(
                    "UPDATE processing_runs SET status = 'interrupted', finished_at = ?, error = ?
                     WHERE id = ? AND status = 'running'",
                    params![now, msg, eff_run],
                );
            }

            let _ = conn.execute(
                "INSERT INTO events (at, vid, kind, message) VALUES (?, ?, 'node_timeout_reclaimed', ?)",
                params![now, t.vid, format!("Node {} timed out; task {} reclaimed to stage {}", nid, t.vid, next_stage)],
            );
            total_reclaimed += 1;
        }
    }

    if total_reclaimed > 0 {
        let _ = crate::processing_scheduler::bump_coordinator_generation(conn);
    }

    Ok(offline_ids)
}

pub fn liveness_check(config: &Config) -> Result<Vec<String>> {
    let mut conn = crate::processing_scheduler::open_or_init(config)?;
    let now = now_sec();
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let offline_nodes = check_liveness_timeouts(&tx, now)?;
    tx.commit()?;
    Ok(offline_nodes)
}

/// Control node acceptance state (admin endpoint).
pub fn control_node(config: &Config, req: NodeControlRequest) -> Result<NodeControlResponse> {
    let mut conn = crate::processing_scheduler::open_or_init(config)?;
    let now = now_sec();

    let accepting_val = match req.action.as_str() {
        "revoke_acceptance" | "drain" => 0,
        "restore_acceptance" => 1,
        other => {
            return Err(Error::new(
                400,
                "invalid_action",
                format!("未知控制操作: {other}"),
            ));
        }
    };

    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;

    let node_row: Option<(String, i32, f64)> = tx
        .query_row(
            "SELECT id, model_ready, last_heartbeat_at FROM agent_nodes WHERE id = ?",
            params![req.node_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .ok();

    let (node_id, model_ready, last_heartbeat_at) = match node_row {
        Some(r) => r,
        None => return Err(Error::new(404, "not_found", "找不到指定的节点编号")),
    };

    let is_offline = last_heartbeat_at > 0.0 && (now - last_heartbeat_at) > 90.0;
    let new_status = if is_offline {
        "offline"
    } else if last_heartbeat_at == 0.0 {
        "registered"
    } else if accepting_val == 1 && model_ready == 1 {
        "accepting"
    } else if model_ready == 1 {
        "model_ready"
    } else {
        "online"
    };

    tx.execute(
        "UPDATE agent_nodes SET accepting = ?, status = ?, updated_at = ? WHERE id = ?",
        params![accepting_val, new_status, now, node_id],
    )?;

    let msg = format!("Node {} action: {}", node_id, req.action);
    let _ = tx.execute(
        "INSERT INTO events (at, vid, kind, message) VALUES (?, NULL, 'agent_control', ?)",
        params![now, msg],
    );

    tx.commit()?;

    Ok(NodeControlResponse {
        ok: true,
        node_id,
        accepting: accepting_val == 1,
        status: new_status.to_string(),
    })
}

/// List all nodes in cluster with dynamic status and cluster-wide aggregation.
pub fn list_nodes(config: &Config) -> Result<Value> {
    let conn = crate::processing_scheduler::open_or_init(config)?;
    let now = now_sec();

    let mut stmt = conn.prepare(
        "SELECT
            id, instance_id, hostname, os, arch, agent_version,
            cpu_cores, ram_bytes, gpu_model, gpu_vram_bytes, gpu_device_uuid,
            device_group_id, status, accepting, model_ready, desired_version,
            current_version, free_ram_bytes, free_vram_bytes, gpu_util_percent,
            running_tasks, last_heartbeat_at, registered_at, updated_at
         FROM agent_nodes
         ORDER BY registered_at ASC",
    )?;

    let mut nodes = Vec::new();
    let mut total_nodes = 0;
    let mut online_nodes = 0;
    let mut accepting_nodes = 0;
    let mut device_groups = std::collections::HashSet::new();

    let rows = stmt.query_map([], |r| {
        let id: String = r.get(0)?;
        let instance_id: String = r.get(1)?;
        let hostname: String = r.get(2)?;
        let os: String = r.get(3)?;
        let arch: String = r.get(4)?;
        let agent_version: String = r.get(5)?;
        let cpu_cores: i32 = r.get(6)?;
        let ram_bytes: i64 = r.get(7)?;
        let gpu_model: Option<String> = r.get(8)?;
        let gpu_vram_bytes: i64 = r.get(9)?;
        let gpu_device_uuid: Option<String> = r.get(10)?;
        let device_group_id: String = r.get(11)?;
        let base_status: String = r.get(12)?;
        let accepting: i32 = r.get(13)?;
        let model_ready: i32 = r.get(14)?;
        let desired_version: String = r.get(15)?;
        let current_version: String = r.get(16)?;
        let free_ram_bytes: i64 = r.get(17)?;
        let free_vram_bytes: i64 = r.get(18)?;
        let gpu_util_percent: f64 = r.get(19)?;
        let running_tasks: i32 = r.get(20)?;
        let last_heartbeat_at: f64 = r.get(21)?;
        let registered_at: f64 = r.get(22)?;
        let updated_at: f64 = r.get(23)?;

        Ok((
            id, instance_id, hostname, os, arch, agent_version,
            cpu_cores, ram_bytes, gpu_model, gpu_vram_bytes, gpu_device_uuid,
            device_group_id, base_status, accepting, model_ready, desired_version,
            current_version, free_ram_bytes, free_vram_bytes, gpu_util_percent,
            running_tasks, last_heartbeat_at, registered_at, updated_at,
        ))
    })?;

    for row_res in rows {
        let (
            id, instance_id, hostname, os, arch, agent_version,
            cpu_cores, ram_bytes, gpu_model, gpu_vram_bytes, gpu_device_uuid,
            device_group_id, _base_status, accepting, model_ready, desired_version,
            current_version, free_ram_bytes, free_vram_bytes, gpu_util_percent,
            running_tasks, last_heartbeat_at, registered_at, updated_at,
        ) = row_res?;

        total_nodes += 1;
        device_groups.insert(device_group_id.clone());

        let is_stale = last_heartbeat_at > 0.0 && (now - last_heartbeat_at) > 90.0;
        let is_registered_only = last_heartbeat_at == 0.0;

        let dynamic_status = if is_stale {
            "offline"
        } else if is_registered_only {
            "registered"
        } else if accepting == 1 && model_ready == 1 {
            "accepting"
        } else if model_ready == 1 {
            "model_ready"
        } else {
            "online"
        };

        if dynamic_status == "accepting" {
            accepting_nodes += 1;
        }
        if dynamic_status != "offline" && dynamic_status != "registered" {
            online_nodes += 1;
        }

        let gpu_val = gpu_model.map(|model| {
            json!({
                "model": model,
                "vram_total_bytes": gpu_vram_bytes,
                "free_vram_bytes": if is_stale || is_registered_only { Value::Null } else { json!(free_vram_bytes) },
                "device_uuid": gpu_device_uuid.clone().unwrap_or_default(),
                "gpu_util_percent": if is_stale || is_registered_only { Value::Null } else { json!(gpu_util_percent) },
            })
        });

        nodes.push(json!({
            "node_id": id,
            "hostname": hostname,
            "os": os,
            "arch": arch,
            "agent_version": agent_version,
            "device_group_id": device_group_id,
            "status": dynamic_status,
            "accepting_tasks": accepting == 1,
            "model_ready": model_ready == 1,
            "registered_at": registered_at,
            "updated_at": updated_at,
            "desired_version": desired_version,
            "current_version": current_version,
            "telemetry": {
                "instance_id": if instance_id.is_empty() { Value::Null } else { json!(instance_id) },
                "cpu_cores": cpu_cores,
                "ram_total_bytes": ram_bytes,
                "free_ram_bytes": if is_stale || is_registered_only { Value::Null } else { json!(free_ram_bytes) },
                "last_heartbeat_at": if last_heartbeat_at == 0.0 { Value::Null } else { json!(last_heartbeat_at) },
                "running_tasks": running_tasks,
                "gpu": gpu_val,
            }
        }));
    }

    Ok(json!({
        "schema_version": 1,
        "cluster": {
            "total_nodes": total_nodes,
            "online_nodes": online_nodes,
            "accepting_nodes": accepting_nodes,
            "device_groups_count": device_groups.len(),
            "evaluated_at": now,
        },
        "nodes": nodes
    }))
}

/// Physical device pool helper: queries active task count across any agent in the same pool.
#[allow(dead_code)]
pub fn device_pool_active_tasks(conn: &Connection, device_group_id: &str) -> Result<i64> {
    let count: i64 = conn.query_row(
        "SELECT COALESCE(SUM(running_tasks), 0)
         FROM agent_nodes
         WHERE device_group_id = ? AND status IN ('model_ready', 'accepting')",
        params![device_group_id],
        |r| r.get(0),
    )?;
    Ok(count)
}
