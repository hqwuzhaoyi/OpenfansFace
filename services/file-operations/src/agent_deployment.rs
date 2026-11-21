use crate::model::{Error, Result};
use crate::Config;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::{SystemTime, UNIX_EPOCH};

pub const DRIFT_TOLERANCE: f64 = 0.05;

pub const DEPLOYMENT_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS agent_packages (
    package_id TEXT PRIMARY KEY,
    manifest_sha256 TEXT NOT NULL,
    model_sha256 TEXT NOT NULL,
    execution_provider TEXT NOT NULL,
    version TEXT NOT NULL,
    is_pinned INTEGER NOT NULL DEFAULT 1,
    created_at REAL NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_agent_packages_version ON agent_packages(version);

CREATE TABLE IF NOT EXISTS agent_deployments (
    deployment_id TEXT PRIMARY KEY,
    node_id TEXT NOT NULL,
    package_id TEXT NOT NULL,
    status TEXT NOT NULL,
    desired_version TEXT NOT NULL,
    actual_version TEXT,
    stage TEXT NOT NULL,
    startup_proof TEXT,
    self_test_passed INTEGER,
    self_test_drift REAL,
    duration_ms INTEGER,
    error TEXT,
    updated_at REAL NOT NULL,
    FOREIGN KEY(node_id) REFERENCES agent_nodes(id),
    FOREIGN KEY(package_id) REFERENCES agent_packages(package_id)
);
CREATE INDEX IF NOT EXISTS idx_agent_deployments_node ON agent_deployments(node_id);
CREATE INDEX IF NOT EXISTS idx_agent_deployments_package ON agent_deployments(package_id);
CREATE INDEX IF NOT EXISTS idx_agent_deployments_status ON agent_deployments(status, stage);
CREATE INDEX IF NOT EXISTS idx_agent_deployments_updated ON agent_deployments(updated_at DESC);
"#;

pub fn now_sec() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

pub fn init_tables(conn: &Connection) -> Result<()> {
    if conn
        .prepare("SELECT package_id FROM agent_packages LIMIT 0")
        .is_ok()
        && conn
            .prepare("SELECT deployment_id FROM agent_deployments LIMIT 0")
            .is_ok()
    {
        return Ok(());
    }
    conn.execute_batch(DEPLOYMENT_SCHEMA)?;
    Ok(())
}

fn default_pinned() -> i32 {
    1
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct RegisterPackageRequest {
    pub package_id: String,
    pub manifest_sha256: String,
    pub model_sha256: String,
    pub execution_provider: String,
    pub version: String,
    #[serde(default = "default_pinned")]
    pub is_pinned: i32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RegisterPackageResponse {
    pub ok: bool,
    pub package_id: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DeployPackageRequest {
    pub node_id: String,
    pub package_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DeployPackageResponse {
    pub status: String,
    pub deployment_id: String,
    pub package_id: String,
    pub manifest_sha256: String,
    pub model_sha256: String,
    pub execution_provider: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct SelfTestResult {
    #[serde(default)]
    pub passed: bool,
    #[serde(default = "default_drift")]
    pub drift: f64,
}

fn default_drift() -> f64 {
    1.0
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DeployReceiptRequest {
    pub deployment_id: String,
    #[serde(default)]
    pub package_id: String,
    pub status: String,
    #[serde(default)]
    pub stage: Option<String>,
    #[serde(default)]
    pub startup_proof: Option<Value>,
    #[serde(default)]
    pub self_test: Option<SelfTestResult>,
    #[serde(default)]
    pub duration_ms: Option<i64>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DeployReceiptResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub marked_failed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct RollbackRequest {
    pub node_id: String,
    #[serde(default)]
    pub target_version: Option<String>,
}

/// Register a pinned immutable package manifest.
pub fn register_package(config: &Config, req: RegisterPackageRequest) -> Result<RegisterPackageResponse> {
    if req.package_id.trim().is_empty() {
        return Err(Error::new(400, "invalid_request", "package_id 不能为空"));
    }
    if req.manifest_sha256.trim().is_empty() {
        return Err(Error::new(400, "invalid_request", "manifest_sha256 不能为空"));
    }
    if req.model_sha256.trim().is_empty() {
        return Err(Error::new(400, "invalid_request", "model_sha256 不能为空"));
    }
    if req.execution_provider.trim().is_empty() {
        return Err(Error::new(400, "invalid_request", "execution_provider 不能为空"));
    }
    if req.version.trim().is_empty() {
        return Err(Error::new(400, "invalid_request", "version 不能为空"));
    }

    let conn = crate::processing_scheduler::open_or_init(config)?;
    let now = now_sec();

    conn.execute(
        "INSERT INTO agent_packages (
            package_id, manifest_sha256, model_sha256, execution_provider, version, is_pinned, created_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(package_id) DO UPDATE SET
            manifest_sha256 = excluded.manifest_sha256,
            model_sha256 = excluded.model_sha256,
            execution_provider = excluded.execution_provider,
            version = excluded.version,
            is_pinned = excluded.is_pinned",
        params![
            req.package_id,
            req.manifest_sha256,
            req.model_sha256,
            req.execution_provider,
            req.version,
            req.is_pinned,
            now,
        ],
    )?;

    Ok(RegisterPackageResponse {
        ok: true,
        package_id: req.package_id,
    })
}

/// Dispatches a package deployment to an online node.
pub fn deploy_package(config: &Config, req: DeployPackageRequest) -> Result<DeployPackageResponse> {
    if req.node_id.trim().is_empty() {
        return Err(Error::new(400, "invalid_request", "node_id 不能为空"));
    }
    if req.package_id.trim().is_empty() {
        return Err(Error::new(400, "invalid_request", "package_id 不能为空"));
    }

    let mut conn = crate::processing_scheduler::open_or_init(config)?;
    let now = now_sec();

    // 1. Verify node exists and is not offline
    let node_row: Option<(String, String, f64)> = conn
        .query_row(
            "SELECT id, status, last_heartbeat_at FROM agent_nodes WHERE id = ?",
            params![req.node_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .ok();

    let (_node_id, node_status, last_heartbeat_at) = match node_row {
        Some(r) => r,
        None => return Err(Error::new(404, "node_not_found", "node_not_found")),
    };

    let is_offline = node_status == "offline" || (last_heartbeat_at > 0.0 && (now - last_heartbeat_at) > 90.0);
    if is_offline {
        return Err(Error::new(409, "node_offline", "node_offline"));
    }

    // 2. Verify package exists
    let pkg_row: Option<(String, String, String, String, String)> = conn
        .query_row(
            "SELECT package_id, manifest_sha256, model_sha256, execution_provider, version FROM agent_packages WHERE package_id = ?",
            params![req.package_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .ok();

    let (package_id, manifest_sha256, model_sha256, execution_provider, pkg_version) = match pkg_row {
        Some(r) => r,
        None => return Err(Error::new(404, "package_not_found", "package_not_found")),
    };

    // 3. Generate deployment_id and insert record
    let short_uuid = &uuid::Uuid::new_v4().simple().to_string()[..6];
    let deployment_id = format!("dep-{}-{}", req.node_id, short_uuid);

    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;

    tx.execute(
        "DELETE FROM agent_deployments WHERE node_id = ?",
        params![req.node_id],
    )?;

    tx.execute(
        "INSERT INTO agent_deployments (
            deployment_id, node_id, package_id, status, desired_version,
            actual_version, stage, startup_proof, self_test_passed, self_test_drift,
            duration_ms, error, updated_at
        ) VALUES (
            ?, ?, ?, 'downloading', ?,
            NULL, 'download', NULL, NULL, NULL,
            NULL, NULL, ?
        )",
        params![
            deployment_id,
            req.node_id,
            package_id,
            pkg_version,
            now,
        ],
    )?;

    // Update node desired_version
    tx.execute(
        "UPDATE agent_nodes SET desired_version = ?, updated_at = ? WHERE id = ?",
        params![pkg_version, now, req.node_id],
    )?;

    // Audit log
    let event_msg = format!(
        "Deployment {} dispatched to node {} (pkg={}, version={})",
        deployment_id, req.node_id, package_id, pkg_version
    );
    let _ = tx.execute(
        "INSERT INTO events (at, vid, kind, message) VALUES (?, NULL, 'deployment_dispatched', ?)",
        params![now, event_msg],
    );

    tx.commit()?;

    Ok(DeployPackageResponse {
        status: "accepted".to_string(),
        deployment_id,
        package_id,
        manifest_sha256,
        model_sha256,
        execution_provider,
    })
}

/// Receives deployment receipt from agent node, validates startup proof and self-test drift.
pub fn submit_deploy_receipt(config: &Config, req: DeployReceiptRequest) -> Result<DeployReceiptResponse> {
    if req.deployment_id.trim().is_empty() {
        return Err(Error::new(400, "invalid_request", "deployment_id 不能为空"));
    }

    let mut conn = crate::processing_scheduler::open_or_init(config)?;
    let now = now_sec();

    // 1. Verify deployment exists
    let dep_row: Option<(String, String, String, String)> = conn
        .query_row(
            "SELECT deployment_id, node_id, package_id, desired_version FROM agent_deployments WHERE deployment_id = ?",
            params![req.deployment_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .ok();

    let (deployment_id, node_id, _pkg_id, desired_version) = match dep_row {
        Some(r) => r,
        None => return Err(Error::new(404, "deployment_not_found", "deployment_not_found")),
    };

    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;

    if req.status == "ready" {
        let (passed, drift) = match &req.self_test {
            Some(st) => (st.passed, st.drift),
            None => (false, 1.0),
        };

        // Drift tolerance gate: <= 0.05, strictly finite and non-negative
        if !passed || !drift.is_finite() || drift < 0.0 || drift > DRIFT_TOLERANCE {
            let record_drift = if drift.is_finite() { Some(drift) } else { None };
            tx.execute(
                "UPDATE agent_deployments SET
                    status = 'failed',
                    stage = 'self_test',
                    self_test_passed = 0,
                    self_test_drift = ?,
                    error = 'self_test_drift_exceeded',
                    updated_at = ?
                WHERE deployment_id = ?",
                params![record_drift, now, deployment_id],
            )?;

            let event_msg = if drift.is_finite() {
                format!(
                    "Deployment {} failed: self-test drift exceeded threshold ({:.4} > {:.2})",
                    deployment_id, drift, DRIFT_TOLERANCE
                )
            } else {
                format!(
                    "Deployment {} failed: self-test drift is non-finite ({})",
                    deployment_id, drift
                )
            };
            let _ = tx.execute(
                "INSERT INTO events (at, vid, kind, message) VALUES (?, NULL, 'deployment_failed', ?)",
                params![now, event_msg],
            );

            tx.commit()?;

            return Err(Error::new(
                422,
                "self_test_drift_exceeded",
                "自检偏差超出阈值 (drift > 0.05)",
            ));
        }

        // Self test passed
        let proof_str = req
            .startup_proof
            .as_ref()
            .map(|p| p.to_string())
            .unwrap_or_else(|| "{}".to_string());
        let duration = req.duration_ms.unwrap_or(0);

        tx.execute(
            "UPDATE agent_deployments SET
                status = 'ready',
                stage = 'ready',
                actual_version = desired_version,
                self_test_passed = 1,
                self_test_drift = ?,
                startup_proof = ?,
                duration_ms = ?,
                error = ?,
                updated_at = ?
            WHERE deployment_id = ?",
            params![drift, proof_str, duration, req.error, now, deployment_id],
        )?;

        // Query current node desired_version and accepting flag
        let (node_desired_version, accepting): (String, i32) = tx
            .query_row(
                "SELECT desired_version, accepting FROM agent_nodes WHERE id = ?",
                params![node_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap_or_default();

        // Version fencing check:
        // Only update agent_nodes.current_version and status if this receipt matches
        // the node's desired_version (i.e. this deployment is the currently active target).
        // Prevents late-arriving receipts from older/superseded deployments from
        // regressing current_version or node readiness.
        if desired_version == node_desired_version {
            let new_status = if accepting == 1 { "accepting" } else { "model_ready" };

            tx.execute(
                "UPDATE agent_nodes SET
                    model_ready = 1,
                    current_version = ?,
                    status = ?,
                    updated_at = ?
                WHERE id = ?",
                params![desired_version, new_status, now, node_id],
            )?;

            let event_msg = format!(
                "Deployment {} succeeded for node {} (version={}, status={})",
                deployment_id, node_id, desired_version, new_status
            );
            let _ = tx.execute(
                "INSERT INTO events (at, vid, kind, message) VALUES (?, NULL, 'deployment_ready', ?)",
                params![now, event_msg],
            );
        } else {
            let event_msg = format!(
                "Deployment {} succeeded for node {} (version={}), but superseded by desired_version {} — current_version unchanged",
                deployment_id, node_id, desired_version, node_desired_version
            );
            let _ = tx.execute(
                "INSERT INTO events (at, vid, kind, message) VALUES (?, NULL, 'deployment_stale_receipt_ignored', ?)",
                params![now, event_msg],
            );
        }

        tx.commit()?;

        Ok(DeployReceiptResponse {
            ok: true,
            marked_failed: None,
            error: None,
        })
    } else {
        // Explicit failure reported
        let stage = req.stage.unwrap_or_else(|| "failed".to_string());
        let error_msg = req.error.unwrap_or_else(|| "unknown".to_string());

        tx.execute(
            "UPDATE agent_deployments SET
                status = 'failed',
                stage = ?,
                error = ?,
                updated_at = ?
            WHERE deployment_id = ?",
            params![stage, error_msg, now, deployment_id],
        )?;

        // Only mark node model_ready = 0 if this failure belongs to the currently desired deployment
        let node_desired_version: String = tx
            .query_row(
                "SELECT desired_version FROM agent_nodes WHERE id = ?",
                params![node_id],
                |r| r.get(0),
            )
            .unwrap_or_default();

        if desired_version == node_desired_version {
            tx.execute(
                "UPDATE agent_nodes SET
                    model_ready = 0,
                    updated_at = ?
                WHERE id = ?",
                params![now, node_id],
            )?;
        }

        let event_msg = format!(
            "Deployment {} marked failed for node {} at stage {} (error={})",
            deployment_id, node_id, stage, error_msg
        );
        let _ = tx.execute(
            "INSERT INTO events (at, vid, kind, message) VALUES (?, NULL, 'deployment_failed', ?)",
            params![now, event_msg],
        );

        tx.commit()?;

        Ok(DeployReceiptResponse {
            ok: true,
            marked_failed: Some(true),
            error: None,
        })
    }
}

/// Lists all deployments joined with package metadata and node info.
pub fn list_deployments(config: &Config) -> Result<Value> {
    let conn = crate::processing_scheduler::open_or_init(config)?;

    let mut stmt = conn.prepare(
        "SELECT
            d.deployment_id,
            d.node_id,
            d.package_id,
            d.status,
            d.desired_version,
            d.actual_version,
            d.stage,
            d.startup_proof,
            d.self_test_passed,
            d.self_test_drift,
            d.duration_ms,
            d.error,
            d.updated_at,
            p.version as pkg_version,
            p.execution_provider,
            p.manifest_sha256,
            p.model_sha256,
            n.hostname,
            n.arch,
            n.os,
            n.status as node_status,
            n.gpu_model
        FROM agent_deployments d
        JOIN agent_packages p ON d.package_id = p.package_id
        LEFT JOIN agent_nodes n ON d.node_id = n.id
        ORDER BY d.updated_at DESC",
    )?;

    let rows = stmt.query_map([], |r| {
        let deployment_id: String = r.get(0)?;
        let node_id: String = r.get(1)?;
        let package_id: String = r.get(2)?;
        let status: String = r.get(3)?;
        let desired_version: String = r.get(4)?;
        let actual_version: Option<String> = r.get(5)?;
        let stage: String = r.get(6)?;
        let startup_proof: Option<String> = r.get(7)?;
        let self_test_passed: Option<i32> = r.get(8)?;
        let self_test_drift: Option<f64> = r.get(9)?;
        let duration_ms: Option<i64> = r.get(10)?;
        let error: Option<String> = r.get(11)?;
        let updated_at: f64 = r.get(12)?;
        let pkg_version: String = r.get(13)?;
        let execution_provider: String = r.get(14)?;
        let manifest_sha256: String = r.get(15)?;
        let model_sha256: String = r.get(16)?;
        let hostname: Option<String> = r.get(17)?;
        let arch: Option<String> = r.get(18)?;
        let os: Option<String> = r.get(19)?;
        let node_status: Option<String> = r.get(20)?;
        let gpu_model: Option<String> = r.get(21)?;

        Ok((
            deployment_id, node_id, package_id, status, desired_version, actual_version, stage,
            startup_proof, self_test_passed, self_test_drift, duration_ms, error, updated_at,
            pkg_version, execution_provider, manifest_sha256, model_sha256,
            hostname, arch, os, node_status, gpu_model,
        ))
    })?;

    let mut deployments = Vec::new();
    for r in rows {
        let (
            deployment_id, node_id, package_id, status, desired_version, actual_version, stage,
            startup_proof_raw, self_test_passed, self_test_drift, duration_ms, error, updated_at,
            pkg_version, execution_provider, manifest_sha256, model_sha256,
            hostname, arch, os, node_status, gpu_model,
        ) = r?;

        let startup_proof = startup_proof_raw.and_then(|raw| {
            serde_json::from_str::<Value>(&raw).ok()
        });

        deployments.push(json!({
            "deployment_id": deployment_id,
            "node_id": node_id,
            "package_id": package_id,
            "status": status,
            "desired_version": desired_version,
            "actual_version": actual_version,
            "stage": stage,
            "startup_proof": startup_proof,
            "self_test_passed": self_test_passed,
            "self_test_drift": self_test_drift,
            "duration_ms": duration_ms,
            "error": error,
            "updated_at": updated_at,
            "pkg_version": pkg_version,
            "execution_provider": execution_provider,
            "manifest_sha256": manifest_sha256,
            "model_sha256": model_sha256,
            "hostname": hostname,
            "arch": arch,
            "os": os,
            "node_status": node_status,
            "gpu_model": gpu_model,
        }));
    }

    let mut pkg_stmt = conn.prepare(
        "SELECT package_id, version, execution_provider, model_sha256, manifest_sha256, is_pinned
         FROM agent_packages
         ORDER BY created_at DESC",
    )?;
    let pkg_rows = pkg_stmt.query_map([], |r| {
        Ok(json!({
            "package_id": r.get::<_, String>(0)?,
            "version": r.get::<_, String>(1)?,
            "execution_provider": r.get::<_, String>(2)?,
            "model_sha256": r.get::<_, String>(3)?,
            "manifest_sha256": r.get::<_, String>(4)?,
            "is_pinned": r.get::<_, i32>(5)?,
        }))
    })?;
    let mut packages = Vec::new();
    for p in pkg_rows {
        packages.push(p?);
    }

    Ok(json!({
        "deployments": deployments,
        "packages": packages,
        "total": deployments.len(),
    }))
}

/// Rolls back a node to a previous version or designated stable release.
pub fn rollback_deployment(config: &Config, req: RollbackRequest) -> Result<DeployPackageResponse> {
    if req.node_id.trim().is_empty() {
        return Err(Error::new(400, "invalid_request", "node_id 不能为空"));
    }

    let mut conn = crate::processing_scheduler::open_or_init(config)?;
    let now = now_sec();

    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;

    // 1. Check node exists and is not offline
    let node_row: Option<(String, String, String, String, f64)> = tx
        .query_row(
            "SELECT id, status, current_version, desired_version, last_heartbeat_at FROM agent_nodes WHERE id = ?",
            params![req.node_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .ok();

    let (_node_id, node_status, current_version, desired_version, last_heartbeat_at) = match node_row {
        Some(r) => r,
        None => return Err(Error::new(404, "node_not_found", "node_not_found")),
    };

    let is_offline = node_status == "offline" || (last_heartbeat_at > 0.0 && (now - last_heartbeat_at) > 90.0);
    if is_offline {
        return Err(Error::new(409, "node_offline", "node_offline"));
    }

    // 2. Idempotency check: if an active deployment already exists for this node, reuse it
    let existing_active: Option<(String, String, String, String, String, String)> = tx
        .query_row(
            "SELECT d.deployment_id, p.package_id, p.manifest_sha256, p.model_sha256, p.execution_provider, p.version
             FROM agent_deployments d
             JOIN agent_packages p ON d.package_id = p.package_id
             WHERE d.node_id = ? AND d.status NOT IN ('ready', 'failed', 'rolled_back')
             ORDER BY d.updated_at DESC LIMIT 1",
            params![req.node_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )
        .ok();

    if let Some((existing_id, pkg_id, manifest, model, ep, active_ver)) = existing_active {
        let is_match = match &req.target_version {
            Some(t_ver) => t_ver == &active_ver,
            None => true,
        };
        if is_match {
            tx.commit()?;
            return Ok(DeployPackageResponse {
                status: "accepted".to_string(),
                deployment_id: existing_id,
                package_id: pkg_id,
                manifest_sha256: manifest,
                model_sha256: model,
                execution_provider: ep,
            });
        }
    }

    // 3. Resolve target package
    let target_package_id: Option<String> = if let Some(target_ver) = &req.target_version {
        tx.query_row(
            "SELECT package_id FROM agent_packages WHERE version = ?",
            params![target_ver],
            |r| r.get(0),
        )
        .ok()
    } else {
        // Resolve the true failing version from un-rolled-back failed deployment
        let failing_ver_opt: Option<String> = tx
            .query_row(
                "SELECT desired_version FROM agent_deployments WHERE node_id = ? AND status = 'failed' ORDER BY updated_at DESC LIMIT 1",
                params![req.node_id],
                |r| r.get(0),
            )
            .ok();

        let failing_ver = match &failing_ver_opt {
            Some(fv) => fv.as_str(),
            None => {
                if !desired_version.is_empty() && desired_version != current_version {
                    &desired_version
                } else {
                    &current_version
                }
            }
        };

        tx.query_row(
            "SELECT package_id FROM agent_deployments WHERE node_id = ? AND status = 'ready' AND desired_version != ? ORDER BY updated_at DESC LIMIT 1",
            params![req.node_id, failing_ver],
            |r| r.get(0),
        )
        .ok()
        .or_else(|| {
            tx.query_row(
                "SELECT package_id FROM agent_deployments WHERE node_id = ? AND status = 'ready' AND desired_version != ? ORDER BY updated_at DESC LIMIT 1",
                params![req.node_id, current_version],
                |r| r.get(0),
            )
            .ok()
        })
        .or_else(|| {
            // Fallback to pinned package
            tx.query_row(
                "SELECT package_id FROM agent_packages WHERE is_pinned = 1 ORDER BY created_at ASC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .ok()
        })
    };

    let package_id = match target_package_id {
        Some(p) => p,
        None => return Err(Error::new(404, "target_package_not_found", "找不到可回滚的目标运行包")),
    };

    // 4. Look up target package metadata
    let pkg_row: Option<(String, String, String, String)> = tx
        .query_row(
            "SELECT manifest_sha256, model_sha256, execution_provider, version FROM agent_packages WHERE package_id = ?",
            params![package_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .ok();

    let (manifest_sha256, model_sha256, execution_provider, pkg_version) = match pkg_row {
        Some(r) => r,
        None => return Err(Error::new(404, "package_not_found", "package_not_found")),
    };

    // 5. Record rollback audit event
    let event_msg = format!("Rollback triggered on node {} to package {}", req.node_id, package_id);
    let _ = tx.execute(
        "INSERT INTO events (at, vid, kind, message) VALUES (?, NULL, 'rollback_triggered', ?)",
        params![now, event_msg],
    );

    // 6. Clear any failed or superseded active deployment state by marking it rolled_back
    let _ = tx.execute(
        "UPDATE agent_deployments SET status = 'rolled_back', updated_at = ? WHERE node_id = ? AND status = 'failed'",
        params![now, req.node_id],
    );
    let _ = tx.execute(
        "UPDATE agent_deployments SET status = 'rolled_back', updated_at = ? WHERE node_id = ? AND status NOT IN ('ready', 'failed', 'rolled_back')",
        params![now, req.node_id],
    );

    // 7. Dispatch new deployment record
    let short_uuid = &uuid::Uuid::new_v4().simple().to_string()[..6];
    let deployment_id = format!("dep-{}-{}", req.node_id, short_uuid);

    tx.execute(
        "INSERT INTO agent_deployments (
            deployment_id, node_id, package_id, status, desired_version,
            actual_version, stage, startup_proof, self_test_passed, self_test_drift,
            duration_ms, error, updated_at
        ) VALUES (
            ?, ?, ?, 'downloading', ?,
            NULL, 'download', NULL, NULL, NULL,
            NULL, NULL, ?
        )",
        params![
            deployment_id,
            req.node_id,
            package_id,
            pkg_version,
            now,
        ],
    )?;

    // 8. Update node desired_version
    tx.execute(
        "UPDATE agent_nodes SET desired_version = ?, updated_at = ? WHERE id = ?",
        params![pkg_version, now, req.node_id],
    )?;

    // 9. Audit log deployment dispatched
    let dispatch_msg = format!(
        "Deployment {} dispatched to node {} (pkg={}, version={})",
        deployment_id, req.node_id, package_id, pkg_version
    );
    let _ = tx.execute(
        "INSERT INTO events (at, vid, kind, message) VALUES (?, NULL, 'deployment_dispatched', ?)",
        params![now, dispatch_msg],
    );

    tx.commit()?;

    Ok(DeployPackageResponse {
        status: "accepted".to_string(),
        deployment_id,
        package_id,
        manifest_sha256,
        model_sha256,
        execution_provider,
    })
}
