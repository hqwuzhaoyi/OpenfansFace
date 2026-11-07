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
#[derive(Clone, Serialize, Deserialize)]
pub struct Document {
    pub name: String,
    pub before: Option<String>,
    pub after: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub documents: Vec<Document>,
    pub videos: Vec<Value>,
    pub jobs: Vec<Value>,
}
pub fn win(config: &Config, path: &str) -> String {
    let prefix = format!("{}/pikpak/", config.media.display());
    match path.strip_prefix(&prefix) {
        Some(rest) => format!("R:\\media\\pikpak\\{}", rest.replace('/', "\\")),
        None => path.into(),
    }
}
pub fn mapped(config: &Config, value: &str, plan: &Plan) -> String {
    let file = plan.file();
    if value == file.src {
        file.dst.clone()
    } else if value == win(config, &file.src) {
        win(config, &file.dst)
    } else {
        value.into()
    }
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
                    && parent["vid"]
                        .as_str()
                        .is_some_and(|id| plan.file().vids.iter().any(|v| v == id))
                    && Some(s.as_str())
                        == Path::new(&plan.file().src)
                            .file_name()
                            .and_then(|v| v.to_str())
                {
                    return json!(
                        Path::new(&plan.file().dst)
                            .file_name()
                            .unwrap()
                            .to_string_lossy()
                    );
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
        for id in &plan.file().vids {
            next[id] = json!(plan.file().dst)
        }
    }
    if name == "archive-history.json" {
        for row in next.as_array_mut().unwrap() {
            if row["vid"]
                .as_str()
                .is_some_and(|id| plan.file().vids.iter().any(|v| v == id))
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
        let stored = archived.or(overrides[id].as_str()).or(row["path"].as_str());
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
        .is_some_and(|id| plan.file().vids.iter().any(|v| v == id))
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
    })
}
pub fn version(snapshot: &Snapshot) -> String {
    hash(snapshot)
}
pub fn apply_faces(config: &Config, plan: &Plan, before: &Snapshot) -> Result<Value> {
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
        let next = if plan.file().vids.iter().any(|v| v == id) {
            plan.file().dst.clone()
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
    let stat = fs::metadata(&plan.file().dst)?;
    for row in &rows {
        let data: Value = serde_json::from_str(
            row["data"]
                .as_str()
                .ok_or_else(|| Error::conflict("任务数据无效"))?,
        )?;
        let size = i64::try_from(stat.len())
            .map_err(|_| Error::new(409, "invalid_request", "文件大小超出 SQLite 范围"))?;
        tx.execute("UPDATE tasks SET path=?,name=?,size=?,mtime_ns=?,data=?,stage='excluded',updated_at=unixepoch('subsec') WHERE vid=?",params![plan.file().dst,Path::new(&plan.file().dst).file_name().unwrap().to_string_lossy(),size,files::mtime_ns(&stat),serde_json::to_string(&transform(config,&data,plan))?,row["vid"].as_str().unwrap()])?;
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
    }
    Ok(())
}
