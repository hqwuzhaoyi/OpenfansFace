//! Native processing scheduler: task discovery, lease lifecycle, timeout/retry, concurrency and operator control.
use crate::{
    config::Config,
    db,
    engine::Engine,
    files,
    model::{Error, Result},
    review,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

const SCHEMA: &str = "
PRAGMA journal_mode=WAL;
CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS tasks (
 vid TEXT PRIMARY KEY, path TEXT NOT NULL UNIQUE, name TEXT NOT NULL,
 size INTEGER NOT NULL, mtime_ns TEXT NOT NULL, duration REAL,
 stage TEXT NOT NULL DEFAULT 'queued', retry_stage TEXT NOT NULL DEFAULT 'text',
 text_attempts INTEGER NOT NULL DEFAULT 0, face_attempts INTEGER NOT NULL DEFAULT 0,
 next_retry REAL NOT NULL DEFAULT 0, priority INTEGER NOT NULL DEFAULT 0,
 is_new INTEGER NOT NULL DEFAULT 0, batch_id TEXT NOT NULL,
 created_at REAL NOT NULL, updated_at REAL NOT NULL,
 error TEXT, manual INTEGER NOT NULL DEFAULT 0, deferred INTEGER NOT NULL DEFAULT 0,
 note TEXT NOT NULL DEFAULT '', data TEXT NOT NULL DEFAULT '{}', run_id TEXT NOT NULL DEFAULT '',
 lease_expires_at REAL NOT NULL DEFAULT 0, coordinator_generation TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS processing_stage ON tasks(stage, priority, is_new, created_at);
CREATE TABLE IF NOT EXISTS events (
 id INTEGER PRIMARY KEY AUTOINCREMENT, at REAL NOT NULL, vid TEXT, kind TEXT NOT NULL, message TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS face_groups (id TEXT PRIMARY KEY, version TEXT NOT NULL, embedding BLOB NOT NULL);
CREATE TABLE IF NOT EXISTS batch_items (
 batch_id TEXT NOT NULL, vid TEXT NOT NULL, name TEXT NOT NULL, path TEXT NOT NULL,
 size INTEGER NOT NULL, mtime_ns TEXT NOT NULL, added_at REAL NOT NULL,
 text_done INTEGER NOT NULL DEFAULT 0, face_required INTEGER, face_done INTEGER NOT NULL DEFAULT 0,
 face_skipped INTEGER NOT NULL DEFAULT 0, completed INTEGER NOT NULL DEFAULT 0,
 completed_at REAL, excluded INTEGER NOT NULL DEFAULT 0, legacy INTEGER NOT NULL DEFAULT 0,
 PRIMARY KEY(batch_id,vid)
);
CREATE TABLE IF NOT EXISTS processing_runs (
 id TEXT PRIMARY KEY, vid TEXT NOT NULL, name TEXT NOT NULL, batch_id TEXT NOT NULL,
 stage TEXT NOT NULL, started_at REAL NOT NULL, finished_at REAL, status TEXT NOT NULL,
 error TEXT, execution TEXT
);
CREATE INDEX IF NOT EXISTS processing_run_time ON processing_runs(started_at DESC);
";

const DEFAULT_LEASE_SECONDS: f64 = 660.0;
const DEFAULT_PIPELINE: &str = "watermark-v1";
const DEFAULT_FACE_VERSION: &str = "buffalo_l-det-rec-8frames-v1";

fn bad(message: impl Into<String>) -> Error {
    Error::new(400, "invalid_request", message)
}

fn conflict(message: impl Into<String>) -> Error {
    Error::new(409, "conflict", message)
}

fn unavailable(message: impl Into<String>) -> Error {
    Error::new(503, "processing_unavailable", message)
}

pub fn db_path(config: &Config) -> PathBuf {
    config.base.join("processing/processing.db")
}

fn ensure_columns(conn: &Connection) -> Result<()> {
    let task_cols = db::columns(conn, "tasks")?;
    if !task_cols.is_empty() {
        if !task_cols.iter().any(|c| c == "run_id") {
            conn.execute(
                "ALTER TABLE tasks ADD COLUMN run_id TEXT NOT NULL DEFAULT ''",
                [],
            )?;
        }
        if !task_cols.iter().any(|c| c == "lease_expires_at") {
            conn.execute(
                "ALTER TABLE tasks ADD COLUMN lease_expires_at REAL NOT NULL DEFAULT 0",
                [],
            )?;
        }
        if !task_cols.iter().any(|c| c == "coordinator_generation") {
            conn.execute(
                "ALTER TABLE tasks ADD COLUMN coordinator_generation TEXT NOT NULL DEFAULT ''",
                [],
            )?;
        }
    }
    let run_cols = db::columns(conn, "processing_runs")?;
    if !run_cols.is_empty() && !run_cols.iter().any(|c| c == "execution") {
        conn.execute("ALTER TABLE processing_runs ADD COLUMN execution TEXT", [])?;
    }
    Ok(())
}

pub fn open_or_init(config: &Config) -> Result<Connection> {
    let path = db_path(config);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if !path.exists() {
        for suffix in ["-wal", "-shm"] {
            let sidecar = PathBuf::from(format!("{}{suffix}", path.display()));
            let _ = fs::remove_file(sidecar);
        }
    }
    let conn = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(Duration::from_secs(15))?;
    conn.execute_batch("PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL;")?;
    conn.execute_batch(SCHEMA)?;
    ensure_columns(&conn)?;
    conn.execute(
        "INSERT OR IGNORE INTO settings VALUES ('paused','false')",
        [],
    )?;
    conn.execute(
        "INSERT OR IGNORE INTO settings VALUES ('concurrency','1')",
        [],
    )?;
    let version = setting(&conn, "batch_ledger_version")?
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    if version == 0 {
        for row in db::rows(&conn, "SELECT * FROM tasks", [])? {
            ensure_member(&conn, &row, true)?;
        }
        set_setting(&conn, "batch_ledger_version", &json!(1))?;
    }
    Ok(conn)
}

pub fn open_existing(config: &Config, missing_message: &str) -> Result<Connection> {
    let path = db_path(config);
    if !path.exists() {
        return Err(unavailable(missing_message));
    }
    let conn = db::open(&path)?;
    conn.busy_timeout(Duration::from_secs(15))?;
    ensure_columns(&conn)?;
    Ok(conn)
}

fn now_sec(conn: &Connection) -> Result<f64> {
    Ok(conn.query_row("SELECT unixepoch('now','subsec')", [], |r| r.get(0))?)
}

fn plain_row(row: Value) -> Value {
    match row {
        Value::Object(map) => {
            if map.len() == 1
                && let Some(n) = map
                    .get("$integer")
                    .and_then(Value::as_str)
                    .and_then(|s| s.parse::<i64>().ok())
            {
                return json!(n);
            }
            Value::Object(map.into_iter().map(|(k, v)| (k, plain_row(v))).collect())
        }
        Value::Array(arr) => Value::Array(arr.into_iter().map(plain_row).collect()),
        other => other,
    }
}

pub fn setting(conn: &Connection, key: &str) -> Result<Option<Value>> {
    let raw: Option<String> = conn
        .query_row("SELECT value FROM settings WHERE key=?", [key], |r| {
            r.get(0)
        })
        .optional()?;
    raw.map(|s| serde_json::from_str(&s).map_err(Into::into))
        .transpose()
}

pub fn set_setting(conn: &Connection, key: &str, value: &Value) -> Result<()> {
    conn.execute(
        "INSERT INTO settings(key,value) VALUES(?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![key, serde_json::to_string(value)?],
    )?;
    Ok(())
}

pub fn event(
    conn: &Connection,
    at: f64,
    vid: Option<&str>,
    kind: &str,
    message: &str,
) -> Result<()> {
    let truncated: String = message.chars().take(500).collect();
    conn.execute(
        "INSERT INTO events(at,vid,kind,message) VALUES(?,?,?,?)",
        params![at, vid, kind, truncated],
    )?;
    Ok(())
}

pub fn effective_concurrency(conn: &Connection, capacity: i64) -> Result<i64> {
    let configured = setting(conn, "concurrency")?
        .and_then(|v| v.as_i64())
        .unwrap_or(1)
        .max(1);
    Ok(capacity.max(1).min(configured))
}

fn ensure_member(conn: &Connection, task: &Value, legacy: bool) -> Result<()> {
    let data = task["data"]
        .as_str()
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .unwrap_or_else(|| json!({}));
    let text_done = data["text_complete"].as_bool().unwrap_or(false);
    let skipped = data["face_skipped"].as_bool().unwrap_or(false);
    let face_required: Option<i64> = if text_done {
        Some(i64::from(!skipped))
    } else {
        None
    };
    let face_done = i64::from(data["face_complete"].as_bool().unwrap_or(false));
    let completed = i64::from(data["ready"].as_bool().unwrap_or(false));
    let excluded = i64::from(task["stage"].as_str() == Some("excluded"));
    let batch_id = task["batch_id"].as_str().unwrap_or("");
    let vid = task["vid"].as_str().unwrap_or("");
    let name = task["name"].as_str().unwrap_or("");
    let path = task["path"].as_str().unwrap_or("");
    let size = review::number(&task["size"]).unwrap_or(0);
    let mtime_ns = task["mtime_ns"].as_str().unwrap_or("");
    let added_at = task["created_at"]
        .as_f64()
        .or_else(|| review::number(&task["created_at"]).map(|n| n as f64))
        .unwrap_or(0.0);
    conn.execute(
        "INSERT OR IGNORE INTO batch_items
        (batch_id,vid,name,path,size,mtime_ns,added_at,text_done,face_required,face_done,face_skipped,completed,excluded,legacy)
        VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
        params![
            batch_id,
            vid,
            name,
            path,
            size,
            mtime_ns,
            added_at,
            i64::from(text_done),
            face_required,
            face_done,
            i64::from(skipped),
            completed,
            excluded,
            i64::from(legacy),
        ],
    )?;
    conn.execute(
        "UPDATE batch_items SET excluded=? WHERE batch_id=? AND vid=? AND completed=0",
        params![excluded, batch_id, vid],
    )?;
    Ok(())
}

pub fn reconcile(
    conn: &Connection,
    items: &[Value],
    batch_id: &str,
    bootstrapped: bool,
) -> Result<usize> {
    let existing = db::rows(conn, "SELECT * FROM tasks", [])?;
    let mut by_path = BTreeMap::new();
    let mut by_vid = BTreeMap::new();
    for row in existing {
        if let Some(p) = row["path"].as_str() {
            by_path.insert(p.to_owned(), row.clone());
        }
        if let Some(v) = row["vid"].as_str() {
            by_vid.insert(v.to_owned(), row);
        }
    }
    let mut active = BTreeSet::new();
    for item in items {
        let item_vid = item["vid"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| bad("发现条目缺少 vid"))?;
        let item_path = item["path"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| bad("发现条目缺少 path"))?;
        let item_name = item["name"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| bad("发现条目缺少 name"))?;
        let item_size =
            review::number(&item["size"]).ok_or_else(|| bad("发现条目缺少有效 size"))?;
        let item_mtime_ns = item["mtime_ns"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| bad("发现条目缺少 mtime_ns"))?;
        let item_duration = item.get("duration").and_then(Value::as_f64);

        let before = by_path
            .get(item_path)
            .or_else(|| by_vid.get(item_vid))
            .cloned();
        let vid = before
            .as_ref()
            .and_then(|b| b["vid"].as_str())
            .unwrap_or(item_vid)
            .to_owned();
        active.insert(vid.clone());
        let stamp = now_sec(conn)?;
        if let Some(before) = &before {
            let b_stage = before["stage"].as_str().unwrap_or("");
            let b_path = before["path"].as_str().unwrap_or("");
            let b_size = review::number(&before["size"]).unwrap_or(-1);
            let b_mtime = before["mtime_ns"].as_str().unwrap_or("");
            let b_batch = before["batch_id"].as_str().unwrap_or("");
            let b_manual = review::number(&before["manual"]).unwrap_or(0) != 0;
            if b_stage == "excluded"
                && b_path == item_path
                && b_size == item_size
                && b_mtime == item_mtime_ns
            {
                let ready = before["data"]
                    .as_str()
                    .and_then(|s| serde_json::from_str::<Value>(s).ok())
                    .and_then(|d| d["ready"].as_bool())
                    .unwrap_or(false);
                let next_stage = if b_manual {
                    "error"
                } else if ready {
                    "review"
                } else {
                    "queued"
                };
                conn.execute(
                    "UPDATE tasks SET stage=?,updated_at=?,run_id='',lease_expires_at=0 WHERE vid=?",
                    params![next_stage, stamp, vid],
                )?;
            } else if b_path != item_path || b_size != item_size || b_mtime != item_mtime_ns {
                conn.execute(
                    "UPDATE batch_items SET excluded=1 WHERE batch_id=? AND vid=? AND completed=0",
                    params![b_batch, vid],
                )?;
                conn.execute(
                    "UPDATE tasks SET path=?,name=?,size=?,mtime_ns=?,duration=?,stage='queued',retry_stage='text',
                    text_attempts=0,face_attempts=0,next_retry=0,error=NULL,data='{}',manual=0,deferred=0,run_id='',
                    lease_expires_at=0,batch_id=?,updated_at=? WHERE vid=?",
                    params![
                        item_path,
                        item_name,
                        item_size,
                        item_mtime_ns,
                        item_duration,
                        batch_id,
                        stamp,
                        vid,
                    ],
                )?;
            }
        } else {
            conn.execute(
                "INSERT OR IGNORE INTO tasks
                (vid,path,name,size,mtime_ns,duration,batch_id,created_at,updated_at,is_new)
                VALUES (?,?,?,?,?,?,?,?,?,?)",
                params![
                    vid,
                    item_path,
                    item_name,
                    item_size,
                    item_mtime_ns,
                    item_duration,
                    batch_id,
                    stamp,
                    stamp,
                    i64::from(bootstrapped),
                ],
            )?;
        }
        if let Some(row) = db::rows(conn, "SELECT * FROM tasks WHERE vid=?", [&vid])?
            .into_iter()
            .next()
        {
            ensure_member(conn, &row, false)?;
            if let Some(p) = row["path"].as_str() {
                by_path.insert(p.to_owned(), row.clone());
            }
            by_vid.insert(vid, row);
        }
    }
    let non_excluded = db::rows(
        conn,
        "SELECT vid,batch_id,stage FROM tasks WHERE stage!='excluded'",
        [],
    )?;
    for row in non_excluded {
        let Some(vid) = row["vid"].as_str() else {
            continue;
        };
        if !active.contains(vid) {
            let stamp = now_sec(conn)?;
            let b_batch = row["batch_id"].as_str().unwrap_or("");
            conn.execute(
                "UPDATE tasks SET stage='excluded',updated_at=?,run_id='',lease_expires_at=0 WHERE vid=?",
                params![stamp, vid],
            )?;
            conn.execute(
                "UPDATE batch_items SET excluded=1 WHERE batch_id=? AND vid=? AND completed=0",
                params![b_batch, vid],
            )?;
        }
    }
    Ok(active.len())
}

pub fn media_available(conn: &Connection, root: &Path) -> Result<bool> {
    let check = fs::metadata(root).and_then(|_| {
        let mut entries = fs::read_dir(root)?;
        let _ = entries.next();
        Ok(())
    });
    match check {
        Err(err) => {
            record_media_outage(conn, &err.to_string())?;
            Ok(false)
        }
        Ok(()) => {
            record_media_recovery(conn)?;
            Ok(true)
        }
    }
}

fn record_media_outage(conn: &Connection, raw_error: &str) -> Result<()> {
    let current = setting(conn, "media_unavailable")?.unwrap_or(json!(false));
    if current == json!(false) || current.is_null() {
        let now = now_sec(conn)?;
        let msg: String = raw_error.chars().take(350).collect();
        event(
            conn,
            now,
            None,
            "media_unavailable",
            &format!("媒体挂载不可读，暂停领取任务：{msg}"),
        )?;
        set_setting(conn, "media_unavailable", &json!({"at": now, "error": msg}))?;
    }
    Ok(())
}

fn record_media_recovery(conn: &Connection) -> Result<()> {
    let current = setting(conn, "media_unavailable")?.unwrap_or(json!(false));
    if current != json!(false) && !current.is_null() {
        let now = now_sec(conn)?;
        set_setting(conn, "media_unavailable", &json!(false))?;
        event(
            conn,
            now,
            None,
            "media_recovered",
            "媒体挂载恢复，继续自动分析",
        )?;
    }
    Ok(())
}

pub fn media_status(config: &Config, body: &Value) -> Result<Value> {
    let conn = open_or_init(config)?;
    let available = if let Some(explicit) = body.get("available").and_then(Value::as_bool) {
        if explicit {
            record_media_recovery(&conn)?;
        } else {
            let err_str = body["error"]
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or("媒体挂载不可读");
            record_media_outage(&conn, err_str)?;
        }
        explicit
    } else {
        let root = body["root"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(Path::new)
            .unwrap_or(&config.media);
        media_available(&conn, root)?
    };
    let issue = setting(&conn, "media_unavailable")?.unwrap_or(json!(false));
    Ok(json!({"ok": true, "available": available, "media_unavailable": issue}))
}

pub fn record_event(config: &Config, body: &Value) -> Result<Value> {
    let conn = open_or_init(config)?;
    let now = now_sec(&conn)?;
    let vid = body["vid"].as_str().filter(|s| !s.is_empty());
    let kind = body["kind"].as_str().unwrap_or("info");
    let message = body
        .get("message")
        .or_else(|| body.get("detail"))
        .and_then(Value::as_str)
        .unwrap_or("");
    event(&conn, now, vid, kind, message)?;
    Ok(json!({"ok": true}))
}

pub fn stage_progress(config: &Config, body: &Value) -> Result<Value> {
    use sha2::{Digest, Sha256};
    let data = body
        .get("data")
        .filter(|v| v.is_object())
        .unwrap_or(body);
    let isolated = body["isolated"].as_bool().unwrap_or(true);
    let id = data["id"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| bad("缺少进度 id"))?;
    let kind = data["kind"].as_str().unwrap_or("text");
    let path = if isolated {
        let digest = format!("{:x}", Sha256::digest(id.as_bytes()));
        config.base.join("jobs/stages").join(format!("{digest}.json"))
    } else {
        config.base.join("jobs").join(format!("{kind}.json"))
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec(data)?;
    files::atomic_write(&path, &bytes)?;
    Ok(json!({"ok": true, "file": path.to_string_lossy()}))
}

fn mapped_path(config: &Config, raw: &str) -> Option<String> {
    if raw.is_empty() {
        return None;
    }
    let normalized = raw.replace('\\', "/");
    if normalized.split('/').any(|p| p == "..") {
        return None;
    }
    let root_str = config.media.to_string_lossy();
    if normalized.starts_with(&format!("{root_str}/")) {
        return Some(normalized);
    }
    normalized
        .split_once("/media/pikpak/")
        .map(|(_, rest)| format!("{root_str}/pikpak/{rest}"))
}

fn source_item(config: &Config, vid: &str, stored: &str) -> Option<Value> {
    let real_root = fs::canonicalize(&config.media).ok()?;
    let creators_root = fs::canonicalize(config.media.join("pikpak/creators"))
        .unwrap_or_else(|_| real_root.join("pikpak/creators"));
    let mut candidates = vec![stored];
    if vid != stored {
        candidates.push(vid);
    }
    for value in candidates {
        let Some(path) = mapped_path(config, value) else {
            continue;
        };
        let file = Path::new(&path);
        let Ok(stat) = fs::metadata(file) else {
            continue;
        };
        if !stat.is_file() || stat.len() == 0 {
            continue;
        }
        let Ok(real) = fs::canonicalize(file) else {
            continue;
        };
        if !real.starts_with(&real_root)
            || real.starts_with(&creators_root)
            || real
                .components()
                .any(|c| c.as_os_str() == ".face-cleanup-staging")
        {
            continue;
        }
        let Some(name) = file.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            continue;
        };
        let Ok(size) = i64::try_from(stat.len()) else {
            continue;
        };
        return Some(json!({
            "vid": vid,
            "path": path,
            "name": name,
            "size": size,
            "mtime_ns": files::mtime_ns(&stat),
        }));
    }
    None
}

fn same_file(checked: &Value, item: &Value) -> bool {
    checked["path"] == item["path"]
        && review::number(&checked["size"]) == review::number(&item["size"])
        && checked["mtime_ns"].as_str() == item["mtime_ns"].as_str()
}

fn write_discovery_progress(
    config: &Config,
    status: &str,
    stage: &str,
    done: Option<usize>,
    total: Option<usize>,
    unit: &str,
    error: Option<&str>,
) {
    let dir = config.base.join("jobs");
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    let path = dir.join("discovery.json");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    let payload = json!({
        "schema_version": 1,
        "id": "discovery",
        "kind": "discovery",
        "title": "媒体发现",
        "status": status,
        "stage": stage,
        "current": Value::Null,
        "done": done,
        "total": total,
        "unit": unit,
        "failed": usize::from(error.is_some()),
        "started_at": now,
        "updated_at": now,
        "finished_at": if status == "running" { Value::Null } else { json!(now) },
        "error": error,
    });
    if let Ok(bytes) = serde_json::to_vec(&payload) {
        let _ = files::atomic_write(&path, &bytes);
    }
}

fn read_ledger_assets(config: &Config) -> Result<Vec<Value>> {
    if !config.ledger.exists() {
        return Err(unavailable("媒体账本不存在"));
    }
    let conn = db::open(&config.ledger)?;
    let rows = db::rows(
        &conn,
        "SELECT id,path,name,size,duration,disposal,mtime FROM asset WHERE medium IS NULL OR medium='video'",
        [],
    )?;
    Ok(rows.into_iter().map(plain_row).collect())
}

fn discover_inner(config: &Config, conn: &mut Connection, body: &Value) -> Result<Value> {
    write_discovery_progress(config, "running", "读取 Peach 账本", None, None, "", None);
    if !media_available(conn, &config.media)? {
        return Err(unavailable("媒体挂载不可读，保留原任务"));
    }
    let assets: Vec<Value> = if let Some(arr) = body.get("assets").and_then(Value::as_array) {
        arr.clone()
    } else {
        read_ledger_assets(config)?
    };
    write_discovery_progress(
        config,
        "running",
        "登记新文件",
        Some(0),
        Some(assets.len()),
        "条记录",
        None,
    );
    let mut sources = BTreeMap::new();
    let faces_path = config.base.join("faces.db");
    if faces_path.exists() {
        let faces = db::open(&faces_path)?;
        for row in db::rows(&faces, "SELECT vid,path FROM videos", [])? {
            if let (Some(vid), Some(path)) = (row["vid"].as_str(), row["path"].as_str()) {
                sources.insert(vid.to_owned(), path.to_owned());
            }
        }
    }
    let mut seen_paths = BTreeSet::new();
    for (vid, p) in &sources {
        if let Some(m) = mapped_path(config, p).or_else(|| mapped_path(config, vid)) {
            seen_paths.insert(m);
        }
    }
    let mut registered = 0usize;
    for asset in &assets {
        if asset.get("disposal").is_some_and(|v| !v.is_null()) {
            continue;
        }
        let Some(raw_path) = asset["path"].as_str() else {
            continue;
        };
        let Some(path) = mapped_path(config, raw_path) else {
            continue;
        };
        if seen_paths.contains(&path) || sources.contains_key(raw_path) {
            continue;
        }
        if source_item(config, raw_path, &path).is_none() {
            continue;
        }
        sources.insert(raw_path.to_owned(), path.clone());
        seen_paths.insert(path);
        registered += 1;
    }

    let queue = review::read(config, "integration-queue.json", json!([]))?.1;
    let history = review::read(config, "archive-history.json", json!([]))?.1;
    let mut resolved = BTreeSet::new();
    let mut resolved_paths = BTreeSet::new();
    for row in queue.as_array().into_iter().flatten() {
        if let Some(vid) = row["vid"].as_str() {
            resolved.insert(vid.to_owned());
            if let Some(p) = sources
                .get(vid)
                .and_then(|s| mapped_path(config, s))
                .or_else(|| mapped_path(config, vid))
            {
                resolved_paths.insert(p);
            }
        }
    }
    for row in history.as_array().into_iter().flatten() {
        if let Some(vid) = row["vid"].as_str() {
            resolved.insert(vid.to_owned());
        }
    }

    let mut disposed = BTreeSet::new();
    let mut active_paths = BTreeSet::new();
    for asset in &assets {
        if let Some(p) = asset["path"].as_str().and_then(|s| mapped_path(config, s)) {
            if asset.get("disposal").is_some_and(|v| !v.is_null()) {
                disposed.insert(p);
            } else {
                active_paths.insert(p);
            }
        }
    }
    for p in &active_paths {
        disposed.remove(p);
    }

    let health = review::read(config, "video-health.json", json!({}))?.1;
    let now = now_sec(conn)?;
    let fresh_health = health["last_success_at"]
        .as_str()
        .and_then(|s| review::timestamp(s).ok())
        .is_some_and(|ts| (now - ts) < 7.0 * 86400.0);

    let mut items = Vec::new();
    let mut paths = BTreeSet::new();
    write_discovery_progress(
        config,
        "running",
        "核对媒体文件",
        Some(0),
        Some(sources.len()),
        "个文件",
        None,
    );
    for (vid, stored) in &sources {
        if resolved.contains(vid) {
            continue;
        }
        let Some(item) = source_item(config, vid, stored) else {
            continue;
        };
        let item_path = item["path"].as_str().unwrap_or("");
        if paths.contains(item_path)
            || resolved_paths.contains(item_path)
            || disposed.contains(item_path)
        {
            continue;
        }
        let checked = &health["records"][vid];
        if fresh_health && checked["hidden"] == true && same_file(checked, &item) {
            continue;
        }
        paths.insert(item_path.to_owned());
        items.push(item);
    }

    let bootstrapped = setting(conn, "bootstrapped")?
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let existing_active = db::rows(
        conn,
        "SELECT vid,path FROM tasks WHERE stage!='excluded'",
        [],
    )?;
    let expected = existing_active
        .iter()
        .filter(|row| {
            let vid = row["vid"].as_str().unwrap_or("");
            let path = row["path"].as_str().unwrap_or("");
            !resolved.contains(vid) && !resolved_paths.contains(path) && !disposed.contains(path)
        })
        .count();
    if expected >= 20 && (items.len() as f64) < (expected as f64) * 0.2 {
        return Err(unavailable(
            "媒体可读数量突然下降，本轮保留原任务，请检查媒体挂载",
        ));
    }

    let batch_id = if let Some(b) = body["batch_id"].as_str().filter(|s| !s.is_empty()) {
        b.to_owned()
    } else {
        conn.query_row("SELECT strftime('%Y%m%dT%H%M%SZ','now')", [], |r| {
            r.get::<_, String>(0)
        })?
    };
    let pipeline = body["pipeline"].as_str().unwrap_or(DEFAULT_PIPELINE);
    let face_version = body["face_version"]
        .as_str()
        .unwrap_or(DEFAULT_FACE_VERSION);
    let references = body
        .get("references")
        .filter(|v| v.is_object())
        .cloned()
        .or_else(|| setting(conn, "references").ok().flatten());

    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let count = reconcile(&tx, &items, &batch_id, bootstrapped)?;
    let stamp = now_sec(&tx)?;
    tx.execute(
        "UPDATE tasks SET stage='queued',retry_stage='text',text_attempts=0,face_attempts=0,next_retry=0,
        updated_at=? WHERE stage='review' AND manual=0 AND json_extract(data,'$.watermark.pipeline')<>?",
        params![stamp, pipeline],
    )?;
    if let Some(refs) = &references
        && let Some(ref_ver) = refs["version"].as_str().filter(|s| !s.is_empty())
    {
        tx.execute(
            "UPDATE tasks SET stage='face_queued',retry_stage='face',face_attempts=0,next_retry=0,
            updated_at=? WHERE stage='review' AND manual=0 AND (json_extract(data,'$.face.reference_version')<>? OR json_extract(data,'$.face.version')<>?)",
            params![stamp, ref_ver, face_version],
        )?;
    }
    set_setting(&tx, "bootstrapped", &json!(true))?;
    set_setting(
        &tx,
        "last_discovery",
        &json!({
            "at": stamp,
            "registered": registered,
            "eligible": count,
            "source_count": assets.len(),
            "error": Value::Null,
        }),
    )?;
    if let Some(refs) = &references {
        set_setting(&tx, "references", refs)?;
    }
    tx.commit()?;
    write_discovery_progress(
        config,
        "done",
        "发现完成",
        Some(count),
        Some(count),
        "个可处理视频",
        None,
    );
    Ok(json!({
        "ok": true,
        "eligible": count,
        "registered": registered,
        "source_count": assets.len(),
        "batch_id": batch_id,
    }))
}

pub fn discover(config: &Config, body: &Value) -> Result<Value> {
    let mut conn = open_or_init(config)?;
    match discover_inner(config, &mut conn, body) {
        Ok(res) => Ok(res),
        Err(err) => {
            write_discovery_progress(
                config,
                "error",
                "发现中断",
                None,
                None,
                "",
                Some(&err.error),
            );
            if let Ok(tx) = conn.transaction_with_behavior(TransactionBehavior::Immediate) {
                let now = now_sec(&tx).unwrap_or(0.0);
                let mut prev = setting(&tx, "last_discovery")
                    .ok()
                    .flatten()
                    .and_then(|v| v.as_object().cloned())
                    .unwrap_or_default();
                let msg: String = err.error.chars().take(500).collect();
                prev.insert("error".into(), json!(msg));
                prev.insert("attempted_at".into(), json!(now));
                let _ = set_setting(&tx, "last_discovery", &Value::Object(prev));
                let _ = event(&tx, now, None, "discovery_error", &err.error);
                let _ = tx.commit();
            }
            Err(err)
        }
    }
}

pub fn reap_expired_leases(conn: &Connection, now: f64) -> Result<usize> {
    let expired = db::rows(
        conn,
        "SELECT * FROM tasks WHERE stage IN ('text','face') AND manual=0 AND lease_expires_at>0 AND lease_expires_at<=?",
        params![now],
    )?;
    let mut count = 0usize;
    for task in expired {
        let Some(vid) = task["vid"].as_str() else {
            continue;
        };
        let stage = task["stage"].as_str().unwrap_or("text");
        let run_id = task["run_id"].as_str().unwrap_or("");
        let attempts_field = if stage == "text" {
            "text_attempts"
        } else {
            "face_attempts"
        };
        let attempts = review::number(&task[attempts_field]).unwrap_or(1);
        let next_stage = if attempts < 3 { "retry" } else { "error" };
        let backoff = (30 * attempts.max(1)).min(300) as f64;
        let next_retry = if next_stage == "retry" {
            now + backoff
        } else {
            0.0
        };
        let message = "执行租约已超时";
        let updated = conn.execute(
            "UPDATE tasks SET stage=?,retry_stage=?,error=?,next_retry=?,run_id='',lease_expires_at=0,updated_at=?
            WHERE vid=? AND stage=? AND run_id=? AND manual=0",
            params![next_stage, stage, message, next_retry, now, vid, stage, run_id],
        )?;
        if updated > 0 {
            event(conn, now, Some(vid), next_stage, message)?;
            if !run_id.is_empty() {
                conn.execute(
                    "UPDATE processing_runs SET status=?,error=?,finished_at=? WHERE id=? AND status='running'",
                    params![next_stage, message, now, run_id],
                )?;
            }
            count += 1;
        }
    }
    Ok(count)
}

pub fn claim(config: &Config, body: &Value) -> Result<Value> {
    let mut conn = open_or_init(config)?;
    let capacity = body["capacity"].as_i64().unwrap_or(1).max(1);
    let lease_ttl = body["lease_ttl"]
        .as_f64()
        .filter(|v| v.is_finite() && *v > 0.0)
        .unwrap_or(DEFAULT_LEASE_SECONDS);
    let check_media = body["check_media"].as_bool().unwrap_or(false);

    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if check_media && !media_available(&tx, &config.media)? {
        tx.commit()?;
        return Ok(json!({"task": Value::Null, "media_available": false}));
    }
    let now = now_sec(&tx)?;
    let reaped = reap_expired_leases(&tx, now)?;
    let paused = setting(&tx, "paused")?
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let active: i64 = tx.query_row(
        "SELECT COUNT(*) FROM tasks WHERE stage IN ('text','face')",
        [],
        |r| r.get(0),
    )?;
    let limit = effective_concurrency(&tx, capacity)?;
    if paused || active >= limit {
        tx.commit()?;
        return Ok(json!({"task": Value::Null, "reaped": reaped}));
    }
    let Some(raw_row) = db::rows(
        &tx,
        "SELECT * FROM tasks WHERE stage IN ('queued','face_queued','retry') AND manual=0 AND next_retry<=?
        ORDER BY priority DESC,is_new DESC,created_at DESC,vid LIMIT 1",
        params![now],
    )?
    .into_iter()
    .next() else {
        tx.commit()?;
        return Ok(json!({"task": Value::Null, "reaped": reaped}));
    };
    let mut task = plain_row(raw_row);
    let current_stage = task["stage"].as_str().unwrap_or("queued");
    let retry_stage = task["retry_stage"].as_str().unwrap_or("text");
    let stage = if current_stage == "face_queued" {
        "face"
    } else if current_stage == "retry" {
        retry_stage
    } else {
        "text"
    }
    .to_owned();
    let field = if stage == "text" {
        "text_attempts"
    } else {
        "face_attempts"
    };
    let run_id = uuid::Uuid::new_v4().to_string();
    let generation = setting(&tx, "coordinator_generation")?
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| "unstarted".into());
    let expiry = now + lease_ttl;
    let vid = task["vid"].as_str().unwrap_or("").to_owned();
    let name = task["name"].as_str().unwrap_or("").to_owned();
    let batch_id = task["batch_id"].as_str().unwrap_or("").to_owned();

    tx.execute(
        &format!(
            "UPDATE tasks SET stage=?,retry_stage=?,{field}={field}+1,updated_at=?,error=NULL,run_id=?,lease_expires_at=?,coordinator_generation=? WHERE vid=?"
        ),
        params![stage, stage, now, run_id, expiry, generation, vid],
    )?;
    tx.execute(
        "INSERT INTO processing_runs(id,vid,name,batch_id,stage,started_at,status) VALUES (?,?,?,?,?,?,?)",
        params![run_id, vid, name, batch_id, stage, now, "running"],
    )?;
    event(
        &tx,
        now,
        Some(&vid),
        "started",
        if stage == "text" {
            "文字线索提取"
        } else {
            "人脸相似度分析"
        },
    )?;
    tx.commit()?;

    let attempts = task[field].as_i64().unwrap_or(0) + 1;
    task["stage"] = json!(stage);
    task["retry_stage"] = json!(stage);
    task[field] = json!(attempts);
    task["updated_at"] = json!(now);
    task["error"] = Value::Null;
    task["run_id"] = json!(run_id);
    task["lease_expires_at"] = json!(expiry);
    task["coordinator_generation"] = json!(generation);
    Ok(json!({"task": task, "reaped": reaped}))
}

pub fn heartbeat(config: &Config, body: &Value) -> Result<Value> {
    let mut conn = open_or_init(config)?;
    let lease_ttl = body["lease_ttl"]
        .as_f64()
        .filter(|v| v.is_finite() && *v > 0.0)
        .unwrap_or(DEFAULT_LEASE_SECONDS);
    let capacity = body["capacity"].as_i64().unwrap_or(1).max(1);
    let mut run_ids = Vec::new();
    if let Some(id) = body["run_id"].as_str().filter(|s| !s.is_empty()) {
        run_ids.push(id.to_owned());
    }
    if let Some(arr) = body["run_ids"].as_array() {
        for v in arr {
            if let Some(id) = v.as_str().filter(|s| !s.is_empty())
                && !run_ids.contains(&id.to_owned())
            {
                run_ids.push(id.to_owned());
            }
        }
    }
    let expected_gen = body["coordinator_generation"]
        .as_str()
        .filter(|s| !s.is_empty());

    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let now = now_sec(&tx)?;
    let reaped = reap_expired_leases(&tx, now)?;
    let expiry = now + lease_ttl;
    let mut renewed = Vec::new();
    let mut stale = Vec::new();
    for run_id in &run_ids {
        let updated = if let Some(generation) = expected_gen {
            tx.execute(
                "UPDATE tasks SET lease_expires_at=?,updated_at=? WHERE run_id=? AND stage IN ('text','face') AND manual=0 AND coordinator_generation=?",
                params![expiry, now, run_id, generation],
            )?
        } else {
            tx.execute(
                "UPDATE tasks SET lease_expires_at=?,updated_at=? WHERE run_id=? AND stage IN ('text','face') AND manual=0",
                params![expiry, now, run_id],
            )?
        };
        if updated > 0 {
            renewed.push(run_id.clone());
        } else {
            stale.push(run_id.clone());
        }
    }
    if body["run_id"].is_string() && renewed.is_empty() && !stale.is_empty() {
        tx.commit()?;
        return Err(conflict("任务租约已失效或被接管"));
    }
    let active: i64 = tx.query_row(
        "SELECT COUNT(*) FROM tasks WHERE stage IN ('text','face')",
        [],
        |r| r.get(0),
    )?;
    let desired = effective_concurrency(&tx, capacity)?;
    let paused = setting(&tx, "paused")?
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let discovery_requested = setting(&tx, "discovery_requested")?
        .and_then(|v| v.as_f64().or_else(|| v.as_i64().map(|n| n as f64)))
        .unwrap_or(0.0);
    tx.commit()?;

    if let Some(beat) = body.get("heartbeat").filter(|v| v.is_object()) {
        let mut beat = beat.clone();
        beat["at"] = json!(now);
        beat["concurrency"] = json!(desired);
        beat["active"] = json!(active);
        let beat_path = config.base.join("processing/heartbeat.json");
        if let Some(parent) = beat_path.parent() {
            fs::create_dir_all(parent)?;
        }
        files::atomic_write(&beat_path, serde_json::to_string(&beat)?.as_bytes())?;
    }

    Ok(json!({
        "ok": true,
        "renewed": renewed,
        "stale": stale,
        "reaped": reaped,
        "active": active,
        "concurrency": desired,
        "paused": paused,
        "discovery_requested": discovery_requested,
        "lease_expires_at": expiry,
    }))
}

fn close_run(
    conn: &Connection,
    run_id: &str,
    status: &str,
    error: Option<&str>,
    now: f64,
) -> Result<()> {
    if run_id.is_empty() {
        return Ok(());
    }
    let truncated = error.map(|s| s.chars().take(500).collect::<String>());
    conn.execute(
        "UPDATE processing_runs SET status=?,error=?,finished_at=? WHERE id=? AND status='running'",
        params![status, truncated, now, run_id],
    )?;
    Ok(())
}

pub fn fail_task(config: &Config, body: &Value) -> Result<Value> {
    let task = body
        .get("task")
        .filter(|v| v.is_object())
        .ok_or_else(|| bad("缺少任务信息"))?;
    let vid = task["vid"].as_str().ok_or_else(|| bad("缺少 vid"))?;
    let stage = task["stage"].as_str().ok_or_else(|| bad("缺少 stage"))?;
    let path = task["path"].as_str().ok_or_else(|| bad("缺少 path"))?;
    let size = review::number(&task["size"]).ok_or_else(|| bad("缺少 size"))?;
    let mtime_ns = task["mtime_ns"]
        .as_str()
        .ok_or_else(|| bad("缺少 mtime_ns"))?;
    let run_id = task["run_id"].as_str().unwrap_or("");
    let attempts_field = if stage == "text" {
        "text_attempts"
    } else {
        "face_attempts"
    };
    let attempts = review::number(&task[attempts_field]).unwrap_or(1);
    let error_msg: String = body["error"]
        .as_str()
        .unwrap_or("执行失败")
        .chars()
        .take(500)
        .collect();

    let mut conn = open_or_init(config)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let now = now_sec(&tx)?;
    let next_stage = if attempts < 3 { "retry" } else { "error" };
    let backoff = (30 * attempts.max(0)).min(300) as f64;
    let next_retry = now + backoff;
    let updated = tx.execute(
        "UPDATE tasks SET stage=?,retry_stage=?,error=?,next_retry=?,run_id='',lease_expires_at=0,updated_at=?
        WHERE vid=? AND stage=? AND path=? AND size=? AND mtime_ns=? AND run_id=? AND manual=0",
        params![
            next_stage,
            stage,
            error_msg,
            next_retry,
            now,
            vid,
            stage,
            path,
            size,
            mtime_ns,
            run_id,
        ],
    )?;
    if updated > 0 {
        event(&tx, now, Some(vid), next_stage, &error_msg)?;
    }
    close_run(
        &tx,
        run_id,
        if updated > 0 {
            next_stage
        } else {
            "superseded"
        },
        Some(&error_msg),
        now,
    )?;
    tx.commit()?;
    Ok(json!({
        "ok": true,
        "matched": updated > 0,
        "stage": if updated > 0 { next_stage } else { "superseded" },
        "next_retry": if updated > 0 { json!(next_retry) } else { Value::Null },
    }))
}

pub fn release_task(config: &Config, body: &Value) -> Result<Value> {
    let task = body
        .get("task")
        .filter(|v| v.is_object())
        .ok_or_else(|| bad("缺少任务信息"))?;
    let vid = task["vid"].as_str().ok_or_else(|| bad("缺少 vid"))?;
    let stage = task["stage"].as_str().ok_or_else(|| bad("缺少 stage"))?;
    let path = task["path"].as_str().ok_or_else(|| bad("缺少 path"))?;
    let size = review::number(&task["size"]).ok_or_else(|| bad("缺少 size"))?;
    let mtime_ns = task["mtime_ns"]
        .as_str()
        .ok_or_else(|| bad("缺少 mtime_ns"))?;
    let run_id = task["run_id"].as_str().unwrap_or("");
    let batch_id = task["batch_id"].as_str().unwrap_or("");
    let excluded = body["excluded"].as_bool().unwrap_or(false);
    let reason = body["reason"].as_str().unwrap_or("等待恢复");
    let retry_delay = body["retry_delay"]
        .as_f64()
        .filter(|v| v.is_finite() && *v > 0.0)
        .unwrap_or(0.0);
    let field = if stage == "text" {
        "text_attempts"
    } else {
        "face_attempts"
    };

    let mut conn = open_or_init(config)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let now = now_sec(&tx)?;
    let next_stage = if excluded { "excluded" } else { "retry" };
    let next_retry = if excluded || retry_delay <= 0.0 {
        0.0
    } else {
        now + retry_delay
    };
    let error_col: Option<&str> = if excluded { None } else { Some(reason) };
    let updated = tx.execute(
        &format!(
            "UPDATE tasks SET stage=?,next_retry=?,{field}=MAX(0,{field}-1),error=?,run_id='',lease_expires_at=0,updated_at=?
            WHERE vid=? AND stage=? AND path=? AND size=? AND mtime_ns=? AND run_id=? AND manual=0"
        ),
        params![
            next_stage,
            next_retry,
            error_col,
            now,
            vid,
            stage,
            path,
            size,
            mtime_ns,
            run_id,
        ],
    )?;
    if updated > 0 && excluded {
        tx.execute(
            "UPDATE batch_items SET excluded=1 WHERE batch_id=? AND vid=? AND completed=0",
            params![batch_id, vid],
        )?;
    }
    let run_status = if updated > 0 {
        if excluded { "cancelled" } else { "waiting" }
    } else {
        "superseded"
    };
    close_run(&tx, run_id, run_status, Some(reason), now)?;
    tx.commit()?;
    Ok(json!({
        "ok": true,
        "matched": updated > 0,
        "stage": if updated > 0 { next_stage } else { "superseded" },
    }))
}

pub fn recover(config: &Config) -> Result<Value> {
    let mut conn = open_or_init(config)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let generation = uuid::Uuid::new_v4().to_string();
    let now = now_sec(&tx)?;
    set_setting(&tx, "coordinator_generation", &json!(generation))?;
    tx.execute(
        "UPDATE processing_runs SET status='interrupted',finished_at=?,error='协调器重启或阶段意外中断' WHERE status='running'",
        params![now],
    )?;
    let recovered = tx.execute(
        "UPDATE tasks SET stage=CASE WHEN
        (stage='text' AND text_attempts>=3) OR (stage='face' AND face_attempts>=3) THEN 'error' ELSE 'retry' END,
        next_retry=0,error='服务重启，中断阶段按重试上限恢复',run_id='',lease_expires_at=0,updated_at=?
        WHERE stage IN ('text','face')",
        params![now],
    )?;
    tx.commit()?;
    Ok(json!({
        "ok": true,
        "coordinator_generation": generation,
        "recovered": recovered,
    }))
}

fn eligible_vids(config: &Config, conn: &Connection) -> Result<BTreeSet<String>> {
    let queue = review::read(config, "integration-queue.json", json!([]))?.1;
    let history = review::read(config, "archive-history.json", json!([]))?.1;
    let mut handled_ids = BTreeSet::new();
    for row in queue
        .as_array()
        .into_iter()
        .flatten()
        .chain(history.as_array().into_iter().flatten())
    {
        if let Some(id) = row["vid"].as_str() {
            handled_ids.insert(id.to_owned());
        }
    }
    let mut sources = if config.base.join("faces.db").exists() {
        review::indexed_sources(config)?
    } else {
        BTreeMap::new()
    };
    for row in db::rows(conn, "SELECT vid,path FROM tasks ORDER BY vid", [])? {
        if let (Some(id), Some(path)) = (row["vid"].as_str(), row["path"].as_str()) {
            sources
                .entry(id.to_owned())
                .or_insert_with(|| path.to_owned());
        }
    }
    let mut queued_paths = BTreeSet::new();
    for row in queue.as_array().into_iter().flatten() {
        if let Some(id) = row["vid"].as_str()
            && let Some(p) = review::source(config, id, &sources)
        {
            queued_paths.insert(p);
        }
    }
    let mut eligible = BTreeSet::new();
    for row in db::rows(
        conn,
        "SELECT vid,path FROM tasks WHERE stage!='excluded'",
        [],
    )? {
        let Some(vid) = row["vid"].as_str() else {
            continue;
        };
        let path = row["path"].as_str().unwrap_or("");
        if !handled_ids.contains(vid) && !queued_paths.contains(path) {
            eligible.insert(vid.to_owned());
        }
    }
    Ok(eligible)
}

pub fn control(config: &Config, body: &Value) -> Result<Value> {
    let action = body["action"].as_str().unwrap_or("");
    if ![
        "pause", "resume", "priority", "retry", "takeover", "defer", "restore", "note",
    ]
    .contains(&action)
    {
        return Err(bad("未知操作"));
    }
    if action == "note"
        && body.get("note").is_some_and(|v| !v.is_null())
        && body["note"]
            .as_str()
            .is_none_or(|s| s.chars().count() > 1000)
    {
        return Err(bad("备注最多 1000 字"));
    }
    let vids = if matches!(action, "pause" | "resume") {
        vec![]
    } else {
        review::vids(&body["vids"])?
    };

    let mut conn = open_existing(config, "分析服务尚未初始化")?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let now = now_sec(&tx)?;
    if matches!(action, "pause" | "resume") {
        let paused = action == "pause";
        set_setting(&tx, "paused", &json!(paused))?;
        event(
            &tx,
            now,
            None,
            action,
            if paused {
                "暂停自动分析"
            } else {
                "恢复自动分析"
            },
        )?;
        tx.commit()?;
        return Ok(json!({"ok": true, "paused": paused}));
    }

    let eligible = eligible_vids(config, &tx)?;
    if vids.iter().any(|vid| !eligible.contains(vid)) {
        return Err(conflict("视频状态已变化，请刷新列表"));
    }

    for vid in &vids {
        let row = db::rows(
            &tx,
            "SELECT stage,manual,run_id FROM tasks WHERE vid=?",
            [vid],
        )?
        .into_iter()
        .next()
        .ok_or_else(|| conflict("视频状态已变化，请刷新列表"))?;
        let stage = row["stage"].as_str().unwrap_or("");
        let manual = review::number(&row["manual"]).unwrap_or(0) != 0;
        let run_id = row["run_id"].as_str().unwrap_or("");

        match action {
            "priority" => {
                tx.execute(
                    "UPDATE tasks SET priority=priority+1,updated_at=? WHERE vid=?",
                    params![now, vid],
                )?;
            }
            "retry" => {
                if !matches!(stage, "error" | "retry") {
                    return Err(conflict("仅异常或等待重试的任务可以重试"));
                }
                tx.execute(
                    "UPDATE tasks SET stage='retry',text_attempts=0,face_attempts=0,next_retry=0,error=NULL,manual=0,run_id='',lease_expires_at=0,updated_at=? WHERE vid=?",
                    params![now, vid],
                )?;
                close_run(&tx, run_id, "superseded", None, now)?;
            }
            "takeover" => {
                if stage != "error" {
                    return Err(conflict("仅异常任务可以接管"));
                }
                tx.execute(
                    "UPDATE tasks SET manual=1,deferred=0,run_id='',lease_expires_at=0,updated_at=? WHERE vid=?",
                    params![now, vid],
                )?;
                close_run(&tx, run_id, "superseded", None, now)?;
            }
            "defer" | "restore" | "note" => {
                if stage != "review" && !manual {
                    return Err(conflict("视频尚未就绪"));
                }
                if action == "note" {
                    let note = body["note"].as_str().unwrap_or("");
                    tx.execute(
                        "UPDATE tasks SET note=?,updated_at=? WHERE vid=?",
                        params![note, now, vid],
                    )?;
                } else {
                    let deferred = i64::from(action == "defer");
                    tx.execute(
                        "UPDATE tasks SET deferred=?,updated_at=? WHERE vid=?",
                        params![deferred, now, vid],
                    )?;
                }
            }
            _ => unreachable!(),
        }
        let message = match action {
            "priority" => "优先处理",
            "retry" => "人工重试",
            "takeover" => "人工接管",
            "defer" => "暂时跳过",
            "restore" => "恢复核对",
            "note" => "更新备注",
            _ => action,
        };
        event(&tx, now, Some(vid), action, message)?;
    }
    tx.commit()?;
    Ok(json!({"ok": true, "vids": vids}))
}

pub fn settings(config: &Config) -> Result<Value> {
    let conn = open_existing(config, "分析服务尚未初始化，设置未知")?;
    settings_with_conn(config, &conn)
}

fn settings_with_conn(config: &Config, conn: &Connection) -> Result<Value> {
    let beat = review::read(config, "processing/heartbeat.json", json!({}))?.1;
    let now = now_sec(conn)?;
    let at = beat["at"]
        .as_f64()
        .or_else(|| beat["at"].as_i64().map(|n| n as f64))
        .unwrap_or(0.0);
    let age = now - at;
    let online = (-30.0..90.0).contains(&age);
    let protocol = beat["protocol"].as_i64().unwrap_or(0);
    let supported = protocol >= 2;
    let maximum = beat["max_concurrency"]
        .as_i64()
        .filter(|_| beat["max_concurrency"].is_i64() || beat["max_concurrency"].is_u64())
        .unwrap_or(1)
        .clamp(1, 4);
    let requested = setting(conn, "concurrency")?
        .and_then(|v| v.as_i64())
        .unwrap_or(1)
        .clamp(1, 4);
    let last_seen = if at > 0.0 {
        beat["at"].clone()
    } else {
        Value::Null
    };
    Ok(json!({
        "schema_version": 1,
        "requested": requested,
        "maximum": maximum,
        "effective": requested.min(maximum),
        "supported": supported,
        "online": online,
        "last_seen": last_seen,
    }))
}

pub fn set_concurrency(config: &Config, body: &Value) -> Result<Value> {
    let mut conn = open_existing(config, "分析服务尚未初始化，设置未知")?;
    let current = settings_with_conn(config, &conn)?;
    if current["supported"] != true || current["online"] != true {
        return Err(unavailable("当前后台尚未连接或未升级，暂不能修改并发"));
    }
    let maximum = current["maximum"].as_i64().unwrap_or(1).clamp(1, 4);
    let requested = body
        .get("concurrency")
        .filter(|v| v.is_i64() || v.is_u64())
        .and_then(Value::as_i64)
        .filter(|n| (1..=maximum).contains(n))
        .ok_or_else(|| bad(format!("并发数必须为 1 至 {maximum} 的整数")))?;

    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let now = now_sec(&tx)?;
    set_setting(&tx, "concurrency", &json!(requested))?;
    event(
        &tx,
        now,
        None,
        "concurrency",
        &format!("并发设置为 {requested} 路；在途任务继续完成"),
    )?;
    tx.commit()?;

    let mut out = current;
    out["requested"] = json!(requested);
    out["effective"] = json!(requested);
    Ok(out)
}

pub fn retry_errors(config: &Config, body: &Value) -> Result<Value> {
    let batch = match body.get("batch") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if !s.is_empty() && s.len() <= 128 => Some(s.as_str()),
        _ => return Err(bad("批次格式无效")),
    };
    let mut conn = open_existing(config, "分析服务尚未初始化")?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let eligible = eligible_vids(config, &tx)?;
    let rows = db::rows(
        &tx,
        "SELECT vid,batch_id,run_id FROM tasks WHERE stage='error' AND manual=0 ORDER BY priority DESC,is_new DESC,created_at DESC,vid",
        [],
    )?;
    let now = now_sec(&tx)?;
    let mut retried = 0usize;
    for row in rows {
        let Some(vid) = row["vid"].as_str() else {
            continue;
        };
        if !eligible.contains(vid) {
            continue;
        }
        if let Some(b) = batch
            && row["batch_id"].as_str() != Some(b)
        {
            continue;
        }
        let run_id = row["run_id"].as_str().unwrap_or("");
        tx.execute(
            "UPDATE tasks SET stage='retry',text_attempts=0,face_attempts=0,next_retry=0,error=NULL,manual=0,run_id='',lease_expires_at=0,updated_at=? WHERE vid=?",
            params![now, vid],
        )?;
        close_run(&tx, run_id, "superseded", None, now)?;
        event(&tx, now, Some(vid), "retry", "人工重试")?;
        retried += 1;
    }
    tx.commit()?;
    Ok(json!({
        "schema_version": 1,
        "retried": retried,
    }))
}

fn read_json_fallback(path: &Path, fallback: Value) -> Value {
    fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(fallback)
}

fn now_wall() -> Result<f64> {
    Ok(Connection::open_in_memory()?
        .query_row("SELECT unixepoch('now','subsec')", [], |r| r.get(0))?)
}

fn canonical_person_fields(value: Value, key: &str, aliases: &Value) -> Value {
    match value {
        Value::String(s) if matches!(key, "name" | "creator" | "suggested") => {
            json!(crate::processing_result::canonical_person(&s, aliases))
        }
        Value::Array(arr) => Value::Array(
            arr.into_iter()
                .map(|v| canonical_person_fields(v, "", aliases))
                .collect(),
        ),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(k, v)| {
                    let next = canonical_person_fields(v, &k, aliases);
                    (k, next)
                })
                .collect(),
        ),
        other => other,
    }
}

fn is_review_available(task: &Value) -> bool {
    let stage = task["stage"].as_str().unwrap_or("");
    let manual = task["manual"].as_i64().unwrap_or(0) != 0;
    let ready = task["data"]["ready"] == true;
    let retry_stage = task["retry_stage"].as_str().unwrap_or("");
    stage == "review"
        || manual
        || (ready
            && (stage == "face"
                || stage == "face_queued"
                || ((stage == "retry" || stage == "error") && retry_stage == "face")))
}

struct ActiveState {
    online: bool,
    paused: bool,
    last_discovery: Value,
    references: Value,
    tasks: Vec<Value>,
    events: Vec<Value>,
}

fn load_active_state(config: &Config, conn: &Connection, now: f64) -> Result<ActiveState> {
    let beat = read_json_fallback(&config.base.join("processing/heartbeat.json"), json!({}));
    let beat_at = beat["at"]
        .as_f64()
        .or_else(|| beat["at"].as_i64().map(|n| n as f64))
        .unwrap_or(0.0);
    let age = now - beat_at;
    let online = (-30.0..90.0).contains(&age);
    let paused = setting(conn, "paused")?
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let last_discovery = setting(conn, "last_discovery")?.unwrap_or(Value::Null);
    let references = setting(conn, "references")?.unwrap_or(Value::Null);
    let eligible = eligible_vids(config, conn)?;
    let aliases = review::read(config, "person-renames.json", json!({}))?.1;
    let raw_tasks = db::rows(
        conn,
        "SELECT * FROM tasks WHERE stage!='excluded' ORDER BY priority DESC,is_new DESC,created_at DESC,vid",
        [],
    )?;
    let mut tasks = Vec::new();
    for raw in raw_tasks {
        let Some(vid) = raw["vid"].as_str() else {
            continue;
        };
        if !eligible.contains(vid) {
            continue;
        }
        let mut row = plain_row(raw);
        let parsed_data = row["data"]
            .as_str()
            .and_then(|s| serde_json::from_str::<Value>(s).ok())
            .unwrap_or_else(|| json!({}));
        row["data"] = canonical_person_fields(parsed_data, "", &aliases);
        tasks.push(row);
    }
    let events = db::rows(conn, "SELECT * FROM events ORDER BY id DESC LIMIT 12", [])?
        .into_iter()
        .map(plain_row)
        .collect();
    Ok(ActiveState {
        online,
        paused,
        last_discovery,
        references,
        tasks,
        events,
    })
}

fn read_run_progress(config: &Config, run: &Value, online: bool) -> Value {
    use sha2::{Digest, Sha256};
    let run_id = run["id"].as_str().unwrap_or("");
    let run_vid = run["vid"].as_str().unwrap_or("");
    let run_name = run["name"].as_str().unwrap_or("");
    let run_stage = run["stage"].as_str().unwrap_or("text");
    let run_status = run["status"].as_str().unwrap_or("");
    let started_at = run["started_at"]
        .as_f64()
        .or_else(|| run["started_at"].as_i64().map(|n| n as f64))
        .unwrap_or(0.0);
    let finished_at = run["finished_at"]
        .as_f64()
        .or_else(|| run["finished_at"].as_i64().map(|n| n as f64))
        .filter(|v| *v != 0.0);
    let error = run["error"].as_str().filter(|s| !s.is_empty());

    let digest = format!("{:x}", Sha256::digest(run_id.as_bytes()));
    let stage_path = config
        .base
        .join("jobs/stages")
        .join(format!("{digest}.json"));
    let raw = read_json_fallback(&stage_path, Value::Null);
    let same = raw.is_object()
        && raw["id"].as_str() == Some(run_id)
        && raw["vid"].as_str() == Some(run_vid);
    let kind = if run_stage == "face" { "face" } else { "text" };
    let status = if run_status == "running" {
        if online { "running" } else { "offline" }
    } else if run_status == "done" {
        "complete"
    } else if matches!(run_status, "error" | "interrupted") {
        "error"
    } else {
        "waiting"
    };
    let stage_text = if run_status == "retry" {
        "等待自动重试".to_owned()
    } else if run_status == "waiting" {
        "等待资源恢复".to_owned()
    } else if same {
        raw["stage"].as_str().unwrap_or("").to_owned()
    } else if status == "running" {
        "准备中".to_owned()
    } else if status == "complete" {
        "阶段完成".to_owned()
    } else {
        error.unwrap_or("阶段已结束").to_owned()
    };
    let done = if same {
        raw["done"]
            .as_f64()
            .filter(|v| v.is_finite() && *v >= 0.0)
            .map(|_| json!(raw["done"]))
            .unwrap_or(Value::Null)
    } else {
        Value::Null
    };
    let total = if same {
        raw["total"]
            .as_f64()
            .filter(|v| v.is_finite() && *v >= 0.0)
            .map(|_| json!(raw["total"]))
            .unwrap_or(Value::Null)
    } else {
        Value::Null
    };
    let unit = if same {
        raw["unit"].as_str().unwrap_or("")
    } else {
        ""
    };
    let updated_at = if same && !raw["updated_at"].is_null() {
        raw["updated_at"].clone()
    } else if let Some(f) = finished_at {
        json!(f)
    } else {
        json!(started_at)
    };
    let heartbeat_at = if same && !raw["heartbeat_at"].is_null() {
        raw["heartbeat_at"].clone()
    } else {
        Value::Null
    };
    let mut job = json!({
        "id": run_id,
        "kind": kind,
        "title": if kind == "text" { "提取水印" } else { "人脸分析" },
        "status": status,
        "stage": stage_text,
        "current": run_name,
        "vid": run_vid,
        "done": done,
        "total": total,
        "unit": unit,
        "failed": if status == "error" { 1 } else { 0 },
        "started_at": started_at,
        "updated_at": updated_at,
        "heartbeat_at": heartbeat_at,
        "href": format!("/operations/tasks?vid={}", review::percent(run_vid)),
    });
    if let Some(f) = finished_at {
        job["finished_at"] = json!(f);
    }
    if let Some(err) = error {
        job["error"] = json!(err);
    }
    job
}

pub fn overview(config: &Config, body: &Value) -> Result<Value> {
    let has_vid = body.as_object().is_some_and(|m| m.contains_key("vid"));
    let requested_vid = body["vid"].as_str();
    if !db_path(config).exists() {
        let mut out = json!({
            "online": false,
            "paused": false,
            "last_discovery": null,
            "references": null,
            "events": [],
            "counts": { "review": 0 },
            "total": 0,
            "page": 0,
            "limit": 25,
            "all_count": 0,
            "rows": [],
            "batches": [],
        });
        if has_vid {
            out["task"] = Value::Null;
        }
        return Ok(out);
    }
    let conn = open_existing(config, "分析服务尚未初始化")?;
    let now = now_sec(&conn)?;
    let state = load_active_state(config, &conn, now)?;
    let mut counts = serde_json::Map::new();
    for task in &state.tasks {
        let stage = if task["manual"].as_i64().unwrap_or(0) != 0 {
            "review"
        } else {
            task["stage"].as_str().unwrap_or("")
        };
        let next = counts.get(stage).and_then(Value::as_u64).unwrap_or(0) + 1;
        counts.insert(stage.to_owned(), json!(next));
    }
    let review_count = state
        .tasks
        .iter()
        .filter(|t| is_review_available(t))
        .count();
    counts.insert("review".into(), json!(review_count));

    let stage_filter = body["stage"].as_str().unwrap_or("");
    let batch_filter = body["batch"].as_str().unwrap_or("");
    let query = body["q"].as_str().unwrap_or("").to_lowercase();

    let filtered: Vec<&Value> = state
        .tasks
        .iter()
        .filter(|t| {
            let t_stage = t["stage"].as_str().unwrap_or("");
            let t_manual = t["manual"].as_i64().unwrap_or(0) != 0;
            let stage_ok = if stage_filter.is_empty() {
                true
            } else if stage_filter == "review" {
                is_review_available(t)
            } else if stage_filter == "error" {
                t_stage == "error" && !t_manual
            } else if stage_filter == "face" {
                matches!(t_stage, "face" | "face_queued")
            } else if stage_filter == "queued" {
                matches!(t_stage, "queued" | "retry")
            } else {
                t_stage == stage_filter
            };
            let batch_ok = batch_filter.is_empty() || t["batch_id"].as_str() == Some(batch_filter);
            let query_ok = query.is_empty() || {
                let text = format!(
                    "{} {}",
                    t["name"].as_str().unwrap_or(""),
                    t["path"].as_str().unwrap_or("")
                )
                .to_lowercase();
                text.contains(&query)
            };
            stage_ok && batch_ok && query_ok
        })
        .collect();

    let limit = 25usize;
    let total = filtered.len();
    let raw_page = body["page"]
        .as_str()
        .and_then(|s| s.parse::<f64>().ok())
        .or_else(|| body["page"].as_f64())
        .filter(|v| v.is_finite())
        .unwrap_or(0.0);
    let max_page = if total == 0 {
        0
    } else {
        total.div_ceil(limit) - 1
    };
    let page = (raw_page.max(0.0) as usize).min(max_page);
    let rows: Vec<Value> = filtered
        .into_iter()
        .skip(page * limit)
        .take(limit)
        .map(|t| {
            let mut copy = t.clone();
            let mut slim_data = serde_json::Map::new();
            for key in ["kind", "suggested", "face_skipped"] {
                if let Some(v) = t["data"].get(key) {
                    slim_data.insert(key.to_owned(), v.clone());
                }
            }
            copy["data"] = Value::Object(slim_data);
            copy
        })
        .collect();

    struct BatchSummary {
        id: String,
        total: usize,
        ready: usize,
        error: usize,
        running: usize,
    }
    let mut batches_map: BTreeMap<String, BatchSummary> = BTreeMap::new();
    for task in &state.tasks {
        let id = task["batch_id"].as_str().unwrap_or("").to_owned();
        let entry = batches_map.entry(id.clone()).or_insert(BatchSummary {
            id,
            total: 0,
            ready: 0,
            error: 0,
            running: 0,
        });
        entry.total += 1;
        entry.ready += usize::from(is_review_available(task));
        entry.error += usize::from(
            task["stage"].as_str() == Some("error") && task["manual"].as_i64().unwrap_or(0) == 0,
        );
        entry.running += usize::from(matches!(task["stage"].as_str(), Some("text" | "face")));
    }
    let mut batches: Vec<BatchSummary> = batches_map.into_values().collect();
    batches.sort_by(|a, b| b.id.cmp(&a.id));
    let batches_json: Vec<Value> = batches
        .into_iter()
        .take(20)
        .map(|b| {
            json!({
                "id": b.id,
                "total": b.total,
                "ready": b.ready,
                "error": b.error,
                "running": b.running,
            })
        })
        .collect();

    let mut out = json!({
        "online": state.online,
        "paused": state.paused,
        "last_discovery": state.last_discovery,
        "references": state.references,
        "events": state.events,
        "counts": counts,
        "total": total,
        "page": page,
        "limit": limit,
        "all_count": state.tasks.len(),
        "rows": rows,
        "batches": batches_json,
    });
    if has_vid {
        out["task"] = requested_vid
            .and_then(|vid| state.tasks.iter().find(|t| t["vid"].as_str() == Some(vid)))
            .cloned()
            .unwrap_or(Value::Null);
    }
    Ok(out)
}

pub fn dashboard(config: &Config, body: &Value) -> Result<Value> {
    let conn = open_existing(config, "处理账本尚未初始化，当前数量未知")?;
    let tx = conn.unchecked_transaction()?;
    let now = now_sec(&tx)?;
    let settings = settings_with_conn(config, &tx)?;
    let state = load_active_state(config, &tx, now)?;
    let beat = read_json_fallback(&config.base.join("processing/heartbeat.json"), json!({}));

    let queue = review::read(config, "integration-queue.json", json!([]))?.1;
    let history_doc = review::read(config, "archive-history.json", json!([]))?.1;
    let mut handled = BTreeSet::new();
    for row in queue
        .as_array()
        .into_iter()
        .flatten()
        .chain(history_doc.as_array().into_iter().flatten())
    {
        if let Some(vid) = row["vid"].as_str() {
            handled.insert(vid.to_owned());
        }
    }

    let tables: BTreeSet<String> =
        db::rows(&tx, "SELECT name FROM sqlite_master WHERE type='table'", [])?
            .into_iter()
            .filter_map(|r| r["name"].as_str().map(str::to_owned))
            .collect();
    let ledger_version = setting(&tx, "batch_ledger_version")?
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let ledger_ready = tables.contains("batch_items") && ledger_version >= 1;

    struct BatchAgg {
        id: String,
        started_at: f64,
        total: usize,
        completed: usize,
        running: usize,
        waiting: usize,
        error: usize,
        excluded: usize,
        text_done: usize,
        face_required: usize,
        face_done: usize,
        face_skipped: usize,
        legacy: bool,
        refreshing: usize,
        refresh_waiting: usize,
        refresh_error: usize,
    }
    let mut groups: BTreeMap<String, BatchAgg> = BTreeMap::new();
    if ledger_ready {
        let items = db::rows(
            &tx,
            "SELECT b.batch_id,b.vid,b.added_at,b.completed,b.text_done,b.face_required,b.face_done,b.face_skipped,b.excluded,b.legacy,
            CASE WHEN t.batch_id=b.batch_id AND t.path=b.path AND t.size=b.size AND t.mtime_ns=b.mtime_ns THEN 1 ELSE 0 END AS current,t.stage,t.manual
            FROM batch_items b LEFT JOIN tasks t ON t.vid=b.vid",
            [],
        )?;
        for raw in items {
            let item = plain_row(raw);
            let batch_id = item["batch_id"].as_str().unwrap_or("").to_owned();
            let vid = item["vid"].as_str().unwrap_or("");
            let added_at = item["added_at"]
                .as_f64()
                .or_else(|| item["added_at"].as_i64().map(|n| n as f64))
                .unwrap_or(0.0);
            let completed = item["completed"].as_i64().unwrap_or(0) != 0;
            let text_done = item["text_done"].as_i64().unwrap_or(0) != 0;
            let face_required = item["face_required"].as_i64();
            let face_done = item["face_done"].as_i64().unwrap_or(0) != 0;
            let face_skipped = item["face_skipped"].as_i64().unwrap_or(0) != 0;
            let excluded = item["excluded"].as_i64().unwrap_or(0) != 0;
            let legacy = item["legacy"].as_i64().unwrap_or(0) != 0;
            let current = item["current"].as_i64().unwrap_or(0) != 0;
            let stage = item["stage"].as_str();
            let manual = item["manual"].as_i64().unwrap_or(0) != 0;
            let is_handled = handled.contains(vid);

            let b = groups.entry(batch_id.clone()).or_insert(BatchAgg {
                id: batch_id,
                started_at: added_at,
                total: 0,
                completed: 0,
                running: 0,
                waiting: 0,
                error: 0,
                excluded: 0,
                text_done: 0,
                face_required: 0,
                face_done: 0,
                face_skipped: 0,
                legacy: false,
                refreshing: 0,
                refresh_waiting: 0,
                refresh_error: 0,
            });
            if added_at < b.started_at {
                b.started_at = added_at;
            }
            b.total += 1;
            if completed {
                b.completed += 1;
            } else if excluded || !current || is_handled || manual || stage == Some("excluded") {
                b.excluded += 1;
            } else if stage == Some("error") {
                b.error += 1;
            } else if matches!(stage, Some("text" | "face")) {
                b.running += 1;
            } else {
                b.waiting += 1;
            }
            b.text_done += usize::from(text_done);
            b.face_required += usize::from(face_required == Some(1));
            b.face_done += usize::from(face_done);
            b.face_skipped += usize::from(face_skipped);
            b.legacy |= legacy;
            if completed && current && !is_handled && !manual {
                if matches!(stage, Some("text" | "face")) {
                    b.refreshing += 1;
                }
                if matches!(stage, Some("queued" | "face_queued" | "retry")) {
                    b.refresh_waiting += 1;
                }
                if stage == Some("error") {
                    b.refresh_error += 1;
                }
            }
        }
    }

    let mut batch_list: Vec<BatchAgg> = groups.into_values().collect();
    batch_list.sort_by(|a, b| {
        b.started_at
            .total_cmp(&a.started_at)
            .then_with(|| b.id.cmp(&a.id))
    });

    let mut totals = json!({
        "total": 0,
        "completed": 0,
        "running": 0,
        "waiting": 0,
        "error": 0,
        "excluded": 0,
        "text_done": 0,
        "face_required": 0,
        "face_done": 0,
        "face_skipped": 0,
        "refreshing": 0,
        "refresh_waiting": 0,
        "refresh_error": 0,
    });
    let batches_json: Vec<Value> = batch_list
        .into_iter()
        .map(|b| {
            totals["total"] = json!(totals["total"].as_u64().unwrap() + b.total as u64);
            totals["completed"] = json!(totals["completed"].as_u64().unwrap() + b.completed as u64);
            totals["running"] = json!(totals["running"].as_u64().unwrap() + b.running as u64);
            totals["waiting"] = json!(totals["waiting"].as_u64().unwrap() + b.waiting as u64);
            totals["error"] = json!(totals["error"].as_u64().unwrap() + b.error as u64);
            totals["excluded"] = json!(totals["excluded"].as_u64().unwrap() + b.excluded as u64);
            totals["text_done"] = json!(totals["text_done"].as_u64().unwrap() + b.text_done as u64);
            totals["face_required"] =
                json!(totals["face_required"].as_u64().unwrap() + b.face_required as u64);
            totals["face_done"] = json!(totals["face_done"].as_u64().unwrap() + b.face_done as u64);
            totals["face_skipped"] =
                json!(totals["face_skipped"].as_u64().unwrap() + b.face_skipped as u64);
            totals["refreshing"] =
                json!(totals["refreshing"].as_u64().unwrap() + b.refreshing as u64);
            totals["refresh_waiting"] =
                json!(totals["refresh_waiting"].as_u64().unwrap() + b.refresh_waiting as u64);
            totals["refresh_error"] =
                json!(totals["refresh_error"].as_u64().unwrap() + b.refresh_error as u64);
            json!({
                "id": b.id,
                "started_at": b.started_at,
                "total": b.total,
                "completed": b.completed,
                "running": b.running,
                "waiting": b.waiting,
                "error": b.error,
                "excluded": b.excluded,
                "text_done": b.text_done,
                "face_required": b.face_required,
                "face_done": b.face_done,
                "face_skipped": b.face_skipped,
                "legacy": b.legacy,
                "refreshing": b.refreshing,
                "refresh_waiting": b.refresh_waiting,
                "refresh_error": b.refresh_error,
            })
        })
        .collect();

    let (active_runs, mut runs) = if tables.contains("processing_runs") {
        let active: Vec<Value> = db::rows(
            &tx,
            "SELECT * FROM processing_runs WHERE status='running' ORDER BY started_at,id",
            [],
        )?
        .into_iter()
        .map(plain_row)
        .collect();
        let recent: Vec<Value> = db::rows(
            &tx,
            "SELECT * FROM processing_runs ORDER BY started_at DESC,id DESC LIMIT 24",
            [],
        )?
        .into_iter()
        .map(plain_row)
        .collect();
        (active, recent)
    } else {
        (Vec::new(), Vec::new())
    };

    let (history, history_source) = if !runs.is_empty() {
        runs.reverse();
        let items: Vec<Value> = runs
            .into_iter()
            .map(|r| {
                let execution = r["execution"]
                    .as_str()
                    .and_then(|s| serde_json::from_str::<Value>(s).ok())
                    .unwrap_or(Value::Null);
                json!({
                    "id": r["id"],
                    "label": r["name"],
                    "status": r["status"],
                    "stage": r["stage"],
                    "at": r["started_at"],
                    "finished_at": r["finished_at"],
                    "error": r["error"],
                    "batch": r["batch_id"],
                    "execution": execution,
                })
            })
            .collect();
        (items, "stage_runs")
    } else {
        let mut legacy_events: Vec<Value> = db::rows(
            &tx,
            "SELECT e.*,t.name FROM events e LEFT JOIN tasks t ON t.vid=e.vid WHERE kind IN ('ready','stage_complete','retry','error') ORDER BY e.id DESC LIMIT 24",
            [],
        )?
        .into_iter()
        .map(plain_row)
        .collect();
        legacy_events.reverse();
        let items: Vec<Value> = legacy_events
            .into_iter()
            .map(|e| {
                let kind = e["kind"].as_str().unwrap_or("");
                let status = if matches!(kind, "ready" | "stage_complete") {
                    "done"
                } else {
                    kind
                };
                let stage = if kind == "stage_complete" {
                    "text"
                } else {
                    "unknown"
                };
                let label = e["name"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .or_else(|| e["vid"].as_str().filter(|s| !s.is_empty()))
                    .unwrap_or("处理记录");
                let error = if matches!(kind, "error" | "retry") {
                    e["message"].clone()
                } else {
                    Value::Null
                };
                json!({
                    "id": format!("event-{}", e["id"].as_i64().unwrap_or(0)),
                    "label": label,
                    "status": status,
                    "stage": stage,
                    "at": e["at"],
                    "finished_at": e["at"],
                    "error": error,
                    "batch": Value::Null,
                    "execution": Value::Null,
                })
            })
            .collect();
        (items, "legacy_events")
    };

    let media_issue = setting(&tx, "media_unavailable")?.unwrap_or(Value::Null);
    tx.commit()?;

    let current = json!({
        "running": state.tasks.iter().filter(|t| matches!(t["stage"].as_str(), Some("text" | "face"))).count(),
        "waiting": state.tasks.iter().filter(|t| matches!(t["stage"].as_str(), Some("queued" | "face_queued" | "retry"))).count(),
        "error": state.tasks.iter().filter(|t| t["stage"].as_str() == Some("error") && t["manual"].as_i64().unwrap_or(0) == 0).count(),
        "review": state.tasks.iter().filter(|t| is_review_available(t)).count(),
    });

    let mut jobs: Vec<Value> = active_runs
        .iter()
        .map(|r| read_run_progress(config, r, state.online))
        .collect();
    if jobs.is_empty() {
        let heartbeat_at = beat.get("at").cloned().unwrap_or(Value::Null);
        for t in state
            .tasks
            .iter()
            .filter(|t| matches!(t["stage"].as_str(), Some("text" | "face")))
        {
            let vid = t["vid"].as_str().unwrap_or("");
            let is_face = t["stage"].as_str() == Some("face");
            jobs.push(json!({
                "id": vid,
                "kind": if is_face { "face" } else { "text" },
                "title": if is_face { "人脸分析" } else { "提取水印" },
                "status": if state.online { "running" } else { "offline" },
                "stage": "处理中",
                "current": t["name"],
                "vid": vid,
                "done": Value::Null,
                "total": Value::Null,
                "unit": "",
                "failed": 0,
                "started_at": t["updated_at"],
                "updated_at": t["updated_at"],
                "heartbeat_at": heartbeat_at,
                "href": format!("/operations/tasks?vid={}", review::percent(vid)),
            }));
        }
    }

    let inference = if let Some(inf) = beat["inference"].as_object() {
        let mut copy = inf.clone();
        let avail = state.online && inf.get("available").and_then(Value::as_bool) == Some(true);
        copy.insert("available".into(), json!(avail));
        Value::Object(copy)
    } else {
        json!({
            "backend": "unknown",
            "available": false,
            "name": "处理来源未知",
            "error": "后台未提供推理来源",
        })
    };

    let preview = body["preview"].as_bool().unwrap_or(false);
    let mut out = json!({
        "schema_version": 1,
        "preview": preview,
        "at": now,
        "online": state.online,
        "paused": state.paused,
        "ledger_ready": ledger_ready,
        "batches": batches_json,
        "totals": totals,
        "current": current,
        "jobs": jobs,
        "history": history,
        "history_source": history_source,
        "settings": settings,
        "media_issue": media_issue,
        "discovery": state.last_discovery,
        "references": state.references,
        "inference": inference,
        "events": state.events,
    });
    if preview && let Some(preview_id) = body["preview_id"].as_str() {
        out["preview_id"] = json!(preview_id);
    }
    Ok(out)
}

fn normalize_home_job(mut job: Value, now: f64, heartbeat: Option<f64>) -> Value {
    let active = job["status"].as_str() == Some("running");
    let online = heartbeat.is_some_and(|hb| hb <= now + 30.0 && now - hb < 90.0);
    job["heartbeat_at"] = heartbeat.map(|h| json!(h)).unwrap_or(Value::Null);
    if active && !online {
        job["status"] = json!("offline");
    }
    let valid_num = |v: &Value| {
        v.as_f64()
            .filter(|n| n.is_finite() && *n >= 0.0)
            .map(|_| v.clone())
            .unwrap_or(Value::Null)
    };
    job["done"] = valid_num(&job["done"]);
    job["total"] = valid_num(&job["total"]);
    job
}

fn reconcile_processing_job(mut job: Value, owner: Option<&Value>, online: bool) -> Value {
    let kind = job["kind"].as_str().unwrap_or("").to_owned();
    if !online
        || job["status"].as_str() != Some("running")
        || !matches!(kind.as_str(), "text" | "face")
        || owner.and_then(|o| o["stage"].as_str()) == Some(kind.as_str())
    {
        return job;
    }
    if let Some(fin) = owner
        .and_then(|o| o.get("updated_at"))
        .filter(|v| !v.is_null())
        .or_else(|| job.get("updated_at").filter(|v| !v.is_null()))
        .cloned()
    {
        job["finished_at"] = fin;
    }
    if let Some(o) = owner {
        let o_stage = o["stage"].as_str().unwrap_or("");
        if matches!(o_stage, "retry" | "error") {
            job["status"] = json!("error");
            job["stage"] = json!(if o_stage == "retry" {
                "等待重试"
            } else {
                "处理失败"
            });
            if !o["error"].is_null() {
                job["error"] = o["error"].clone();
            }
            return job;
        }
        if o_stage == "review" || (kind == "text" && o_stage == "face_queued") {
            job["status"] = json!("complete");
            job["stage"] = json!("阶段完成");
            return job;
        }
    }
    job["status"] = json!("waiting");
    job["stage"] = json!("已离开当前阶段");
    job
}

pub fn home_tasks(config: &Config, body: &Value) -> Result<Value> {
    let now = now_wall()?;
    let beat_doc = read_json_fallback(&config.base.join("processing/heartbeat.json"), json!({}));
    let heartbeat = beat_doc["at"]
        .as_f64()
        .or_else(|| beat_doc["at"].as_i64().map(|n| n as f64))
        .filter(|v| v.is_finite());
    let online = heartbeat.is_some_and(|hb| (-30.0..90.0).contains(&(now - hb)));
    let initialized = db_path(config).exists();

    let mut jobs: Vec<Value> = Vec::new();
    let mut counts = serde_json::Map::new();
    let mut stage_runs: Vec<Value> = Vec::new();
    let mut parallel_progress = false;
    let mut tasks: Vec<Value> = Vec::new();
    let mut paused = false;
    let mut events: Vec<Value> = Vec::new();

    if initialized {
        let conn = open_existing(config, "分析服务尚未初始化")?;
        paused = setting(&conn, "paused")?
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let queue = review::read(config, "integration-queue.json", json!([]))?.1;
        let history = review::read(config, "archive-history.json", json!([]))?.1;
        let mut handled = BTreeSet::new();
        for row in queue
            .as_array()
            .into_iter()
            .flatten()
            .chain(history.as_array().into_iter().flatten())
        {
            if let Some(vid) = row["vid"].as_str() {
                handled.insert(vid.to_owned());
            }
        }
        let raw_tasks = db::rows(
            &conn,
            "SELECT vid,name,path,stage,batch_id,updated_at,priority,error,manual,next_retry FROM tasks WHERE stage!='excluded' ORDER BY priority DESC,is_new DESC,created_at DESC,vid",
            [],
        )?;
        for raw in raw_tasks {
            let row = plain_row(raw);
            let Some(vid) = row["vid"].as_str() else {
                continue;
            };
            if handled.contains(vid) {
                continue;
            }
            let key = if row["manual"].as_i64().unwrap_or(0) != 0 {
                "review"
            } else {
                row["stage"].as_str().unwrap_or("")
            };
            let next = counts.get(key).and_then(Value::as_u64).unwrap_or(0) + 1;
            counts.insert(key.to_owned(), json!(next));
            tasks.push(row);
        }
        parallel_progress = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='processing_runs')",
            [],
            |r| r.get(0),
        )?;
        if parallel_progress {
            stage_runs = db::rows(
                &conn,
                "SELECT * FROM processing_runs WHERE status='running' ORDER BY started_at",
                [],
            )?
            .into_iter()
            .map(plain_row)
            .collect();
            for kind in ["text", "face"] {
                if !stage_runs.iter().any(|r| r["stage"].as_str() == Some(kind))
                    && let Some(latest) = db::rows(
                        &conn,
                        "SELECT * FROM processing_runs WHERE stage=? ORDER BY started_at DESC LIMIT 1",
                        [kind],
                    )?
                    .into_iter()
                    .next()
                {
                    stage_runs.push(plain_row(latest));
                }
            }
        }
        events = db::rows(
            &conn,
            "SELECT id,at,kind,message FROM events ORDER BY id DESC LIMIT 10",
            [],
        )?
        .into_iter()
        .map(plain_row)
        .collect();
    }

    if parallel_progress {
        for run in &stage_runs {
            jobs.push(read_run_progress(config, run, online));
        }
    }

    for kind in ["text", "face", "discovery", "archive", "watermark"] {
        if parallel_progress && matches!(kind, "text" | "face") {
            continue;
        }
        let raw = read_json_fallback(
            &config.base.join("jobs").join(format!("{kind}.json")),
            Value::Null,
        );
        let finished_ok = !raw["finished_at"]
            .as_f64()
            .filter(|v| *v != 0.0)
            .is_some_and(|f| now - f >= 86400.0);
        if raw.is_object()
            && raw["kind"].as_str() == Some(kind)
            && raw["id"].as_str().is_some_and(|s| !s.is_empty())
            && finished_ok
        {
            let beat = if kind == "archive" {
                raw["heartbeat_at"]
                    .as_f64()
                    .or_else(|| raw["heartbeat_at"].as_i64().map(|n| n as f64))
                    .filter(|v| v.is_finite())
            } else if kind == "watermark" {
                let worker =
                    read_json_fallback(&config.base.join("watermarks/worker.json"), json!({}));
                worker["at"]
                    .as_str()
                    .and_then(|s| review::timestamp(s).ok())
                    .filter(|v| v.is_finite())
            } else {
                heartbeat
            };
            let owner = raw["vid"]
                .as_str()
                .and_then(|vid| tasks.iter().find(|t| t["vid"].as_str() == Some(vid)));
            let reconciled = reconcile_processing_job(raw, owner, online);
            jobs.push(normalize_home_job(reconciled, now, beat));
        }
    }

    if !jobs.iter().any(|j| j["kind"].as_str() == Some("watermark")) {
        let request = read_json_fallback(&config.base.join("watermarks/job.json"), Value::Null);
        let progress_path = config.base.join("watermarks/progress.json");
        let progress = read_json_fallback(&progress_path, Value::Null);
        if request.is_object()
            && !request["id"].is_null()
            && progress["job_id"] == request["id"]
            && progress["status"].as_str() == Some("running")
        {
            let worker = read_json_fallback(&config.base.join("watermarks/worker.json"), json!({}));
            let beat = worker["at"]
                .as_str()
                .and_then(|s| review::timestamp(s).ok())
                .filter(|v| v.is_finite());
            let started_at = request["created_at"]
                .as_str()
                .and_then(|s| review::timestamp(s).ok())
                .filter(|v| v.is_finite())
                .map(|v| json!(v))
                .unwrap_or(Value::Null);
            let updated_at = fs::metadata(&progress_path)
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| json!(d.as_secs_f64()))
                .unwrap_or(Value::Null);
            let stage_str = progress["stage"]
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or("提取水印");
            let wm_job = json!({
                "id": request["id"],
                "kind": "watermark",
                "title": "水印批次",
                "status": "running",
                "stage": stage_str,
                "done": progress.get("processed").cloned().unwrap_or(Value::Null),
                "total": progress.get("total").cloned().unwrap_or(Value::Null),
                "unit": "个视频",
                "failed": progress["failed"].as_i64().unwrap_or(0),
                "started_at": started_at,
                "updated_at": updated_at,
                "heartbeat_at": beat.map(|b| json!(b)).unwrap_or(Value::Null),
                "href": "/watermarks",
            });
            jobs.push(normalize_home_job(wm_job, now, beat));
        }
    }

    for kind in ["text", "face"] {
        let current = tasks.iter().find(|t| t["stage"].as_str() == Some(kind));
        let old_idx = jobs.iter().position(|j| j["kind"].as_str() == Some(kind));
        if parallel_progress && old_idx.is_some() {
            continue;
        }
        let should_replace = if let Some(curr) = current {
            match old_idx.map(|i| &jobs[i]) {
                None => true,
                Some(old) => {
                    old["vid"] != curr["vid"]
                        || matches!(old["status"].as_str(), Some("complete" | "error"))
                }
            }
        } else {
            false
        };
        if let (true, Some(curr)) = (should_replace, current) {
            if let Some(i) = old_idx {
                jobs.remove(i);
            }
            let vid = curr["vid"].as_str().unwrap_or("");
            jobs.push(json!({
                "id": format!("{kind}:{vid}"),
                "kind": kind,
                "title": if kind == "text" { "提取水印" } else { "人脸分析" },
                "status": if online { "running" } else { "offline" },
                "stage": if kind == "text" { "提取文字线索" } else { "分析人脸" },
                "current": curr["name"],
                "vid": vid,
                "done": Value::Null,
                "total": Value::Null,
                "unit": "",
                "failed": 0,
                "started_at": curr["updated_at"],
                "updated_at": curr["updated_at"],
                "heartbeat_at": heartbeat.map(|h| json!(h)).unwrap_or(Value::Null),
                "href": format!("/review?vid={}", review::percent(vid)),
            }));
        } else if old_idx.is_none() {
            jobs.push(json!({
                "id": kind,
                "kind": kind,
                "title": if kind == "text" { "提取水印" } else { "人脸分析" },
                "status": if !online { "offline" } else if paused { "paused" } else { "waiting" },
                "stage": if paused {
                    "已暂停领取新任务"
                } else if kind == "text" {
                    "等待视频进入队列"
                } else {
                    "等待文字线索完成"
                },
                "done": Value::Null,
                "total": Value::Null,
                "unit": "",
                "failed": 0,
                "started_at": Value::Null,
                "updated_at": Value::Null,
                "heartbeat_at": heartbeat.map(|h| json!(h)).unwrap_or(Value::Null),
                "href": "/#task-queue",
            }));
        }
    }

    let stage_filter = body["stage"].as_str().unwrap_or("");
    let q = body["q"].as_str().unwrap_or("").trim().to_lowercase();
    let filtered: Vec<Value> = tasks
        .into_iter()
        .filter(|t| {
            let actual = if t["manual"].as_i64().unwrap_or(0) != 0 {
                "review"
            } else {
                t["stage"].as_str().unwrap_or("")
            };
            let stage_ok = if stage_filter.is_empty() {
                true
            } else if stage_filter == "queued" {
                matches!(actual, "queued" | "retry")
            } else if stage_filter == "face" {
                matches!(actual, "face" | "face_queued")
            } else {
                actual == stage_filter
            };
            let q_ok = q.is_empty() || {
                let text = format!(
                    "{} {}",
                    t["name"].as_str().unwrap_or(""),
                    t["path"].as_str().unwrap_or("")
                )
                .to_lowercase();
                text.contains(&q)
            };
            stage_ok && q_ok
        })
        .collect();

    let limit = 20usize;
    let total = filtered.len();
    let raw_page = body["page"]
        .as_str()
        .and_then(|s| s.parse::<f64>().ok())
        .or_else(|| body["page"].as_f64())
        .filter(|v| v.is_finite())
        .unwrap_or(0.0);
    let max_page = if total == 0 {
        0
    } else {
        total.div_ceil(limit) - 1
    };
    let page = (raw_page.floor().max(0.0) as usize).min(max_page);
    let rows: Vec<Value> = filtered
        .into_iter()
        .skip(page * limit)
        .take(limit)
        .collect();

    let kind_rank = |k: &str| match k {
        "face" => 0,
        "text" => 1,
        "discovery" => 2,
        "archive" => 3,
        "watermark" => 4,
        _ => 5,
    };
    jobs.sort_by(|a, b| {
        let a_run = a["status"].as_str() == Some("running");
        let b_run = b["status"].as_str() == Some("running");
        let a_err = a["status"].as_str() == Some("error");
        let b_err = b["status"].as_str() == Some("error");
        b_run
            .cmp(&a_run)
            .then_with(|| b_err.cmp(&a_err))
            .then_with(|| {
                kind_rank(a["kind"].as_str().unwrap_or(""))
                    .cmp(&kind_rank(b["kind"].as_str().unwrap_or("")))
            })
    });

    Ok(json!({
        "at": now,
        "initialized": initialized,
        "online": online,
        "paused": paused,
        "counts": counts,
        "jobs": jobs,
        "rows": rows,
        "total": total,
        "page": page,
        "limit": limit,
        "events": events,
    }))
}

fn value_as_id_str(v: &Value) -> Option<String> {
    if let Some(s) = v.as_str() {
        Some(s.to_owned())
    } else if let Some(n) = v.as_i64() {
        Some(n.to_string())
    } else {
        v.as_u64().map(|n| n.to_string())
    }
}

fn safe_video_sources(config: &Config) -> BTreeMap<String, String> {
    let mut sources = BTreeMap::new();
    let faces_path = config.base.join("faces.db");
    if faces_path.exists()
        && let Ok(fconn) = db::open(&faces_path)
        && let Ok(rows) = db::rows(&fconn, "SELECT vid,path FROM videos ORDER BY vid", [])
    {
        for row in rows {
            if let (Some(id), Some(p)) = (row["vid"].as_str(), row["path"].as_str()) {
                sources.insert(id.to_owned(), p.to_owned());
            }
        }
    }
    if let Ok((_, overrides)) = review::read(config, "video-path-overrides.json", json!({}))
        && let Some(obj) = overrides.as_object()
    {
        for (id, p) in obj {
            if let Some(s) = p.as_str() {
                sources.insert(id.clone(), s.to_owned());
            }
        }
    }
    let jobs_path = db_path(config);
    if jobs_path.exists()
        && let Ok(jconn) = db::open(&jobs_path)
        && let Ok(rows) = db::rows(&jconn, "SELECT vid,path FROM tasks ORDER BY vid", [])
    {
        for row in rows {
            if let (Some(id), Some(p)) = (row["vid"].as_str(), row["path"].as_str()) {
                sources.entry(id.to_owned()).or_insert_with(|| p.to_owned());
            }
        }
    }
    sources
}

pub fn verify_attempt_artifact(config: &Config, body: &Value) -> Result<Value> {
    let deny = || Ok(json!({"ok": true, "accepted": false, "valid": false, "file": Value::Null}));
    let Some(attempt) = body["attempt"].as_str() else {
        return deny();
    };
    let relative_owned = if let Some(rel) = body["relative"].as_str().filter(|s| !s.is_empty()) {
        rel.to_owned()
    } else if let (Some(folder), Some(name)) = (
        body["folder"].as_str().filter(|s| !s.is_empty()),
        body["name"].as_str().filter(|s| !s.is_empty()),
    ) {
        if folder == "watermarks" {
            format!("watermarks/evidence/{name}")
        } else {
            format!("{folder}/{name}")
        }
    } else {
        return deny();
    };
    let relative = relative_owned.as_str();
    if crate::model::validate_id(attempt).is_err() {
        return deny();
    }
    if Path::new(relative)
        .components()
        .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return deny();
    }
    let Ok(base_real) = fs::canonicalize(&config.base) else {
        return deny();
    };
    let directory = base_real.join("processing/attempts").join(attempt);
    let db_file = db_path(config);
    if !db_file.exists() {
        return deny();
    }
    let Ok(conn) = db::open(&db_file) else {
        return deny();
    };
    let has_table: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='_face_admin_processing_results')",
            [],
            |r| r.get(0),
        )
        .unwrap_or(false);
    if !has_table {
        return deny();
    }
    let Ok(Some(raw)) = conn
        .query_row(
            "SELECT result FROM _face_admin_processing_results WHERE id=?",
            [attempt],
            |r| r.get::<_, String>(0),
        )
        .optional()
    else {
        return deny();
    };
    let Ok(receipt) = serde_json::from_str::<Value>(&raw) else {
        return deny();
    };
    let task = &receipt["task"];
    if receipt["ok"] != true
        || receipt["accepted"] != true
        || task["run_id"].as_str() != Some(attempt)
    {
        return deny();
    }
    let vid = receipt["vid"]
        .as_str()
        .or_else(|| task["vid"].as_str())
        .unwrap_or("");
    let stage = receipt["stage"]
        .as_str()
        .or_else(|| task["stage"].as_str())
        .unwrap_or("");
    let file = directory.join(relative);

    // Ensure no newer accepted receipt for the same (vid, stage) has superseded this attempt
    // without retaining a reference to `file`.
    if !vid.is_empty()
        && let Ok(rows) = db::rows(
            &conn,
            "SELECT id,result FROM _face_admin_processing_results ORDER BY rowid DESC",
            [],
        )
    {
        for row in rows {
            let Some(other_id) = row["id"].as_str() else {
                continue;
            };
            if other_id == attempt {
                break;
            }
            let Some(other_raw) = row["result"].as_str() else {
                continue;
            };
            let Ok(other) = serde_json::from_str::<Value>(other_raw) else {
                continue;
            };
            if other["ok"] != true || other["accepted"] != true {
                continue;
            }
            let other_vid = other["vid"]
                .as_str()
                .or_else(|| other["task"]["vid"].as_str())
                .unwrap_or("");
            let other_stage = other["stage"]
                .as_str()
                .or_else(|| other["task"]["stage"].as_str())
                .unwrap_or("");
            if other_vid == vid && (stage.is_empty() || other_stage == stage) {
                let still_referenced = other["data"]["face"]["thumbs"]
                    .as_array()
                    .is_some_and(|arr| {
                        arr.iter().any(|v| {
                            v.as_str().is_some_and(|s| {
                                Path::new(s) == file
                                    || fs::canonicalize(s).ok().as_deref() == Some(file.as_path())
                            })
                        })
                    })
                    || other["data"]["watermark"]["artifact_attempt"].as_str() == Some(attempt);
                if !still_referenced {
                    return deny();
                }
                break;
            }
        }
    }

    let sources = safe_video_sources(config);
    let resolved_source = review::source(config, vid, &sources)
        .or_else(|| task["path"].as_str().map(str::to_owned))
        .unwrap_or_default();
    let Ok(current_source) = fs::canonicalize(&resolved_source) else {
        return deny();
    };
    let Ok(source_stat) = fs::metadata(&current_source) else {
        return deny();
    };
    let current_id = files::identity(&source_stat);
    let identity = &receipt["source_identity"];
    let Some(expected_dev) = value_as_id_str(&identity["dev"]) else {
        return deny();
    };
    let Some(expected_ino) = value_as_id_str(&identity["ino"]) else {
        return deny();
    };
    let Some(expected_size) = review::number(&task["size"]) else {
        return deny();
    };
    let Some(expected_mtime) = task["mtime_ns"].as_str() else {
        return deny();
    };
    if current_id["dev"].as_str() != Some(expected_dev.as_str())
        || current_id["ino"].as_str() != Some(expected_ino.as_str())
        || i64::try_from(source_stat.len()).ok() != Some(expected_size)
        || files::mtime_ns(&source_stat) != expected_mtime
    {
        return deny();
    }
    let face = &receipt["data"]["face"];
    let watermark = &receipt["data"]["watermark"];
    let thumb_ref = face["thumbs"].as_array().is_some_and(|arr| {
        arr.iter().any(|v| {
            v.as_str().is_some_and(|s| {
                Path::new(s) == file
                    || fs::canonicalize(s).ok().as_deref() == Some(file.as_path())
            })
        })
    });
    let watermark_ref = watermark["artifact_attempt"].as_str() == Some(attempt)
        && watermark["candidates"].as_array().is_some_and(|arr| {
            arr.iter().any(|c| {
                c["evidence"].as_str().is_some_and(|ev| {
                    Path::new("watermarks/evidence").join(ev) == Path::new(relative)
                })
            })
        });
    if !(thumb_ref || watermark_ref)
        || !file.starts_with(&directory)
        || fs::canonicalize(&file).ok().as_deref() != Some(file.as_path())
        || !fs::metadata(&file).is_ok_and(|m| m.is_file())
    {
        return deny();
    }
    Ok(json!({
        "ok": true,
        "accepted": true,
        "valid": true,
        "file": file.to_string_lossy(),
    }))
}

fn load_active_face_groups(config: &Config, face_version: &str) -> Result<Vec<Value>> {
    let db_file = db_path(config);
    if !db_file.exists() {
        return Ok(Vec::new());
    }
    let conn = db::open(&db_file)?;
    let has_table: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='face_groups')",
            [],
            |r| r.get(0),
        )
        .unwrap_or(false);
    if !has_table {
        return Ok(Vec::new());
    }
    let mut handled = BTreeSet::new();
    for name in ["integration-queue.json", "archive-history.json"] {
        if let Ok((_, doc)) = review::read(config, name, json!([]))
            && let Some(arr) = doc.as_array()
        {
            for r in arr {
                if let Some(id) = r["vid"].as_str() {
                    handled.insert(id.to_owned());
                }
            }
        }
    }
    let mut active = BTreeSet::new();
    if let Ok(rows) = db::rows(
        &conn,
        "SELECT vid,data FROM tasks WHERE stage IN ('review','face') AND manual=0 AND json_extract(data,'$.kind')='none'",
        [],
    ) {
        for row in rows {
            if !row["vid"].as_str().is_some_and(|id| handled.contains(id))
                && let Ok(data) = review::task_data(&row)
                && let Some(id) = data["face"]["unknown_group"].as_str()
            {
                active.insert(id.to_owned());
            }
        }
    }
    let mut out = Vec::new();
    if let Ok(rows) = db::rows(
        &conn,
        "SELECT * FROM face_groups WHERE version=? ORDER BY id",
        [face_version],
    ) {
        for row in rows {
            let Some(id) = row["id"].as_str() else {
                continue;
            };
            if !active.contains(id) {
                continue;
            }
            let Ok(bytes) = serde_json::from_value::<Vec<u8>>(row["embedding"]["$blob"].clone())
            else {
                continue;
            };
            if bytes.len() != 2048 {
                continue;
            }
            let floats: Vec<f32> = bytes
                .chunks_exact(4)
                .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
                .collect();
            if floats.iter().any(|x| !x.is_finite()) {
                continue;
            }
            out.push(json!({
                "id": id,
                "version": face_version,
                "embedding": floats,
            }));
        }
    }
    Ok(out)
}

fn validate_task_source(config: &Config, path: &str) -> Option<fs::Metadata> {
    let p = Path::new(path);
    if p.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return None;
    }
    let sym = fs::symlink_metadata(p).ok()?;
    if sym.file_type().is_symlink() || !sym.is_file() {
        return None;
    }
    let real = fs::canonicalize(p).ok()?;
    let real_root = fs::canonicalize(&config.media).ok()?;
    let creators_root = fs::canonicalize(config.creators())
        .unwrap_or_else(|_| real_root.join("pikpak/creators"));
    if !real.starts_with(&real_root)
        || real.starts_with(&creators_root)
        || real
            .components()
            .any(|c| c.as_os_str() == ".face-cleanup-staging")
    {
        return None;
    }
    fs::metadata(&real).ok()
}

pub fn cache_check(engine: &Engine, body: &Value) -> Result<Value> {
    let config = &engine.config;
    let stage = body["stage"].as_str().unwrap_or("text");
    let vid = body["vid"].as_str().unwrap_or("");
    let path = body["path"].as_str().unwrap_or("");
    let size = review::number(&body["size"]).unwrap_or(-1);
    let mtime_ns = body["mtime_ns"].as_str().unwrap_or("");
    let pipeline = body["pipeline"].as_str().unwrap_or(DEFAULT_PIPELINE);
    let face_version = body["face_version"]
        .as_str()
        .unwrap_or(DEFAULT_FACE_VERSION);

    let Some(stat) = validate_task_source(config, path) else {
        return Ok(json!({
            "ok": true,
            "hit": false,
            "text_valid": false,
            "face_feature_valid": false,
            "stage": stage,
            "reason": "source_missing_or_unsafe",
        }));
    };
    if i64::try_from(stat.len()).ok() != Some(size) || files::mtime_ns(&stat) != mtime_ns {
        return Ok(json!({
            "ok": true,
            "hit": false,
            "text_valid": false,
            "face_feature_valid": false,
            "stage": stage,
            "reason": "source_fingerprint_changed",
        }));
    }
    let current_identity = files::identity(&stat);

    let mut text_valid = false;
    let mut text_cached = Value::Null;
    let results = review::read(config, "watermarks/results.json", json!({}))?.1;
    let existing = &results[vid];
    if existing.is_object()
        && existing["status"] == "done"
        && existing["pipeline"].as_str() == Some(pipeline)
        && existing["path"].as_str() == Some(path)
        && review::number(&existing["size"]) == Some(size)
        && existing["mtime_ns"].as_str() == Some(mtime_ns)
        && existing["candidates"].is_array()
    {
        let mut attempt_ok = true;
        if let Some(attempt) = existing["artifact_attempt"].as_str() {
            if crate::model::validate_id(attempt).is_err() {
                attempt_ok = false;
            } else {
                let db_file = db_path(config);
                if db_file.exists()
                    && let Ok(conn) = db::open(&db_file)
                    && conn
                        .query_row(
                            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='_face_admin_processing_results')",
                            [],
                            |r| r.get::<_, bool>(0),
                        )
                        .unwrap_or(false)
                {
                    let receipt_ok = conn
                        .query_row(
                            "SELECT result FROM _face_admin_processing_results WHERE id=?",
                            [attempt],
                            |r| r.get::<_, String>(0),
                        )
                        .optional()
                        .ok()
                        .flatten()
                        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
                        .is_some_and(|rec| {
                            let id = &rec["source_identity"];
                            rec["ok"] == true
                                && rec["accepted"] == true
                                && value_as_id_str(&id["dev"]).as_deref()
                                    == current_identity["dev"].as_str()
                                && value_as_id_str(&id["ino"]).as_deref()
                                    == current_identity["ino"].as_str()
                        });
                    if !receipt_ok {
                        attempt_ok = false;
                    }
                }
            }
        }
        if attempt_ok {
            for cand in existing["candidates"].as_array().into_iter().flatten() {
                if let Some(ev) = cand["evidence"].as_str() {
                    let ev_path = if let Some(attempt) = existing["artifact_attempt"].as_str() {
                        config
                            .base
                            .join("processing/attempts")
                            .join(attempt)
                            .join("watermarks/evidence")
                            .join(ev)
                    } else {
                        config.base.join("watermarks/evidence").join(ev)
                    };
                    if fs::canonicalize(&ev_path).ok().as_deref() != Some(ev_path.as_path())
                        || !fs::metadata(&ev_path).is_ok_and(|m| m.is_file())
                    {
                        attempt_ok = false;
                        break;
                    }
                }
            }
        }
        if attempt_ok {
            text_valid = true;
            text_cached = existing.clone();
        }
    }

    let active_groups = load_active_face_groups(config, face_version).unwrap_or_default();
    let mut face_feature_valid = false;
    let mut face_feature_cache = Value::Null;
    let mut face_feature_sha = Value::Null;
    let db_file = db_path(config);
    if db_file.exists()
        && let Ok(conn) = db::open(&db_file)
    {
        let has_results: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='_face_admin_processing_results')",
                [],
                |r| r.get(0),
            )
            .unwrap_or(false);
        if has_results
            && let Ok(rows) = db::rows(
                &conn,
                "SELECT result FROM _face_admin_processing_results WHERE json_extract(result,'$.vid')=? OR json_extract(result,'$.task.vid')=? ORDER BY rowid DESC",
                [vid, vid],
            )
        {
            for row in rows {
                let Some(raw) = row["result"].as_str() else {
                    continue;
                };
                let Ok(rec) = serde_json::from_str::<Value>(raw) else {
                    continue;
                };
                if rec["ok"] != true || rec["accepted"] != true {
                    continue;
                }
                let face = &rec["data"]["face"];
                if !face.is_object() || face["version"].as_str() != Some(face_version) {
                    continue;
                }
                if let Some(expected_hashes) = body.get("model_hashes").filter(|v| v.is_object())
                    && face["execution"]["model_hashes"].is_object()
                    && &face["execution"]["model_hashes"] != expected_hashes
                {
                    continue;
                }
                let Some(cache_str) = face["feature_cache"].as_str().filter(|s| !s.is_empty()) else {
                    continue;
                };
                let Some(expected_sha) = face["feature_cache_sha256"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                else {
                    continue;
                };
                let cache_path = Path::new(cache_str);
                let attempts_root = config.base.join("processing/attempts");
                let real_attempts = fs::canonicalize(&attempts_root).unwrap_or(attempts_root);
                let Ok(real_cache) = fs::canonicalize(cache_path) else {
                    continue;
                };
                if !real_cache.starts_with(&real_attempts)
                    || crate::processing_result::artifact_hash(&real_cache)
                        .ok()
                        .as_deref()
                        != Some(expected_sha)
                {
                    continue;
                }
                let thumbs_ok = face["thumbs"].as_array().is_none_or(|arr| {
                    arr.iter().all(|t| {
                        t.as_str().is_some_and(|tp| {
                            fs::canonicalize(tp)
                                .ok()
                                .is_some_and(|rp| rp.starts_with(&real_attempts) && rp.is_file())
                        })
                    })
                });
                if !thumbs_ok {
                    continue;
                }
                let rec_id = face
                    .get("source_identity")
                    .filter(|v| v.is_object())
                    .unwrap_or(&rec["source_identity"]);
                let rec_dev = value_as_id_str(&rec_id["dev"]);
                let rec_ino = value_as_id_str(&rec_id["ino"]);
                let same_origin = rec["task"]["path"].as_str() == Some(path)
                    && review::number(&rec["task"]["size"]) == Some(size)
                    && rec["task"]["mtime_ns"].as_str() == Some(mtime_ns)
                    && rec_dev.as_deref() == current_identity["dev"].as_str()
                    && rec_ino.as_deref() == current_identity["ino"].as_str();
                let relocated_ok = if same_origin {
                    true
                } else if let (Some(dev), Some(ino)) = (rec_dev, rec_ino) {
                    let origin = json!({
                        "path": rec["task"]["path"],
                        "size": review::number(&rec["task"]["size"]).unwrap_or(0).to_string(),
                        "mtime_ns": rec["task"]["mtime_ns"],
                        "dev": dev,
                        "ino": ino,
                    });
                    crate::reference_relocation::verify(
                        engine,
                        &json!({
                            "vid": vid,
                            "source": path,
                            "cache": real_cache.to_string_lossy(),
                            "cache_sha256": expected_sha,
                            "origin": origin,
                            "current_identity": current_identity,
                        }),
                    )
                    .is_ok_and(|p| p["verified"] == true)
                } else {
                    false
                };
                if !relocated_ok {
                    continue;
                }
                face_feature_valid = true;
                face_feature_cache = json!(real_cache.to_string_lossy());
                face_feature_sha = json!(expected_sha);
                break;
            }
        }
    }

    let hit = if stage == "face" {
        face_feature_valid
    } else {
        text_valid
    };
    Ok(json!({
        "ok": true,
        "hit": hit,
        "stage": stage,
        "text_valid": text_valid,
        "text_cached": text_cached,
        "watermark": text_cached,
        "face_feature_valid": face_feature_valid,
        "feature_cache": face_feature_cache,
        "feature_cache_sha256": face_feature_sha,
        "source_identity": current_identity,
        "active_face_groups": active_groups,
    }))
}

pub fn reference_state(engine: &Engine) -> Result<Value> {
    let config = &engine.config;
    let input_version = crate::shared_metadata::analysis_state(config)?["version"]
        .as_str()
        .unwrap_or("")
        .to_owned();
    let renames = review::read(config, "person-renames.json", json!({}))?.1;
    let report = review::read(config, "report.json", json!({}))?.1;
    let queue = review::read(config, "integration-queue.json", json!([]))?.1;
    let history = review::read(config, "archive-history.json", json!([]))?.1;
    let mut assigned = serde_json::Map::new();
    for row in queue
        .as_array()
        .into_iter()
        .flatten()
        .chain(history.as_array().into_iter().flatten())
    {
        if row["file_state"].as_str() == Some("staged") {
            continue;
        }
        if let (Some(vid), Some(creator)) = (row["vid"].as_str(), row["creator"].as_str()) {
            assigned.insert(
                vid.to_owned(),
                json!(crate::processing_result::canonical_person(creator, &renames)),
            );
        }
    }
    let sources = safe_video_sources(config);
    let creators_prefix = format!("{}/pikpak/creators/", config.media.to_string_lossy());
    for (vid, p) in &sources {
        let resolved = mapped_path(config, p).or_else(|| mapped_path(config, vid));
        if let Some(rp) = resolved
            && let Some(rest) = rp.strip_prefix(&creators_prefix)
            && let Some(person) = rest.split('/').next().filter(|s| !s.is_empty())
        {
            assigned.entry(vid.clone()).or_insert_with(|| {
                json!(crate::processing_result::canonical_person(person, &renames))
            });
        }
    }
    let ref_file = config.base.join("processing/references.npz");
    let mut current_refs = Value::Null;
    let mut valid = false;
    let db_file = db_path(config);
    if db_file.exists()
        && let Ok(conn) = db::open(&db_file)
        && let Ok(Some(saved)) = setting(&conn, "references")
    {
        let sha_ok = saved["sha256"].as_str().is_none_or(|expected| {
            crate::processing_result::artifact_hash(&ref_file)
                .ok()
                .as_deref()
                == Some(expected)
        });
        if ref_file.is_file()
            && saved["input_version"].as_str() == Some(input_version.as_str())
            && sha_ok
        {
            valid = true;
        }
        current_refs = saved;
    }

    let mut accepted_receipts = serde_json::Map::new();
    if db_file.exists()
        && let Ok(conn) = db::open(&db_file)
        && conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='_face_admin_processing_results')",
                [],
                |r| r.get::<_, bool>(0),
            )
            .unwrap_or(false)
        && let Ok(rows) = db::rows(
            &conn,
            "SELECT result FROM _face_admin_processing_results ORDER BY rowid DESC",
            [],
        )
    {
        let attempts_root = fs::canonicalize(config.base.join("processing/attempts"))
            .unwrap_or_else(|_| config.base.join("processing/attempts"));
        for row in rows {
            let Some(raw) = row["result"].as_str() else {
                continue;
            };
            let Ok(rec) = serde_json::from_str::<Value>(raw) else {
                continue;
            };
            if rec["ok"] != true || rec["accepted"] != true {
                continue;
            }
            let Some(vid) = rec["vid"].as_str() else {
                continue;
            };
            if !assigned.contains_key(vid) {
                continue;
            }
            let face = &rec["data"]["face"];
            let Some(cache) = face["feature_cache"].as_str() else {
                continue;
            };
            let Some(sha) = face["feature_cache_sha256"].as_str() else {
                continue;
            };
            let cp = Path::new(cache);
            if cp.file_name().and_then(|s| s.to_str()) != Some("features.npz") {
                continue;
            }
            let Ok(real_cp) = fs::canonicalize(cp) else {
                continue;
            };
            if !real_cp.starts_with(&attempts_root) {
                continue;
            }
            let identity = face
                .get("source_identity")
                .filter(|v| v.is_object())
                .cloned()
                .or_else(|| rec.get("source_identity").cloned())
                .unwrap_or_else(|| json!({}));
            accepted_receipts
                .entry(vid.to_owned())
                .or_insert_with(|| json!([]))
                .as_array_mut()
                .unwrap()
                .push(json!({
                    "cache": real_cp.to_string_lossy(),
                    "feature_cache": real_cp.to_string_lossy(),
                    "sha256": sha,
                    "feature_cache_sha256": sha,
                    "source_identity": identity,
                }));
        }
    }

    Ok(json!({
        "ok": true,
        "version": input_version,
        "input_version": input_version,
        "valid": valid,
        "references": current_refs,
        "renames": renames,
        "report": report,
        "report_identities": report.get("identities").cloned().unwrap_or_else(|| json!([])),
        "assigned": assigned,
        "sources": sources,
        "accepted_receipts": accepted_receipts,
    }))
}

pub fn publish_references(engine: &Engine, body: &Value) -> Result<Value> {
    let config = &engine.config;
    let staged_raw = body["staged_path"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| bad("缺少待发布参考文件路径"))?;
    let expected_sha = body["sha256"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| bad("缺少待发布参考文件校验和"))?;
    let version = body["version"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| bad("缺少参考特征版本"))?;
    let input_version = body["input_version"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| bad("缺少参考输入版本"))?;
    let people = review::number(&body["people"]).unwrap_or(0);
    let templates = review::number(&body["templates"]).unwrap_or(0);

    let tmp_root = config.base.join("processing/tmp");
    fs::create_dir_all(&tmp_root)?;
    let real_tmp = fs::canonicalize(&tmp_root)?;
    let staged_path = Path::new(staged_raw);
    let real_staged = fs::canonicalize(staged_path)
        .map_err(|_| bad("待发布参考文件不存在或路径无效"))?;
    if !real_staged.starts_with(&real_tmp)
        || real_staged.parent() != Some(real_tmp.as_path())
        || !fs::symlink_metadata(&real_staged)?.is_file()
    {
        return Err(bad("待发布参考文件必须位于受限临时目录 processing/tmp 下"));
    }
    let current_input = crate::shared_metadata::analysis_state(config)?["version"]
        .as_str()
        .unwrap_or("")
        .to_owned();
    if current_input != input_version {
        let _ = fs::remove_file(&real_staged);
        return Err(conflict("人物参考资料已变化，等待重新建立参考"));
    }
    let actual_sha = crate::processing_result::artifact_hash(&real_staged)?;
    if actual_sha != expected_sha {
        let _ = fs::remove_file(&real_staged);
        return Err(conflict("待发布参考文件校验和不匹配"));
    }
    let target = config.base.join("processing/references.npz");
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&real_staged, fs::Permissions::from_mode(0o644));
    }
    fs::rename(&real_staged, &target)?;
    files::sync_parent(&target)?;

    let conn = open_or_init(config)?;
    let now = now_sec(&conn)?;
    let meta = json!({
        "version": version,
        "input_version": input_version,
        "people": people,
        "templates": templates,
        "sha256": actual_sha,
        "updated_at": now,
    });
    set_setting(&conn, "references", &meta)?;
    Ok(json!({
        "ok": true,
        "published": true,
        "version": version,
        "input_version": input_version,
        "people": people,
        "templates": templates,
        "sha256": actual_sha,
    }))
}

fn extract_attempt_from_path(attempts_roots: &[PathBuf], raw_path: &str) -> Option<String> {
    let p = Path::new(raw_path);
    let mut candidates = vec![p.to_path_buf()];
    if let Ok(real) = fs::canonicalize(p)
        && !candidates.contains(&real)
    {
        candidates.push(real);
    }
    for cand in candidates {
        for root in attempts_roots {
            if let Ok(rel) = cand.strip_prefix(root)
                && let Some(std::path::Component::Normal(first)) = rel.components().next()
                && let Some(id) = first.to_str()
                && crate::model::validate_id(id).is_ok()
            {
                return Some(id.to_owned());
            }
        }
    }
    None
}

fn entry_age_seconds(path: &Path) -> f64 {
    let now = std::time::SystemTime::now();
    let newest_mtime = |p: &Path| -> Option<std::time::SystemTime> {
        let mut latest = fs::symlink_metadata(p).ok()?.modified().ok()?;
        if p.is_dir()
            && let Ok(entries) = fs::read_dir(p)
        {
            for entry in entries.flatten() {
                if let Ok(meta) = entry.metadata()
                    && let Ok(m) = meta.modified()
                    && m > latest
                {
                    latest = m;
                }
            }
        }
        Some(latest)
    };
    newest_mtime(path)
        .and_then(|m| now.duration_since(m).ok())
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

pub fn evict_artifacts(engine: &Engine, body: &Value) -> Result<Value> {
    let config = &engine.config;
    let parse_age = |key: &str| -> Option<f64> {
        body.get(key)
            .and_then(|v| v.as_f64().or_else(|| v.as_i64().map(|n| n as f64)))
            .filter(|v| v.is_finite() && *v >= 0.0)
    };
    let grace_seconds = parse_age("grace_seconds")
        .or_else(|| parse_age("tmp_max_age_sec"))
        .unwrap_or(300.0);
    let tmp_max_age_sec = parse_age("tmp_max_age_sec")
        .or_else(|| parse_age("grace_seconds"))
        .unwrap_or(grace_seconds);

    let attempts_dir = config.base.join("processing/attempts");
    let mut attempts_roots = vec![attempts_dir.clone()];
    if let Ok(real) = fs::canonicalize(&attempts_dir)
        && !attempts_roots.contains(&real)
    {
        attempts_roots.push(real);
    }

    let mut protected_attempts = BTreeSet::new();
    let mut rolled_back_ops = BTreeSet::new();
    let mut superseded_receipts = BTreeSet::new();

    if let Ok(rows) = db::rows(&engine.store.db, "SELECT id,status FROM operations", []) {
        for row in rows {
            if let (Some(id), Some(status)) = (row["id"].as_str(), row["status"].as_str()) {
                if matches!(
                    status,
                    "accepted" | "running" | "recovery_needed" | "rolling_back"
                ) {
                    protected_attempts.insert(id.to_owned());
                } else if status == "rolled_back" {
                    rolled_back_ops.insert(id.to_owned());
                }
            }
        }
    }

    let mut assigned_vids = BTreeSet::new();
    for doc_name in ["integration-queue.json", "archive-history.json"] {
        if let Ok((_, doc)) = review::read(config, doc_name, json!([]))
            && let Some(arr) = doc.as_array()
        {
            for r in arr {
                if r["file_state"].as_str() != Some("staged")
                    && let Some(vid) = r["vid"].as_str()
                {
                    assigned_vids.insert(vid.to_owned());
                }
            }
        }
    }
    let sources = safe_video_sources(config);
    let creators_prefix = format!("{}/pikpak/creators/", config.media.to_string_lossy());
    for (vid, p) in &sources {
        let resolved = mapped_path(config, p).or_else(|| mapped_path(config, vid));
        if let Some(rp) = resolved
            && rp.starts_with(&creators_prefix)
        {
            assigned_vids.insert(vid.clone());
        }
    }

    if let Ok((_, wm_results)) = review::read(config, "watermarks/results.json", json!({}))
        && let Some(obj) = wm_results.as_object()
    {
        for rec in obj.values() {
            if let Some(attempt) = rec["artifact_attempt"].as_str()
                && crate::model::validate_id(attempt).is_ok()
            {
                protected_attempts.insert(attempt.to_owned());
            }
        }
    }

    let db_file = db_path(config);
    if db_file.exists() {
        let conn = open_or_init(config)?;
        let now = now_sec(&conn)?;
        let mut current_tasks = BTreeMap::new();
        for row in db::rows(
            &conn,
            "SELECT vid,path,size,mtime_ns,stage,run_id,lease_expires_at,data FROM tasks",
            [],
        )? {
            let vid = row["vid"].as_str().unwrap_or("").to_owned();
            let path = row["path"].as_str().unwrap_or("").to_owned();
            let size = review::number(&row["size"]).unwrap_or(-1);
            let mtime = row["mtime_ns"].as_str().unwrap_or("").to_owned();
            current_tasks.insert(vid, (path, size, mtime));

            let run_id = row["run_id"].as_str().unwrap_or("");
            let stage = row["stage"].as_str().unwrap_or("");
            let expiry = row["lease_expires_at"].as_f64().unwrap_or(0.0);
            if !run_id.is_empty() && (matches!(stage, "text" | "face") || expiry > now) {
                protected_attempts.insert(run_id.to_owned());
            }
            if let Ok(data) = review::task_data(&row) {
                if let Some(attempt) = data["watermark"]["artifact_attempt"].as_str()
                    && crate::model::validate_id(attempt).is_ok()
                {
                    protected_attempts.insert(attempt.to_owned());
                }
                if let Some(fc) = data["face"]["feature_cache"].as_str()
                    && let Some(attempt) = extract_attempt_from_path(&attempts_roots, fc)
                {
                    protected_attempts.insert(attempt);
                }
                for thumb in data["face"]["thumbs"].as_array().into_iter().flatten() {
                    if let Some(tp) = thumb.as_str()
                        && let Some(attempt) = extract_attempt_from_path(&attempts_roots, tp)
                    {
                        protected_attempts.insert(attempt);
                    }
                }
                for m in data["face"]["matches"].as_array().into_iter().flatten() {
                    if let Some(tp) = m["thumb"].as_str()
                        && let Some(attempt) = extract_attempt_from_path(&attempts_roots, tp)
                    {
                        protected_attempts.insert(attempt);
                    }
                }
            }
        }
        for row in db::rows(
            &conn,
            "SELECT id FROM processing_runs WHERE status='running'",
            [],
        )? {
            if let Some(id) = row["id"].as_str().filter(|s| !s.is_empty()) {
                protected_attempts.insert(id.to_owned());
            }
        }

        let has_results: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='_face_admin_processing_results')",
                [],
                |r| r.get(0),
            )
            .unwrap_or(false);
        if has_results {
            let mut seen_vid_stage = BTreeSet::new();
            for row in db::rows(
                &conn,
                "SELECT id,result FROM _face_admin_processing_results ORDER BY rowid DESC",
                [],
            )? {
                let Some(id) = row["id"].as_str() else {
                    continue;
                };
                let Some(raw) = row["result"].as_str() else {
                    continue;
                };
                let Ok(res) = serde_json::from_str::<Value>(raw) else {
                    superseded_receipts.insert(id.to_owned());
                    continue;
                };
                if res["ok"] != true || res["accepted"] != true {
                    superseded_receipts.insert(id.to_owned());
                    continue;
                }
                let vid = res["vid"]
                    .as_str()
                    .or_else(|| res["task"]["vid"].as_str())
                    .unwrap_or("")
                    .to_owned();
                let stage = res["stage"]
                    .as_str()
                    .or_else(|| res["task"]["stage"].as_str())
                    .unwrap_or("face")
                    .to_owned();
                let t_path = res["task"]["path"].as_str().unwrap_or("");
                let t_size = review::number(&res["task"]["size"]).unwrap_or(-1);
                let t_mtime = res["task"]["mtime_ns"].as_str().unwrap_or("");
                let matches_current = current_tasks
                    .get(&vid)
                    .is_none_or(|(p, sz, mt)| p == t_path && *sz == t_size && mt == t_mtime);
                let is_assigned = assigned_vids.contains(&vid);
                let is_latest = seen_vid_stage.insert((vid.clone(), stage));
                if is_latest && (matches_current || is_assigned) {
                    protected_attempts.insert(id.to_owned());
                    if let Some(wm_att) = res["data"]["watermark"]["artifact_attempt"].as_str()
                        && crate::model::validate_id(wm_att).is_ok()
                    {
                        protected_attempts.insert(wm_att.to_owned());
                    }
                    if let Some(fc) = res["data"]["face"]["feature_cache"].as_str()
                        && let Some(att) = extract_attempt_from_path(&attempts_roots, fc)
                    {
                        protected_attempts.insert(att);
                    }
                    for thumb in res["data"]["face"]["thumbs"]
                        .as_array()
                        .into_iter()
                        .flatten()
                    {
                        if let Some(tp) = thumb.as_str()
                            && let Some(att) = extract_attempt_from_path(&attempts_roots, tp)
                        {
                            protected_attempts.insert(att);
                        }
                    }
                } else {
                    superseded_receipts.insert(id.to_owned());
                }
            }
        }
    }

    let mut evicted_attempts = Vec::new();
    let mut preserved_attempts = Vec::new();
    if attempts_dir.is_dir() {
        for entry in fs::read_dir(&attempts_dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = entry.path();
            if path.join("publication.json").exists()
                && !path.join("receipt.json").exists()
                && !rolled_back_ops.contains(&name)
            {
                protected_attempts.insert(name.clone());
            }
            if protected_attempts.contains(&name) || entry_age_seconds(&path) < grace_seconds {
                preserved_attempts.push(name);
                continue;
            }
            if path.is_dir() {
                fs::remove_dir_all(&path)?;
            } else {
                fs::remove_file(&path)?;
            }
            evicted_attempts.push(name);
        }
    }

    if db_file.exists() && (!evicted_attempts.is_empty() || !superseded_receipts.is_empty()) {
        let conn = open_or_init(config)?;
        let has_results: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='_face_admin_processing_results')",
                [],
                |r| r.get(0),
            )
            .unwrap_or(false);
        if has_results {
            for id in &evicted_attempts {
                if !protected_attempts.contains(id) {
                    let _ = conn.execute(
                        "DELETE FROM _face_admin_processing_results WHERE id=?",
                        [id],
                    );
                }
            }
            for id in &superseded_receipts {
                if !protected_attempts.contains(id) && !attempts_dir.join(id).exists() {
                    let _ = conn.execute(
                        "DELETE FROM _face_admin_processing_results WHERE id=?",
                        [id],
                    );
                }
            }
        }
    }

    let mut evicted_tmp = Vec::new();
    let mut preserved_tmp = Vec::new();
    let tmp_dir = config.base.join("processing/tmp");
    if tmp_dir.is_dir() {
        for entry in fs::read_dir(&tmp_dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = entry.path();
            if entry_age_seconds(&path) < tmp_max_age_sec {
                preserved_tmp.push(name);
                continue;
            }
            if path.is_dir() {
                fs::remove_dir_all(&path)?;
            } else {
                fs::remove_file(&path)?;
            }
            evicted_tmp.push(name);
        }
    }

    evicted_attempts.sort();
    preserved_attempts.sort();
    evicted_tmp.sort();
    preserved_tmp.sort();
    Ok(json!({
        "ok": true,
        "evicted_attempts": evicted_attempts,
        "preserved_attempts": preserved_attempts,
        "evicted_tmp": evicted_tmp,
        "preserved_tmp": preserved_tmp,
    }))
}

fn read_catalog_people(config: &Config) -> Vec<String> {
    let renames = read_json_fallback(&config.base.join("person-renames.json"), json!({}));
    let mut raw_names = BTreeSet::new();
    let report = read_json_fallback(&config.base.join("report.json"), json!({}));
    if let Some(identities) = report["identities"].as_array() {
        for item in identities {
            if let Some(name) = item["name"].as_str().filter(|s| !s.is_empty()) {
                raw_names.insert(name.to_owned());
            }
        }
    }
    if let Some(manual) = read_json_fallback(&config.base.join("people-manual.json"), json!([]))
        .as_array()
    {
        for item in manual {
            if let Some(name) = item["name"].as_str().filter(|s| !s.is_empty()) {
                raw_names.insert(name.to_owned());
            }
        }
    }
    for file in ["integration-queue.json", "archive-history.json"] {
        if let Some(rows) = read_json_fallback(&config.base.join(file), json!([])).as_array() {
            for item in rows {
                if let Some(creator) = item["creator"].as_str().filter(|s| !s.is_empty()) {
                    raw_names.insert(creator.to_owned());
                }
            }
        }
    }
    let mut canonical_set = BTreeSet::new();
    for name in raw_names {
        let c = crate::processing_result::canonical_person(&name, &renames);
        if !c.is_empty() {
            canonical_set.insert(c);
        }
    }
    canonical_set.into_iter().collect()
}

fn read_handled_vids(config: &Config) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    for file in ["integration-queue.json", "archive-history.json"] {
        if let Some(rows) = read_json_fallback(&config.base.join(file), json!([])).as_array() {
            for item in rows {
                if let Some(vid) = item["vid"].as_str().filter(|s| !s.is_empty()) {
                    ids.insert(vid.to_owned());
                }
            }
        }
    }
    ids
}

fn http_post_json(
    origin: &str,
    endpoint: &str,
    payload: &Value,
) -> std::result::Result<Value, String> {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::time::Duration;

    let trimmed = origin.trim().trim_end_matches('/');
    let without_scheme = trimmed
        .strip_prefix("http://")
        .ok_or_else(|| format!("不支持的计算服务地址: {origin}"))?;
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
        serde_json::to_vec(payload).map_err(|e| format!("序列化计算请求失败: {e}"))?;

    let mut stream = TcpStream::connect(&connect_addr)
        .map_err(|e| format!("连接计算服务 {connect_addr} 失败: {e}"))?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(600)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(60)));

    let request_head = format!(
        "POST {req_path} HTTP/1.1\r\nHost: {host_port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body_bytes.len()
    );
    stream
        .write_all(request_head.as_bytes())
        .and_then(|_| stream.write_all(&body_bytes))
        .and_then(|_| stream.flush())
        .map_err(|e| format!("发送计算请求失败: {e}"))?;

    let mut response_bytes = Vec::new();
    stream
        .read_to_end(&mut response_bytes)
        .map_err(|e| format!("读取计算响应失败: {e}"))?;
    let response_str = String::from_utf8_lossy(&response_bytes);
    let (headers_part, body_part) = response_str
        .split_once("\r\n\r\n")
        .ok_or_else(|| "计算服务返回了无效的 HTTP 响应".to_owned())?;
    let status_code = headers_part
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or(500);
    let parsed: Value = serde_json::from_str(body_part.trim())
        .unwrap_or_else(|_| json!({"ok": false, "error": body_part.trim()}));
    if status_code >= 400 || parsed["ok"].as_bool() == Some(false) {
        let msg = parsed["error"]
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or("计算服务调用失败");
        return Err(msg.to_owned());
    }
    Ok(parsed)
}

pub fn step(engine: &mut Engine, body: &Value) -> Result<Value> {
    let conn = open_or_init(&engine.config)?;
    if !media_available(&conn, &engine.config.media)? {
        return Ok(json!({
            "ok": true,
            "stepped": false,
            "reason": "media_unavailable",
        }));
    }
    drop(conn);

    let claimed = claim(&engine.config, body)?;
    let mut task_val = claimed["task"].clone();
    if task_val.is_null() {
        return Ok(json!({
            "ok": true,
            "stepped": false,
            "task": Value::Null,
        }));
    }

    let vid = task_val["vid"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let run_id = task_val["run_id"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let stage = task_val["stage"]
        .as_str()
        .unwrap_or("text")
        .to_owned();
    let path = task_val["path"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let size = task_val["size"].as_i64().unwrap_or(0);
    let mtime_ns = task_val["mtime_ns"]
        .as_str()
        .map(str::to_owned)
        .or_else(|| task_val["mtime_ns"].as_i64().map(|n| n.to_string()))
        .unwrap_or_default();

    let handled_ids = read_handled_vids(&engine.config);
    if handled_ids.contains(&vid) {
        release_task(
            &engine.config,
            &json!({
                "task": task_val,
                "reason": "已由人工处理",
                "excluded": true,
            }),
        )?;
        return Ok(json!({
            "ok": true,
            "stepped": true,
            "status": "released",
            "vid": vid,
            "run_id": run_id,
        }));
    }

    let pipeline = body["pipeline"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_PIPELINE);
    let face_version = body["face_version"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_FACE_VERSION);

    let cache_info = cache_check(
        engine,
        &json!({
            "stage": stage,
            "vid": vid,
            "path": path,
            "size": size,
            "mtime_ns": mtime_ns,
            "pipeline": pipeline,
            "face_version": face_version,
        }),
    )?;
    task_val["_cache_check"] = cache_info.clone();
    if cache_info["active_face_groups"].is_array() {
        task_val["_active_face_groups"] = cache_info["active_face_groups"].clone();
    }

    let task_data: Value = task_val["data"]
        .as_str()
        .and_then(|s| serde_json::from_str(s).ok())
        .or_else(|| task_val["data"].as_object().map(|m| Value::Object(m.clone())))
        .unwrap_or_else(|| json!({}));
    let people = read_catalog_people(&engine.config);
    let mappings = read_json_fallback(&engine.config.base.join("watermarks/mappings.json"), json!({}));
    let mut watermark_keys = BTreeSet::new();
    if let Some(map) = mappings.as_object() {
        for k in map.keys() {
            watermark_keys.insert(k.clone());
        }
    }
    if let Some(cands) = task_data["watermark"]["candidates"].as_array() {
        for c in cands {
            if let Some(k) = c["key"].as_str().filter(|s| !s.is_empty()) {
                watermark_keys.insert(k.to_owned());
            }
        }
    }
    let scope = json!({
        "vids": [vid],
        "people": people,
        "watermark_keys": watermark_keys.into_iter().collect::<Vec<_>>(),
        "all_watermark_rules": stage == "text",
    });
    let state_out =
        crate::shared_metadata::execute(engine, "state", json!({"scope": scope.clone()}))?;
    let material_version = state_out["version"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    task_val["_material"] = json!({
        "scope": scope.clone(),
        "version": material_version.clone(),
    });

    let compute_url = body["compute_url"]
        .as_str()
        .map(str::to_owned)
        .or_else(|| std::env::var("PROCESSING_COMPUTE_URL").ok())
        .unwrap_or_default();

    if stage == "face" {
        let mut ref_state = reference_state(engine)?;
        let input_v = ref_state["input_version"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        task_val["_reference_input_version"] = json!(input_v.clone());
        if !ref_state["valid"].as_bool().unwrap_or(false)
            && !compute_url.is_empty()
            && let Ok(ref_resp) = http_post_json(
                &compute_url,
                "/v1/compute/references",
                &json!({
                    "base": engine.config.base.to_string_lossy(),
                    "root": engine.config.media.to_string_lossy(),
                    "input_version": input_v,
                    "state": ref_state,
                }),
            )
        {
            let staged = &ref_resp["result"];
            if staged["staged_path"].is_string() {
                let _ = publish_references(engine, staged);
                ref_state = reference_state(engine)?;
            }
        }
        let ref_meta = &ref_state["references"];
        let ref_path = engine.config.base.join("processing/references.npz");
        task_val["_references"] = json!({
            "path": ref_path.to_string_lossy(),
            "sha256": ref_meta["sha256"],
            "version": ref_meta["version"],
            "people": ref_meta["people"],
            "templates": ref_meta["templates"],
        });
    }

    let compute_outcome: std::result::Result<Value, String> = if stage == "text"
        && cache_info["text_valid"].as_bool() == Some(true)
        && cache_info["text_cached"].is_object()
        && compute_url.is_empty()
    {
        Ok(json!({"watermark": cache_info["text_cached"].clone()}))
    } else if compute_url.is_empty() {
        Err("计算服务 PROCESSING_COMPUTE_URL 未配置".to_owned())
    } else {
        let endpoint = if stage == "text" {
            "/v1/compute/ocr"
        } else {
            "/v1/compute/face"
        };
        http_post_json(
            &compute_url,
            endpoint,
            &json!({
                "task": task_val,
                "base": engine.config.base.to_string_lossy(),
                "root": engine.config.media.to_string_lossy(),
            }),
        )
        .map(|resp| {
            if stage == "text" {
                json!({"watermark": resp["result"].clone()})
            } else {
                json!({"face": resp["result"].clone()})
            }
        })
    };

    let raw_data = match compute_outcome {
        Ok(data) => data,
        Err(err_msg) => {
            let handled_after = read_handled_vids(&engine.config);
            if handled_after.contains(&vid) {
                release_task(
                    &engine.config,
                    &json!({
                        "task": task_val,
                        "reason": "分析期间已由人工处理",
                        "excluded": true,
                    }),
                )?;
                return Ok(json!({
                    "ok": true,
                    "stepped": true,
                    "status": "released",
                    "vid": vid,
                    "run_id": run_id,
                }));
            }
            let conn = open_or_init(&engine.config)?;
            let media_ok = media_available(&conn, &engine.config.media)?;
            drop(conn);
            if media_ok {
                fail_task(
                    &engine.config,
                    &json!({
                        "task": task_val,
                        "error": err_msg,
                    }),
                )?;
                return Ok(json!({
                    "ok": true,
                    "stepped": true,
                    "status": "failed",
                    "vid": vid,
                    "run_id": run_id,
                    "error": err_msg,
                }));
            } else {
                release_task(
                    &engine.config,
                    &json!({
                        "task": task_val,
                        "reason": "媒体挂载不可读，等待恢复",
                    }),
                )?;
                return Ok(json!({
                    "ok": true,
                    "stepped": true,
                    "status": "released",
                    "vid": vid,
                    "run_id": run_id,
                }));
            }
        }
    };

    let handled_after = read_handled_vids(&engine.config);
    if handled_after.contains(&vid) {
        release_task(
            &engine.config,
            &json!({
                "task": task_val,
                "reason": "分析期间已由人工处理",
                "excluded": true,
            }),
        )?;
        return Ok(json!({
            "ok": true,
            "stepped": true,
            "status": "released",
            "vid": vid,
            "run_id": run_id,
        }));
    }

    let mut pub_body = json!({
        "operation_id": run_id,
        "scope": scope,
        "version": material_version,
        "task": {
            "vid": task_val["vid"],
            "path": task_val["path"],
            "size": task_val["size"],
            "mtime_ns": task_val["mtime_ns"],
            "stage": task_val["stage"],
            "run_id": task_val["run_id"],
            "batch_id": task_val["batch_id"],
            "lease_expires_at": task_val["lease_expires_at"],
            "coordinator_generation": task_val["coordinator_generation"],
        },
        "data": raw_data,
    });
    if stage == "face" {
        pub_body["reference_input_version"] = task_val["_reference_input_version"].clone();
    }

    let attempt_dir = engine
        .config
        .base
        .join("processing/attempts")
        .join(&run_id);
    fs::create_dir_all(&attempt_dir)?;
    files::atomic_write(
        &attempt_dir.join("publication.json"),
        &serde_json::to_vec(&pub_body)?,
    )?;
    let pub_result = crate::processing_result::execute(engine, pub_body)?;
    files::atomic_write(
        &attempt_dir.join("receipt.json"),
        &serde_json::to_vec(&pub_result)?,
    )?;

    Ok(json!({
        "ok": true,
        "stepped": true,
        "status": "published",
        "vid": vid,
        "run_id": run_id,
        "stage": stage,
        "result": pub_result,
    }))
}

pub fn execute(engine: &mut Engine, command: &str, body: Value) -> Result<Value> {
    if engine.store.blocked()? {
        return Err(Error::new(
            503,
            "recovery_required",
            "存在未核对操作，已暂停新写入",
        ));
    }
    if matches!(
        command,
        "processing-settings"
            | "processing-inventory"
            | "processing-overview"
            | "processing-dashboard"
            | "processing-home-tasks"
            | "processing-evaluate-rules"
            | "processing-artifact-check"
            | "processing-cache-check"
            | "processing-reference-state"
    ) {
        return match command {
            "processing-settings" => settings(&engine.config),
            "processing-inventory" => Ok(json!({"assets": read_ledger_assets(&engine.config)?})),
            "processing-overview" => overview(&engine.config, &body),
            "processing-dashboard" => dashboard(&engine.config, &body),
            "processing-home-tasks" => home_tasks(&engine.config, &body),
            "processing-evaluate-rules" => crate::processing_result::evaluate_rules(&body),
            "processing-artifact-check" => verify_attempt_artifact(&engine.config, &body),
            "processing-cache-check" => cache_check(engine, &body),
            "processing-reference-state" => reference_state(engine),
            _ => unreachable!(),
        };
    }
    engine.config.require_writer()?;
    match fs::symlink_metadata(engine.config.base.join(".review-write.lock")) {
        Ok(_) => return Err(Error::new(409, "writer_busy", "正在保存资料，请稍后重试")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => return Err(e.into()),
    }
    match command {
        "processing-init" => {
            open_or_init(&engine.config)?;
            Ok(json!({"ok": true}))
        }
        "processing-discover" => discover(&engine.config, &body),
        "processing-reconcile" => {
            let items = body["items"]
                .as_array()
                .ok_or_else(|| bad("缺少 items 列表"))?;
            let batch_id = body["batch_id"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| bad("缺少 batch_id"))?;
            let mut conn = open_or_init(&engine.config)?;
            let bootstrapped = body["bootstrapped"]
                .as_bool()
                .or_else(|| {
                    setting(&conn, "bootstrapped")
                        .ok()
                        .flatten()
                        .and_then(|v| v.as_bool())
                })
                .unwrap_or(false);
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let count = reconcile(&tx, items, batch_id, bootstrapped)?;
            tx.commit()?;
            Ok(json!({"ok": true, "eligible": count}))
        }
        "processing-media-status" => media_status(&engine.config, &body),
        "processing-claim" => claim(&engine.config, &body),
        "processing-heartbeat" => heartbeat(&engine.config, &body),
        "processing-fail" => fail_task(&engine.config, &body),
        "processing-release" => release_task(&engine.config, &body),
        "processing-recover" => recover(&engine.config),
        "processing-control" => control(&engine.config, &body),
        "processing-concurrency" => set_concurrency(&engine.config, &body),
        "processing-retry-errors" => retry_errors(&engine.config, &body),
        "processing-publish-references" => publish_references(engine, &body),
        "processing-evict-artifacts" => evict_artifacts(engine, &body),
        "processing-stage-progress" => stage_progress(&engine.config, &body),
        "processing-event" => record_event(&engine.config, &body),
        "processing-step" => step(engine, &body),
        _ => Err(bad("未知分析调度命令")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    struct TestEnv {
        root: PathBuf,
        config: Config,
    }

    impl TestEnv {
        fn new() -> Self {
            let id = COUNTER.fetch_add(1, Ordering::Relaxed);
            let raw_root = std::env::temp_dir().join(format!(
                "face-admin-rust-scheduler-{}-{id}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&raw_root);
            fs::create_dir_all(&raw_root).unwrap();
            let root = fs::canonicalize(&raw_root).unwrap();
            let base = root.join("base");
            let media = root.join("media");
            let state = root.join("state");
            fs::create_dir_all(media.join("pikpak/My Pack")).unwrap();
            fs::create_dir_all(media.join("pikpak/creators/Alice")).unwrap();
            fs::create_dir_all(&base).unwrap();
            fs::create_dir_all(&state).unwrap();
            fs::write(root.join(".fixture-marker"), "rust-scheduler-unit").unwrap();

            let faces = Connection::open(base.join("faces.db")).unwrap();
            faces
                .execute_batch("CREATE TABLE videos (vid TEXT PRIMARY KEY, path TEXT NOT NULL);")
                .unwrap();
            let ledger_path = root.join("ledger.db");
            let ledger = Connection::open(&ledger_path).unwrap();
            ledger
                .execute_batch(
                    "CREATE TABLE asset (
                        id INTEGER PRIMARY KEY,
                        path TEXT NOT NULL,
                        name TEXT,
                        size INTEGER,
                        duration REAL,
                        disposal TEXT,
                        mtime INTEGER,
                        medium TEXT DEFAULT 'video'
                    );",
                )
                .unwrap();

            let config = Config {
                bind: "127.0.0.1:0".into(),
                base,
                media,
                ledger: ledger_path,
                token: None,
                fixture: Some(root.clone()),
                parity_root: None,
            };
            Self { root, config }
        }

        fn write_media(&self, rel: &str, content: &[u8]) -> PathBuf {
            let path = self.config.media.join(rel);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(&path, content).unwrap();
            path
        }
    }

    impl Drop for TestEnv {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn discovery_deduplicates_excludes_and_guards_mass_drop() {
        let env = TestEnv::new();
        let a = env.write_media("pikpak/My Pack/a.mp4", b"video-a");
        let _b = env.write_media("pikpak/My Pack/b.mp4", b"video-b");
        let _disposed = env.write_media("pikpak/My Pack/trash.mp4", b"video-trash");
        let creator_file = env.write_media("pikpak/creators/Alice/done.mp4", b"video-done");
        let hidden = env.write_media("pikpak/My Pack/hidden.mp4", b"video-hidden");
        let queued = env.write_media("pikpak/My Pack/queued.mp4", b"video-queued");

        let faces = db::open(&env.config.base.join("faces.db")).unwrap();
        faces
            .execute(
                "INSERT INTO videos VALUES (?,?)",
                params!["R:\\media\\pikpak\\My Pack\\a.mp4", a.to_string_lossy()],
            )
            .unwrap();

        fs::write(
            env.config.base.join("integration-queue.json"),
            serde_json::to_vec(&json!([{"vid": queued.to_string_lossy(), "creator": "Alice"}]))
                .unwrap(),
        )
        .unwrap();

        let hidden_stat = fs::metadata(&hidden).unwrap();
        let now_iso: String = Connection::open_in_memory()
            .unwrap()
            .query_row("SELECT strftime('%Y-%m-%dT%H:%M:%SZ','now')", [], |r| {
                r.get(0)
            })
            .unwrap();
        fs::write(
            env.config.base.join("video-health.json"),
            serde_json::to_vec(&json!({
                "last_success_at": now_iso,
                "records": {
                    hidden.to_string_lossy().to_string(): {
                        "path": hidden.to_string_lossy(),
                        "size": hidden_stat.len(),
                        "mtime_ns": files::mtime_ns(&hidden_stat),
                        "hidden": true
                    }
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let assets = json!([
            {"id": 1, "path": "/media/pikpak/My Pack/a.mp4", "name": "a.mp4", "disposal": null},
            {"id": 2, "path": "/media/pikpak/My Pack/b.mp4", "name": "b.mp4", "disposal": null},
            {"id": 3, "path": "/media/pikpak/My Pack/trash.mp4", "name": "trash.mp4", "disposal": "trash"},
            {"id": 4, "path": creator_file.to_string_lossy(), "name": "done.mp4", "disposal": null},
            {"id": 5, "path": hidden.to_string_lossy(), "name": "hidden.mp4", "disposal": null},
            {"id": 6, "path": queued.to_string_lossy(), "name": "queued.mp4", "disposal": null}
        ]);

        let res = discover(
            &env.config,
            &json!({"assets": assets, "batch_id": "20260416T000000Z"}),
        )
        .unwrap();
        assert_eq!(res["eligible"], 2);
        // `a.mp4` was already in faces.db by path, so only `b`, `hidden`, `queued` were registered as new sources
        assert_eq!(res["registered"], 3);

        // Seed 20 eligible tasks to verify mass-drop protection
        let mut many_assets = Vec::new();
        for i in 0..22 {
            let p = env.write_media(&format!("pikpak/My Pack/v{i}.mp4"), b"video");
            many_assets.push(json!({"id": 100 + i, "path": p.to_string_lossy(), "disposal": null}));
        }
        let res2 = discover(&env.config, &json!({"assets": many_assets})).unwrap();
        assert_eq!(res2["eligible"], 23);

        let drop_err =
            discover(&env.config, &json!({"assets": [many_assets[0].clone()]})).unwrap_err();
        assert_eq!(drop_err.status, 503);
        assert!(drop_err.error.contains("媒体可读数量突然下降"));
    }

    #[test]
    fn claim_lease_expiry_retry_and_concurrency_draining() {
        let env = TestEnv::new();
        let conn = open_or_init(&env.config).unwrap();
        reconcile(
            &conn,
            &[
                json!({"vid": "v1", "path": "/m/1.mp4", "name": "1.mp4", "size": 10, "mtime_ns": "1"}),
                json!({"vid": "v2", "path": "/m/2.mp4", "name": "2.mp4", "size": 10, "mtime_ns": "1"}),
                json!({"vid": "v3", "path": "/m/3.mp4", "name": "3.mp4", "size": 10, "mtime_ns": "1"}),
            ],
            "batch-1",
            true,
        )
        .unwrap();
        set_setting(&conn, "concurrency", &json!(2)).unwrap();
        drop(conn);

        let c1 = claim(&env.config, &json!({"capacity": 2})).unwrap()["task"].clone();
        let c2 = claim(&env.config, &json!({"capacity": 2})).unwrap()["task"].clone();
        assert!(c1.is_object() && c2.is_object());
        assert_ne!(c1["vid"], c2["vid"]);
        assert!(claim(&env.config, &json!({"capacity": 2})).unwrap()["task"].is_null());

        // Lower concurrency from 2 -> 1 while 2 are running: in-flight tasks continue, no new task claimed after 1 finishes
        let beat_path = env.config.base.join("processing/heartbeat.json");
        let now = now_sec(&open_or_init(&env.config).unwrap()).unwrap();
        fs::write(
            &beat_path,
            serde_json::to_vec(&json!({"at": now, "protocol": 3, "max_concurrency": 2})).unwrap(),
        )
        .unwrap();
        let updated = set_concurrency(&env.config, &json!({"concurrency": 1})).unwrap();
        assert_eq!(updated["effective"], 1);

        // Renew c1 via heartbeat
        let hb = heartbeat(&env.config, &json!({"run_id": c1["run_id"], "capacity": 2})).unwrap();
        assert_eq!(hb["active"], 2);
        assert_eq!(hb["concurrency"], 1);

        // Release c2 due to mount error -> does not consume attempt budget
        release_task(
            &env.config,
            &json!({"task": c2, "reason": "媒体挂载不可读，等待恢复"}),
        )
        .unwrap();
        let conn = open_or_init(&env.config).unwrap();
        let c2_row = plain_row(
            db::rows(
                &conn,
                "SELECT * FROM tasks WHERE vid=?",
                [c2["vid"].as_str().unwrap()],
            )
            .unwrap()
            .remove(0),
        );
        assert_eq!(c2_row["stage"], "retry");
        assert_eq!(c2_row["text_attempts"], 0);
        drop(conn);

        // Still 1 active (c1) and concurrency is 1, so claim returns null (draining 2 -> 1 smoothly)
        assert!(claim(&env.config, &json!({"capacity": 2})).unwrap()["task"].is_null());

        // Expire c1's lease and verify next claim reaps c1 and claims a ready task
        let conn = open_or_init(&env.config).unwrap();
        conn.execute(
            "UPDATE tasks SET lease_expires_at=1 WHERE vid=?",
            [c1["vid"].as_str().unwrap()],
        )
        .unwrap();
        drop(conn);

        let next = claim(&env.config, &json!({"capacity": 2})).unwrap();
        assert_eq!(next["reaped"], 1);
        assert!(next["task"].is_object());
    }

    #[test]
    fn control_actions_and_batch_error_retry_enforce_guards() {
        let env = TestEnv::new();
        let conn = open_or_init(&env.config).unwrap();
        reconcile(
            &conn,
            &[
                json!({"vid": "ready-1", "path": "/m/r1.mp4", "name": "r1.mp4", "size": 10, "mtime_ns": "1"}),
                json!({"vid": "err-1", "path": "/m/e1.mp4", "name": "e1.mp4", "size": 10, "mtime_ns": "1"}),
            ],
            "batch-a",
            false,
        )
        .unwrap();
        conn.execute(
            "UPDATE tasks SET stage='review',data='{\"ready\":true}' WHERE vid='ready-1'",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE tasks SET stage='error',text_attempts=3,error='boom' WHERE vid='err-1'",
            [],
        )
        .unwrap();
        drop(conn);

        assert_eq!(
            control(&env.config, &json!({"action": "pause"})).unwrap()["paused"],
            true
        );
        assert_eq!(
            control(&env.config, &json!({"action": "resume"})).unwrap()["paused"],
            false
        );

        // retry on review task -> 409
        let err = control(
            &env.config,
            &json!({"action": "retry", "vids": ["ready-1"]}),
        )
        .unwrap_err();
        assert_eq!(err.status, 409);

        // defer, restore, note on review task -> 200
        control(
            &env.config,
            &json!({"action": "defer", "vids": ["ready-1"]}),
        )
        .unwrap();
        control(
            &env.config,
            &json!({"action": "restore", "vids": ["ready-1"]}),
        )
        .unwrap();
        control(
            &env.config,
            &json!({"action": "note", "vids": ["ready-1"], "note": "测试备注"}),
        )
        .unwrap();

        // batch retry errors
        let retried = retry_errors(&env.config, &json!({"batch": "batch-a"})).unwrap();
        assert_eq!(retried["retried"], 1);

        // set back to error and test takeover
        let conn = open_or_init(&env.config).unwrap();
        conn.execute("UPDATE tasks SET stage='error' WHERE vid='err-1'", [])
            .unwrap();
        drop(conn);
        control(
            &env.config,
            &json!({"action": "takeover", "vids": ["err-1"]}),
        )
        .unwrap();
        let conn = open_or_init(&env.config).unwrap();
        let row = plain_row(
            db::rows(&conn, "SELECT * FROM tasks WHERE vid='err-1'", [])
                .unwrap()
                .remove(0),
        );
        assert_eq!(row["manual"], 1);
    }

    #[test]
    fn overview_dashboard_and_home_tasks_preserve_cohort_invariants() {
        let env = TestEnv::new();
        // Before DB exists: overview and home_tasks do not create DB, dashboard fails closed with 503
        let ov0 = overview(&env.config, &json!({})).unwrap();
        assert_eq!(ov0["all_count"], 0);
        let ht0 = home_tasks(&env.config, &json!({})).unwrap();
        assert_eq!(ht0["initialized"], false);
        assert!(!db_path(&env.config).exists());
        assert_eq!(dashboard(&env.config, &json!({})).unwrap_err().status, 503);

        let conn = open_or_init(&env.config).unwrap();
        reconcile(
            &conn,
            &[
                json!({"vid": "done-archived", "path": "/m/1.mp4", "name": "1.mp4", "size": 10, "mtime_ns": "1"}),
                json!({"vid": "manual-takeover", "path": "/m/2.mp4", "name": "2.mp4", "size": 10, "mtime_ns": "1"}),
                json!({"vid": "queued-unknown-face", "path": "/m/3.mp4", "name": "3.mp4", "size": 10, "mtime_ns": "1"}),
            ],
            "batch-x",
            false,
        )
        .unwrap();
        conn.execute(
            "UPDATE tasks SET stage='review',data='{\"ready\":true,\"text_complete\":true,\"face_skipped\":true,\"kind\":\"suggested\",\"suggested\":\"Alice\"}' WHERE vid='done-archived'",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE batch_items SET text_done=1,face_required=0,face_skipped=1,completed=1,completed_at=100 WHERE vid='done-archived'",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE tasks SET stage='error',manual=1 WHERE vid='manual-takeover'",
            [],
        )
        .unwrap();
        fs::write(
            env.config.base.join("archive-history.json"),
            serde_json::to_vec(&json!([{"vid": "done-archived", "creator": "Alice"}])).unwrap(),
        )
        .unwrap();
        drop(conn);

        let dash = dashboard(&env.config, &json!({})).unwrap();
        assert_eq!(dash["totals"]["total"], 3);
        assert_eq!(dash["totals"]["completed"], 1);
        assert_eq!(dash["totals"]["excluded"], 1);
        assert_eq!(dash["totals"]["waiting"], 1);
        assert_eq!(dash["totals"]["face_required"], 0);

        let ov = overview(&env.config, &json!({})).unwrap();
        assert_eq!(ov["all_count"], 2);
        assert_eq!(ov["counts"]["review"], 1);

        let ht = home_tasks(&env.config, &json!({})).unwrap();
        assert_eq!(ht["initialized"], true);
        assert_eq!(ht["total"], 2);
        assert_eq!(ht["counts"]["review"], 1);
        assert_eq!(ht["counts"]["queued"], 1);
    }

    #[test]
    fn verify_attempt_artifact_enforces_latest_accepted_receipt_and_source_identity() {
        let env = TestEnv::new();
        let conn = open_or_init(&env.config).unwrap();
        conn.execute(
            "CREATE TABLE IF NOT EXISTS _face_admin_processing_results (
                id TEXT PRIMARY KEY,
                request_hash TEXT NOT NULL,
                result TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )",
            [],
        )
        .unwrap();

        let video = env.config.media.join("inbox/v1.mp4");
        fs::create_dir_all(video.parent().unwrap()).unwrap();
        fs::write(&video, b"video-bytes-v1").unwrap();
        let meta = fs::metadata(&video).unwrap();
        let vpath = video.to_string_lossy().into_owned();
        let mtime = files::mtime_ns(&meta);
        let identity = files::identity(&meta);

        let run1 = "11111111-1111-4111-8111-111111111111";
        let run2 = "22222222-2222-4222-8222-222222222222";
        let unaccepted = "33333333-3333-4333-8333-333333333333";

        for run in [run1, run2, unaccepted] {
            let wm_dir = env
                .config
                .base
                .join("processing/attempts")
                .join(run)
                .join("watermarks/evidence");
            let th_dir = env
                .config
                .base
                .join("processing/attempts")
                .join(run)
                .join("thumbs");
            fs::create_dir_all(&wm_dir).unwrap();
            fs::create_dir_all(&th_dir).unwrap();
            fs::write(wm_dir.join("crop.jpg"), b"wm").unwrap();
            fs::write(th_dir.join("face.jpg"), b"face").unwrap();
        }

        let r1 = json!({
            "ok": true,
            "accepted": true,
            "vid": "v1",
            "stage": "text",
            "task": {
                "vid": "v1",
                "run_id": run1,
                "stage": "text",
                "path": vpath,
                "size": meta.len(),
                "mtime_ns": mtime,
            },
            "source_identity": identity,
            "data": {
                "watermark": {
                    "artifact_attempt": run1,
                    "candidates": [{"evidence": "crop.jpg"}]
                }
            }
        });
        conn.execute(
            "INSERT INTO _face_admin_processing_results VALUES (?,'h1',?,'1')",
            params![run1, r1.to_string()],
        )
        .unwrap();

        // Accepted run1 watermark is valid
        let ok1 = verify_attempt_artifact(
            &env.config,
            &json!({"attempt": run1, "relative": "watermarks/evidence/crop.jpg"}),
        )
        .unwrap();
        assert_eq!(ok1["accepted"], true);
        assert_eq!(ok1["valid"], true);

        // Unaccepted run is rejected
        let bad_unaccepted = verify_attempt_artifact(
            &env.config,
            &json!({"attempt": unaccepted, "relative": "watermarks/evidence/crop.jpg"}),
        )
        .unwrap();
        assert_eq!(bad_unaccepted["accepted"], false);

        // Supersede run1 with run2 for stage 'text'
        let r2 = json!({
            "ok": true,
            "accepted": true,
            "vid": "v1",
            "stage": "text",
            "task": {
                "vid": "v1",
                "run_id": run2,
                "stage": "text",
                "path": vpath,
                "size": meta.len(),
                "mtime_ns": mtime,
            },
            "source_identity": identity,
            "data": {
                "watermark": {
                    "artifact_attempt": run2,
                    "candidates": [{"evidence": "crop.jpg"}]
                }
            }
        });
        conn.execute(
            "INSERT INTO _face_admin_processing_results VALUES (?,'h2',?,'2')",
            params![run2, r2.to_string()],
        )
        .unwrap();

        let superseded = verify_attempt_artifact(
            &env.config,
            &json!({"attempt": run1, "relative": "watermarks/evidence/crop.jpg"}),
        )
        .unwrap();
        assert_eq!(superseded["accepted"], false);

        let ok2 = verify_attempt_artifact(
            &env.config,
            &json!({"attempt": run2, "relative": "watermarks/evidence/crop.jpg"}),
        )
        .unwrap();
        assert_eq!(ok2["accepted"], true);

        // Mutating the underlying source video invalidates the accepted artifact
        fs::write(&video, b"video-bytes-v1-modified-longer").unwrap();
        let mutated = verify_attempt_artifact(
            &env.config,
            &json!({"attempt": run2, "relative": "watermarks/evidence/crop.jpg"}),
        )
        .unwrap();
        assert_eq!(mutated["accepted"], false);
    }

    #[test]
    fn cache_check_and_reference_publication_enforce_versioned_inputs() {
        let env = TestEnv::new();
        let engine = Engine::new(env.config.clone()).unwrap();
        let conn = open_or_init(&env.config).unwrap();
        conn.execute(
            "CREATE TABLE IF NOT EXISTS _face_admin_processing_results (
                id TEXT PRIMARY KEY,
                request_hash TEXT NOT NULL,
                result TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )",
            [],
        )
        .unwrap();

        let video = env.config.media.join("inbox/v1.mp4");
        fs::create_dir_all(video.parent().unwrap()).unwrap();
        fs::write(&video, b"video-bytes-for-cache").unwrap();
        let meta = fs::metadata(&video).unwrap();
        let vpath = video.to_string_lossy().into_owned();
        let vsize = i64::try_from(meta.len()).unwrap();
        let vmtime = files::mtime_ns(&meta);
        let identity = files::identity(&meta);

        let run_face = "11111111-1111-4111-8111-111111111111";
        let feat_dir = env.config.base.join("processing/attempts").join(run_face);
        fs::create_dir_all(&feat_dir).unwrap();
        let feat_file = feat_dir.join("features.npz");
        fs::write(&feat_file, b"fake-npz-bytes").unwrap();
        let feat_sha = crate::processing_result::artifact_hash(&feat_file).unwrap();

        let receipt = json!({
            "ok": true,
            "accepted": true,
            "vid": "v1",
            "stage": "face",
            "task": {
                "vid": "v1",
                "run_id": run_face,
                "path": vpath,
                "size": vsize,
                "mtime_ns": vmtime,
            },
            "source_identity": identity,
            "data": {
                "face": {
                    "version": "buffalo_l-det-rec-8frames-v1",
                    "feature_cache": feat_file.to_string_lossy(),
                    "feature_cache_sha256": feat_sha,
                    "source_identity": identity,
                    "thumbs": []
                }
            }
        });
        conn.execute(
            "INSERT INTO _face_admin_processing_results VALUES (?,'h1',?,'1')",
            params![run_face, receipt.to_string()],
        )
        .unwrap();
        drop(conn);

        let wm_dir = env.config.base.join("watermarks");
        fs::create_dir_all(&wm_dir).unwrap();
        fs::write(
            wm_dir.join("results.json"),
            serde_json::to_vec(&json!({
                "v1": {
                    "vid": "v1",
                    "path": vpath,
                    "size": vsize,
                    "mtime_ns": vmtime,
                    "status": "done",
                    "pipeline": "pipe-v1",
                    "candidates": []
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let hit = cache_check(
            &engine,
            &json!({
                "vid": "v1",
                "path": vpath,
                "size": vsize,
                "mtime_ns": vmtime,
                "pipeline": "pipe-v1",
                "face_version": "buffalo_l-det-rec-8frames-v1"
            }),
        )
        .unwrap();
        assert_eq!(hit["text_valid"], true);
        assert_eq!(hit["face_feature_valid"], true);

        // Source size/mtime change invalidates both caches
        let miss = cache_check(
            &engine,
            &json!({
                "vid": "v1",
                "path": vpath,
                "size": vsize + 1,
                "mtime_ns": vmtime,
                "pipeline": "pipe-v1",
                "face_version": "buffalo_l-det-rec-8frames-v1"
            }),
        )
        .unwrap();
        assert_eq!(miss["text_valid"], false);
        assert_eq!(miss["face_feature_valid"], false);

        // Reference state & publication validation
        let faces_conn = db::open(&env.config.base.join("faces.db")).unwrap();
        faces_conn
            .execute(
                "CREATE TABLE IF NOT EXISTS faces (id INTEGER PRIMARY KEY, vid TEXT, score REAL, emb BLOB, thumb TEXT)",
                [],
            )
            .unwrap();
        drop(faces_conn);
        let state = reference_state(&engine).unwrap();
        let input_v = state["input_version"].as_str().unwrap().to_owned();
        let tmp_dir = env.config.base.join("processing/tmp");
        fs::create_dir_all(&tmp_dir).unwrap();
        let staged = tmp_dir.join("references-test.npz");
        let staged_bytes = b"PK\x03\x04fake-npz-zip-header";
        fs::write(&staged, staged_bytes).unwrap();
        let staged_sha = crate::processing_result::artifact_hash(&staged).unwrap();

        let pub_ok = publish_references(
            &engine,
            &json!({
                "staged_path": staged.to_string_lossy(),
                "sha256": staged_sha,
                "version": "ref-v1",
                "input_version": input_v,
                "people": 3,
                "templates": 5,
            }),
        )
        .unwrap();
        assert_eq!(pub_ok["published"], true);
        assert!(env.config.base.join("processing/references.npz").exists());

        // Stale input_version must fail with 409 and remove staged file without overwriting references.npz
        let staged2 = tmp_dir.join("references-stale.npz");
        fs::write(&staged2, b"PK\x03\x04stale-npz").unwrap();
        let stale_sha = crate::processing_result::artifact_hash(&staged2).unwrap();
        let err = publish_references(
            &engine,
            &json!({
                "staged_path": staged2.to_string_lossy(),
                "sha256": stale_sha,
                "version": "ref-stale",
                "input_version": "0000000000000000000000000000000000000000000000000000000000000000",
                "people": 1,
                "templates": 1,
            }),
        )
        .unwrap_err();
        assert_eq!(err.status, 409);
        assert!(!staged2.exists());
        assert_eq!(
            fs::read(env.config.base.join("processing/references.npz")).unwrap(),
            staged_bytes
        );
    }

    #[test]
    fn evict_artifacts_preserves_active_pending_accepted_and_reused_while_evicting_stale() {
        let env = TestEnv::new();
        let engine = Engine::new(env.config.clone()).unwrap();
        let conn = open_or_init(&env.config).unwrap();
        conn.execute(
            "CREATE TABLE IF NOT EXISTS _face_admin_processing_results (
                id TEXT PRIMARY KEY,
                request_hash TEXT NOT NULL,
                result TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )",
            [],
        )
        .unwrap();

        let active_run = "11111111-1111-4111-8111-111111111111";
        let pending_pub_run = "22222222-2222-4222-8222-222222222222";
        let accepted_run = "33333333-3333-4333-8333-333333333333";
        let reused_feat_run = "44444444-4444-4444-8444-444444444444";
        let superseded_run = "55555555-5555-4555-8555-555555555555";
        let failed_run = "66666666-6666-4666-8666-666666666666";

        for run in [
            active_run,
            pending_pub_run,
            accepted_run,
            reused_feat_run,
            superseded_run,
            failed_run,
        ] {
            let dir = env.config.base.join("processing/attempts").join(run);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("artifact.bin"), b"data").unwrap();
        }
        // Pending publication journal on pending_pub_run
        fs::write(
            env.config
                .base
                .join("processing/attempts")
                .join(pending_pub_run)
                .join("publication.json"),
            b"{}",
        )
        .unwrap();

        // Active lease on active_run
        let now = now_sec(&conn).unwrap();
        conn.execute(
            "INSERT INTO tasks (vid,path,name,size,mtime_ns,stage,priority,retry_stage,text_attempts,face_attempts,next_retry,lease_expires_at,coordinator_generation,error,manual,note,run_id,batch_id,data,created_at,updated_at)
             VALUES ('v-active','/m/a.mp4','a.mp4',10,'1','face',10,'face',0,0,0,?,1,'',0,'',?,'b1','{}',?,?)",
            params![now + 600.0, active_run, now, now],
        )
        .unwrap();

        // Superseded receipt followed by latest accepted receipt that reuses features from reused_feat_run
        let r_old = json!({"ok": true, "accepted": true, "vid": "v-done", "stage": "face", "task": {"vid": "v-done", "run_id": superseded_run, "stage": "face", "path": "/m/done.mp4", "size": 10, "mtime_ns": "1"}, "data": {}});
        let reused_path = env
            .config
            .base
            .join("processing/attempts")
            .join(reused_feat_run)
            .join("features.npz");
        let r_new = json!({
            "ok": true,
            "accepted": true,
            "vid": "v-done",
            "stage": "face",
            "task": {
                "vid": "v-done",
                "run_id": accepted_run,
                "stage": "face",
                "path": "/m/done.mp4",
                "size": 10,
                "mtime_ns": "1"
            },
            "data": {
                "face": {
                    "feature_cache": reused_path.to_string_lossy()
                }
            }
        });
        conn.execute(
            "INSERT INTO _face_admin_processing_results VALUES (?,'h1',?,'1')",
            params![superseded_run, r_old.to_string()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO _face_admin_processing_results VALUES (?,'h2',?,'2')",
            params![accepted_run, r_new.to_string()],
        )
        .unwrap();
        drop(conn);

        let res = evict_artifacts(&engine, &json!({"grace_seconds": 0})).unwrap();
        assert_eq!(res["ok"], true);
        assert_eq!(res["evicted_attempts"].as_array().unwrap().len(), 2);

        // Protected attempts must still exist
        for protected in [active_run, pending_pub_run, accepted_run, reused_feat_run] {
            assert!(
                env.config
                    .base
                    .join("processing/attempts")
                    .join(protected)
                    .exists(),
                "expected protected attempt {protected} to remain"
            );
        }
        // Superseded and failed unaccepted attempts must be evicted
        for evicted in [superseded_run, failed_run] {
            assert!(
                !env.config
                    .base
                    .join("processing/attempts")
                    .join(evicted)
                    .exists(),
                "expected terminal unaccepted/superseded attempt {evicted} to be evicted"
            );
        }
    }

    #[test]
    fn media_status_and_record_event_and_stage_progress_roundtrip() {
        let env = TestEnv::new();
        let out_down = media_status(
            &env.config,
            &json!({"available": false, "error": "mount offline"}),
        )
        .unwrap();
        assert_eq!(out_down["available"], false);
        assert!(out_down["media_unavailable"].is_object());

        let out_up = media_status(&env.config, &json!({"available": true})).unwrap();
        assert_eq!(out_up["available"], true);
        assert_eq!(out_up["media_unavailable"], false);

        record_event(
            &env.config,
            &json!({"vid": "v-evt", "kind": "publication_unknown", "detail": "timeout detail"}),
        )
        .unwrap();
        let conn = open_or_init(&env.config).unwrap();
        let msg: String = conn
            .query_row(
                "SELECT message FROM events WHERE vid='v-evt' AND kind='publication_unknown' ORDER BY id DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(msg, "timeout detail");

        let sp_out = stage_progress(
            &env.config,
            &json!({"isolated": true, "data": {"id": "run-123", "kind": "text", "status": "running"}}),
        )
        .unwrap();
        let sp_file = PathBuf::from(sp_out["file"].as_str().unwrap());
        assert!(sp_file.exists());
    }
}
