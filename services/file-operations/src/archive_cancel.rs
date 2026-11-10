//! Archive coordination and reconciliation with external Peach background scanning.
//! Supports lease-bounded scanner coordination, atomic adoption of pure unreferenced
//! scanner-inserted target stub rows for published archive files, and safe rollback
//! fencing when another failure aborts commit.
use crate::{
    config::Config,
    creator_delta, db, files, ledger, metadata,
    model::{Error, Plan, Result},
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const DEFAULT_SCANNER_LEASE_MS: i64 = 300_000;

#[track_caller]
fn conflict() -> Error {
    let location = std::panic::Location::caller();
    Error::conflict(format!(
        "归档账本取消依据不足，保留恢复状态（{}:{}）",
        location.file().rsplit('/').next().unwrap_or(location.file()),
        location.line()
    ))
}
fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
fn lease_ttl_ms() -> i64 {
    std::env::var("FILE_OPERATIONS_SCANNER_LEASE_MS")
        .ok()
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|&v| v > 0)
        .unwrap_or(DEFAULT_SCANNER_LEASE_MS)
}

pub fn peach_state_dir(config: &Config) -> Option<PathBuf> {
    if let Ok(raw) = std::env::var("PEACH_STATE_DIR") {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            let path = PathBuf::from(trimmed);
            if path.is_dir() {
                return Some(std::fs::canonicalize(&path).unwrap_or(path));
            }
        }
    }
    if let Some(parent) = config.ledger.parent() {
        let direct = parent.join("state");
        if direct.is_dir() {
            return Some(std::fs::canonicalize(&direct).unwrap_or(direct));
        }
        if let Some(grand) = parent.parent() {
            let sibling = grand.join("state");
            if sibling.is_dir() {
                return Some(std::fs::canonicalize(&sibling).unwrap_or(sibling));
            }
        }
    }
    None
}

fn ensure_lease_table(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _face_admin_scanner_leases (
            id TEXT PRIMARY KEY,
            action TEXT NOT NULL,
            state TEXT NOT NULL,
            scope TEXT NOT NULL,
            acquired_at INTEGER NOT NULL,
            expires_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        )",
    )?;
    Ok(())
}

pub fn supports_scanner_reconciliation(plan: &Plan) -> bool {
    matches!(plan.action.as_str(), "archive" | "merge-person")
}

fn write_coordination_files(config: &Config, payload: &Value) {
    let local = config
        .state_db()
        .with_file_name("scanner-coordination.json");
    if payload["action"] == "archive" || local.exists() {
        if let Some(parent) = local.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(bytes) = serde_json::to_vec(payload) {
            let _ = files::atomic_write(&local, &bytes);
        }
    }
    if let Some(state_dir) = peach_state_dir(config) {
        let state_file = state_dir.join("archive-scanner-coordination.json");
        if let Ok(bytes) = serde_json::to_vec(payload) {
            let _ = files::atomic_write(&state_file, &bytes);
        }
        let control_file = state_dir.join("library-processing-control.json");
        if control_file.is_file()
            && let Ok(Some(raw)) = crate::control::read(&control_file, crate::control::MARKER_LIMIT)
            && let Ok(mut control) = serde_json::from_str::<Value>(&raw)
            && let Some(obj) = control.as_object_mut()
        {
            let active = payload["state"] == "active";
            let op_id = payload["operation_id"].as_str().unwrap_or("");
            if active {
                let user_paused = obj.get("paused").and_then(Value::as_bool) == Some(true)
                    && obj.get("archive_lease").is_none();
                if !user_paused {
                    obj.insert("paused".into(), json!(true));
                    obj.insert(
                        "archive_lease".into(),
                        json!({
                            "holder": "face-file-operations",
                            "operation_id": op_id,
                            "acquired_at": payload["acquired_at"],
                            "expires_at": payload["expires_at"],
                        }),
                    );
                    if let Ok(bytes) = serde_json::to_vec(&control) {
                        let _ = files::atomic_write(&control_file, &bytes);
                    }
                }
            } else if let Some(lease) = obj.get("archive_lease").cloned() {
                let expired = lease["expires_at"]
                    .as_i64()
                    .is_some_and(|exp| exp <= now_ms());
                if lease["operation_id"].as_str() == Some(op_id) || op_id.is_empty() || expired {
                    obj.remove("archive_lease");
                    obj.insert("paused".into(), json!(false));
                    if let Ok(bytes) = serde_json::to_vec(&control) {
                        let _ = files::atomic_write(&control_file, &bytes);
                    }
                }
            }
        }
    }
}

pub fn acquire_scanner_lease(config: &Config, plan: &Plan) -> Result<Value> {
    if plan.action != "archive"
        && !(supports_scanner_reconciliation(plan) && peach_state_dir(config).is_some())
    {
        return Ok(Value::Null);
    }
    let now = now_ms();
    let expires_at = now + lease_ttl_ms();
    let sources: Vec<String> = plan
        .files
        .iter()
        .map(|f| metadata::win(config, &f.src))
        .collect();
    let targets: Vec<String> = plan
        .files
        .iter()
        .map(|f| metadata::win(config, &f.dst))
        .collect();
    let scope = json!({
        "version": plan.version,
        "sources": sources,
        "targets": targets,
    });
    let mut conn = db::open(&config.ledger)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    ensure_lease_table(&tx)?;
    tx.execute(
        "UPDATE _face_admin_scanner_leases SET state='expired',updated_at=?1 WHERE state='active' AND expires_at<=?1",
        params![now],
    )?;
    tx.execute(
        "INSERT INTO _face_admin_scanner_leases(id,action,state,scope,acquired_at,expires_at,updated_at)
         VALUES(?1,?2,'active',?3,?4,?5,?4)
         ON CONFLICT(id) DO UPDATE SET state='active',scope=excluded.scope,expires_at=excluded.expires_at,updated_at=excluded.updated_at",
        params![plan.id, plan.action, serde_json::to_string(&scope)?, now, expires_at],
    )?;
    tx.commit()?;
    let payload = json!({
        "operation_id": plan.id,
        "action": plan.action,
        "state": "active",
        "scope": scope,
        "acquired_at": now,
        "expires_at": expires_at,
        "updated_at": now,
    });
    write_coordination_files(config, &payload);
    Ok(payload)
}

pub fn release_scanner_lease(config: &Config, operation_id: &str, terminal_state: &str) {
    let now = now_ms();
    let mut leased = false;
    if let Ok(mut conn) = db::open(&config.ledger)
        && let Ok(tx) = conn.transaction_with_behavior(TransactionBehavior::Immediate)
        && ensure_lease_table(&tx).is_ok()
    {
        let _ = tx.execute(
            "UPDATE _face_admin_scanner_leases SET state='expired',updated_at=?1 WHERE state='active' AND expires_at<=?1",
            params![now],
        );
        leased = tx
            .execute(
                "UPDATE _face_admin_scanner_leases SET state=?2,updated_at=?3 WHERE id=?1 AND state='active'",
                params![operation_id, terminal_state, now],
            )
            .unwrap_or(0)
            > 0;
        let _ = tx.commit();
    }
    let local = config
        .state_db()
        .with_file_name("scanner-coordination.json");
    if !leased
        && !local.exists()
        && !peach_state_dir(config)
            .is_some_and(|d| d.join("archive-scanner-coordination.json").exists())
    {
        return;
    }
    let payload = json!({
        "operation_id": operation_id,
        "action": "archive",
        "state": terminal_state,
        "updated_at": now,
    });
    write_coordination_files(config, &payload);
}

pub fn recover_scanner_leases(config: &Config, active_ids: &BTreeSet<String>) {
    let now = now_ms();
    if let Ok(mut conn) = db::open(&config.ledger)
        && let Ok(tx) = conn.transaction_with_behavior(TransactionBehavior::Immediate)
        && ensure_lease_table(&tx).is_ok()
    {
        let _ = tx.execute(
            "UPDATE _face_admin_scanner_leases SET state='expired',updated_at=?1 WHERE state='active' AND expires_at<=?1",
            params![now],
        );
        if let Ok(rows) = db::rows(
            &tx,
            "SELECT id FROM _face_admin_scanner_leases WHERE state='active'",
            [],
        ) {
            for row in rows {
                if let Some(id) = row["id"].as_str()
                    && !active_ids.contains(id)
                {
                    let _ = tx.execute(
                        "UPDATE _face_admin_scanner_leases SET state='recovered',updated_at=?2 WHERE id=?1 AND state='active'",
                        params![id, now],
                    );
                }
            }
        }
        let _ = tx.commit();
    }
    let local = config
        .state_db()
        .with_file_name("scanner-coordination.json");
    if let Ok(Some(raw)) = crate::control::read(&local, crate::control::MARKER_LIMIT)
        && let Ok(current) = serde_json::from_str::<Value>(&raw)
        && current["state"] == "active"
    {
        let op_id = current["operation_id"].as_str().unwrap_or("");
        let expired = current["expires_at"].as_i64().is_some_and(|exp| exp <= now);
        if expired || !active_ids.contains(op_id) {
            let payload = json!({
                "operation_id": op_id,
                "action": "archive",
                "state": if expired { "expired" } else { "recovered" },
                "updated_at": now,
            });
            write_coordination_files(config, &payload);
        }
    }
}

struct ReconcilableRow {
    stub: Value,
    original: Value,
    file: crate::model::FileMove,
    in_place: bool,
}

fn in_place_scanner_match(actual: &Value, expected: &Value, dst_name: Option<&str>) -> Result<()> {
    let actual_obj = actual.as_object().ok_or_else(conflict)?;
    let expected_obj = expected.as_object().ok_or_else(conflict)?;
    if actual_obj.len() != expected_obj.len() {
        return Err(conflict());
    }
    for (key, expected_val) in expected_obj {
        let Some(actual_val) = actual_obj.get(key) else {
            return Err(conflict());
        };
        if actual_val == expected_val {
            continue;
        }
        match key.as_str() {
            "name" => {
                if dst_name.is_none() || actual_val.as_str() != dst_name {
                    return Err(conflict());
                }
            }
            "last_seen" => {
                if actual_val.as_str().is_none_or(|s| s.trim().is_empty()) {
                    return Err(conflict());
                }
            }
            "mtime" => {
                let valid_str = actual_val.as_str().is_some_and(|s| !s.trim().is_empty());
                if !valid_str && db::integer(actual_val).is_err() {
                    return Err(conflict());
                }
            }
            "snapshot_path" => {
                if !expected_val.is_null()
                    || actual_val.as_str().is_none_or(|s| s.trim().is_empty())
                {
                    return Err(conflict());
                }
            }
            _ => return Err(conflict()),
        }
    }
    Ok(())
}

fn apply_in_place_asset_state(
    delta: &mut db::Delta,
    asset_id: &Value,
    win_src: &str,
    win_dst: &str,
    before_row: &Value,
    after_row: &Value,
    already_at_dst: bool,
) {
    for table in &mut delta.tables {
        if table.table == db::Table::Asset {
            for row in &mut table.before {
                if &row["id"] == asset_id {
                    *row = before_row.clone();
                }
            }
            for row in &mut table.after {
                if &row["id"] == asset_id {
                    *row = after_row.clone();
                }
            }
        }
    }
    for guard in &mut delta.guards {
        match guard {
            db::Guard::AssetPath {
                path,
                before,
                after,
            } => {
                if path == win_src {
                    if already_at_dst {
                        before.retain(|r| &r["id"] != asset_id);
                    } else {
                        for row in before.iter_mut() {
                            if &row["id"] == asset_id {
                                *row = before_row.clone();
                            }
                        }
                    }
                } else if path == win_dst {
                    if already_at_dst {
                        *before = vec![before_row.clone()];
                    }
                    for row in after.iter_mut() {
                        if &row["id"] == asset_id {
                            *row = after_row.clone();
                        }
                    }
                }
            }
            db::Guard::AssetPrefix {
                prefix,
                before,
                after,
            } => {
                let src_matches = win_src.starts_with(prefix.as_str());
                let dst_matches = win_dst.starts_with(prefix.as_str());
                if already_at_dst {
                    if src_matches && !dst_matches {
                        before.retain(|r| &r["id"] != asset_id);
                    } else if dst_matches {
                        if let Some(existing) = before.iter_mut().find(|r| &r["id"] == asset_id) {
                            *existing = before_row.clone();
                        } else {
                            before.push(before_row.clone());
                        }
                    }
                } else if src_matches {
                    for row in before.iter_mut() {
                        if &row["id"] == asset_id {
                            *row = before_row.clone();
                        }
                    }
                }
                if dst_matches {
                    for row in after.iter_mut() {
                        if &row["id"] == asset_id {
                            *row = after_row.clone();
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

pub(crate) fn is_pure_scanner_stub(
    conn: &Connection,
    row: &Value,
    original: &Value,
    expected_name: Option<&str>,
    expected_size: Option<i64>,
) -> Result<()> {
    let known: BTreeSet<&str> = "id location path name medium size mtime hash_kind hash creator studio series code duration width height vcodec fps has_audio ctx_length ctx_orient ctx_quality ctx_pace ctx_people play_count last_played rating o_count watch_ratio source_id stash_scene_id snapshot_path first_seen last_seen feedback disposal leave_ratio play_seconds feedback_at seek_count max_reached release_date catalog_title original_title field_owners mutation_revision region disposal_reason".split_whitespace().collect();
    let index: BTreeSet<&str> = "id location path name medium size mtime hash_kind hash duration width height vcodec fps has_audio ctx_length ctx_orient ctx_quality first_seen last_seen".split_whitespace().collect();
    let id = db::integer(&row["id"])?;
    if id <= 0
        || id == db::integer(&original["id"])?
        || row["location"] != original["location"]
        || row["name"].as_str() != expected_name
        || row["medium"] != "video"
        || Some(db::integer(&row["size"])?) != expected_size
    {
        return Err(conflict());
    }
    for (name, value) in row.as_object().ok_or_else(conflict)? {
        if !known.contains(name.as_str()) {
            return Err(conflict());
        }
        if index.contains(name.as_str()) {
            if value.is_object() && db::integer(value).is_err() {
                return Err(conflict());
            }
        } else if name == "play_count" || name == "mutation_revision" {
            if db::integer(value)? != 0 {
                return Err(conflict());
            }
        } else if !value.is_null() {
            return Err(conflict());
        }
    }
    crate::delta::protect_references(conn, db::Table::Asset, row, None, &["id".into()])?;
    // Peach also has semantic IDs without declared FKs. Do not mistake the
    // derived FTS index for a user reference; never write any of these tables.
    let tables = db::rows(
        conn,
        "SELECT name,sql FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
        [],
    )?;
    if tables.len() > 512 {
        return Err(conflict());
    }
    for table in tables {
        let name = table["name"].as_str().ok_or_else(conflict)?;
        let definition = table["sql"]
            .as_str()
            .unwrap_or("")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_ascii_lowercase();
        let is_derived_fts = name.eq_ignore_ascii_case("asset_search")
            && definition.starts_with("create virtual table ")
            && definition.split_once(" using ").is_some_and(|(_, module)| {
                module.starts_with("fts5(") || module.starts_with("fts5 (")
            });
        if is_derived_fts {
            continue;
        }
        let columns = db::rows(conn, &format!("PRAGMA table_info({})", quoted(name)), [])?;
        if columns.len() > 256 {
            return Err(conflict());
        }
        for column in columns {
            let col = column["name"].as_str().ok_or_else(conflict)?;
            let semantic = col.to_ascii_lowercase();
            if semantic == "asset_id" || semantic.ends_with("_asset_id") {
                let exists: bool = conn.query_row(
                    &format!(
                        "SELECT EXISTS(SELECT 1 FROM {} c JOIN asset p ON p.id=c.{} WHERE p.id=?)",
                        quoted(name),
                        quoted(col)
                    ),
                    [id],
                    |r| r.get(0),
                )?;
                if exists {
                    return Err(conflict());
                }
            } else if semantic.contains("asset_ids") {
                // An unfamiliar plural/JSON reference format is not absence proof.
                return Err(conflict());
            }
        }
    }
    Ok(())
}

fn index_only(
    conn: &Connection,
    row: &Value,
    original: &Value,
    file: &crate::model::FileMove,
) -> Result<()> {
    is_pure_scanner_stub(
        conn,
        row,
        original,
        Path::new(&file.dst).file_name().and_then(|s| s.to_str()),
        Some(file.size as i64),
    )
}

fn observe_scanner_rows(
    conn: &Connection,
    config: &Config,
    plan: &Plan,
    delta: &ledger::Delta,
) -> Result<(Vec<ReconcilableRow>, db::Delta)> {
    if !supports_scanner_reconciliation(plan) {
        return Err(conflict());
    }
    let assets = delta
        .changes
        .tables
        .iter()
        .find(|t| t.table == db::Table::Asset)
        .ok_or_else(conflict)?;
    let targets: Vec<(String, Value)> = if plan.action == "archive" {
        delta
            .changes
            .guards
            .iter()
            .filter_map(|guard| match guard {
                db::Guard::AssetPath {
                    path,
                    before,
                    after,
                } if before.is_empty() && after.len() == 1 => {
                    Some((path.clone(), after[0].clone()))
                }
                _ => None,
            })
            .collect()
    } else {
        assets
            .after
            .iter()
            .filter_map(|after_row| {
                let path = after_row["path"].as_str()?;
                let occupied_before = delta.changes.guards.iter().any(|guard| match guard {
                    db::Guard::AssetPrefix { prefix, before, .. }
                        if path.starts_with(prefix.as_str()) =>
                    {
                        before.iter().any(|r| r["path"] == path)
                    }
                    _ => false,
                });
                (!occupied_before).then(|| (path.to_owned(), after_row.clone()))
            })
            .collect()
    };
    let mut observed = delta.changes.clone();
    let mut application = delta.changes.clone();
    let mut pairs = vec![];
    for (path, after_row) in &targets {
        let Some(file) = plan.files.iter().find(|f| {
            Path::new(&f.dst).starts_with(config.creators())
                && metadata::win(config, &f.dst) == *path
        }) else {
            continue;
        };
        let original = assets
            .before
            .iter()
            .find(|row| row["id"] == after_row["id"])
            .ok_or_else(conflict)?;
        let win_src = metadata::win(config, &file.src);
        if original["path"] != win_src {
            return Err(conflict());
        }
        let orig_id = db::integer(&original["id"])?;
        let current = db::rows(
            conn,
            "SELECT * FROM asset WHERE path=? ORDER BY id LIMIT 2",
            [path.as_str()],
        )?;
        if current.len() > 1 {
            return Err(conflict());
        }
        if let Some(dst_row) = current.first() {
            let stub_id = db::integer(&dst_row["id"])?;
            if stub_id == orig_id {
                let src_rows = db::rows(
                    conn,
                    "SELECT * FROM asset WHERE path=? ORDER BY id LIMIT 1",
                    [win_src.as_str()],
                )?;
                if !src_rows.is_empty() {
                    return Err(conflict());
                }
                let dst_name = Path::new(&file.dst).file_name().and_then(|s| s.to_str());
                in_place_scanner_match(dst_row, after_row, dst_name)?;
                apply_in_place_asset_state(
                    &mut observed,
                    &original["id"],
                    &win_src,
                    path,
                    dst_row,
                    dst_row,
                    true,
                );
                apply_in_place_asset_state(
                    &mut application,
                    &original["id"],
                    &win_src,
                    path,
                    dst_row,
                    dst_row,
                    true,
                );
                pairs.push(ReconcilableRow {
                    stub: dst_row.clone(),
                    original: original.clone(),
                    file: file.clone(),
                    in_place: true,
                });
                continue;
            }
            if assets
                .before
                .iter()
                .any(|row| db::integer(&row["id"]).ok() == Some(stub_id))
            {
                return Err(conflict());
            }
            index_only(conn, dst_row, original, file)?;
            if let Some(db::Guard::AssetPath {
                before: obs_before, ..
            }) = observed
                .guards
                .iter_mut()
                .find(|g| matches!(g, db::Guard::AssetPath { path: p, .. } if p == path))
            {
                *obs_before = vec![dst_row.clone()];
            }
            for obs_guard in &mut observed.guards {
                if let db::Guard::AssetPrefix {
                    prefix,
                    before: obs_before,
                    ..
                } = obs_guard
                    && path.starts_with(prefix.as_str())
                    && !obs_before.iter().any(|r| r["id"] == dst_row["id"])
                {
                    obs_before.push(dst_row.clone());
                }
            }
            let src_actual = db::rows(conn, "SELECT * FROM asset WHERE id=? LIMIT 1", [orig_id])?;
            if let Some(src_row) = src_actual.first()
                && src_row != original
            {
                in_place_scanner_match(src_row, original, None)?;
                let mut updated_after = after_row.clone();
                updated_after["last_seen"] = src_row["last_seen"].clone();
                updated_after["mtime"] = src_row["mtime"].clone();
                if updated_after.get("snapshot_path") == Some(&Value::Null)
                    && src_row.get("snapshot_path").is_some_and(|v| !v.is_null())
                {
                    updated_after["snapshot_path"] = src_row["snapshot_path"].clone();
                }
                apply_in_place_asset_state(
                    &mut observed,
                    &original["id"],
                    &win_src,
                    path,
                    src_row,
                    &updated_after,
                    false,
                );
                apply_in_place_asset_state(
                    &mut application,
                    &original["id"],
                    &win_src,
                    path,
                    src_row,
                    &updated_after,
                    false,
                );
            }
            pairs.push(ReconcilableRow {
                stub: dst_row.clone(),
                original: original.clone(),
                file: file.clone(),
                in_place: false,
            });
        } else {
            let src_actual = db::rows(conn, "SELECT * FROM asset WHERE id=? LIMIT 1", [orig_id])?;
            if let Some(src_row) = src_actual.first()
                && src_row != original
            {
                in_place_scanner_match(src_row, original, None)?;
                let mut updated_after = after_row.clone();
                updated_after["last_seen"] = src_row["last_seen"].clone();
                updated_after["mtime"] = src_row["mtime"].clone();
                if updated_after.get("snapshot_path") == Some(&Value::Null)
                    && src_row.get("snapshot_path").is_some_and(|v| !v.is_null())
                {
                    updated_after["snapshot_path"] = src_row["snapshot_path"].clone();
                }
                apply_in_place_asset_state(
                    &mut observed,
                    &original["id"],
                    &win_src,
                    path,
                    src_row,
                    &updated_after,
                    false,
                );
                apply_in_place_asset_state(
                    &mut application,
                    &original["id"],
                    &win_src,
                    path,
                    src_row,
                    &updated_after,
                    false,
                );
                pairs.push(ReconcilableRow {
                    stub: src_row.clone(),
                    original: original.clone(),
                    file: file.clone(),
                    in_place: true,
                });
            }
        }
    }
    if plan.action != "archive" {
        for file in &plan.files {
            if !Path::new(&file.dst).starts_with(config.creators()) {
                continue;
            }
            let path = metadata::win(config, &file.dst);
            if targets.iter().any(|(p, _)| p == &path) {
                continue;
            }
            let occupied_before = delta.changes.guards.iter().any(|guard| match guard {
                db::Guard::AssetPrefix { prefix, before, .. }
                    if path.starts_with(prefix.as_str()) =>
                {
                    before.iter().any(|r| r["path"] == path)
                }
                _ => false,
            });
            if occupied_before {
                continue;
            }
            let current = db::rows(
                conn,
                "SELECT * FROM asset WHERE path=? ORDER BY id LIMIT 2",
                [path.as_str()],
            )?;
            if current.len() > 1 {
                return Err(conflict());
            }
            if let Some(dst_row) = current.first() {
                let stub_id = db::integer(&dst_row["id"])?;
                if assets
                    .before
                    .iter()
                    .any(|row| db::integer(&row["id"]).ok() == Some(stub_id))
                {
                    return Err(conflict());
                }
                index_only(
                    conn,
                    dst_row,
                    &json!({"id":{"$integer":"0"},"location":"local"}),
                    file,
                )?;
                for obs_guard in &mut observed.guards {
                    if let db::Guard::AssetPrefix {
                        prefix,
                        before: obs_before,
                        ..
                    } = obs_guard
                        && path.starts_with(prefix.as_str())
                        && !obs_before.iter().any(|r| r["id"] == dst_row["id"])
                    {
                        obs_before.push(dst_row.clone());
                    }
                }
                pairs.push(ReconcilableRow {
                    stub: dst_row.clone(),
                    original: Value::Null,
                    file: file.clone(),
                    in_place: false,
                });
            }
        }
        if !plan.target_person.is_empty() {
            let target_dir = config.creators().join(&plan.target_person);
            let win_target_prefix = format!(
                "{}\\",
                metadata::win(config, &target_dir.to_string_lossy())
            );
            let prefix_rows = db::rows(
                conn,
                "SELECT * FROM asset WHERE substr(path,1,length(?))=? ORDER BY id",
                [win_target_prefix.as_str(), win_target_prefix.as_str()],
            )?;
            for row in prefix_rows {
                let already_observed = observed.guards.iter().any(|g| match g {
                    db::Guard::AssetPrefix { prefix, before, .. }
                        if *prefix == win_target_prefix =>
                    {
                        before.iter().any(|r| r["id"] == row["id"])
                    }
                    _ => false,
                });
                if already_observed {
                    continue;
                }
                let Some(row_path) = row["path"].as_str() else {
                    return Err(conflict());
                };
                if assets
                    .before
                    .iter()
                    .chain(&assets.after)
                    .any(|r| r["id"] == row["id"] || r["path"] == row_path)
                    || plan.files.iter().any(|f| {
                        metadata::win(config, &f.src) == row_path
                            || metadata::win(config, &f.dst) == row_path
                    })
                    || observed.guards.iter().any(|g| match g {
                        db::Guard::AssetPrefix { before, after, .. } => before
                            .iter()
                            .chain(after)
                            .any(|r| r["id"] == row["id"] || r["path"] == row_path),
                        _ => false,
                    })
                {
                    return Err(conflict());
                }
                let rel = row_path
                    .strip_prefix(&win_target_prefix)
                    .ok_or_else(conflict)?;
                let mut native_path = target_dir.clone();
                for part in rel.split('\\').filter(|s| !s.is_empty()) {
                    if part == "." || part == ".." {
                        return Err(conflict());
                    }
                    native_path.push(part);
                }
                let meta = std::fs::symlink_metadata(&native_path).map_err(|_| conflict())?;
                if meta.file_type().is_symlink() || !meta.is_file() {
                    return Err(conflict());
                }
                let dummy_file = crate::model::FileMove {
                    vids: vec![],
                    src: String::new(),
                    dst: native_path.to_string_lossy().into_owned(),
                    size: meta.len(),
                    version: String::new(),
                };
                index_only(
                    conn,
                    &row,
                    &json!({"id":{"$integer":"0"},"location":"local"}),
                    &dummy_file,
                )?;
                for guard in observed.guards.iter_mut().chain(application.guards.iter_mut()) {
                    if let db::Guard::AssetPrefix {
                        prefix,
                        before,
                        after,
                    } = guard
                        && row_path.starts_with(prefix.as_str())
                    {
                        if !before.iter().any(|r| r["id"] == row["id"]) {
                            before.push(row.clone());
                        }
                        if !after.iter().any(|r| r["id"] == row["id"]) {
                            after.push(row.clone());
                        }
                    }
                }
                pairs.push(ReconcilableRow {
                    stub: row,
                    original: Value::Null,
                    file: dummy_file,
                    in_place: true,
                });
            }
        }
    }
    if pairs.is_empty() {
        return Err(conflict());
    }
    creator_delta::inspect_before(conn, &observed, &delta.creators)?;
    Ok((pairs, application))
}

fn read_ops_receipt(conn: &Connection, id: &str) -> Result<Option<(String, String, String)>> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='_face_admin_person_ops')",
        [],
        |r| r.get(0),
    )?;
    if !exists {
        return Ok(None);
    }
    conn.query_row(
        "SELECT signature,state,data FROM _face_admin_person_ops WHERE id=?",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )
    .optional()
    .map_err(Into::into)
}

fn reconcile_peach_state_files(config: &Config, pairs: &[ReconcilableRow]) {
    let Some(state_dir) = peach_state_dir(config) else {
        return;
    };
    if pairs.is_empty() {
        return;
    }
    let deleted_stub_ids: BTreeSet<i64> = pairs
        .iter()
        .filter(|p| !p.in_place)
        .filter_map(|p| db::integer(&p.stub["id"]).ok())
        .collect();
    if !deleted_stub_ids.is_empty() {
        let new_assets_path = state_dir.join("library-new-assets.json");
        if new_assets_path.is_file()
            && let Ok(Some(raw)) =
                crate::control::read(&new_assets_path, crate::control::MARKER_LIMIT)
            && let Ok(Value::Array(items)) = serde_json::from_str::<Value>(&raw)
        {
            let filtered: Vec<Value> = items
                .into_iter()
                .filter(|v| {
                    let id = v.as_i64().or_else(|| db::integer(v).ok());
                    id.is_none_or(|n| !deleted_stub_ids.contains(&n))
                })
                .collect();
            if let Ok(bytes) = serde_json::to_vec(&Value::Array(filtered)) {
                let _ = files::atomic_write(&new_assets_path, &bytes);
            }
        }
    }
    let identities_path = state_dir.join("local-file-identities.json");
    if identities_path.is_file()
        && let Ok(Some(raw)) = crate::control::read(&identities_path, crate::control::MARKER_LIMIT)
        && let Ok(Value::Object(mut map)) = serde_json::from_str::<Value>(&raw)
    {
        for pair in pairs {
            let Ok(stub_id) = db::integer(&pair.stub["id"]) else {
                continue;
            };
            let removed = if pair.in_place {
                None
            } else {
                map.remove(&stub_id.to_string())
            };
            let Ok(original_id) = db::integer(&pair.original["id"]) else {
                continue;
            };
            let identity = removed
                .as_ref()
                .and_then(|v| v.get("identity"))
                .cloned()
                .or_else(|| {
                    map.get(&original_id.to_string())
                        .and_then(|v| v.get("identity"))
                        .cloned()
                })
                .unwrap_or_else(|| json!([]));
            let win_dst = metadata::win(config, &pair.file.dst);
            let location = pair.original["location"].as_str().unwrap_or("local");
            map.insert(
                original_id.to_string(),
                json!({
                    "id": original_id,
                    "location": location,
                    "path": win_dst,
                    "native": pair.file.dst,
                    "identity": identity,
                }),
            );
        }
        if let Ok(bytes) = serde_json::to_vec(&Value::Object(map)) {
            let _ = files::atomic_write(&identities_path, &bytes);
        }
    }
}

pub fn reserve_with_scanner_index(
    config: &Config,
    plan: &Plan,
    delta: &ledger::Delta,
    allow_absent: bool,
) -> Result<Vec<Value>> {
    let mut conn = db::open(&config.ledger)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let (pairs, _) = observe_scanner_rows(&tx, config, plan, delta)?;
    let retained: Vec<Value> = pairs.iter().map(|p| p.stub.clone()).collect();
    if let Some((signature, state, raw)) = read_ops_receipt(&tx, &plan.id)? {
        let evidence: Value = serde_json::from_str(&raw)?;
        if signature != plan.version
            || state != "prepared"
            || evidence["backend"] != "rust"
            || evidence["kind"].as_str() != Some(plan.action.as_str())
            || evidence["delta"] != serde_json::to_value(delta)?
        {
            return Err(conflict());
        }
        return Ok(retained);
    }
    if !allow_absent {
        return Err(conflict());
    }
    tx.execute_batch("CREATE TABLE IF NOT EXISTS _face_admin_person_ops (id TEXT PRIMARY KEY, signature TEXT NOT NULL, state TEXT NOT NULL, data TEXT NOT NULL)")?;
    tx.execute(
        "INSERT INTO _face_admin_person_ops VALUES(?,?,'prepared',?)",
        params![
            plan.id,
            plan.version,
            serde_json::to_string(&json!({"backend":"rust","kind":plan.action,"delta":delta}))?
        ],
    )?;
    tx.commit()?;
    Ok(retained)
}

pub fn apply_with_scanner_reconciliation(
    config: &Config,
    plan: &Plan,
    delta: &ledger::Delta,
) -> Result<Value> {
    let mut conn = db::open(&config.ledger)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let (signature, state, raw) = read_ops_receipt(&tx, &plan.id)?.ok_or_else(conflict)?;
    let evidence: Value = serde_json::from_str(&raw)?;
    if signature != plan.version
        || state != "prepared"
        || evidence["backend"] != "rust"
        || evidence["kind"].as_str() != Some(plan.action.as_str())
        || evidence["delta"] != serde_json::to_value(delta)?
    {
        return Err(conflict());
    }
    let (pairs, application) = observe_scanner_rows(&tx, config, plan, delta)?;
    let retained: Vec<Value> = pairs.iter().map(|p| p.stub.clone()).collect();
    let adopted_ids: Vec<i64> = pairs
        .iter()
        .map(|p| db::integer(&p.stub["id"]))
        .collect::<Result<Vec<_>>>()?;
    for pair in &pairs {
        if pair.in_place {
            continue;
        }
        let stub_id = db::integer(&pair.stub["id"])?;
        let location = pair.stub["location"].as_str().ok_or_else(conflict)?;
        let path = pair.stub["path"].as_str().ok_or_else(conflict)?;
        let deleted = tx.execute(
            "DELETE FROM asset WHERE id=?1 AND location=?2 AND path=?3",
            params![stub_id, location, path],
        )?;
        if deleted != 1 {
            return Err(conflict());
        }
    }
    let resolved = creator_delta::apply(&tx, &application, &delta.creators)?;
    let committed_evidence = json!({
        "backend": "rust",
        "kind": plan.action,
        "delta": delta,
        "resolved": resolved,
        "reconciled_scanner_assets": adopted_ids,
        "reconciled_scanner_rows": retained,
    });
    let changed = tx.execute(
        "UPDATE _face_admin_person_ops SET state='committed',data=? WHERE id=? AND signature=? AND state='prepared'",
        params![
            serde_json::to_string(&committed_evidence)?,
            plan.id,
            plan.version
        ],
    )?;
    if changed != 1 {
        return Err(conflict());
    }
    tx.commit()?;
    reconcile_peach_state_files(config, &pairs);
    Ok(committed_evidence)
}

pub fn committed_reconciled_assets(config: &Config, plan: &Plan) -> Result<Vec<i64>> {
    let conn = db::open(&config.ledger)?;
    let Some((signature, state, raw)) = read_ops_receipt(&conn, &plan.id)? else {
        return Ok(vec![]);
    };
    if signature != plan.version || state != "committed" {
        return Ok(vec![]);
    }
    let evidence: Value = serde_json::from_str(&raw)?;
    let ids: Vec<i64> = evidence["reconciled_scanner_assets"]
        .as_array()
        .map(|arr| arr.iter().filter_map(Value::as_i64).collect())
        .unwrap_or_default();
    if !ids.is_empty()
        && let Some(stubs) = evidence["reconciled_scanner_rows"].as_array()
        && let Some(assets) = evidence["delta"]["changes"]["tables"]
            .as_array()
            .and_then(|tables| tables.iter().find(|t| t["table"] == "Asset"))
    {
        let before = assets["before"].as_array().cloned().unwrap_or_default();
        let after = assets["after"].as_array().cloned().unwrap_or_default();
        let mut pairs = vec![];
        for stub in stubs {
            if let Some(after_row) = after
                .iter()
                .find(|r| r["path"] == stub["path"] || r["id"] == stub["id"])
                && let Some(original) = before.iter().find(|r| r["id"] == after_row["id"])
                && let Some(file) = plan.files.iter().find(|f| {
                    metadata::win(config, &f.dst) == after_row["path"].as_str().unwrap_or("")
                })
            {
                let in_place = stub["id"] == original["id"];
                pairs.push(ReconcilableRow {
                    stub: stub.clone(),
                    original: original.clone(),
                    file: file.clone(),
                    in_place,
                });
            }
        }
        reconcile_peach_state_files(config, &pairs);
    }
    Ok(ids)
}

fn proof(
    conn: &Connection,
    config: &Config,
    plan: &Plan,
    delta: &ledger::Delta,
    allow_unreserved: bool,
) -> Result<(String, Value, Vec<Value>, bool)> {
    let (pairs, _) = observe_scanner_rows(conn, config, plan, delta)?;
    let has_in_place = pairs.iter().any(|p| p.in_place);
    let retained: Vec<Value> = pairs.iter().map(|p| p.stub.clone()).collect();
    let Some((signature, state, raw)) = read_ops_receipt(conn, &plan.id)? else {
        if !allow_unreserved {
            return Err(conflict());
        }
        let evidence = json!({"backend":"rust","kind":plan.action,"delta":delta});
        return Ok(("absent".into(), evidence, retained, has_in_place));
    };
    let evidence: Value = serde_json::from_str(&raw)?;
    if signature != plan.version
        || !["prepared", "cancelled"].contains(&state.as_str())
        || evidence["backend"] != "rust"
        || evidence["kind"].as_str() != Some(plan.action.as_str())
        || evidence["delta"] != serde_json::to_value(delta)?
    {
        return Err(conflict());
    }
    if let Some(previous) = evidence.get("archive_cancel_indexes")
        && previous != &json!(retained)
    {
        return Err(conflict());
    }
    Ok((state, evidence, retained, has_in_place))
}

pub fn inspect(
    config: &Config,
    plan: &Plan,
    delta: &ledger::Delta,
    allow_unreserved: bool,
) -> Result<Vec<Value>> {
    let mut conn = db::open(&config.ledger)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    Ok(proof(&tx, config, plan, delta, allow_unreserved)?.2)
}

pub fn cancel(
    config: &Config,
    plan: &Plan,
    delta: &ledger::Delta,
    expected: &[Value],
    allow_unreserved: bool,
) -> Result<()> {
    let mut conn = db::open(&config.ledger)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let (state, mut evidence, rows, has_in_place) =
        proof(&tx, config, plan, delta, allow_unreserved)?;
    if rows != expected || has_in_place {
        return Err(conflict());
    }
    evidence["archive_cancel_indexes"] = json!(rows);
    if state == "absent" {
        tx.execute_batch("CREATE TABLE IF NOT EXISTS _face_admin_person_ops (id TEXT PRIMARY KEY, signature TEXT NOT NULL, state TEXT NOT NULL, data TEXT NOT NULL)")?;
        tx.execute(
            "INSERT INTO _face_admin_person_ops VALUES(?,?,'cancelled',?)",
            params![plan.id, plan.version, serde_json::to_string(&evidence)?],
        )?;
    } else if tx.execute(
        "UPDATE _face_admin_person_ops SET state='cancelled',data=? WHERE id=? AND signature=? AND state=?",
        params![serde_json::to_string(&evidence)?, plan.id, plan.version, state],
    )? != 1
    {
        return Err(conflict());
    }
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FileMove, Request};
    use std::fs;

    fn test_config_and_plan() -> (PathBuf, Config, Plan) {
        let root = fs::canonicalize("/tmp")
            .unwrap()
            .join(format!("face-admin-scanner-coord-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("base")).unwrap();
        fs::create_dir_all(root.join("media/Creators/甲")).unwrap();
        fs::create_dir_all(root.join("state")).unwrap();
        let config = Config {
            base: root.join("base"),
            media: root.join("media"),
            ledger: root.join("ledger.db"),
            bind: "127.0.0.1:0".into(),
            token: None,
            parity_root: None,
            fixture: Some(root.clone()),
        };
        let conn = Connection::open(&config.ledger).unwrap();
        conn.execute_batch("CREATE TABLE IF NOT EXISTS asset (id INTEGER PRIMARY KEY)")
            .unwrap();
        let dst = config.creators().join("甲/clip.mp4");
        fs::create_dir_all(dst.parent().unwrap()).unwrap();
        fs::write(&dst, b"video-bytes").unwrap();
        let stat = fs::metadata(&dst).unwrap();
        let dst_str = dst.display().to_string();
        let src_str = config
            .media
            .join("Downloads/clip.mp4")
            .display()
            .to_string();
        let id = uuid::Uuid::new_v4().to_string();
        let plan = Plan {
            id: id.clone(),
            request: Request {
                id,
                action: "archive".into(),
                person: "甲".into(),
                files: vec![],
                filename: String::new(),
                target_person: None,
                target_folder: None,
                payload: None,
            },
            version: "v1".into(),
            action: "archive".into(),
            person: "甲".into(),
            target_person: String::new(),
            total_bytes: stat.len(),
            files: vec![FileMove {
                src: src_str,
                dst: dst_str.clone(),
                vids: vec!["v1".into()],
                size: stat.len(),
                version: files::fingerprint(&dst_str, &stat),
            }],
            directories: vec![],
            metadata_version: String::new(),
            ledger_version: String::new(),
            execution: None,
        };
        (root, config, plan)
    }

    #[test]
    fn scanner_lease_lifecycle_and_user_pause_preservation() {
        let (root, config, plan) = test_config_and_plan();
        let control_path = root.join("state/library-processing-control.json");
        fs::write(&control_path, r#"{"paused":false}"#).unwrap();

        acquire_scanner_lease(&config, &plan).unwrap();
        let control: Value =
            serde_json::from_str(&fs::read_to_string(&control_path).unwrap()).unwrap();
        assert_eq!(control["paused"], true);
        assert_eq!(control["archive_lease"]["operation_id"], plan.id);
        let conn = db::open(&config.ledger).unwrap();
        let state: String = conn
            .query_row(
                "SELECT state FROM _face_admin_scanner_leases WHERE id=?1",
                [&plan.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(state, "active");

        release_scanner_lease(&config, &plan.id, "committed");
        let control: Value =
            serde_json::from_str(&fs::read_to_string(&control_path).unwrap()).unwrap();
        assert_eq!(control["paused"], false);
        assert!(control.get("archive_lease").is_none());
        let state: String = conn
            .query_row(
                "SELECT state FROM _face_admin_scanner_leases WHERE id=?1",
                [&plan.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(state, "committed");

        // User-initiated pause must be preserved across archive lease acquire & release.
        fs::write(&control_path, r#"{"paused":true,"reason":"manual"}"#).unwrap();
        acquire_scanner_lease(&config, &plan).unwrap();
        let control: Value =
            serde_json::from_str(&fs::read_to_string(&control_path).unwrap()).unwrap();
        assert_eq!(control["paused"], true);
        assert_eq!(control["reason"], "manual");
        assert!(control.get("archive_lease").is_none());
        release_scanner_lease(&config, &plan.id, "committed");
        let control: Value =
            serde_json::from_str(&fs::read_to_string(&control_path).unwrap()).unwrap();
        assert_eq!(control["paused"], true);
        assert_eq!(control["reason"], "manual");

        // Expired lease recovery clears stale archive_lease pause.
        fs::write(&control_path, r#"{"paused":false}"#).unwrap();
        acquire_scanner_lease(&config, &plan).unwrap();
        conn.execute(
            "UPDATE _face_admin_scanner_leases SET expires_at=1 WHERE id=?1",
            [&plan.id],
        )
        .unwrap();
        let local_coord = config
            .state_db()
            .with_file_name("scanner-coordination.json");
        let mut coord: Value =
            serde_json::from_str(&fs::read_to_string(&local_coord).unwrap()).unwrap();
        coord["expires_at"] = json!(1);
        fs::write(&local_coord, serde_json::to_vec(&coord).unwrap()).unwrap();
        recover_scanner_leases(&config, &BTreeSet::new());
        let state: String = conn
            .query_row(
                "SELECT state FROM _face_admin_scanner_leases WHERE id=?1",
                [&plan.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(state, "expired");
        let control: Value =
            serde_json::from_str(&fs::read_to_string(&control_path).unwrap()).unwrap();
        assert_eq!(control["paused"], false);
        assert!(control.get("archive_lease").is_none());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn peach_sidecar_state_reconciliation_cleans_stub_and_updates_identity() {
        let (root, config, plan) = test_config_and_plan();
        let new_assets_path = root.join("state/library-new-assets.json");
        let identities_path = root.join("state/local-file-identities.json");
        fs::write(&new_assets_path, r#"[1,1001,42]"#).unwrap();
        fs::write(
            &identities_path,
            r#"{"1":{"id":1,"location":"local","path":"Downloads\\clip.mp4","identity":[11,22]},"1001":{"id":1001,"location":"local","path":"creator-videos\\甲\\clip.mp4","identity":[33,44]}}"#,
        )
        .unwrap();

        let pair = ReconcilableRow {
            stub: json!({
                "id": {"$integer": "1001"},
                "location": "local",
                "path": "creator-videos\\甲\\clip.mp4",
            }),
            original: json!({
                "id": {"$integer": "1"},
                "location": "local",
                "path": "Downloads\\clip.mp4",
            }),
            file: plan.files[0].clone(),
            in_place: false,
        };
        reconcile_peach_state_files(&config, &[pair]);

        let new_assets: Value =
            serde_json::from_str(&fs::read_to_string(&new_assets_path).unwrap()).unwrap();
        assert_eq!(new_assets, json!([1, 42]));

        let identities: Value =
            serde_json::from_str(&fs::read_to_string(&identities_path).unwrap()).unwrap();
        assert!(identities.get("1001").is_none());
        assert_eq!(identities["1"]["id"], 1);
        assert_eq!(
            identities["1"]["path"],
            metadata::win(&config, &plan.files[0].dst)
        );
        assert_eq!(identities["1"]["identity"], json!([33, 44]));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn in_place_scanner_move_reconciles_and_applies_creator_link() {
        let (root, config, mut plan) = test_config_and_plan();
        plan.files[0].dst = config
            .creators()
            .join("甲/clip (保留两份-e39819aa).mp4")
            .display()
            .to_string();
        let conn = db::open(&config.ledger).unwrap();
        conn.execute_batch(
            "DROP TABLE IF EXISTS asset;
             CREATE TABLE asset (
                 id INTEGER PRIMARY KEY,
                 location TEXT NOT NULL,
                 path TEXT NOT NULL,
                 name TEXT NOT NULL,
                 medium TEXT NOT NULL,
                 size INTEGER NOT NULL,
                 mtime TEXT NOT NULL,
                 last_seen TEXT NOT NULL
             );
             CREATE TABLE entity (
                 id INTEGER PRIMARY KEY,
                 kind TEXT NOT NULL,
                 canonical_name TEXT NOT NULL,
                 normalized_name TEXT NOT NULL
             );
             CREATE TABLE asset_entity (
                 asset_id INTEGER NOT NULL REFERENCES asset(id),
                 entity_id INTEGER NOT NULL REFERENCES entity(id),
                 role TEXT NOT NULL,
                 source TEXT NOT NULL,
                 PRIMARY KEY(asset_id, entity_id, role, source)
             );
             INSERT INTO entity(id, kind, canonical_name, normalized_name) VALUES(7, 'creator', '甲', '甲');",
        )
        .unwrap();
        let win_src = metadata::win(&config, &plan.files[0].src);
        let win_dst = metadata::win(&config, &plan.files[0].dst);
        let size = plan.files[0].size as i64;
        let before_asset = json!({
            "id": {"$integer": "1"},
            "location": "local",
            "path": win_src,
            "name": "clip.mp4",
            "medium": "video",
            "size": {"$integer": size.to_string()},
            "mtime": "2026-03-28",
            "last_seen": "2026-11-08 16:40:26",
        });
        let after_asset = json!({
            "id": {"$integer": "1"},
            "location": "local",
            "path": win_dst,
            "name": "clip.mp4",
            "medium": "video",
            "size": {"$integer": size.to_string()},
            "mtime": "2026-03-28",
            "last_seen": "2026-11-08 16:40:26",
        });
        let link = json!({
            "asset_id": {"$integer": "1"},
            "entity_id": {"$integer": "7"},
            "role": "creator",
            "source": "manual",
        });
        let delta = ledger::Delta {
            changes: db::Delta {
                tables: vec![
                    crate::delta::TableDelta {
                        table: db::Table::Asset,
                        key_columns: vec!["id".into()],
                        scope: vec![json!({"id": {"$integer": "1"}})],
                        before: vec![before_asset.clone()],
                        after: vec![after_asset.clone()],
                    },
                    crate::delta::TableDelta {
                        table: db::Table::AssetEntity,
                        key_columns: vec![
                            "asset_id".into(),
                            "entity_id".into(),
                            "role".into(),
                            "source".into(),
                        ],
                        scope: vec![link.clone()],
                        before: vec![],
                        after: vec![link.clone()],
                    },
                ],
                guards: vec![
                    db::Guard::AssetPath {
                        path: win_src.clone(),
                        before: vec![before_asset],
                        after: vec![],
                    },
                    db::Guard::AssetPath {
                        path: win_dst.clone(),
                        before: vec![],
                        after: vec![after_asset],
                    },
                    db::Guard::AssetLinks {
                        asset_id: json!({"$integer": "1"}),
                        before: vec![],
                        after: vec![link.clone()],
                    },
                ],
            },
            creators: vec![],
        };

        // Simulate Peach scanner having moved id=1 in-place to win_dst with suffixed name and refreshed last_seen.
        conn.execute(
            "INSERT INTO asset(id, location, path, name, medium, size, mtime, last_seen) VALUES(1, 'local', ?1, 'clip (保留两份-e39819aa).mp4', 'video', ?2, '2026-03-28', '2026-11-08 17:06:49')",
            params![win_dst, size],
        )
        .unwrap();
        let new_assets_path = root.join("state/library-new-assets.json");
        fs::write(&new_assets_path, r#"[1,42]"#).unwrap();

        let reserved = reserve_with_scanner_index(&config, &plan, &delta, true).unwrap();
        assert_eq!(reserved.len(), 1);
        assert_eq!(reserved[0]["id"], json!({"$integer": "1"}));

        let inspected = inspect(&config, &plan, &delta, false).unwrap();
        assert_eq!(inspected, reserved);
        assert!(cancel(&config, &plan, &delta, &inspected, false).is_err());

        let applied = apply_with_scanner_reconciliation(&config, &plan, &delta).unwrap();
        assert_eq!(applied["reconciled_scanner_assets"], json!([1]));
        assert_eq!(
            ledger::inspect_delta(&config, &plan, &delta).unwrap(),
            "committed"
        );
        // Original asset id=1 must NOT be removed from library-new-assets.json on in-place move.
        let new_assets: Value =
            serde_json::from_str(&fs::read_to_string(&new_assets_path).unwrap()).unwrap();
        assert_eq!(new_assets, json!([1, 42]));

        let _ = fs::remove_dir_all(root);
    }
}
