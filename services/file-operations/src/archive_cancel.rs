//! Cancellation-only proof for a prepared archive interrupted by late index rows.
//! Never authorizes ledger commit or changes/deletes any external asset/reference.
use crate::{
    config::Config,
    creator_delta, db, ledger, metadata,
    model::{Error, Plan, Result},
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde_json::{Value, json};
use std::{collections::BTreeSet, path::Path};

fn conflict() -> Error {
    Error::conflict("归档账本取消依据不足，保留恢复状态")
}
fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn index_only(
    conn: &Connection,
    row: &Value,
    original: &Value,
    file: &crate::model::FileMove,
) -> Result<()> {
    let known: BTreeSet<&str> = "id location path name medium size mtime hash_kind hash creator studio series code duration width height vcodec fps has_audio ctx_length ctx_orient ctx_quality ctx_pace ctx_people play_count last_played rating o_count watch_ratio source_id stash_scene_id snapshot_path first_seen last_seen feedback disposal leave_ratio play_seconds feedback_at seek_count max_reached release_date catalog_title original_title field_owners mutation_revision region disposal_reason".split_whitespace().collect();
    let index: BTreeSet<&str> = "id location path name medium size mtime hash_kind hash duration width height vcodec fps has_audio ctx_length ctx_orient ctx_quality first_seen last_seen".split_whitespace().collect();
    let id = db::integer(&row["id"])?;
    if id <= 0
        || id == db::integer(&original["id"])?
        || row["location"] != original["location"]
        || row["name"].as_str() != Path::new(&file.dst).file_name().and_then(|s| s.to_str())
        || row["medium"] != "video"
        || db::integer(&row["size"])? != file.size as i64
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
        "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
        [],
    )?;
    if tables.len() > 512 {
        return Err(conflict());
    }
    for table in tables {
        let name = table["name"].as_str().ok_or_else(conflict)?;
        if [
            "asset_search",
            "asset_search_data",
            "asset_search_idx",
            "asset_search_content",
            "asset_search_docsize",
            "asset_search_config",
        ]
        .contains(&name)
        {
            continue;
        }
        let columns = db::rows(conn, &format!("PRAGMA table_info({})", quoted(name)), [])?;
        if columns.len() > 256 {
            return Err(conflict());
        }
        for column in columns {
            let col = column["name"].as_str().ok_or_else(conflict)?;
            if col == "asset_id" || col.ends_with("_asset_id") {
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
            } else if col.contains("asset_ids") {
                // An unfamiliar plural/JSON reference format is not absence proof.
                return Err(conflict());
            }
        }
    }
    Ok(())
}

fn proof(
    conn: &Connection,
    config: &Config,
    plan: &Plan,
    delta: &ledger::Delta,
) -> Result<(String, Value, Vec<Value>)> {
    if plan.action != "archive" {
        return Err(conflict());
    }
    let receipt: Option<(String, String, String)> = conn
        .query_row(
            "SELECT signature,state,data FROM _face_admin_person_ops WHERE id=?",
            [&plan.id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let (signature, state, raw) = receipt.ok_or_else(conflict)?;
    let evidence: Value = serde_json::from_str(&raw)?;
    if signature != plan.version
        || !["prepared", "cancelled"].contains(&state.as_str())
        || evidence["backend"] != "rust"
        || evidence["kind"] != "archive"
        || evidence["delta"] != serde_json::to_value(delta)?
    {
        return Err(conflict());
    }
    let assets = delta
        .changes
        .tables
        .iter()
        .find(|t| t.table == db::Table::Asset)
        .ok_or_else(conflict)?;
    let mut observed = delta.changes.clone();
    let mut retained = vec![];
    for guard in &mut observed.guards {
        let db::Guard::AssetPath {
            path,
            before,
            after,
        } = guard
        else {
            continue;
        };
        // Only empty accepted destinations of this archive's original-ID update.
        // Canonical duplicate destinations and non-archive scopes are untouched.
        if !before.is_empty() || after.len() != 1 {
            continue;
        }
        let Some(file) = plan.files.iter().find(|f| {
            Path::new(&f.dst).starts_with(config.creators())
                && metadata::win(config, &f.dst) == *path
        }) else {
            continue;
        };
        let original = assets
            .before
            .iter()
            .find(|row| row["id"] == after[0]["id"])
            .ok_or_else(conflict)?;
        let current = db::rows(
            conn,
            "SELECT * FROM asset WHERE path=? ORDER BY id LIMIT 2",
            [path.as_str()],
        )?;
        if current.is_empty() {
            continue;
        }
        if current.len() != 1 {
            return Err(conflict());
        }
        if original["path"] != metadata::win(config, &file.src) {
            return Err(conflict());
        }
        index_only(conn, &current[0], original, file)?;
        retained.extend(current.iter().cloned());
        // A transaction-local observation only. The accepted delta/receipt is
        // unchanged; this adjusted guard can prove cancellation, never commit.
        *before = current;
    }
    if retained.is_empty() {
        return Err(conflict());
    }
    creator_delta::inspect_before(conn, &observed, &delta.creators)?;
    if let Some(previous) = evidence.get("archive_cancel_indexes")
        && previous != &json!(retained)
    {
        return Err(conflict());
    }
    Ok((state, evidence, retained))
}

pub fn inspect(config: &Config, plan: &Plan, delta: &ledger::Delta) -> Result<Vec<Value>> {
    let mut conn = db::open(&config.ledger)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    Ok(proof(&tx, config, plan, delta)?.2)
}

pub fn cancel(
    config: &Config,
    plan: &Plan,
    delta: &ledger::Delta,
    expected: &[Value],
) -> Result<()> {
    let mut conn = db::open(&config.ledger)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let (state, mut evidence, rows) = proof(&tx, config, plan, delta)?;
    if rows != expected {
        return Err(conflict());
    }
    evidence["archive_cancel_indexes"] = json!(rows);
    if tx.execute("UPDATE _face_admin_person_ops SET state='cancelled',data=? WHERE id=? AND signature=? AND state=?",params![serde_json::to_string(&evidence)?,plan.id,plan.version,state])? != 1 { return Err(conflict()); }
    tx.commit()?;
    Ok(())
}
