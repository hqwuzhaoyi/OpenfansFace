//! Rename-only port of person-file-metadata.ts. Semantic fields change; stable IDs/OCR do not.
use crate::{
    config::Config,
    db, files,
    model::{Error, Plan, Result, hash},
};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{fs, path::Path};

const DOCUMENTS: &[&str] = &[
    "people-deleted.json",
    "report.json",
    "claims-detail.json",
    "clusters.json",
    "integration-queue.json",
    "archive-history.json",
    "people-manual.json",
    "template-overrides.json",
    "watermarks/mappings.json",
    "person-rejections.json",
    "video-path-overrides.json",
    "person-renames.json",
    "external-profiles/manual-links.json",
    "external-profiles/reviewed-links.json",
];
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    pub name: String,
    pub before: Option<String>,
    pub after: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub documents: Vec<Document>,
    pub videos: Vec<Value>,
    pub jobs: Vec<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub faces_delta: Option<db::Delta>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jobs_delta: Option<db::Delta>,
}
pub fn win(config: &Config, path: &str) -> String {
    let prefix = format!("{}/pikpak/", config.media.display());
    match path.strip_prefix(&prefix) {
        Some(rest) => format!("R:\\media\\pikpak\\{}", rest.replace('/', "\\")),
        None => path.into(),
    }
}
pub fn mapped(config: &Config, value: &str, plan: &Plan) -> String {
    for file in &plan.files {
        if value == file.src {
            return file.dst.clone();
        }
        if value == win(config, &file.src) {
            return win(config, &file.dst);
        }
    }
    if plan.action == "rename-person" {
        return crate::person_rename::mapped(config, value, plan);
    }
    value.into()
}
pub fn transform(config: &Config, value: &Value, plan: &Plan) -> Value {
    fn visit(config: &Config, v: &Value, key: &str, parent: &Value, plan: &Plan) -> Value {
        match v {
            Value::String(s) => {
                if ["vid", "original_vid"].contains(&key) {
                    return v.clone();
                }
                if [
                    "path",
                    "src",
                    "dst",
                    "win_dst",
                    "win_src",
                    "original_dst",
                    "canonical_dst",
                    "collection",
                    "root",
                ]
                .contains(&key)
                {
                    return json!(mapped(config, s, plan));
                }
                if key == "name"
                    && let Some(file) = plan.files.iter().find(|file| {
                        parent["vid"]
                            .as_str()
                            .is_some_and(|id| file.vids.iter().any(|v| v == id))
                            && Some(s.as_str())
                                == Path::new(&file.src).file_name().and_then(|v| v.to_str())
                    })
                {
                    return json!(Path::new(&file.dst).file_name().unwrap().to_string_lossy());
                }
                v.clone()
            }
            Value::Array(items) => Value::Array(
                items
                    .iter()
                    .map(|item| visit(config, item, "", &Value::Null, plan))
                    .collect(),
            ),
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(k, v)| {
                        (
                            k.clone(),
                            visit(config, v, k, &Value::Object(map.clone()), plan),
                        )
                    })
                    .collect(),
            ),
            _ => v.clone(),
        }
    }
    visit(config, value, "", &Value::Null, plan)
}
fn updated(config: &Config, name: &str, value: &Value, plan: &Plan) -> Value {
    if name.starts_with("external-profiles/") {
        return value.clone();
    }
    let mut next = transform(config, value, plan);
    if name == "video-path-overrides.json" {
        next = Value::Object(
            value
                .as_object()
                .unwrap()
                .iter()
                .map(|(id, p)| {
                    (
                        id.clone(),
                        p.as_str()
                            .map(|s| json!(mapped(config, s, plan)))
                            .unwrap_or(p.clone()),
                    )
                })
                .collect(),
        );
        for file in &plan.files {
            for id in &file.vids {
                next[id] = json!(file.dst);
            }
        }
    }
    if name == "archive-history.json" {
        for row in next.as_array_mut().unwrap() {
            if row["vid"]
                .as_str()
                .is_some_and(|id| plan.files.iter().any(|f| f.vids.iter().any(|v| v == id)))
            {
                row["last_file_operation"] = json!(plan.id)
            }
        }
    }
    if name == "person-rejections.json" {
        next = Value::Object(
            next.as_object()
                .unwrap()
                .values()
                .map(|row| {
                    (
                        serde_json::to_string(&json!([row["name"], row["vid"]])).unwrap(),
                        row.clone(),
                    )
                })
                .collect(),
        );
    }
    next
}
fn is_array(name: &str) -> bool {
    [
        "claims-detail.json",
        "promoted.json",
        "clusters.json",
        "integration-queue.json",
        "archive-history.json",
        "people-manual.json",
    ]
    .contains(&name)
}
fn document(config: &Config, name: &str) -> Result<(Option<String>, Value)> {
    let path = config.base.join(name);
    if path
        .symlink_metadata()
        .is_ok_and(|s| s.file_type().is_symlink())
    {
        return Err(Error::conflict("元数据不能是符号链接").at(path.display().to_string()));
    }
    let raw = files::read_optional(&path)?;
    let value = raw
        .as_deref()
        .map(serde_json::from_str)
        .transpose()?
        .unwrap_or_else(|| if is_array(name) { json!([]) } else { json!({}) });
    if (is_array(name) && !value.is_array()) || (!is_array(name) && !value.is_object()) {
        return Err(
            Error::new(503, "metadata_contract", "元数据格式不符").at(path.display().to_string())
        );
    }
    Ok((raw, value))
}
pub fn registered_ids(config: &Config, src: &str) -> Result<Vec<String>> {
    let db = db::open(&config.base.join("faces.db"))?;
    let (_, history) = document(config, "archive-history.json")?;
    let (_, overrides) = document(config, "video-path-overrides.json")?;
    fn disk(config: &Config, path: &str) -> Option<String> {
        let normal = path.replace('\\', "/");
        if normal.split('/').any(|v| v == "..") {
            return None;
        }
        if normal.starts_with(&format!("{}/", config.media.display())) {
            return Some(normal);
        }
        normal
            .split_once("/media/pikpak/")
            .map(|(_, rest)| format!("{}/pikpak/{rest}", config.media.display()))
    }
    let mut ids = vec![];
    for row in db::rows(&db, "SELECT vid,path FROM videos ORDER BY vid", [])? {
        let id = row["vid"]
            .as_str()
            .ok_or_else(|| Error::conflict("视频记录缺少稳定编号"))?;
        let archived = history
            .as_array()
            .unwrap()
            .iter()
            .rev()
            .find(|r| r["vid"] == id)
            .and_then(|r| r["win_dst"].as_str().or(r["dst"].as_str()));
        // Explicit current locations outrank historical canonical archive targets.
        let stored = overrides[id].as_str().or(archived).or(row["path"].as_str());
        let candidates: Vec<String> = stored
            .into_iter()
            .chain(std::iter::once(id))
            .filter_map(|p| disk(config, p))
            .collect();
        let actual = candidates
            .iter()
            .find(|p| Path::new(p).is_file())
            .or(candidates.first());
        if actual.is_some_and(|p| p == src) {
            ids.push(id.to_owned())
        }
    }
    Ok(ids)
}
fn affected(config: &Config, row: &Value, plan: &Plan) -> bool {
    row["vid"]
        .as_str()
        .is_some_and(|id| plan.files.iter().any(|f| f.vids.iter().any(|v| v == id)))
        || row["path"]
            .as_str()
            .is_some_and(|s| mapped(config, s, plan) != s)
}
pub fn snapshot(config: &Config, plan: &Plan) -> Result<Snapshot> {
    let mut documents = vec![];
    for name in DOCUMENTS {
        let (before, value) = document(config, name)?;
        let next = updated(config, name, &value, plan);
        let after = if next == value {
            before.clone()
        } else {
            Some(serde_json::to_string_pretty(&next)?)
        };
        documents.push(Document {
            name: (*name).into(),
            before,
            after,
        });
    }
    let videos = db::rows(
        &db::open(&config.base.join("faces.db"))?,
        "SELECT * FROM videos ORDER BY vid",
        [],
    )?
    .into_iter()
    .filter(|row| affected(config, row, plan))
    .collect();
    let jobs_path = config.base.join("processing/processing.db");
    let jobs = if jobs_path.exists() {
        db::rows(
            &db::open(&jobs_path)?,
            "SELECT * FROM tasks ORDER BY vid",
            [],
        )?
        .into_iter()
        .filter(|row| affected(config, row, plan))
        .collect()
    } else {
        vec![]
    };
    Ok(Snapshot {
        documents,
        videos,
        jobs,
        faces_delta: None,
        jobs_delta: None,
    })
}
pub fn version(snapshot: &Snapshot) -> String {
    hash(snapshot)
}
pub fn apply_faces(config: &Config, plan: &Plan, before: &Snapshot) -> Result<Value> {
    if let Some(delta) = &before.faces_delta {
        return apply_delta_domain(config, plan, delta, false);
    }
    let mut db = db::open(&config.base.join("faces.db"))?;
    let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let rows = db::rows(&tx, "SELECT * FROM videos ORDER BY vid", [])?
        .into_iter()
        .filter(|r| affected(config, r, plan))
        .collect::<Vec<_>>();
    if rows != before.videos {
        return Err(Error::conflict("视频索引已变化"));
    }
    db::receipt_table(&tx)?;
    let columns = db::columns(&tx, "videos")?;
    for row in rows {
        let id = row["vid"].as_str().unwrap();
        let next = if let Some(file) = plan.files.iter().find(|f| f.vids.iter().any(|v| v == id)) {
            file.dst.clone()
        } else {
            mapped(config, row["path"].as_str().unwrap(), plan)
        };
        tx.execute("UPDATE videos SET path=? WHERE vid=?", params![next, id])?;
        if columns.iter().any(|c| c == "name") {
            tx.execute(
                "UPDATE videos SET name=? WHERE vid=?",
                params![Path::new(&next).file_name().unwrap().to_string_lossy(), id],
            )?;
        }
        if columns.iter().any(|c| c == "collection") {
            tx.execute(
                "UPDATE videos SET collection=? WHERE vid=?",
                params![Path::new(&next).parent().unwrap().to_string_lossy(), id],
            )?;
        }
    }
    let after = db::rows(&tx, "SELECT * FROM videos ORDER BY vid", [])?
        .into_iter()
        .filter(|row| {
            row["vid"]
                .as_str()
                .is_some_and(|id| before.videos.iter().any(|r| r["vid"] == id))
        })
        .collect::<Vec<_>>();
    let evidence = json!({"before":before.videos,"after":after});
    tx.execute(
        "INSERT INTO _face_admin_rust_commits VALUES(?,?,?)",
        params![plan.id, plan.version, serde_json::to_string(&evidence)?],
    )?;
    tx.commit()?;
    Ok(evidence)
}
pub fn apply_jobs(config: &Config, plan: &Plan, before: &Snapshot) -> Result<Value> {
    if let Some(delta) = &before.jobs_delta {
        return apply_delta_domain(config, plan, delta, true);
    }
    let path = config.base.join("processing/processing.db");
    if !path.exists() {
        if !before.jobs.is_empty() {
            return Err(Error::conflict("处理任务数据库消失"));
        }
        return Ok(json!({"skipped":true}));
    }
    let mut db = db::open(&path)?;
    let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let rows = db::rows(&tx, "SELECT * FROM tasks ORDER BY vid", [])?
        .into_iter()
        .filter(|r| affected(config, r, plan))
        .collect::<Vec<_>>();
    if rows != before.jobs {
        return Err(Error::conflict("相关处理任务已变化"));
    }
    db::receipt_table(&tx)?;
    for row in &rows {
        let file = plan
            .files
            .iter()
            .find(|file| {
                row["vid"]
                    .as_str()
                    .is_some_and(|id| file.vids.iter().any(|v| v == id))
                    || row["path"] == file.src
                    || row["path"] == win(config, &file.src)
            })
            .ok_or_else(|| Error::conflict("处理任务缺少对应物理文件"))?;
        let stat = fs::metadata(&file.dst)?;
        let data: Value = serde_json::from_str(
            row["data"]
                .as_str()
                .ok_or_else(|| Error::conflict("任务数据无效"))?,
        )?;
        let size = i64::try_from(stat.len())
            .map_err(|_| Error::new(409, "invalid_request", "文件大小超出 SQLite 范围"))?;
        tx.execute("UPDATE tasks SET path=?,name=?,size=?,mtime_ns=?,data=?,stage='excluded',updated_at=unixepoch('subsec') WHERE vid=?",params![file.dst,Path::new(&file.dst).file_name().unwrap().to_string_lossy(),size,files::mtime_ns(&stat),serde_json::to_string(&transform(config,&data,plan))?,row["vid"].as_str().unwrap()])?;
    }
    let after = db::rows(&tx, "SELECT * FROM tasks ORDER BY vid", [])?
        .into_iter()
        .filter(|row| {
            row["vid"]
                .as_str()
                .is_some_and(|id| before.jobs.iter().any(|r| r["vid"] == id))
        })
        .collect::<Vec<_>>();
    let evidence = json!({"before":rows,"after":after});
    tx.execute(
        "INSERT INTO _face_admin_rust_commits VALUES(?,?,?)",
        params![plan.id, plan.version, serde_json::to_string(&evidence)?],
    )?;
    tx.commit()?;
    Ok(evidence)
}
pub fn apply_document(config: &Config, item: &Document) -> Result<()> {
    let path = config.base.join(&item.name);
    if files::read_optional(&path)? != item.before {
        return Err(Error::conflict("后台已更新记录，需核对恢复").at(path.display().to_string()));
    }
    if let Some(raw) = &item.after {
        fs::create_dir_all(path.parent().unwrap())?;
        files::atomic_write(&path, raw.as_bytes())?;
    } else if item.before.is_some() {
        fs::remove_file(&path)?;
        files::sync_parent(&path)?;
    }
    Ok(())
}

/// Native receipts are transaction-local evidence, never merely an engine phase label.
/// Return true only when this domain is committed and its exact after-image still exists.
pub fn inspect_domain(config: &Config, plan: &Plan, before: &Snapshot, jobs: bool) -> Result<bool> {
    if let Some(delta) = if jobs {
        &before.jobs_delta
    } else {
        &before.faces_delta
    } {
        return inspect_delta_domain(config, plan, delta, jobs);
    }
    let path = config.base.join(if jobs {
        "processing/processing.db"
    } else {
        "faces.db"
    });
    let expected = if jobs { &before.jobs } else { &before.videos };
    if !path.exists() && jobs && expected.is_empty() {
        return Ok(false);
    }
    let db = db::open(&path)?;
    inspect_rows(
        &db,
        config,
        plan,
        expected,
        if jobs { "tasks" } else { "videos" },
    )
}
fn inspect_rows(
    db: &rusqlite::Connection,
    config: &Config,
    plan: &Plan,
    before: &[Value],
    table: &str,
) -> Result<bool> {
    let receipt = db::receipt(db, &plan.id, &plan.version)?;
    let current = db::rows(db, &format!("SELECT * FROM {table} ORDER BY vid"), [])?
        .into_iter()
        .filter(|row| affected(config, row, plan) || before.iter().any(|r| r["vid"] == row["vid"]))
        .collect::<Vec<_>>();
    let expected = match &receipt {
        Some(r) => {
            if r["before"] != json!(before) {
                return Err(Error::conflict("本地事务原始证据不匹配"));
            }
            if r["state"] == "rolled_back" {
                &r["before"]
            } else {
                &r["after"]
            }
        }
        None => &json!(before),
    };
    if current
        != *expected
            .as_array()
            .ok_or_else(|| Error::conflict("本地回执格式无效"))?
    {
        return Err(Error::conflict(format!(
            "{table} 已被外部修改，不能自动恢复"
        )));
    }
    Ok(receipt.is_some_and(|r| r["state"] != "rolled_back"))
}
pub fn inspect_documents(config: &Config, before: &Snapshot) -> Result<()> {
    for item in &before.documents {
        let (raw, _) = document(config, &item.name)?;
        if raw != item.before && raw != item.after {
            return Err(Error::conflict("JSON 记录被外部修改，不能恢复").at(config
                .base
                .join(&item.name)
                .display()
                .to_string()));
        }
    }
    Ok(())
}
pub fn compensate_domain(
    config: &Config,
    plan: &Plan,
    before: &Snapshot,
    jobs: bool,
) -> Result<()> {
    if let Some(delta) = if jobs {
        &before.jobs_delta
    } else {
        &before.faces_delta
    } {
        return compensate_delta_domain(config, plan, delta, jobs);
    }
    let path = config.base.join(if jobs {
        "processing/processing.db"
    } else {
        "faces.db"
    });
    let original = if jobs { &before.jobs } else { &before.videos };
    if !path.exists() && jobs && original.is_empty() {
        return Ok(());
    }
    let table = if jobs { "tasks" } else { "videos" };
    let mut db = db::open(&path)?;
    let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    if !inspect_rows(&tx, config, plan, original, table)? {
        return Ok(());
    }
    let mut receipt = db::receipt(&tx, &plan.id, &plan.version)?.unwrap();
    for row in original {
        let fields = row
            .as_object()
            .unwrap()
            .keys()
            .filter(|k| k.as_str() != "vid")
            .collect::<Vec<_>>();
        let assignments = fields
            .iter()
            .map(|k| format!("\"{}\"=?", k.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(",");
        let mut args = fields
            .iter()
            .map(|k| db::sql_value(&row[*k]))
            .collect::<Result<Vec<_>>>()?;
        args.push(db::sql_value(&row["vid"])?);
        tx.execute(
            &format!("UPDATE {table} SET {assignments} WHERE vid=?"),
            rusqlite::params_from_iter(args),
        )?;
    }
    receipt["state"] = json!("rolled_back");
    tx.execute(
        "UPDATE _face_admin_rust_commits SET data=? WHERE id=? AND signature=?",
        params![serde_json::to_string(&receipt)?, plan.id, plan.version],
    )?;
    tx.commit()?;
    Ok(())
}
pub fn compensate_document(config: &Config, item: &Document) -> Result<()> {
    let path = config.base.join(&item.name);
    let (raw, _) = document(config, &item.name)?;
    if raw == item.before {
        return Ok(());
    }
    if raw != item.after {
        return Err(Error::conflict("JSON 补偿遇到外部修改").at(path.display().to_string()));
    }
    if let Some(raw) = &item.before {
        files::atomic_write(&path, raw.as_bytes())?;
    } else {
        fs::remove_file(&path)?;
        files::sync_parent(&path)?;
    }
    Ok(())
}

fn domain_path(config: &Config, jobs: bool) -> std::path::PathBuf {
    config.base.join(if jobs {
        "processing/processing.db"
    } else {
        "faces.db"
    })
}
fn validate_delta_domain(delta: &db::Delta, jobs: bool) -> Result<()> {
    if delta.tables.iter().any(|t| {
        if jobs {
            !matches!(t.table, db::Table::Tasks | db::Table::Settings)
        } else {
            t.table != db::Table::Videos
        }
    }) {
        return Err(Error::conflict("本地事务包含错误的存储表"));
    }
    let expected = if jobs {
        db::Table::Tasks
    } else {
        db::Table::Videos
    };
    if delta
        .guards
        .iter()
        .any(|guard| !matches!(guard, db::Guard::LocalFiles { table, .. } | db::Guard::LocalIdentity { table, .. } if *table == expected))
    {
        return Err(Error::conflict("本地事务包含其他存储域的查询保护"));
    }
    Ok(())
}
fn apply_delta_domain(
    config: &Config,
    plan: &Plan,
    delta: &db::Delta,
    jobs: bool,
) -> Result<Value> {
    validate_delta_domain(delta, jobs)?;
    let mut conn = db::open(&domain_path(config, jobs))?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    db::receipt_table(&tx)?;
    if db::receipt(&tx, &plan.id, &plan.version)?.is_some() {
        return Err(Error::conflict("本地事务已有回执"));
    }
    crate::delta::apply(&tx, delta)?;
    let evidence = json!({"delta":delta});
    tx.execute(
        "INSERT INTO _face_admin_rust_commits VALUES(?,?,?)",
        params![plan.id, plan.version, serde_json::to_string(&evidence)?],
    )?;
    tx.commit()?;
    Ok(evidence)
}
fn inspect_delta_domain(
    config: &Config,
    plan: &Plan,
    delta: &db::Delta,
    jobs: bool,
) -> Result<bool> {
    validate_delta_domain(delta, jobs)?;
    let mut conn = db::open(&domain_path(config, jobs))?;
    let tx = conn.transaction()?;
    let receipt = db::receipt(&tx, &plan.id, &plan.version)?;
    let after = if let Some(receipt) = receipt {
        if receipt["delta"] != serde_json::to_value(delta)? {
            return Err(Error::conflict("本地事务快照不匹配"));
        }
        if !receipt["state"].is_null() && receipt["state"] != "rolled_back" {
            return Err(Error::conflict("本地事务状态未知"));
        }
        receipt["state"] != "rolled_back"
    } else {
        false
    };
    crate::delta::inspect(&tx, delta, after)?;
    Ok(after)
}
fn compensate_delta_domain(
    config: &Config,
    plan: &Plan,
    delta: &db::Delta,
    jobs: bool,
) -> Result<()> {
    if !inspect_delta_domain(config, plan, delta, jobs)? {
        return Ok(());
    }
    let mut conn = db::open(&domain_path(config, jobs))?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let mut receipt = db::receipt(&tx, &plan.id, &plan.version)?
        .ok_or_else(|| Error::conflict("缺少本地事务回执"))?;
    if receipt["delta"] != serde_json::to_value(delta)? || receipt["state"] == "rolled_back" {
        return Err(Error::conflict("本地回执发生变化"));
    }
    crate::delta::restore(&tx, delta)?;
    receipt["state"] = json!("rolled_back");
    tx.execute(
        "UPDATE _face_admin_rust_commits SET data=? WHERE id=?",
        params![serde_json::to_string(&receipt)?, plan.id],
    )?;
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod guard_domain_tests {
    use super::*;
    fn local(table: db::Table) -> db::Guard {
        db::Guard::LocalFiles {
            table,
            vids: vec![],
            paths: vec![],
            before: vec![],
            after: vec![],
        }
    }
    #[test]
    fn local_transactions_reject_guards_for_other_stores() {
        for jobs in [false, true] {
            let expected = if jobs {
                db::Table::Tasks
            } else {
                db::Table::Videos
            };
            let other = if jobs {
                db::Table::Videos
            } else {
                db::Table::Tasks
            };
            let mut delta = db::Delta {
                tables: vec![],
                guards: vec![local(expected)],
            };
            validate_delta_domain(&delta, jobs).unwrap();
            for guard in [
                local(other),
                local(db::Table::Settings),
                db::Guard::AssetPath {
                    path: "a".into(),
                    before: vec![],
                    after: vec![],
                },
                db::Guard::CreatorName {
                    canonical: "a".into(),
                    normalized: "a".into(),
                    before: vec![],
                    after: vec![],
                },
                db::Guard::CreatorCanonical {
                    canonical: "a".into(),
                    before: vec![],
                    after: vec![],
                },
                db::Guard::AssetLinks {
                    asset_id: json!({"$integer":"1"}),
                    before: vec![],
                    after: vec![],
                },
            ] {
                delta.guards = vec![guard];
                assert!(validate_delta_domain(&delta, jobs).is_err());
            }
        }
    }
}
