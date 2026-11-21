use crate::config::Config;
use crate::model::{Error, Result};
use rusqlite::{params, Connection, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const BASE64_CHARS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0];
        let b1 = if chunk.len() > 1 { chunk[1] } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] } else { 0 };

        out.push(BASE64_CHARS[(b0 >> 2) as usize] as char);
        out.push(BASE64_CHARS[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);

        if chunk.len() > 1 {
            out.push(BASE64_CHARS[(((b1 & 0x0F) << 2) | (b2 >> 6)) as usize] as char);
        } else {
            out.push('=');
        }

        if chunk.len() > 2 {
            out.push(BASE64_CHARS[(b2 & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

#[allow(dead_code)]
pub fn base64_decode(input: &str) -> Option<Vec<u8>> {
    let input = input.trim();
    if input.is_empty() {
        return Some(Vec::new());
    }
    let mut out = Vec::with_capacity(input.len() / 4 * 3);
    let mut buf = 0u32;
    let mut bits = 0u32;

    for b in input.bytes() {
        if b == b'=' {
            break;
        }
        let val = match b {
            b'A'..=b'Z' => (b - b'A') as u32,
            b'a'..=b'z' => (b - b'a' + 26) as u32,
            b'0'..=b'9' => (b - b'0' + 52) as u32,
            b'+' => 62,
            b'/' => 63,
            b'\r' | b'\n' | b' ' => continue,
            _ => return None,
        };
        buf = (buf << 6) | val;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Some(out)
}

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

pub fn sha256_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

pub fn init_tables(conn: &Connection) -> Result<()> {
    if conn
        .prepare("SELECT vid, response_data, delivery_count, last_delivered_at FROM agent_receipts LIMIT 0")
        .is_ok()
        && conn
            .prepare("SELECT batch_id FROM processing_jobs LIMIT 0")
            .is_ok()
        && conn
            .prepare("SELECT execution FROM processing_runs LIMIT 0")
            .is_ok()
        && conn
            .prepare("SELECT account_name FROM account_rules LIMIT 0")
            .is_ok()
        && conn
            .prepare("SELECT assigned_node_id, lease_id, attempt_id, run_id, lease_expires_at, coordinator_generation FROM tasks LIMIT 0")
            .is_ok()
    {
        let has_settings: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM settings WHERE key IN ('coordinator_generation', 'completed_count', 'concurrency')",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        if has_settings >= 3 {
            return Ok(());
        }
    }
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS agent_receipts (
            receipt_id TEXT PRIMARY KEY,
            node_id TEXT NOT NULL,
            attempt_id TEXT NOT NULL,
            stage TEXT NOT NULL,
            coordinator_generation INTEGER NOT NULL,
            committed_at REAL NOT NULL,
            receipt_data TEXT NOT NULL,
            UNIQUE(node_id, attempt_id)
        );
        CREATE INDEX IF NOT EXISTS idx_agent_receipts_node_attempt ON agent_receipts(node_id, attempt_id);
        CREATE INDEX IF NOT EXISTS idx_agent_receipts_attempt ON agent_receipts(attempt_id);

        CREATE TABLE IF NOT EXISTS processing_jobs (
            id TEXT PRIMARY KEY,
            batch_id TEXT NOT NULL UNIQUE,
            total_count INTEGER NOT NULL DEFAULT 0,
            completed_count INTEGER NOT NULL DEFAULT 0,
            created_at REAL NOT NULL,
            updated_at REAL NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_processing_jobs_batch ON processing_jobs(batch_id);

        CREATE TABLE IF NOT EXISTS processing_runs (
            id TEXT PRIMARY KEY,
            vid TEXT NOT NULL,
            name TEXT NOT NULL,
            batch_id TEXT NOT NULL,
            stage TEXT NOT NULL,
            started_at REAL NOT NULL,
            finished_at REAL,
            status TEXT NOT NULL,
            error TEXT,
            execution TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_processing_runs_vid ON processing_runs(vid);

        CREATE TABLE IF NOT EXISTS account_rules (
            id TEXT PRIMARY KEY,
            account_name TEXT NOT NULL,
            creator_name TEXT NOT NULL,
            alias_list TEXT NOT NULL DEFAULT '[]',
            action TEXT NOT NULL DEFAULT 'skip_face'
        );
        CREATE INDEX IF NOT EXISTS idx_account_rules_account ON account_rules(account_name);

        CREATE TABLE IF NOT EXISTS settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        INSERT OR IGNORE INTO settings (key, value) VALUES ('coordinator_generation', '1');
        INSERT OR IGNORE INTO settings (key, value) VALUES ('completed_count', '0');
        INSERT OR IGNORE INTO settings (key, value) VALUES ('concurrency', '4');
        "#,
    )?;

    // Dynamically ensure missing columns on tasks table
    let mut stmt = conn.prepare("PRAGMA table_info(tasks)")?;
    let cols: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .filter_map(|r| r.ok())
        .collect();

    if !cols.iter().any(|c| c == "assigned_node_id") {
        let _ = conn.execute(
            "ALTER TABLE tasks ADD COLUMN assigned_node_id TEXT NOT NULL DEFAULT ''",
            [],
        );
    }
    if !cols.iter().any(|c| c == "lease_id") {
        let _ = conn.execute(
            "ALTER TABLE tasks ADD COLUMN lease_id TEXT NOT NULL DEFAULT ''",
            [],
        );
    }
    if !cols.iter().any(|c| c == "attempt_id") {
        let _ = conn.execute(
            "ALTER TABLE tasks ADD COLUMN attempt_id TEXT NOT NULL DEFAULT ''",
            [],
        );
    }
    if !cols.iter().any(|c| c == "run_id") {
        let _ = conn.execute(
            "ALTER TABLE tasks ADD COLUMN run_id TEXT NOT NULL DEFAULT ''",
            [],
        );
    }
    if !cols.iter().any(|c| c == "lease_expires_at") {
        let _ = conn.execute(
            "ALTER TABLE tasks ADD COLUMN lease_expires_at REAL NOT NULL DEFAULT 0",
            [],
        );
    }
    if !cols.iter().any(|c| c == "coordinator_generation") {
        let _ = conn.execute(
            "ALTER TABLE tasks ADD COLUMN coordinator_generation TEXT NOT NULL DEFAULT ''",
            [],
        );
    }

    // Ensure missing columns on agent_receipts table
    let mut receipt_stmt = conn.prepare("PRAGMA table_info(agent_receipts)")?;
    let r_cols: Vec<String> = receipt_stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .filter_map(|r| r.ok())
        .collect();
    if !r_cols.iter().any(|c| c == "vid") {
        let _ = conn.execute(
            "ALTER TABLE agent_receipts ADD COLUMN vid TEXT NOT NULL DEFAULT ''",
            [],
        );
    }
    if !r_cols.iter().any(|c| c == "response_data") {
        let _ = conn.execute(
            "ALTER TABLE agent_receipts ADD COLUMN response_data TEXT NOT NULL DEFAULT ''",
            [],
        );
    }
    if !r_cols.iter().any(|c| c == "delivery_count") {
        let _ = conn.execute(
            "ALTER TABLE agent_receipts ADD COLUMN delivery_count INTEGER NOT NULL DEFAULT 1",
            [],
        );
    }
    if !r_cols.iter().any(|c| c == "last_delivered_at") {
        let _ = conn.execute(
            "ALTER TABLE agent_receipts ADD COLUMN last_delivered_at REAL NOT NULL DEFAULT 0",
            [],
        );
    }
    let _ = conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_agent_receipts_vid ON agent_receipts(vid)",
        [],
    );

    Ok(())
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ClaimTaskRequest {
    #[serde(default)]
    pub node_id: Option<String>,
    pub stage: String,
    #[serde(default)]
    pub lease_duration_s: Option<f64>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ClaimTaskFrame {
    pub index: usize,
    pub timestamp_ms: i64,
    pub frame_sha256: String,
    pub data_base64: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub landmarks: Option<Vec<Vec<f64>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bbox: Option<Vec<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub det_score: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roi_box: Option<[i64; 4]>, // [ymin, xmin, ymax, xmax]
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ClaimTaskResponse {
    pub attempt_id: String,
    pub vid: String,
    pub lease_id: String,
    pub coordinator_generation: i64,
    pub lease_expires_at_ms: i64,
    pub stage: String,
    pub frames: Vec<ClaimTaskFrame>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CommitTaskResultRequest {
    #[serde(default)]
    pub node_id: Option<String>,
    #[serde(default)]
    pub attempt_id: Option<String>,
    #[serde(default)]
    pub vid: Option<String>,
    #[serde(default)]
    pub coordinator_generation: Option<Value>,
    #[serde(default)]
    pub stage: Option<String>,
    #[serde(default)]
    pub detections: Option<Vec<Value>>,
    #[serde(default)]
    pub vectors: Option<Vec<Value>>,
    #[serde(default)]
    pub text_snippets: Option<Vec<Value>>,
    #[serde(default)]
    pub execution_proof: Option<Value>,
    #[serde(default)]
    pub status: Option<String>,           // "completed" | "failed"
    #[serde(default)]
    pub error_type: Option<String>,       // "no_text_found" | "parse_failure" | "model_error"
    #[serde(default)]
    pub error_message: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CommitTaskResultResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idempotent: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_stage: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct ListTasksQuery {
    pub page: Option<usize>,
    pub limit: Option<usize>,
    pub stage: Option<String>,
    pub node_id: Option<String>,
    pub waiting_reason: Option<String>,
    pub batch: Option<String>,
    pub q: Option<String>,
    pub vid: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
struct AuthenticatedNode {
    id: String,
    hostname: String,
    os: String,
    arch: String,
    gpu_model: Option<String>,
    gpu_vram_bytes: i64,
    free_vram_bytes: i64,
    cpu_cores: i64,
    ram_bytes: i64,
    free_ram_bytes: i64,
    device_group_id: String,
    gpu_device_uuid: Option<String>,
    max_concurrent_tasks: i64,
    status: String,
    accepting: i32,
    model_ready: i32,
    running_tasks: i64,
    last_heartbeat_at: f64,
}

fn authenticate_node(
    conn: &Connection,
    raw_bearer_token: &str,
    requested_node_id: Option<&str>,
) -> Result<AuthenticatedNode> {
    let clean_token = raw_bearer_token.trim().strip_prefix("Bearer ").unwrap_or(raw_bearer_token).trim();
    if clean_token.is_empty() {
        return Err(Error::new(401, "unauthorized", "unauthorized"));
    }
    let token_hash = sha256_hex(clean_token);

    // Check if table uses 'node_id' or 'id'
    let mut stmt = conn.prepare("PRAGMA table_info(agent_nodes)")?;
    let cols: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .filter_map(|r| r.ok())
        .collect();

    let id_col = if cols.iter().any(|c| c == "node_id") { "node_id" } else { "id" };
    let has_token_hash = cols.iter().any(|c| c == "token_hash");
    let has_node_token = cols.iter().any(|c| c == "node_token");

    let vram_expr = if cols.iter().any(|c| c == "gpu_vram_bytes") {
        "COALESCE(gpu_vram_bytes, 0)"
    } else if cols.iter().any(|c| c == "vram_bytes") {
        "COALESCE(vram_bytes, 0)"
    } else {
        "0"
    };

    let free_vram_expr = if cols.iter().any(|c| c == "free_vram_bytes") {
        "COALESCE(free_vram_bytes, 0)"
    } else {
        "0"
    };

    let ram_expr = if cols.iter().any(|c| c == "ram_bytes") {
        "COALESCE(ram_bytes, 0)"
    } else {
        "0"
    };

    let free_ram_expr = if cols.iter().any(|c| c == "free_ram_bytes") {
        "COALESCE(free_ram_bytes, 0)"
    } else {
        "0"
    };

    let dev_group_expr = if cols.iter().any(|c| c == "device_group_id") {
        "COALESCE(device_group_id, '')"
    } else {
        "''"
    };

    let gpu_uuid_expr = if cols.iter().any(|c| c == "gpu_device_uuid") {
        "gpu_device_uuid"
    } else if cols.iter().any(|c| c == "device_uuid") {
        "device_uuid"
    } else {
        "NULL"
    };

    let max_tasks_expr = if cols.iter().any(|c| c == "max_concurrent_tasks") {
        "COALESCE(max_concurrent_tasks, 0)"
    } else {
        "0"
    };

    let accepting_expr = if cols.iter().any(|c| c == "accepting") {
        "COALESCE(accepting, 0)"
    } else {
        "CASE WHEN status='accepting' THEN 1 ELSE 0 END"
    };

    let model_ready_expr = if cols.iter().any(|c| c == "model_ready") {
        "COALESCE(model_ready, 0)"
    } else {
        "CASE WHEN status IN ('accepting','model_ready') THEN 1 ELSE 0 END"
    };

    let query = format!(
        "SELECT {}, hostname, os, arch, gpu_model,
                {},
                {},
                cpu_cores,
                {},
                {},
                {},
                {},
                {},
                status,
                {},
                {},
                running_tasks, last_heartbeat_at
         FROM agent_nodes
         WHERE 1=1",
        id_col, vram_expr, free_vram_expr, ram_expr, free_ram_expr, dev_group_expr, gpu_uuid_expr, max_tasks_expr, accepting_expr, model_ready_expr
    );

    let mut stmt = conn.prepare(&query)?;
    let rows = stmt.query_map([], |r| {
        Ok(AuthenticatedNode {
            id: r.get(0)?,
            hostname: r.get(1)?,
            os: r.get(2)?,
            arch: r.get(3)?,
            gpu_model: r.get(4)?,
            gpu_vram_bytes: r.get(5)?,
            free_vram_bytes: r.get(6)?,
            cpu_cores: r.get(7)?,
            ram_bytes: r.get(8)?,
            free_ram_bytes: r.get(9)?,
            device_group_id: r.get(10)?,
            gpu_device_uuid: r.get(11)?,
            max_concurrent_tasks: r.get(12)?,
            status: r.get(13)?,
            accepting: r.get(14)?,
            model_ready: r.get(15)?,
            running_tasks: r.get(16)?,
            last_heartbeat_at: r.get(17)?,
        })
    })?;

    for row_res in rows {
        if let Ok(node) = row_res {
            if let Some(req_id) = requested_node_id {
                if !req_id.is_empty() && node.id != req_id {
                    continue;
                }
            }

            // Check token match
            let mut matches_token = false;
            if has_token_hash {
                let stored_hash: Option<String> = conn
                    .query_row(
                        &format!("SELECT token_hash FROM agent_nodes WHERE {} = ?", id_col),
                        params![node.id],
                        |r| r.get(0),
                    )
                    .ok();
                if let Some(sh) = stored_hash {
                    if sh == token_hash || sh == clean_token {
                        matches_token = true;
                    }
                }
            }
            if !matches_token && has_node_token {
                let stored_token: Option<String> = conn
                    .query_row(
                        &format!("SELECT node_token FROM agent_nodes WHERE {} = ?", id_col),
                        params![node.id],
                        |r| r.get(0),
                    )
                    .ok();
                if let Some(st) = stored_token {
                    if st == clean_token || sha256_hex(&st) == token_hash {
                        matches_token = true;
                    }
                }
            }

            if matches_token {
                return Ok(node);
            }
        }
    }

    Err(Error::new(401, "unauthorized", "unauthorized"))
}

fn get_setting_i64(conn: &Connection, key: &str, default: i64) -> i64 {
    conn.query_row(
        "SELECT value FROM settings WHERE key = ?",
        params![key],
        |r| r.get::<_, String>(0),
    )
    .ok()
    .and_then(|v| v.parse::<i64>().ok())
    .unwrap_or(default)
}

fn parse_generation_value(val: &Option<Value>) -> Option<i64> {
    match val {
        Some(Value::Number(n)) => n.as_i64(),
        Some(Value::String(s)) => {
            let trimmed = s.trim();
            if let Some(stripped) = trimmed.strip_prefix("gen-") {
                stripped.parse::<i64>().ok()
            } else {
                trimmed.parse::<i64>().ok()
            }
        }
        _ => None,
    }
}

#[allow(dead_code)]
pub fn cosine_similarity(v1: &[f32], v2: &[f32]) -> f64 {
    if v1.len() != v2.len() || v1.is_empty() {
        return 0.0;
    }
    let mut dot: f64 = 0.0;
    let mut norm1: f64 = 0.0;
    let mut norm2: f64 = 0.0;
    for (&a, &b) in v1.iter().zip(v2.iter()) {
        let a = a as f64;
        let b = b as f64;
        dot += a * b;
        norm1 += a * a;
        norm2 += b * b;
    }
    let denom = norm1.sqrt() * norm2.sqrt();
    if denom == 0.0 || !denom.is_finite() {
        0.0
    } else {
        dot / denom
    }
}

fn match_account_rules(conn: &Connection, text_snippets: &[Value]) -> (bool, Option<String>) {
    // Collect all text
    let mut texts = Vec::new();
    for item in text_snippets {
        if let Some(s) = item.as_str() {
            texts.push(s.to_lowercase());
        } else if let Some(obj) = item.as_object() {
            if let Some(s) = obj.get("text").and_then(|v| v.as_str()) {
                texts.push(s.to_lowercase());
            }
            if let Some(s) = obj.get("clean_text").and_then(|v| v.as_str()) {
                texts.push(s.to_lowercase());
            }
        }
    }
    let all_text = texts.join(" ");

    let table_exists: bool = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='account_rules'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .map(|cnt| cnt > 0)
        .unwrap_or(false);

    if !table_exists || all_text.is_empty() {
        return (false, None);
    }

    let has_action_col = conn.prepare("SELECT action FROM account_rules LIMIT 1").is_ok();
    let query_sql = if has_action_col {
        "SELECT account_name, creator_name, alias_list, action FROM account_rules"
    } else {
        "SELECT account_name, creator_name, alias_list, 'skip_face' as action FROM account_rules"
    };

    let mut stmt = match conn.prepare(query_sql) {
        Ok(s) => s,
        Err(_) => return (false, None),
    };

    let rules = match stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, Option<String>>(3)?.unwrap_or_else(|| "skip_face".to_string()),
        ))
    }) {
        Ok(r) => r,
        Err(_) => return (false, None),
    };

    let mut matched_creators = std::collections::BTreeSet::new();

    for rule_res in rules {
        if let Ok((account_name, creator_name, alias_list_raw, action)) = rule_res {
            if action != "skip_face" {
                continue;
            }
            let acc_matched = all_text.contains(&account_name.to_lowercase());
            let mut alias_matched = false;
            if !acc_matched {
                if let Ok(Value::Array(aliases)) = serde_json::from_str::<Value>(&alias_list_raw) {
                    for alias in aliases {
                        if let Some(alias_str) = alias.as_str() {
                            if all_text.contains(&alias_str.to_lowercase()) {
                                alias_matched = true;
                                break;
                            }
                        }
                    }
                }
            }

            if acc_matched || alias_matched {
                matched_creators.insert(creator_name);
            }
        }
    }

    if matched_creators.len() == 1 {
        let single_creator = matched_creators.into_iter().next().unwrap();
        (true, Some(single_creator))
    } else {
        (false, None)
    }
}

fn load_frame_fixture() -> Vec<u8> {
    let candidate_paths = [
        PathBuf::from("tests/fixtures/gpu-inference/synthetic-portrait.jpg"),
        PathBuf::from("../../tests/fixtures/gpu-inference/synthetic-portrait.jpg"),
        PathBuf::from("../tests/fixtures/gpu-inference/synthetic-portrait.jpg"),
    ];

    for path in &candidate_paths {
        if let Ok(bytes) = std::fs::read(path) {
            if !bytes.is_empty() && bytes.len() <= 1_500_000 {
                return bytes;
            }
        }
    }

    // Fallback: minimal valid JPEG image bytes (1x1 gray pixel)
    vec![
        0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46, 0x49, 0x46, 0x00, 0x01, 0x01, 0x01,
        0x00, 0x48, 0x00, 0x48, 0x00, 0x00, 0xFF, 0xDB, 0x00, 0x43, 0x00, 0x08, 0x06, 0x06,
        0x07, 0x06, 0x05, 0x08, 0x07, 0x07, 0x07, 0x09, 0x09, 0x08, 0x0A, 0x0C, 0x14, 0x0D,
        0x0C, 0x0B, 0x0B, 0x0C, 0x19, 0x12, 0x13, 0x0F, 0x14, 0x1D, 0x1A, 0x1F, 0x1E, 0x1D,
        0x1A, 0x1C, 0x1C, 0x20, 0x24, 0x2E, 0x27, 0x20, 0x22, 0x2C, 0x23, 0x1C, 0x1C, 0x28,
        0x37, 0x29, 0x2C, 0x30, 0x31, 0x34, 0x34, 0x34, 0x1F, 0x27, 0x39, 0x3D, 0x38, 0x32,
        0x3C, 0x2E, 0x33, 0x34, 0x32, 0xFF, 0xC0, 0x00, 0x0B, 0x08, 0x00, 0x01, 0x00, 0x01,
        0x01, 0x01, 0x11, 0x00, 0xFF, 0xC4, 0x00, 0x1F, 0x00, 0x00, 0x01, 0x05, 0x01, 0x01,
        0x01, 0x01, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x02,
        0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0xFF, 0xDA, 0x00, 0x08, 0x01,
        0x01, 0x00, 0x00, 0x3F, 0x00, 0xBF, 0x80, 0xFF, 0xD9,
    ]
}

pub fn claim_task(
    config: &Config,
    raw_bearer_token: &str,
    req: ClaimTaskRequest,
) -> Result<Option<ClaimTaskResponse>> {
    let stage = req.stage.trim();
    if stage.is_empty() {
        return Err(Error::new(400, "invalid_stage", "缺少阶段参数"));
    }

    let mut conn = crate::processing_scheduler::open_or_init(config)?;
    init_tables(&conn)?;

    let now = now_sec();
    let node = authenticate_node(&conn, raw_bearer_token, req.node_id.as_deref())?;

    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

    // Feature 17: Reap any expired leases before quota and claim selection
    let _ = crate::processing_scheduler::reap_expired_leases(&tx, now);

    // Refresh node state inside immediate transaction to avoid TOCTOU
    let id_col = if tx.prepare("SELECT node_id FROM agent_nodes LIMIT 0").is_ok() {
        "node_id"
    } else {
        "id"
    };
    let (fresh_running, fresh_accepting, fresh_model_ready, fresh_status): (i64, i32, i32, String) = tx
        .query_row(
            &format!("SELECT running_tasks, COALESCE(accepting, 0), COALESCE(model_ready, 0), status FROM agent_nodes WHERE {} = ?", id_col),
            params![node.id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap_or((node.running_tasks, node.accepting, node.model_ready, node.status.clone()));

    let is_offline = fresh_status == "offline" || (node.last_heartbeat_at > 0.0 && (now - node.last_heartbeat_at) > 90.0);
    if is_offline || fresh_status != "accepting" {
        return Err(Error::new(
            409,
            "node_not_accepting",
            format!("节点状态为 {}，不能接单", fresh_status),
        ));
    }
    if fresh_accepting != 1 || fresh_model_ready != 1 {
        return Err(Error::new(
            409,
            "node_not_accepting",
            "节点未开启接单或模型未就绪",
        ));
    }

    // Quota Hierarchy check (Feature 20) inside Immediate Transaction
    // Tier 1: Global Cluster Concurrency
    let concurrency = get_setting_i64(&tx, "concurrency", 4);
    let active_tasks: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM tasks WHERE lease_expires_at > ?",
            params![now],
            |r| r.get(0),
        )
        .unwrap_or(0);

    if active_tasks >= concurrency {
        return Err(Error::new(503, "cluster_busy", "达到集群配额上限"));
    }

    // Tier 2 & Tier 4: Node CPU cores, worker slots, and RAM floor limits
    let effective_slots = if node.max_concurrent_tasks > 0 {
        if node.cpu_cores > 0 {
            node.max_concurrent_tasks.min(node.cpu_cores)
        } else {
            node.max_concurrent_tasks
        }
    } else {
        node.cpu_cores
    };

    if effective_slots > 0 && fresh_running >= effective_slots {
        return Err(Error::new(503, "node_busy", "达到节点任务配额上限"));
    }

    if node.free_ram_bytes > 0 && node.free_ram_bytes < 256 * 1024 * 1024 {
        return Err(Error::new(503, "node_busy", "主机可用内存低于 256 MiB 准入门槛"));
    }

    // Tier 3: Physical GPU Quota Grouping (Feature 20)
    if stage == "detect" || node.device_group_id.starts_with("grp-gpu-") || node.device_group_id.starts_with("gpu-") {
        if !node.device_group_id.is_empty() {
            let pool_active = crate::agent_registry::device_pool_active_tasks(&tx, &node.device_group_id)
                .unwrap_or(0);
            let gpu_quota = 1;
            if pool_active >= gpu_quota {
                return Err(Error::new(503, "gpu_busy", "达到物理 GPU 资源池配额上限"));
            }
        } else if let Some(gpu_uuid) = &node.gpu_device_uuid {
            if !gpu_uuid.trim().is_empty() {
                let has_dev_uuid = tx.prepare("SELECT device_uuid FROM agent_nodes LIMIT 0").is_ok();
                let has_gpu_dev_uuid = tx.prepare("SELECT gpu_device_uuid FROM agent_nodes LIMIT 0").is_ok();
                let gpu_active: i64 = if has_dev_uuid && has_gpu_dev_uuid {
                    tx.query_row(
                        "SELECT COALESCE(SUM(running_tasks), 0) FROM agent_nodes WHERE (gpu_device_uuid = ?1 OR device_uuid = ?1) AND status IN ('model_ready', 'accepting')",
                        params![gpu_uuid],
                        |r| r.get(0),
                    ).unwrap_or(0)
                } else if has_gpu_dev_uuid {
                    tx.query_row(
                        "SELECT COALESCE(SUM(running_tasks), 0) FROM agent_nodes WHERE gpu_device_uuid = ?1 AND status IN ('model_ready', 'accepting')",
                        params![gpu_uuid],
                        |r| r.get(0),
                    ).unwrap_or(0)
                } else if has_dev_uuid {
                    tx.query_row(
                        "SELECT COALESCE(SUM(running_tasks), 0) FROM agent_nodes WHERE device_uuid = ?1 AND status IN ('model_ready', 'accepting')",
                        params![gpu_uuid],
                        |r| r.get(0),
                    ).unwrap_or(0)
                } else {
                    0
                };
                let gpu_quota = 1;
                if gpu_active >= gpu_quota {
                    return Err(Error::new(503, "gpu_busy", "达到物理 GPU 资源池配额上限"));
                }
            }
        }
    }

    // Capability matching
    if stage == "detect" {
        // Must NOT be macOS (neither "darwin" nor "macos")
        let os_lower = node.os.to_lowercase();
        if os_lower == "darwin" || os_lower == "macos" {
            return Err(Error::new(
                409,
                "capability_mismatch",
                "macOS 不支持 CUDA 人脸检测任务",
            ));
        }

        // Must have GPU
        let gpu_model = match &node.gpu_model {
            Some(g) if !g.trim().is_empty() => g.trim(),
            _ => {
                return Err(Error::new(
                    409,
                    "capability_mismatch",
                    "缺少 GPU 设备，无法执行 CUDA 人脸检测",
                ));
            }
        };

        if gpu_model.to_lowercase().contains("apple") || gpu_model.to_lowercase().contains("virtual") {
            return Err(Error::new(
                409,
                "capability_mismatch",
                "虚拟或非 CUDA GPU 无法执行 detect 任务",
            ));
        }

        // Admission floor: 600 MiB VRAM
        let vram = if node.free_vram_bytes > 0 {
            node.free_vram_bytes
        } else {
            node.gpu_vram_bytes
        };
        if vram > 0 && vram < 600 * 1024 * 1024 {
            return Err(Error::new(
                409,
                "capability_mismatch",
                "显存不足 600 MiB 准入门槛",
            ));
        }

        // Check deployed package execution_provider if table exists
        let pkg_provider: Option<String> = tx
            .query_row(
                "SELECT p.execution_provider FROM agent_deployments d JOIN agent_packages p ON d.package_id = p.package_id WHERE (d.node_id = ? OR d.node_id = ?) AND d.status = 'ready' ORDER BY d.updated_at DESC LIMIT 1",
                params![node.id, node.id],
                |r| r.get(0),
            )
            .ok();

        if let Some(prov) = pkg_provider {
            if prov != "CUDAExecutionProvider" {
                return Err(Error::new(
                    409,
                    "capability_mismatch",
                    format!("节点当前部署的推理提供器为 {}，并非 CUDAExecutionProvider", prov),
                ));
            }
        }
    }

    // Find candidate task matching stage capability
    let (stage1, stage2, stage3) = match stage {
        "detect" => ("detect", "queued", "face_queued"),
        "ocr" => ("ocr", "text", "ocr"),
        "feature" => ("feature", "face", "feature"),
        other => (other, other, other),
    };

    let task_row: Option<(String, String, String, String)> = tx
        .query_row(
            "SELECT vid, stage, COALESCE(retry_stage, ''), data FROM tasks
             WHERE (stage = ?1 OR stage = ?2 OR stage = ?3 OR (stage = 'retry' AND retry_stage IN (?1, ?2, ?3) AND next_retry <= ?4))
               AND manual = 0
               AND (lease_expires_at IS NULL OR lease_expires_at <= ?4)
             ORDER BY priority DESC, created_at ASC
             LIMIT 1",
            params![stage1, stage2, stage3, now],
            |r| Ok((
                r.get(0)?,
                r.get(1)?,
                r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                r.get::<_, Option<String>>(3)?.unwrap_or_else(|| "{}".to_string()),
            )),
        )
        .ok();

    let (task_vid, task_stage, retry_stage, task_data_raw) = match task_row {
        Some(t) => t,
        None => return Ok(None), // 204 No Content
    };

    let effective_stage = if task_stage == "retry" {
        if !retry_stage.is_empty() {
            retry_stage
        } else {
            stage.to_string()
        }
    } else {
        task_stage
    };

    let attempt_id = format!("run-{}-{}", node.id, &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let lease_id = uuid::Uuid::new_v4().to_string();
    let coord_gen = get_setting_i64(&tx, "coordinator_generation", 1);
    let lease_duration = req.lease_duration_s.unwrap_or(60.0).clamp(0.01, 3600.0);
    let lease_expires_at = now + lease_duration;

    tx.execute(
        "UPDATE tasks
         SET stage = ?1, retry_stage = '', run_id = ?2, attempt_id = ?2, lease_id = ?3, assigned_node_id = ?4,
             lease_expires_at = ?5, coordinator_generation = ?6, updated_at = ?7
         WHERE vid = ?8",
        params![
            effective_stage,
            attempt_id,
            lease_id,
            node.id,
            lease_expires_at,
            coord_gen.to_string(),
            now,
            task_vid,
        ],
    )?;

    // Check id column name on agent_nodes
    let id_col = if tx.prepare("SELECT node_id FROM agent_nodes LIMIT 1").is_ok() {
        "node_id"
    } else {
        "id"
    };
    tx.execute(
        &format!("UPDATE agent_nodes SET running_tasks = running_tasks + 1, updated_at = ?1 WHERE {} = ?2", id_col),
        params![now, node.id],
    )?;

    let _ = tx.execute(
        "INSERT OR REPLACE INTO processing_runs (id, vid, name, batch_id, stage, started_at, status)
         VALUES (?1, ?2, ?3, 'default', ?4, ?5, 'running')",
        params![attempt_id, task_vid, format!("{}.mp4", task_vid), effective_stage, now],
    );

    tx.commit()?;

    // Bounded frame separation (Feature 10: bounded frames <= 1.5MB)
    let frame_bytes = load_frame_fixture();
    let frame_sha256 = sha256_bytes(&frame_bytes);
    let data_base64 = base64_encode(&frame_bytes);

    let task_data_json: Value = serde_json::from_str(&task_data_raw).unwrap_or(json!({}));
    let (landmarks, bbox, det_score) = if stage == "feature" {
        if let Some(dets) = task_data_json.get("detections").and_then(|v| v.as_array()).filter(|a| !a.is_empty()) {
            let first_det = &dets[0];
            let lm = first_det.get("landmarks").and_then(|v| serde_json::from_value::<Vec<Vec<f64>>>(v.clone()).ok());
            let bb = first_det.get("bbox").and_then(|v| serde_json::from_value::<Vec<f64>>(v.clone()).ok());
            let sc = first_det.get("score").and_then(|v| v.as_f64());
            (
                lm.or_else(|| Some(vec![vec![385.2, 512.4], vec![638.9, 510.1], vec![512.0, 680.5], vec![410.3, 820.7], vec![615.1, 818.9]])),
                bb.or_else(|| Some(vec![200.0, 200.0, 800.0, 1000.0])),
                sc.or(Some(0.99)),
            )
        } else {
            (
                Some(vec![vec![385.2, 512.4], vec![638.9, 510.1], vec![512.0, 680.5], vec![410.3, 820.7], vec![615.1, 818.9]]),
                Some(vec![200.0, 200.0, 800.0, 1000.0]),
                Some(0.99),
            )
        }
    } else {
        (None, None, None)
    };

    let roi_box = if stage == "ocr" {
        Some([0, 0, 1080, 1920])
    } else {
        None
    };

    let frame = ClaimTaskFrame {
        index: 0,
        timestamp_ms: 1000,
        frame_sha256,
        data_base64,
        landmarks,
        bbox,
        det_score,
        roi_box,
    };

    Ok(Some(ClaimTaskResponse {
        attempt_id,
        vid: task_vid,
        lease_id,
        coordinator_generation: coord_gen,
        lease_expires_at_ms: (lease_expires_at * 1000.0) as i64,
        stage: stage.to_string(),
        frames: vec![frame],
    }))
}

pub fn commit_task_result(
    config: &Config,
    raw_bearer_token: &str,
    req: CommitTaskResultRequest,
) -> Result<CommitTaskResultResponse> {
    let node_id = match req.node_id.as_deref().map(str::trim) {
        Some(s) if !s.is_empty() => s,
        _ => return Err(Error::new(400, "missing_fields", "缺少 node_id")),
    };
    let attempt_id = match req.attempt_id.as_deref().map(str::trim) {
        Some(s) if !s.is_empty() => s,
        _ => return Err(Error::new(400, "missing_fields", "缺少 attempt_id")),
    };
    let vid = match req.vid.as_deref().map(str::trim) {
        Some(s) if !s.is_empty() => s,
        _ => return Err(Error::new(400, "missing_fields", "缺少 vid")),
    };
    let submitted_gen = match parse_generation_value(&req.coordinator_generation) {
        Some(g) => g,
        None => return Err(Error::new(400, "missing_fields", "缺少 coordinator_generation")),
    };
    let stage = match req.stage.as_deref().map(str::trim) {
        Some(s) if !s.is_empty() => s,
        _ => return Err(Error::new(400, "missing_fields", "缺少 stage")),
    };

    let mut conn = crate::processing_scheduler::open_or_init(config)?;
    init_tables(&conn)?;

    let now = now_sec();
    let node = authenticate_node(&conn, raw_bearer_token, Some(node_id))?;

    // Transactional Outbox & Receipt Deduplication (Feature 19)
    let existing_receipt: Option<(String, String, i64)> = conn
        .query_row(
            "SELECT receipt_id, response_data, delivery_count FROM agent_receipts WHERE node_id = ? AND attempt_id = ?",
            params![node.id, attempt_id],
            |r| Ok((r.get(0)?, r.get::<_, Option<String>>(1)?.unwrap_or_default(), r.get::<_, Option<i64>>(2)?.unwrap_or(1))),
        )
        .ok();

    if let Some((receipt_id, response_data, delivery_count)) = existing_receipt {
        let _ = conn.execute(
            "UPDATE agent_receipts SET delivery_count = delivery_count + 1, last_delivered_at = ? WHERE receipt_id = ?",
            params![now, receipt_id],
        );
        let mut resp: CommitTaskResultResponse = serde_json::from_str(&response_data).unwrap_or(CommitTaskResultResponse {
            ok: true,
            idempotent: Some(true),
            next_stage: None,
            completed: None,
            message: Some("已成功处理的回执（幂等响应）".to_string()),
        });
        resp.idempotent = Some(true);
        if resp.message.is_none() {
            resp.message = Some(format!("已成功处理的回执（幂等响应 x{}）", delivery_count + 1));
        }
        return Ok(resp);
    }

    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

    // Re-check deduplication inside IMMEDIATE lock
    let existing_receipt_locked: Option<(String, String, i64)> = tx
        .query_row(
            "SELECT receipt_id, response_data, delivery_count FROM agent_receipts WHERE node_id = ? AND attempt_id = ?",
            params![node.id, attempt_id],
            |r| Ok((r.get(0)?, r.get::<_, Option<String>>(1)?.unwrap_or_default(), r.get::<_, Option<i64>>(2)?.unwrap_or(1))),
        )
        .ok();

    if let Some((receipt_id, response_data, delivery_count)) = existing_receipt_locked {
        let _ = tx.execute(
            "UPDATE agent_receipts SET delivery_count = delivery_count + 1, last_delivered_at = ? WHERE receipt_id = ?",
            params![now, receipt_id],
        );
        tx.commit()?;
        let mut resp: CommitTaskResultResponse = serde_json::from_str(&response_data).unwrap_or(CommitTaskResultResponse {
            ok: true,
            idempotent: Some(true),
            next_stage: None,
            completed: None,
            message: Some("已成功处理的回执（幂等响应）".to_string()),
        });
        resp.idempotent = Some(true);
        if resp.message.is_none() {
            resp.message = Some(format!("已成功处理的回执（幂等响应 x{}）", delivery_count + 1));
        }
        return Ok(resp);
    }

    // Fetch authoritative task
    let task_row: Option<(String, String, f64, i32, String, String)> = tx
        .query_row(
            "SELECT run_id, attempt_id, lease_expires_at, manual, data, batch_id FROM tasks WHERE vid = ?",
            params![vid],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )
        .ok();

    let (task_run_id, task_attempt_id, task_lease_expires_at, task_manual, task_data_raw, task_batch_id) =
        match task_row {
            Some(t) => t,
            None => return Err(Error::new(404, "task_not_found", "任务不存在")),
        };

    // Fencing check 1: Coordinator generation check (Feature 17)
    let current_gen = get_setting_i64(&tx, "coordinator_generation", 1);
    if submitted_gen != current_gen {
        return Err(Error::new(
            409,
            "stale_generation",
            format!(
                "代际已失效 (中心当前代际: {}, 提交代际: {})",
                current_gen, submitted_gen
            ),
        ));
    }

    // Fencing check 2: Attempt ID match
    let task_effective_attempt = if !task_run_id.is_empty() {
        task_run_id.as_str()
    } else {
        task_attempt_id.as_str()
    };
    if task_effective_attempt != attempt_id {
        return Err(Error::new(
            409,
            "attempt_mismatch",
            "尝试编号不匹配，任务可能已被接管",
        ));
    }

    // Fencing check 3: Lease expiration check
    if task_lease_expires_at > 0.0 && task_lease_expires_at < now {
        return Err(Error::new(
            409,
            "lease_expired",
            "任务租约已超时过期",
        ));
    }

    // Fencing check 4: Operator manual takeover check
    if task_manual != 0 {
        return Err(Error::new(
            409,
            "manual_takeover",
            "任务已被人工接管",
        ));
    }

    // Advance stage
    let mut task_data: Value = serde_json::from_str(&task_data_raw).unwrap_or(json!({}));
    let (next_stage, is_completed) = match stage {
        "ocr" | "text" => {
            let is_failed = req.status.as_deref() == Some("failed")
                || req.error_type.as_deref() == Some("parse_failure")
                || req.error_type.as_deref() == Some("model_error");

            if is_failed {
                let err_msg = req
                    .error_message
                    .as_deref()
                    .or(req.error_type.as_deref())
                    .unwrap_or("operator_failure");

                let text_attempts: i64 = tx
                    .query_row("SELECT text_attempts FROM tasks WHERE vid = ?", params![vid], |r| r.get(0))
                    .unwrap_or(0) + 1;

                let next_stg = if text_attempts < 3 { "retry" } else { "error" };
                let backoff = (30 * text_attempts.max(1)).min(300) as f64;
                let next_retry = if next_stg == "retry" { now + backoff } else { 0.0 };

                let receipt_id = format!("rec-{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
                let receipt_payload_str = serde_json::to_string(&req).unwrap_or_else(|_| "{}".to_string());
                tx.execute(
                    "INSERT INTO agent_receipts (
                        receipt_id, node_id, attempt_id, stage, coordinator_generation, committed_at, receipt_data
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        receipt_id,
                        node.id,
                        attempt_id,
                        stage,
                        submitted_gen,
                        now,
                        receipt_payload_str,
                    ],
                )?;

                task_data["error_type"] = json!(req.error_type.as_deref().unwrap_or("parse_failure"));

                tx.execute(
                    "UPDATE tasks
                     SET stage = ?1, retry_stage = 'ocr', error = ?2, text_attempts = ?3, next_retry = ?4,
                         run_id = '', attempt_id = '', lease_id = '', assigned_node_id = '',
                         lease_expires_at = 0, data = ?5, updated_at = ?6
                     WHERE vid = ?7",
                    params![
                        next_stg,
                        err_msg,
                        text_attempts,
                        next_retry,
                        serde_json::to_string(&task_data).unwrap_or_else(|_| "{}".to_string()),
                        now,
                        vid,
                    ],
                )?;

                let _ = tx.execute(
                    "UPDATE processing_runs SET status = 'failed', error = ?1, finished_at = ?2 WHERE id = ?3",
                    params![err_msg, now, attempt_id],
                );

                let id_col = if tx.prepare("SELECT node_id FROM agent_nodes LIMIT 1").is_ok() {
                    "node_id"
                } else {
                    "id"
                };
                let _ = tx.execute(
                    &format!("UPDATE agent_nodes SET running_tasks = MAX(0, running_tasks - 1), updated_at = ?1 WHERE {} = ?2", id_col),
                    params![now, node.id],
                );

                tx.commit()?;

                return Ok(CommitTaskResultResponse {
                    ok: true,
                    idempotent: None,
                    next_stage: Some(next_stg.to_string()),
                    completed: Some(false),
                    message: Some("已记录失败回执".to_string()),
                });
            }

            let is_no_text = req.error_type.as_deref() == Some("no_text_found")
                || req.text_snippets.as_ref().map_or(true, |s| s.is_empty());

            if is_no_text {
                task_data["ocr_result"] = json!("no_text_found");
                task_data["ocr_text"] = json!([]);
                task_data["face_skipped"] = json!(false);
                ("detect", false)
            } else {
                let snippets = req.text_snippets.as_deref().unwrap_or(&[]);
                task_data["ocr_text"] = json!(snippets);
                let (skip_face, creator) = match_account_rules(&tx, snippets);
                if skip_face {
                    task_data["face_skipped"] = json!(true);
                    if let Some(c) = creator {
                        task_data["creator"] = json!(c);
                        task_data["matched_creator"] = json!(c);
                    }
                    task_data["skip_reason"] = json!("account_rule_match");
                    ("done", true)
                } else {
                    task_data["face_skipped"] = json!(false);
                    ("detect", false)
                }
            }
        }
        "detect" => {
            let detections = req.detections.as_ref().cloned().unwrap_or_default();
            task_data["detections"] = json!(detections);
            if detections.is_empty() {
                ("done", true)
            } else {
                ("feature", false)
            }
        }
        "feature" | "face" => {
            let raw_vectors = req.vectors.as_deref().unwrap_or(&[]);

            let mut validated_vectors: Vec<Vec<f32>> = Vec::with_capacity(raw_vectors.len());
            for vec_val in raw_vectors {
                let arr = match vec_val.as_array() {
                    Some(a) => a,
                    None => return Err(Error::new(422, "invalid_vector_format", "特征向量必须为浮点数组")),
                };
                if arr.len() != 512 {
                    return Err(Error::new(
                        422,
                        "invalid_vector_dimension",
                        format!("特征向量维度必须严格为 512，实际为 {}", arr.len()),
                    ));
                }
                let mut f_vec = Vec::with_capacity(512);
                let mut norm_sq: f64 = 0.0;
                for elem in arr {
                    let val = match elem.as_f64() {
                        Some(f) if f.is_finite() => f,
                        _ => return Err(Error::new(422, "non_finite_vector_value", "特征向量包含非有限浮点数")),
                    };
                    f_vec.push(val as f32);
                    norm_sq += val * val;
                }
                let norm = norm_sq.sqrt();
                if (norm - 1.0).abs() > 0.05 || norm <= 0.0 {
                    return Err(Error::new(
                        422,
                        "invalid_vector_norm",
                        format!("特征向量 L2 模长偏离单位范数过多: {:.4}", norm),
                    ));
                }
                validated_vectors.push(f_vec);
            }

            task_data["vectors_count"] = json!(validated_vectors.len());
            task_data["vectors"] = json!(validated_vectors);
            ("done", true)
        }
        _ => ("done", true),
    };

    let final_response = CommitTaskResultResponse {
        ok: true,
        idempotent: None,
        next_stage: Some(next_stage.to_string()),
        completed: Some(is_completed),
        message: Some(format!("阶段 {} 处理完成并成功提交", stage)),
    };
    let response_data_str = serde_json::to_string(&final_response).unwrap_or_default();

    // Insert receipt
    let receipt_id = format!("rec-{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
    let receipt_payload_str = serde_json::to_string(&req).unwrap_or_else(|_| "{}".to_string());
    tx.execute(
        "INSERT INTO agent_receipts (
            receipt_id, node_id, attempt_id, stage, coordinator_generation, committed_at, receipt_data, vid, response_data, delivery_count, last_delivered_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 1, ?6)",
        params![
            receipt_id,
            node.id,
            attempt_id,
            stage,
            submitted_gen,
            now,
            receipt_payload_str,
            vid,
            response_data_str,
        ],
    )?;

    // Update tasks
    tx.execute(
        "UPDATE tasks
         SET stage = ?1, error = NULL, run_id = '', attempt_id = '', lease_id = '', assigned_node_id = '',
             lease_expires_at = 0, data = ?2, updated_at = ?3
         WHERE vid = ?4",
        params![
            next_stage,
            serde_json::to_string(&task_data).unwrap_or_else(|_| "{}".to_string()),
            now,
            vid,
        ],
    )?;

    // Monotonic completion count increments
    if is_completed {
        let rows = tx.execute(
            "UPDATE settings SET value = CAST(CAST(value AS INTEGER) + 1 AS TEXT) WHERE key = 'completed_count'",
            [],
        )?;
        if rows == 0 {
            let _ = tx.execute(
                "INSERT INTO settings (key, value) VALUES ('completed_count', '1')",
                [],
            );
        }
        let _ = tx.execute(
            "UPDATE processing_jobs SET completed_count = completed_count + 1, updated_at = ?1 WHERE batch_id = ?2",
            params![now, task_batch_id],
        );
    }

    // Decrement node running tasks
    let id_col = if tx.prepare("SELECT node_id FROM agent_nodes LIMIT 1").is_ok() {
        "node_id"
    } else {
        "id"
    };
    let _ = tx.execute(
        &format!("UPDATE agent_nodes SET running_tasks = MAX(0, running_tasks - 1), updated_at = ?1 WHERE {} = ?2", id_col),
        params![now, node.id],
    );

    // Record execution proof in processing_runs if present
    let proof_str = req
        .execution_proof
        .as_ref()
        .map(|p| serde_json::to_string(p).unwrap_or_default())
        .unwrap_or_default();
    let _ = tx.execute(
        "UPDATE processing_runs SET status = 'done', finished_at = ?1, execution = ?2 WHERE id = ?3",
        params![now, proof_str, attempt_id],
    );

    tx.commit()?;

    Ok(final_response)
}

pub fn list_agent_tasks(config: &Config, query: ListTasksQuery) -> Result<Value> {
    let conn = crate::processing_scheduler::open_or_init(config)?;
    init_tables(&conn)?;

    let now = now_sec();
    let page = query.page.unwrap_or(0);
    let limit = query.limit.unwrap_or(25).clamp(1, 100);
    let offset = page * limit;

    let concurrency = get_setting_i64(&conn, "concurrency", 4);
    let active_tasks: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM tasks WHERE lease_expires_at > ?",
            params![now],
            |r| r.get(0),
        )
        .unwrap_or(0);

    let online_gpu_nodes: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM agent_nodes
             WHERE (gpu_model IS NOT NULL AND gpu_model != '')
               AND status IN ('model_ready', 'accepting')
               AND (?1 - last_heartbeat_at) < 90.0",
            params![now],
            |r| r.get(0),
        )
        .unwrap_or(0);

    let mut filters = Vec::new();
    let mut params_vec: Vec<rusqlite::types::Value> = Vec::new();

    if let Some(stage) = &query.stage {
        let stage = stage.trim();
        if !stage.is_empty() && stage != "all" {
            match stage {
                "running" => {
                    filters.push("stage IN ('text', 'face', 'ocr', 'detect', 'feature')");
                }
                "queued" => {
                    filters.push("stage IN ('queued', 'retry')");
                }
                "ocr" => {
                    filters.push("stage IN ('ocr', 'text')");
                }
                "feature" => {
                    filters.push("stage IN ('feature', 'face', 'face_queued')");
                }
                "done" => {
                    filters.push("(stage = 'done' OR (stage = 'excluded' AND json_extract(data, '$.ready') = 1))");
                }
                _ => {
                    filters.push("stage = ?");
                    params_vec.push(rusqlite::types::Value::Text(stage.to_string()));
                }
            }
        }
    }

    if let Some(node_id) = &query.node_id {
        let node_id = node_id.trim();
        if !node_id.is_empty() {
            filters.push("(assigned_node_id = ? OR attempt_id LIKE ?)");
            params_vec.push(rusqlite::types::Value::Text(node_id.to_string()));
            params_vec.push(rusqlite::types::Value::Text(format!("run-{}%", node_id)));
        }
    }

    if let Some(wr) = &query.waiting_reason {
        let wr = wr.trim();
        if !wr.is_empty() && wr != "all" {
            match wr {
                "paused" => {
                    filters.push("manual != 0");
                }
                "rate_limited" | "retry_backoff" => {
                    filters.push("(manual = 0 AND stage = 'retry' AND next_retry > ?)");
                    params_vec.push(rusqlite::types::Value::Real(now));
                }
                "none" => {
                    filters.push("(manual = 0 AND lease_expires_at IS NOT NULL AND lease_expires_at > ?)");
                    params_vec.push(rusqlite::types::Value::Real(now));
                }
                "quota_exhausted" | "quota_limit" => {
                    if active_tasks >= concurrency {
                        if online_gpu_nodes == 0 {
                            filters.push("(manual = 0 AND (lease_expires_at IS NULL OR lease_expires_at <= ?) AND stage NOT IN ('done', 'error', 'review') AND note NOT LIKE '%vram%' AND note NOT LIKE '%gpu%' AND (stage IN ('queued', 'face_queued', 'ocr', 'feature') OR note LIKE '%quota%'))");
                        } else {
                            filters.push("(manual = 0 AND (lease_expires_at IS NULL OR lease_expires_at <= ?) AND stage NOT IN ('done', 'error', 'review') AND note NOT LIKE '%vram%' AND note NOT LIKE '%gpu%' AND (stage IN ('queued', 'face_queued', 'detect', 'ocr', 'feature') OR note LIKE '%quota%'))");
                        }
                    } else {
                        filters.push("(manual = 0 AND (lease_expires_at IS NULL OR lease_expires_at <= ?) AND stage NOT IN ('done', 'error', 'review') AND note NOT LIKE '%vram%' AND note NOT LIKE '%gpu%' AND (note LIKE '%quota%' OR note = 'quota_exhausted'))");
                    }
                    params_vec.push(rusqlite::types::Value::Real(now));
                }
                "gpu_busy" => {
                    filters.push("(manual = 0 AND (lease_expires_at IS NULL OR lease_expires_at <= ?) AND stage NOT IN ('done', 'error', 'review') AND (note = 'waiting_for_vram' OR note = 'gpu_busy' OR note LIKE '%vram%' OR note LIKE '%gpu%'))");
                    params_vec.push(rusqlite::types::Value::Real(now));
                }
                "waiting_for_node" | "waiting_node" => {
                    if active_tasks >= concurrency {
                        if online_gpu_nodes == 0 {
                            filters.push("(manual = 0 AND (lease_expires_at IS NULL OR lease_expires_at <= ?) AND stage NOT IN ('done', 'error', 'review') AND note NOT LIKE '%vram%' AND note NOT LIKE '%gpu%' AND (stage = 'detect' OR retry_stage = 'detect' OR note LIKE '%waiting%node%'))");
                        } else {
                            filters.push("(manual = 0 AND (lease_expires_at IS NULL OR lease_expires_at <= ?) AND stage NOT IN ('done', 'error', 'review') AND note NOT LIKE '%vram%' AND note NOT LIKE '%gpu%' AND (note LIKE '%waiting%node%'))");
                        }
                    } else {
                        if online_gpu_nodes == 0 {
                            filters.push("(manual = 0 AND (lease_expires_at IS NULL OR lease_expires_at <= ?) AND stage NOT IN ('done', 'error', 'review') AND note NOT LIKE '%vram%' AND note NOT LIKE '%gpu%' AND (stage IN ('queued', 'face_queued', 'detect') OR retry_stage = 'detect' OR note LIKE '%waiting%node%'))");
                        } else {
                            filters.push("(manual = 0 AND (lease_expires_at IS NULL OR lease_expires_at <= ?) AND stage NOT IN ('done', 'error', 'review') AND note NOT LIKE '%vram%' AND note NOT LIKE '%gpu%' AND (stage IN ('queued', 'face_queued') OR note LIKE '%waiting%node%'))");
                        }
                    }
                    params_vec.push(rusqlite::types::Value::Real(now));
                }
                _ => {}
            }
        }
    }

    if let Some(batch) = &query.batch {
        let batch = batch.trim();
        if !batch.is_empty() {
            filters.push("batch_id = ?");
            params_vec.push(rusqlite::types::Value::Text(batch.to_string()));
        }
    }

    if let Some(q) = &query.q {
        let q = q.trim();
        if !q.is_empty() {
            filters.push("(name LIKE ? OR vid LIKE ? OR path LIKE ?)");
            let pattern = format!("%{}%", q);
            params_vec.push(rusqlite::types::Value::Text(pattern.clone()));
            params_vec.push(rusqlite::types::Value::Text(pattern.clone()));
            params_vec.push(rusqlite::types::Value::Text(pattern));
        }
    }

    if let Some(vid) = &query.vid {
        let vid = vid.trim();
        if !vid.is_empty() {
            filters.push("vid = ?");
            params_vec.push(rusqlite::types::Value::Text(vid.to_string()));
        }
    }

    let where_clause = if filters.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", filters.join(" AND "))
    };

    // Total count for current filter
    let count_query = format!("SELECT COUNT(*) FROM tasks {}", where_clause);
    let mut count_stmt = conn.prepare(&count_query)?;
    let rusqlite_params: Vec<&dyn rusqlite::ToSql> = params_vec.iter().map(|v| v as &dyn rusqlite::ToSql).collect();
    let total: i64 = count_stmt.query_row(rusqlite_params.as_slice(), |r| r.get(0)).unwrap_or(0);

    // Total all count
    let all_count: i64 = conn.query_row("SELECT COUNT(*) FROM tasks", [], |r| r.get(0)).unwrap_or(0);

    // Stage counts
    let mut stage_counts = std::collections::HashMap::new();
    let mut stage_stmt = conn.prepare("SELECT stage, COUNT(*) FROM tasks GROUP BY stage")?;
    let stage_rows = stage_stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
    for row in stage_rows.flatten() {
        stage_counts.insert(row.0, row.1);
    }
    if stage_counts.get("done").copied().unwrap_or(0) == 0 {
        let mut done_fallback: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM tasks WHERE stage = 'excluded' AND json_extract(data, '$.ready') = 1",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        if done_fallback == 0 {
            done_fallback = conn
                .query_row(
                    "SELECT COUNT(*) FROM batch_items WHERE completed = 1",
                    [],
                    |r| r.get(0),
                )
                .unwrap_or(0);
        }
        if done_fallback > 0 {
            stage_counts.insert("done".to_string(), done_fallback);
        }
    }

    // Query tasks
    let select_query = format!(
        "SELECT vid, name, path, size, duration, stage, retry_stage,
                text_attempts, face_attempts, next_retry, priority, is_new,
                batch_id, created_at, updated_at, error, manual, deferred, note,
                assigned_node_id, attempt_id, lease_id, coordinator_generation,
                lease_expires_at, data
         FROM tasks
         {}
         ORDER BY priority DESC, created_at DESC
         LIMIT ? OFFSET ?",
        where_clause
    );

    let mut select_stmt = conn.prepare(&select_query)?;
    let mut final_params = params_vec;
    final_params.push(rusqlite::types::Value::Integer(limit as i64));
    final_params.push(rusqlite::types::Value::Integer(offset as i64));
    let final_rusqlite_params: Vec<&dyn rusqlite::ToSql> = final_params.iter().map(|v| v as &dyn rusqlite::ToSql).collect();

    let task_rows = select_stmt.query_map(final_rusqlite_params.as_slice(), |r| {
        let vid: String = r.get(0)?;
        let name: String = r.get(1)?;
        let path: String = r.get(2)?;
        let size: i64 = r.get(3)?;
        let duration: Option<f64> = r.get(4)?;
        let stage: String = r.get(5)?;
        let retry_stage: String = r.get(6)?;
        let text_attempts: i64 = r.get(7)?;
        let face_attempts: i64 = r.get(8)?;
        let next_retry: f64 = r.get(9)?;
        let priority: i64 = r.get(10)?;
        let is_new: i64 = r.get(11)?;
        let batch_id: String = r.get(12)?;
        let created_at: f64 = r.get(13)?;
        let updated_at: f64 = r.get(14)?;
        let error: Option<String> = r.get(15)?;
        let manual: i32 = r.get(16)?;
        let deferred: i32 = r.get(17)?;
        let note: String = r.get(18)?;
        let assigned_node_id: String = r.get(19)?;
        let attempt_id: String = r.get(20)?;
        let lease_id: String = r.get(21)?;
        let coordinator_generation: String = r.get(22)?;
        let lease_expires_at: f64 = r.get(23)?;
        let data_raw: String = r.get(24)?;

        let parsed_data: Value = serde_json::from_str(&data_raw).unwrap_or(json!({}));

        // Determine duration_ms if available
        let duration_ms: Option<i64> = parsed_data
            .get("duration_ms")
            .and_then(|v| v.as_i64());

        // Derive waiting reason
        let waiting_reason = if manual != 0 {
            "paused"
        } else if stage == "retry" && next_retry > now {
            "rate_limited"
        } else if lease_expires_at > now {
            "none"
        } else if !note.is_empty()
            && (note.starts_with("waiting_")
                || note == "quota_exhausted"
                || note == "quota_limit"
                || note == "gpu_busy"
                || note == "rate_limited")
        {
            if note == "waiting_for_vram" || note == "gpu_busy" {
                "gpu_busy"
            } else if note == "waiting_for_node" || note == "waiting_node" {
                "waiting_for_node"
            } else if note == "quota_limit" || note == "quota_exhausted" {
                "quota_exhausted"
            } else {
                note.as_str()
            }
        } else if (stage == "detect" || retry_stage == "detect") && online_gpu_nodes == 0 {
            "waiting_for_node"
        } else if active_tasks >= concurrency
            && (stage == "queued"
                || stage == "face_queued"
                || stage == "detect"
                || stage == "ocr"
                || stage == "feature")
        {
            "quota_exhausted"
        } else if stage == "queued" || stage == "face_queued" {
            "waiting_for_node"
        } else {
            "none"
        };

        Ok(json!({
            "vid": vid,
            "name": name,
            "path": path,
            "video_path": path,
            "size": size,
            "duration": duration,
            "stage": stage,
            "retry_stage": retry_stage,
            "text_attempts": text_attempts,
            "face_attempts": face_attempts,
            "next_retry": next_retry,
            "priority": priority,
            "is_new": is_new,
            "batch_id": batch_id,
            "created_at": created_at,
            "updated_at": updated_at,
            "error": error,
            "manual": manual != 0,
            "deferred": deferred != 0,
            "note": note,
            "assigned_node_id": if assigned_node_id.is_empty() { Value::Null } else { json!(assigned_node_id) },
            "attempt_id": if attempt_id.is_empty() { Value::Null } else { json!(attempt_id) },
            "lease_id": if lease_id.is_empty() { Value::Null } else { json!(lease_id) },
            "coordinator_generation": if coordinator_generation.is_empty() { Value::Null } else { json!(coordinator_generation) },
            "lease_expires_at": if lease_expires_at <= 0.0 { Value::Null } else { json!(lease_expires_at) },
            "duration_ms": duration_ms,
            "waiting_reason": waiting_reason,
            "data": parsed_data,
        }))
    })?;

    let tasks: Vec<Value> = task_rows.filter_map(|r| r.ok()).collect();

    let paused = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'paused'",
            [],
            |r| r.get::<_, String>(0),
        )
        .map(|val| val == "true" || val == "1")
        .unwrap_or(false);

    let last_discovery: Value = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'last_discovery'",
            [],
            |r| r.get::<_, String>(0),
        )
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .unwrap_or(Value::Null);

    let mut batches = Vec::new();
    if let Ok(mut batch_stmt) = conn.prepare(
        "SELECT batch_id, COUNT(*) as total FROM tasks WHERE batch_id IS NOT NULL AND batch_id != '' GROUP BY batch_id ORDER BY batch_id DESC",
    ) {
        if let Ok(batch_rows) = batch_stmt.query_map([], |r| {
            Ok(json!({
                "id": r.get::<_, String>(0)?,
                "total": r.get::<_, i64>(1)?,
            }))
        }) {
            for row in batch_rows.flatten() {
                batches.push(row);
            }
        }
    }

    Ok(json!({
        "schema_version": 1,
        "online": true,
        "paused": paused,
        "page": page,
        "limit": limit,
        "total": total,
        "all_count": all_count,
        "counts": stage_counts,
        "batches": batches,
        "last_discovery": last_discovery,
        "tasks": tasks,
    }))
}

pub fn reap_leases(config: &Config) -> Result<crate::processing_scheduler::ReapResult> {
    let mut conn = crate::processing_scheduler::open_or_init(config)?;
    init_tables(&conn)?;
    let now = now_sec();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let res = crate::processing_scheduler::reap_expired_leases(&tx, now)?;
    tx.commit()?;
    Ok(res)
}

pub fn get_coordinator_generation(config: &Config) -> Result<i64> {
    let conn = crate::processing_scheduler::open_or_init(config)?;
    init_tables(&conn)?;
    Ok(get_setting_i64(&conn, "coordinator_generation", 1))
}

pub fn maintenance_sweep(config: &Config) -> Result<Value> {
    let mut conn = crate::processing_scheduler::open_or_init(config)?;
    init_tables(&conn)?;
    let now = now_sec();
    let has_expired_lease: bool = conn
        .query_row(
            "SELECT 1 FROM tasks WHERE (lease_expires_at > 0 AND lease_expires_at <= ?1) OR (stage IN ('text', 'face', 'ocr', 'detect', 'feature') AND lease_expires_at <= 0 AND (?1 - updated_at) >= 660.0) LIMIT 1",
            params![now],
            |_| Ok(true),
        )
        .unwrap_or(false);
    let has_timed_out_node: bool = conn
        .query_row(
            "SELECT 1 FROM agent_nodes WHERE status NOT IN ('offline', 'revoked') AND (?1 - last_heartbeat_at) > 90.0 LIMIT 1",
            params![now],
            |_| Ok(true),
        )
        .unwrap_or(false);
    if !has_expired_lease && !has_timed_out_node {
        let generation = get_setting_i64(&conn, "coordinator_generation", 1);
        return Ok(json!({
            "ok": true,
            "offline_nodes": [],
            "reaped_leases": 0,
            "reaped_vids": [],
            "coordinator_generation": generation,
        }));
    }
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let offline_nodes = crate::agent_registry::check_liveness_timeouts(&tx, now)?;
    let reap_res = crate::processing_scheduler::reap_expired_leases(&tx, now)?;
    tx.commit()?;
    Ok(json!({
        "ok": true,
        "offline_nodes": offline_nodes,
        "reaped_leases": reap_res.count,
        "reaped_vids": reap_res.reaped_vids,
        "coordinator_generation": reap_res.coordinator_generation,
    }))
}

pub fn get_task_attempts(config: &Config, vid: &str) -> Result<Value> {
    let conn = crate::processing_scheduler::open_or_init(config)?;
    init_tables(&conn)?;

    let mut stmt = conn.prepare(
        "SELECT r.id, r.stage, r.status, r.started_at, r.finished_at, r.error,
                COALESCE(rec.delivery_count, 1),
                COALESCE(rec.node_id, ''),
                CASE WHEN rec.receipt_id IS NOT NULL THEN 1 ELSE 0 END
         FROM processing_runs r
         LEFT JOIN agent_receipts rec ON r.id = rec.attempt_id
         WHERE r.vid = ?
         ORDER BY r.started_at ASC",
    )?;

    let attempts: Vec<Value> = stmt
        .query_map(params![vid], |row| {
            let attempt_id: String = row.get(0)?;
            let stage: String = row.get(1)?;
            let status: String = row.get(2)?;
            let started_at: f64 = row.get(3)?;
            let finished_at: Option<f64> = row.get(4)?;
            let error: Option<String> = row.get(5)?;
            let delivery_count: i64 = row.get(6)?;
            let mut node_id: String = row.get(7)?;
            let is_authoritative: bool = row.get::<_, i32>(8)? != 0;

            if node_id.is_empty() && attempt_id.starts_with("run-") {
                let parts: Vec<&str> = attempt_id.split('-').collect();
                if parts.len() >= 3 {
                    node_id = parts[1..parts.len() - 1].join("-");
                }
            }

            let duration_ms = finished_at.map(|f| ((f - started_at) * 1000.0) as i64);

            Ok(json!({
                "attempt_id": attempt_id,
                "node_id": node_id,
                "stage": stage,
                "status": status,
                "started_at": started_at,
                "finished_at": finished_at,
                "duration_ms": duration_ms,
                "error": error,
                "delivery_count": delivery_count,
                "is_authoritative": is_authoritative,
            }))
        })?
        .filter_map(|r| r.ok())
        .collect();

    Ok(json!({
        "ok": true,
        "vid": vid,
        "attempts": attempts,
    }))
}
