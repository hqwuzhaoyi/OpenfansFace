//! Durable, immutable review evidence. Model progress never becomes human authority.
use crate::{
    config::Config,
    db,
    engine::Engine,
    model::{Error, Result},
    review, shared_metadata,
};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Deserializer, de};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};
const TTL: i64 = 8 * 60 * 60;
pub const MAX_ACTIVE_SESSIONS: i64 = 16;

struct CachedSession {
    db_path: PathBuf,
    id: String,
    expires: i64,
    snapshot: Arc<Value>,
}

fn session_cache() -> &'static Mutex<VecDeque<CachedSession>> {
    static CACHE: OnceLock<Mutex<VecDeque<CachedSession>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(VecDeque::new()))
}

fn session_cache_put(db_path: &Path, id: &str, expires: i64, snapshot: Arc<Value>) {
    let now_epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    if let Ok(mut guard) = session_cache().lock() {
        guard.retain(|entry| {
            entry.expires > now_epoch && !(entry.db_path == db_path && entry.id == id)
        });
        if guard.len() >= 2 {
            guard.pop_front();
        }
        guard.push_back(CachedSession {
            db_path: db_path.to_path_buf(),
            id: id.to_owned(),
            expires,
            snapshot,
        });
    }
}
fn expired() -> Error {
    Error::new(
        409,
        "review_session_expired",
        "本次核对已过期或不存在，请明确更新列表；当前选择尚未提交",
    )
}
fn changed() -> Error {
    Error::new(
        409,
        "review_material_changed",
        "文件、文字账号规则、人工处置或体检状态已变化，请更新后重新核对",
    )
}
fn schema(config: &Config) -> Result<rusqlite::Connection> {
    db::open(&config.state_db())
}
fn schema_readonly(config: &Config) -> Result<rusqlite::Connection> {
    db::open_readonly(&config.state_db())
}
fn sidecar_write(config: &Config) -> Result<rusqlite::Connection> {
    let path = config.review_sessions_db();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = rusqlite::Connection::open_with_flags(
        &path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
            | rusqlite::OpenFlags::SQLITE_OPEN_CREATE
            | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(std::time::Duration::from_secs(30))?;
    let has_snapshots: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='session_snapshots')",
        [],
        |r| r.get(0),
    )?;
    let has_items: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='session_items')",
        [],
        |r| r.get(0),
    )?;
    let has_deliveries: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='review_deliveries')",
        [],
        |r| r.get(0),
    )?;
    if !has_snapshots {
        conn.execute_batch("PRAGMA auto_vacuum = INCREMENTAL; PRAGMA journal_mode = WAL;")?;
    }
    conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA synchronous = NORMAL;")?;
    if !has_snapshots || !has_items || !has_deliveries {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS session_snapshots(id TEXT PRIMARY KEY, expires INTEGER NOT NULL, snapshot TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS session_items(session TEXT NOT NULL, vid TEXT NOT NULL, evidence TEXT NOT NULL, PRIMARY KEY(session, vid), FOREIGN KEY(session) REFERENCES session_snapshots(id) ON DELETE CASCADE);
             CREATE TABLE IF NOT EXISTS review_deliveries(token TEXT PRIMARY KEY, session TEXT NOT NULL, vid TEXT NOT NULL, evidence TEXT NOT NULL, FOREIGN KEY(session) REFERENCES session_snapshots(id) ON DELETE CASCADE);
             CREATE INDEX IF NOT EXISTS review_delivery_session ON review_deliveries(session, vid);",
        )?;
    }
    Ok(conn)
}
fn sidecar_readonly(config: &Config) -> Result<rusqlite::Connection> {
    db::open_readonly(&config.review_sessions_db())
}
fn sidecar_has_items_table(conn: &rusqlite::Connection) -> bool {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='session_items')",
        [],
        |r| r.get(0),
    )
    .unwrap_or(false)
}
fn sidecar_has_deliveries_table(conn: &rusqlite::Connection) -> bool {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='review_deliveries')",
        [],
        |r| r.get(0),
    )
    .unwrap_or(false)
}
fn maybe_incremental_vacuum(
    conn: &rusqlite::Connection,
    freelist_threshold: i64,
    max_pages: i64,
) {
    let freelist: i64 = conn
        .pragma_query_value(None, "freelist_count", |r| r.get(0))
        .unwrap_or(0);
    if freelist > freelist_threshold {
        let _ = conn.execute_batch(&format!("PRAGMA incremental_vacuum({max_pages});"));
    }
}
pub fn cleanup_sidecar(config: &Config) -> Result<()> {
    if !config.review_sessions_db().exists() {
        return Ok(());
    }
    let _guard = session_write_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let mut conn = sidecar_write(config)?;
    {
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute("DELETE FROM session_snapshots WHERE expires<=unixepoch()", [])?;
        tx.execute(
            "DELETE FROM session_snapshots WHERE id NOT IN (SELECT id FROM session_snapshots ORDER BY expires DESC, rowid DESC LIMIT ?1)",
            [MAX_ACTIVE_SESSIONS],
        )?;
        tx.commit()?;
    }
    maybe_incremental_vacuum(&conn, 0, 4000);
    Ok(())
}
fn split_material(mut value: Value) -> (String, Value) {
    let manual = value
        .as_object_mut()
        .and_then(|o| o.remove("manual_tasks"))
        .unwrap_or(Value::Null);
    (crate::model::hash(&value), manual)
}
fn compact_clusters(value: Value) -> Value {
    let Some(items) = value.as_array() else {
        return json!([]);
    };
    Value::Array(
        items
            .iter()
            .map(|c| {
                let vids: Vec<&str> = c["vids"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|v| v.as_str().or_else(|| v["vid"].as_str()))
                    .collect();
                json!({"cid": c["cid"], "vids": vids})
            })
            .collect(),
    )
}
const FILE_CACHE_CAP: usize = 8;

#[derive(Clone, PartialEq, Eq)]
struct FileStamp {
    dev: u64,
    ino: u64,
    len: u64,
    mtime_ns: String,
    ctime: i64,
    ctime_nsec: i64,
}

fn file_stamp(path: &Path) -> Result<Option<FileStamp>> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    if meta.file_type().is_symlink() {
        return Err(Error::conflict("元数据不能是符号链接").at(path.display().to_string()));
    }
    Ok(Some(FileStamp {
        dev: meta.dev(),
        ino: meta.ino(),
        len: meta.len(),
        mtime_ns: crate::files::mtime_ns(&meta),
        ctime: meta.ctime(),
        ctime_nsec: meta.ctime_nsec(),
    }))
}

fn cached_stamped_file<T>(
    cache: &Mutex<VecDeque<(PathBuf, FileStamp, Arc<T>)>>,
    path: &Path,
    empty: impl FnOnce() -> T,
    loader: impl FnOnce() -> Result<T>,
) -> Result<Arc<T>> {
    let evict = || {
        if let Ok(mut guard) = cache.lock() {
            guard.retain(|(p, _, _)| p != path);
        }
    };
    let stamp = match file_stamp(path) {
        Ok(Some(stamp)) => stamp,
        Ok(None) => {
            evict();
            return Ok(Arc::new(empty()));
        }
        Err(e) => {
            evict();
            return Err(e);
        }
    };
    if let Ok(mut guard) = cache.lock()
        && let Some(pos) = guard.iter().position(|(p, s, _)| p == path && s == &stamp)
    {
        let hit = guard.remove(pos).unwrap();
        let val = hit.2.clone();
        guard.push_back(hit);
        return Ok(val);
    }
    let loaded = match loader() {
        Ok(val) => Arc::new(val),
        Err(e) => {
            evict();
            return Err(e);
        }
    };
    if let Ok(mut guard) = cache.lock() {
        guard.retain(|(p, _, _)| p != path);
        while guard.len() >= FILE_CACHE_CAP {
            guard.pop_front();
        }
        guard.push_back((path.to_path_buf(), stamp, loaded.clone()));
    }
    Ok(loaded)
}

fn archived_vids_cache() -> &'static Mutex<VecDeque<(PathBuf, FileStamp, Arc<BTreeSet<String>>)>> {
    static CACHE: OnceLock<Mutex<VecDeque<(PathBuf, FileStamp, Arc<BTreeSet<String>>)>>> =
        OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(VecDeque::new()))
}

fn archived_vids(config: &Config) -> Result<Arc<BTreeSet<String>>> {
    #[derive(Deserialize)]
    struct ArchiveRow {
        #[serde(default)]
        vid: Option<String>,
        #[serde(default)]
        undone: Value,
    }
    let path = config.base.join("archive-history.json");
    cached_stamped_file(
        archived_vids_cache(),
        &path,
        BTreeSet::new,
        || {
            let Some(raw) = crate::files::read_optional(&path)? else {
                return Ok(BTreeSet::new());
            };
            match serde_json::from_str::<Vec<ArchiveRow>>(&raw) {
                Ok(rows) => Ok(rows
                    .into_iter()
                    .filter(|r| r.undone != Value::Bool(true))
                    .filter_map(|r| r.vid)
                    .collect()),
                Err(_) => {
                    let value = review::read(config, "archive-history.json", json!([]))?.1;
                    Ok(value
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|r| r["undone"] != true)
                        .filter_map(|r| r["vid"].as_str().map(str::to_owned))
                        .collect())
                }
            }
        },
    )
}

fn clusters_cache() -> &'static Mutex<VecDeque<(PathBuf, FileStamp, Arc<Value>)>> {
    static CACHE: OnceLock<Mutex<VecDeque<(PathBuf, FileStamp, Arc<Value>)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(VecDeque::new()))
}

fn cached_clusters(config: &Config) -> Result<Arc<Value>> {
    let path = config.base.join("clusters.json");
    cached_stamped_file(
        clusters_cache(),
        &path,
        || json!([]),
        || Ok(compact_clusters(review::read(config, "clusters.json", json!([]))?.1)),
    )
}
fn material_matches(live: Value, e: &Value) -> bool {
    if let Some(expected) = e.get("material_hash").and_then(Value::as_str) {
        let (actual_hash, actual_manual) = split_material(live);
        actual_hash == expected && actual_manual == e["manual_tasks"]
    } else {
        live == e["material"]
    }
}
fn text_matches(live_task: &Value, e: &Value) -> bool {
    if e.get("text").is_some_and(|v| !v.is_null()) {
        text_evidence(live_task) == e["text"]
    } else {
        text_evidence(live_task) == text_evidence(&e["task"])
    }
}
fn session_write_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}
fn now(conn: &rusqlite::Connection) -> Result<i64> {
    Ok(conn.query_row("SELECT unixepoch()", [], |r| r.get(0))?)
}
fn has_watermark_candidate(data: &Value) -> bool {
    data["candidates"].as_array().is_some_and(|cs| {
        cs.iter().any(|c| {
            c["sources"]
                .as_array()
                .is_some_and(|ss| ss.iter().any(|s| s == "watermark"))
        })
    })
}
pub fn available(t: &Value) -> bool {
    let stage = t["stage"].as_str().unwrap_or("");
    let retry_stage = t["retry_stage"].as_str().unwrap_or("");
    review::number(&t["manual"]).unwrap_or(0) != 0
        || stage == "review"
        || (t["data"]["ready"] == true
            && (stage == "queued"
                || stage == "text"
                || stage == "face"
                || stage == "face_queued"
                || (stage == "retry" && (retry_stage == "face" || retry_stage == "text"))
                || (stage == "error" && retry_stage == "face")))
}
fn public(mut t: Value) -> Result<Value> {
    for v in t.as_object_mut().unwrap().values_mut() {
        if let Some(n) = review::number(v) {
            *v = json!(n)
        }
    }
    t["data"] = review::task_data(&t)?;
    Ok(t)
}
fn candidate_names(data: &Value) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for candidate in data["candidates"]
        .as_array()
        .into_iter()
        .flatten()
        .chain(data["folder"].as_array().into_iter().flatten())
        .chain(data["text_candidates"].as_array().into_iter().flatten())
        .chain(data["face"]["matches"].as_array().into_iter().flatten())
    {
        if let Some(name) = candidate["name"].as_str().filter(|name| !name.is_empty()) {
            names.insert(name.to_owned());
        }
    }
    if let Some(name) = data["suggested"].as_str().filter(|name| !name.is_empty()) {
        names.insert(name.to_owned());
    }
    names
}
#[derive(Default)]
struct LightFace {
    unknown_group: Option<String>,
    matches: Value,
}

enum FaceField {
    UnknownGroup,
    Matches,
    Ignore,
}

impl<'de> Deserialize<'de> for FaceField {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct FieldVisitor;
        impl<'de> de::Visitor<'de> for FieldVisitor {
            type Value = FaceField;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("face object key")
            }
            fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<FaceField, E> {
                Ok(match v {
                    "unknown_group" => FaceField::UnknownGroup,
                    "matches" => FaceField::Matches,
                    _ => FaceField::Ignore,
                })
            }
        }
        deserializer.deserialize_identifier(FieldVisitor)
    }
}

impl<'de> Deserialize<'de> for LightFace {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct FaceVisitor;
        impl<'de> de::Visitor<'de> for FaceVisitor {
            type Value = LightFace;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("face metadata or null")
            }
            fn visit_unit<E: de::Error>(self) -> std::result::Result<LightFace, E> {
                Ok(LightFace::default())
            }
            fn visit_none<E: de::Error>(self) -> std::result::Result<LightFace, E> {
                Ok(LightFace::default())
            }
            fn visit_bool<E: de::Error>(self, _: bool) -> std::result::Result<LightFace, E> {
                Ok(LightFace::default())
            }
            fn visit_i64<E: de::Error>(self, _: i64) -> std::result::Result<LightFace, E> {
                Ok(LightFace::default())
            }
            fn visit_u64<E: de::Error>(self, _: u64) -> std::result::Result<LightFace, E> {
                Ok(LightFace::default())
            }
            fn visit_f64<E: de::Error>(self, _: f64) -> std::result::Result<LightFace, E> {
                Ok(LightFace::default())
            }
            fn visit_str<E: de::Error>(self, _: &str) -> std::result::Result<LightFace, E> {
                Ok(LightFace::default())
            }
            fn visit_seq<A: de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> std::result::Result<LightFace, A::Error> {
                while seq.next_element::<de::IgnoredAny>()?.is_some() {}
                Ok(LightFace::default())
            }
            fn visit_map<A: de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> std::result::Result<LightFace, A::Error> {
                let mut unknown_group = None;
                let mut matches = Value::Null;
                while let Some(key) = map.next_key::<FaceField>()? {
                    match key {
                        FaceField::UnknownGroup => {
                            if let Value::String(s) = map.next_value::<Value>()? {
                                unknown_group = Some(s);
                            }
                        }
                        FaceField::Matches => {
                            matches = map.next_value::<Value>()?;
                        }
                        FaceField::Ignore => {
                            let _: de::IgnoredAny = map.next_value()?;
                        }
                    }
                }
                Ok(LightFace {
                    unknown_group,
                    matches,
                })
            }
        }
        deserializer.deserialize_any(FaceVisitor)
    }
}

#[derive(Deserialize)]
struct LightTaskData {
    #[serde(default)]
    ready: Value,
    #[serde(default)]
    kind: Value,
    #[serde(default)]
    suggested: Value,
    #[serde(default)]
    candidates: Value,
    #[serde(default)]
    text_candidates: Value,
    #[serde(default)]
    face: LightFace,
}

struct LightweightScan {
    items: Vec<Value>,
    live_ids: BTreeSet<String>,
    waiting: usize,
    refreshing: usize,
}

fn sql_ref_to_public_value(value: rusqlite::types::ValueRef<'_>) -> rusqlite::Result<Value> {
    use rusqlite::types::{Type, ValueRef};
    match value {
        ValueRef::Null => Ok(Value::Null),
        ValueRef::Integer(n) => Ok(json!(n)),
        ValueRef::Real(n) if n.is_finite() => Ok(json!(n)),
        ValueRef::Real(_) => Err(rusqlite::Error::FromSqlConversionFailure(
            0,
            Type::Real,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "non-finite SQLite REAL cannot be recorded losslessly",
            )),
        )),
        ValueRef::Text(s) => Ok(json!(std::str::from_utf8(s).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(0, Type::Text, Box::new(error))
        })?)),
        ValueRef::Blob(b) => Ok(json!({"$blob": b})),
    }
}

fn excluded_vids_and_paths(config: &Config) -> Result<(BTreeSet<String>, BTreeSet<String>)> {
    let queue = review::read(config, "integration-queue.json", json!([]))?.1;
    let archived = archived_vids(config)?;
    let queue_vids: BTreeSet<String> = queue
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r["vid"].as_str().map(str::to_owned))
        .collect();
    let sources = review::sources_for_vids(config, &queue_vids)?;
    let paths: BTreeSet<_> = queue_vids
        .iter()
        .filter_map(|v| review::source(config, v, &sources))
        .collect();
    let mut excluded_vids = queue_vids;
    excluded_vids.extend(archived.iter().cloned());
    Ok((excluded_vids, paths))
}

fn load_lightweight_tasks(
    config: &Config,
    aliases: &Value,
    health: &HealthReader,
) -> Result<LightweightScan> {
    let path = config.base.join("processing/processing.db");
    if !path.exists() {
        return Ok(LightweightScan {
            items: vec![],
            live_ids: BTreeSet::new(),
            waiting: 0,
            refreshing: 0,
        });
    }
    let (excluded_vids, paths) = excluded_vids_and_paths(config)?;
    let conn = db::open_readonly(&path)?;
    let cols = db::columns(&conn, "tasks")?;
    let opt_col = |c: &'static str| -> &'static str {
        if cols.iter().any(|x| x == c) { c } else { "NULL" }
    };
    let sql = format!(
        "SELECT vid, path, stage, manual, deferred, note, data, {}, {}, {}, {} \
         FROM tasks WHERE stage!='excluded' \
         ORDER BY priority DESC, is_new DESC, created_at DESC, vid",
        opt_col("name"),
        opt_col("size"),
        opt_col("mtime_ns"),
        opt_col("retry_stage"),
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query([])?;
    let mut live_ids = BTreeSet::new();
    let mut staged: Vec<(bool, Value)> = vec![];
    let mut waiting = 0usize;
    let mut refreshing = 0usize;
    while let Some(row) = rows.next()? {
        let vid = row.get::<_, Option<String>>(0)?.unwrap_or_default();
        let row_path = row.get::<_, Option<String>>(1)?.unwrap_or_default();
        if excluded_vids.contains(&vid) || paths.contains(&row_path) {
            continue;
        }
        live_ids.insert(vid.clone());
        let stage = row.get::<_, Option<String>>(2)?.unwrap_or_default();
        let manual_val = sql_ref_to_public_value(row.get_ref(3)?)?;
        let manual = review::number(&manual_val).unwrap_or(0);
        if stage != "review" && manual == 0 {
            waiting += 1;
        }
        let deferred_val = sql_ref_to_public_value(row.get_ref(4)?)?;
        let deferred = review::number(&deferred_val).unwrap_or(0);
        let note = sql_ref_to_public_value(row.get_ref(5)?)?;
        let raw_data = match row.get_ref(6)? {
            rusqlite::types::ValueRef::Text(bytes) => bytes,
            _ => return Err(Error::conflict("任务数据无效")),
        };
        let name = sql_ref_to_public_value(row.get_ref(7)?)?;
        let size = sql_ref_to_public_value(row.get_ref(8)?)?;
        let mtime_ns = sql_ref_to_public_value(row.get_ref(9)?)?;
        let retry_stage = match row.get_ref(10)? {
            rusqlite::types::ValueRef::Text(s) => std::str::from_utf8(s).ok().map(str::to_owned),
            _ => None,
        };
        let light: LightTaskData = serde_json::from_slice(raw_data)?;
        let rs = retry_stage.as_deref().unwrap_or("");
        let is_available = manual != 0
            || stage == "review"
            || (light.ready == Value::Bool(true)
                && (stage == "queued"
                    || stage == "text"
                    || stage == "face"
                    || stage == "face_queued"
                    || (stage == "retry" && (rs == "face" || rs == "text"))
                    || (stage == "error" && rs == "face")));
        if !is_available {
            continue;
        }
        if stage != "review" && manual == 0 {
            refreshing += 1;
        }
        if health.is_hidden(config, &vid, &row_path)? {
            continue;
        }
        let mut light_data = json!({
            "kind": light.kind,
            "suggested": light.suggested,
            "candidates": light.candidates,
            "text_candidates": light.text_candidates,
            "face": {
                "matches": light.face.matches,
            },
        });
        canonical_fields(&mut light_data, "", aliases)?;
        crate::processing_result::re_resolve_task_data(&mut light_data);
        if let Some(obj) = light_data.as_object_mut() {
            obj.remove("text_candidates");
            obj.remove("face");
        }
        let mut hasher = Sha256::new();
        hasher.update(raw_data);
        hasher.update([0u8]);
        if let Ok(resolved_bytes) = serde_json::to_vec(&light_data) {
            hasher.update(&resolved_bytes);
        }
        let full_hex = format!("{:x}", hasher.finalize());
        let data_digest = &full_hex[..32];
        let grp = group_from(manual, &light_data, light.face.unknown_group.as_deref());
        let has_wm = has_watermark_candidate(&light_data);
        staged.push((
            has_wm,
            json!({
                "vid": vid,
                "path": row_path,
                "name": name,
                "size": size,
                "mtime_ns": mtime_ns,
                "manual": manual,
                "deferred": deferred,
                "note": note,
                "kind": light_data["kind"],
                "suggested": light_data["suggested"],
                "candidates": light_data["candidates"],
                "group": grp,
                "data_digest": data_digest,
                "pending_seen": deferred == 0,
                "deferred_seen": deferred != 0,
            }),
        ));
    }
    staged.sort_by_key(|(has_wm, _)| !*has_wm);
    let mut items = Vec::with_capacity(staged.len());
    for (index, (_, mut item)) in staged.into_iter().enumerate() {
        let key = if item["pending_seen"] == true {
            "pending_order"
        } else {
            "deferred_order"
        };
        item[key] = json!(index);
        items.push(item);
    }
    assign_folder_groups(config, &mut items);
    Ok(LightweightScan {
        items,
        live_ids,
        waiting,
        refreshing,
    })
}

fn assign_folder_groups(config: &Config, items: &mut [Value]) {
    let mut counts: BTreeMap<(bool, String), usize> = BTreeMap::new();
    let mut parts_by_idx: Vec<Option<(bool, String, String)>> = vec![None; items.len()];
    for (idx, item) in items.iter().enumerate() {
        if item["group"]["key"].as_str() != Some("none") {
            continue;
        }
        let deferred = item["deferred"].as_i64().unwrap_or(0) != 0;
        if let Some((key, leaf)) =
            review::folder_group_parts(config, item["path"].as_str().unwrap_or(""))
        {
            *counts.entry((deferred, key.clone())).or_default() += 1;
            parts_by_idx[idx] = Some((deferred, key, leaf));
        }
    }
    let mut leaf_keys: BTreeMap<(bool, String), BTreeSet<String>> = BTreeMap::new();
    for ((deferred, key), count) in &counts {
        if *count < 2 {
            continue;
        }
        if let Some(leaf) = key
            .strip_prefix("folder:")
            .and_then(|rel| rel.rsplit('/').next())
        {
            leaf_keys
                .entry((*deferred, leaf.to_owned()))
                .or_default()
                .insert(key.clone());
        }
    }
    for (idx, maybe_part) in parts_by_idx.into_iter().enumerate() {
        let Some((deferred, key, leaf)) = maybe_part else {
            continue;
        };
        if counts.get(&(deferred, key.clone())).copied().unwrap_or(0) < 2 {
            continue;
        }
        let multi_leaf = leaf_keys
            .get(&(deferred, leaf.clone()))
            .is_some_and(|keys| keys.len() > 1);
        let label = if multi_leaf {
            let rel = key.strip_prefix("folder:").unwrap_or(&key);
            let segs: Vec<&str> = rel.split('/').collect();
            if segs.len() >= 2 {
                format!("{}/{}", segs[segs.len() - 2], segs[segs.len() - 1])
            } else {
                leaf
            }
        } else {
            leaf
        };
        items[idx]["group"] = json!({"key": key, "label": label, "kind": "none"});
    }
}

#[cfg(test)]
fn tasks(config: &Config, aliases: &Value) -> Result<Vec<Value>> {
    tasks_filtered(config, aliases, None, false)
}
fn tasks_for_review(config: &Config, aliases: &Value) -> Result<Vec<Value>> {
    tasks_filtered(config, aliases, None, true)
}
fn tasks_filtered(
    config: &Config,
    aliases: &Value,
    only_vids: Option<&BTreeSet<String>>,
    skip_unavailable_resolve: bool,
) -> Result<Vec<Value>> {
    let path = config.base.join("processing/processing.db");
    if !path.exists() {
        return Ok(vec![]);
    }
    let (excluded_vids, paths) = excluded_vids_and_paths(config)?;
    let conn = db::open_readonly(&path)?;
    let raw_rows = if let Some(filter) = only_vids {
        if filter.is_empty() {
            return Ok(vec![]);
        }
        let placeholders = std::iter::repeat_n("?", filter.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT * FROM tasks WHERE stage!='excluded' AND vid IN ({placeholders}) ORDER BY priority DESC,is_new DESC,created_at DESC,vid"
        );
        db::rows(&conn, &sql, rusqlite::params_from_iter(filter.iter()))?
    } else {
        db::rows(
            &conn,
            "SELECT * FROM tasks WHERE stage!='excluded' ORDER BY priority DESC,is_new DESC,created_at DESC,vid",
            [],
        )?
    };
    let mut out = vec![];
    for row in raw_rows {
        let id = row["vid"].as_str().unwrap_or("");
        if excluded_vids.contains(id) || paths.contains(row["path"].as_str().unwrap_or("")) {
            continue;
        }
        let mut t = public(row)?;
        if !skip_unavailable_resolve || available(&t) {
            t["__review_raw_people"] = json!(candidate_names(&t["data"]));
            canonical_fields(&mut t["data"], "", aliases)?;
            crate::processing_result::re_resolve_task_data(&mut t["data"]);
        }
        out.push(t)
    }
    Ok(out)
}
fn waiting_count(config: &Config) -> Result<usize> {
    let path = config.base.join("processing/processing.db");
    if !path.exists() {
        return Ok(0);
    }
    let (excluded_vids, paths) = excluded_vids_and_paths(config)?;
    let conn = db::open_readonly(&path)?;
    let mut stmt = conn.prepare(
        "SELECT vid,path FROM tasks WHERE stage!='excluded' AND stage!='review' AND manual=0",
    )?;
    let mut rows = stmt.query([])?;
    let mut count = 0usize;
    while let Some(row) = rows.next()? {
        let id: String = row.get::<_, Option<String>>(0)?.unwrap_or_default();
        let row_path: String = row.get::<_, Option<String>>(1)?.unwrap_or_default();
        if excluded_vids.contains(&id) || paths.contains(&row_path) {
            continue;
        }
        count += 1;
    }
    Ok(count)
}
fn canonical_fields(value: &mut Value, key: &str, aliases: &Value) -> Result<()> {
    if ["name", "creator", "suggested"].contains(&key) && value.is_string() {
        let mut name = value.as_str().unwrap().to_owned();
        let mut seen = BTreeSet::new();
        while let Some(next) = aliases[&name].as_str() {
            if !seen.insert(name.clone()) {
                return Err(Error::conflict("人物别名包含循环"));
            }
            name = next.to_owned()
        }
        *value = json!(name);
    } else if let Some(rows) = value.as_array_mut() {
        for row in rows {
            canonical_fields(row, "", aliases)?
        }
    } else if let Some(fields) = value.as_object_mut() {
        for (key, v) in fields {
            canonical_fields(v, key, aliases)?
        }
    }
    Ok(())
}
fn group_from(manual: i64, data: &Value, unknown_group: Option<&str>) -> Value {
    let kind = if manual != 0 {
        "manual"
    } else {
        data["kind"].as_str().unwrap_or("none")
    };
    let suggested = data["suggested"].as_str().unwrap_or("");
    let face = unknown_group.unwrap_or("");
    let (key, label) = if kind == "suggested" && !suggested.is_empty() {
        (format!("person:{suggested}"), suggested.into())
    } else if kind == "none" && !face.is_empty() {
        (
            format!("face:{face}"),
            format!(
                "未命名相似组 {}",
                face.chars()
                    .rev()
                    .take(5)
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect::<String>()
            ),
        )
    } else {
        (
            kind.into(),
            match kind {
                "manual" => "人工接管",
                "conflict" => "证据冲突",
                _ => "无候选人物",
            }
            .into(),
        )
    };
    json!({"key":key,"label":label,"kind":kind})
}
fn group(t: &Value) -> Value {
    group_from(
        review::number(&t["manual"]).unwrap_or(0),
        &t["data"],
        t["data"]["face"]["unknown_group"].as_str(),
    )
}
fn text_evidence(t: &Value) -> Value {
    let w = &t["data"]["watermark"];
    json!({"folder":t["data"]["folder"],"watermark":if w.is_null(){Value::Null}else{json!({"pipeline":w["pipeline"],"status":w["status"],"candidates":w["candidates"]})}})
}
struct HealthReader {
    candidates: Arc<BTreeSet<String>>,
}
impl HealthReader {
    fn new(config: &Config) -> Result<Self> {
        Ok(Self {
            candidates: crate::video_health::hidden_candidates(config)?,
        })
    }
    fn is_hidden(&self, config: &Config, id: &str, path: &str) -> Result<bool> {
        if self.candidates.contains(id) {
            std::fs::metadata(path)
                .ok()
                .map(|s| review::health_hidden(config, id, path, &s))
                .transpose()
                .map(|opt| opt.unwrap_or(false))
        } else {
            Ok(false)
        }
    }
    fn state(&self, config: &Config, t: &Value) -> Result<Value> {
        let id = t["vid"].as_str().unwrap_or("");
        let path = t["path"].as_str().unwrap_or("");
        let hidden = self.is_hidden(config, id, path)?;
        Ok(json!({"hidden":hidden}))
    }
}
#[derive(Clone, PartialEq, Eq)]
struct DispositionsStamp {
    dev: u64,
    ino: u64,
    count: i64,
    max_rowid: i64,
    sum_rowid: i64,
}
fn dispositions_cache()
-> &'static Mutex<VecDeque<(PathBuf, DispositionsStamp, Arc<BTreeMap<String, Value>>)>> {
    static CACHE: OnceLock<
        Mutex<VecDeque<(PathBuf, DispositionsStamp, Arc<BTreeMap<String, Value>>)>>,
    > = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(VecDeque::new()))
}
fn dispositions(config: &Config) -> Result<Arc<BTreeMap<String, Value>>> {
    let db_path = config.state_db();
    let conn = schema_readonly(config)?;
    let meta = std::fs::metadata(&db_path)?;
    let (count, max_rowid, sum_rowid): (i64, i64, i64) = conn.query_row(
        "SELECT COUNT(*), COALESCE(MAX(rowid), 0), COALESCE(SUM(rowid), 0) \
         FROM operations INDEXED BY idx_operations_complete_action \
         WHERE status='complete' \
           AND json_extract(plan,'$.action') IN ('queue-confirm','queue-undo','review-note','review-defer','review-restore','review-delete','claim-decide')",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let stamp = DispositionsStamp {
        dev: meta.dev(),
        ino: meta.ino(),
        count,
        max_rowid,
        sum_rowid,
    };
    if let Ok(mut guard) = dispositions_cache().lock()
        && let Some(pos) = guard
            .iter()
            .position(|(p, s, _)| p == &db_path && s == &stamp)
    {
        let hit = guard.remove(pos).unwrap();
        let val = hit.2.clone();
        guard.push_back(hit);
        return Ok(val);
    }
    let mut stmt = conn.prepare(
        "SELECT id, \
                CASE WHEN json_type(plan,'$.request.payload.vids')='array' THEN json_extract(plan,'$.request.payload.vids') END, \
                CASE WHEN json_type(plan,'$.request.payload.vid')='text' THEN json_extract(plan,'$.request.payload.vid') END \
         FROM operations INDEXED BY idx_operations_complete_action \
         WHERE status='complete' \
           AND json_extract(plan,'$.action') IN ('queue-confirm','queue-undo','review-note','review-defer','review-restore','review-delete','claim-decide') \
         ORDER BY rowid",
    )?;
    let mut rows = stmt.query([])?;
    let mut result = BTreeMap::new();
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        if let rusqlite::types::ValueRef::Text(raw_vids) = row.get_ref(1)?
            && let Ok(Value::Array(arr)) = serde_json::from_slice::<Value>(raw_vids)
        {
            for vid in arr {
                if let Some(vid) = vid.as_str() {
                    result.insert(vid.to_owned(), Value::String(id.clone()));
                }
            }
        }
        if let rusqlite::types::ValueRef::Text(single) = row.get_ref(2)?
            && let Ok(vid) = std::str::from_utf8(single)
        {
            result.insert(vid.to_owned(), Value::String(id.clone()));
        }
    }
    let arc = Arc::new(result);
    if let Ok(mut guard) = dispositions_cache().lock() {
        guard.retain(|(p, _, _)| p != &db_path);
        while guard.len() >= FILE_CACHE_CAP {
            guard.pop_front();
        }
        guard.push_back((db_path, stamp, arc.clone()));
    }
    Ok(arc)
}
pub fn validate_undo(config: &Config, body: &Value) -> Result<()> {
    let Some(session) = body["review_session"].as_str() else {
        return Ok(());
    };
    read(config, session)?;
    let conn = schema_readonly(config)?;
    let queue = review::read(config, "integration-queue.json", json!([]))?.1;
    for vid in review::vids(&body["vids"])? {
        let batch = queue
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["vid"] == vid)
            .and_then(|r| r["batch_id"].as_str())
            .ok_or_else(changed)?;
        let owner:Option<String>=conn.query_row("SELECT json_extract(plan,'$.request.payload.review_session') FROM operations WHERE id=? AND status='complete'",[batch],|r|r.get(0)).optional()?.flatten();
        if owner.as_deref() != Some(session) {
            return Err(Error::conflict(
                "这次归名已被其他会话处置，请核对待归档记录",
            ));
        }
    }
    Ok(())
}
fn evidence(
    config: &Config,
    t: &Value,
    material: &shared_metadata::MaterialReader,
    health: &HealthReader,
    dispositions: &BTreeMap<String, Value>,
) -> Result<Value> {
    let mut names = BTreeSet::new();
    for c in t["data"]["candidates"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .chain(t["data"]["folder"].as_array().unwrap_or(&vec![]))
    {
        if let Some(n) = c["name"].as_str() {
            match material.canonical(n) {
                Ok(_) => {
                    names.insert(n.to_owned());
                }
                Err(e) if e.status == 400 => {}
                Err(e) => return Err(e),
            }
        }
    }
    if let Some(n) = t["data"]["suggested"].as_str() {
        match material.canonical(n) {
            Ok(_) => {
                names.insert(n.to_owned());
            }
            Err(e) if e.status == 400 => {}
            Err(e) => return Err(e),
        }
    }
    let keys: Vec<_> = t["data"]["watermark"]["candidates"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .filter_map(|c| c["key"].as_str().map(str::to_owned))
        .collect();
    let mut scope = json!({"vids":[t["vid"]],"people":names,"watermark_keys":keys});
    if t["__review_raw_people"]
        .as_array()
        .is_some_and(|rows| !rows.is_empty())
    {
        scope["alias_inputs"] = t["__review_raw_people"].clone();
    }
    let mut displayed = t.clone();
    displayed
        .as_object_mut()
        .unwrap()
        .remove("__review_raw_people");
    let (material_hash, manual_tasks) = split_material(material.material_only(config, &scope)?);
    Ok(json!({
        "task": displayed,
        "group": group(t),
        "scope": scope,
        "material_hash": material_hash,
        "manual_tasks": manual_tasks,
        "health": health.state(config, t)?,
        "disposition": dispositions.get(t["vid"].as_str().unwrap_or("")),
        "pending_seen": review::number(&t["deferred"]).unwrap_or(0) == 0,
        "deferred_seen": review::number(&t["deferred"]).unwrap_or(0) != 0,
    }))
}
fn read_session(config: &Config, id: &str) -> Result<(Arc<Value>, bool)> {
    let sidecar_path = config.review_sessions_db();
    if sidecar_path.exists() {
        let sconn = sidecar_readonly(config)?;
        let expires: Option<i64> = sconn
            .query_row(
                "SELECT expires FROM session_snapshots WHERE id=? AND expires>unixepoch()",
                [id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(expires) = expires {
            if let Ok(mut guard) = session_cache().lock()
                && let Some(pos) = guard.iter().position(|entry| {
                    entry.db_path == sidecar_path && entry.id == id && entry.expires == expires
                })
            {
                let hit = guard.remove(pos).unwrap();
                let snapshot = hit.snapshot.clone();
                guard.push_back(hit);
                return Ok((snapshot, true));
            }
            let raw: Option<String> = sconn
                .query_row(
                    "SELECT snapshot FROM session_snapshots WHERE id=? AND expires>unixepoch()",
                    [id],
                    |r| r.get(0),
                )
                .optional()?;
            let value: Value = serde_json::from_str(&raw.ok_or_else(expired)?)?;
            if !matches!(value["schema_version"].as_i64(), Some(1 | 2 | 3)) {
                return Err(expired());
            }
            let arc = Arc::new(value);
            session_cache_put(&sidecar_path, id, expires, arc.clone());
            return Ok((arc, true));
        }
    }

    let db_path = config.state_db();
    let conn = schema_readonly(config)?;
    let expires: Option<i64> = conn
        .query_row(
            "SELECT expires FROM review_sessions WHERE id=? AND expires>unixepoch()",
            [id],
            |r| r.get(0),
        )
        .optional()?;
    let expires = expires.ok_or_else(expired)?;
    if let Ok(mut guard) = session_cache().lock()
        && let Some(pos) = guard
            .iter()
            .position(|entry| entry.db_path == db_path && entry.id == id && entry.expires == expires)
    {
        let hit = guard.remove(pos).unwrap();
        let snapshot = hit.snapshot.clone();
        guard.push_back(hit);
        return Ok((snapshot, false));
    }
    let raw: Option<String> = conn
        .query_row(
            "SELECT snapshot FROM review_sessions WHERE id=? AND expires>unixepoch()",
            [id],
            |r| r.get(0),
        )
        .optional()?;
    let value: Value = serde_json::from_str(&raw.ok_or_else(expired)?)?;
    if value["sidecar"] == true {
        return Err(expired());
    }
    if value["schema_version"] != 1 && value["schema_version"] != 2 {
        return Err(expired());
    }
    let arc = Arc::new(value);
    session_cache_put(&db_path, id, expires, arc.clone());
    Ok((arc, false))
}
fn read(config: &Config, id: &str) -> Result<Arc<Value>> {
    Ok(read_session(config, id)?.0)
}
struct AmendedSnapshot {
    base: Arc<Value>,
    overrides: BTreeMap<usize, Value>,
}
impl AmendedSnapshot {
    fn is_v3(&self) -> bool {
        self.base["schema_version"] == 3
    }
    fn base_items(&self) -> &[Value] {
        let key = if self.is_v3() { "items" } else { "evidence" };
        self.base[key].as_array().map_or(&[], Vec::as_slice)
    }
    fn base_item(&self, index: usize) -> &Value {
        &self.base_items()[index]
    }
    fn is_amended(&self, index: usize) -> bool {
        self.overrides.contains_key(&index)
    }
    #[cfg(test)]
    fn iter(&self) -> impl Iterator<Item = &Value> {
        let base = self.base_items();
        (0..base.len()).map(|i| self.overrides.get(&i).unwrap_or(&base[i]))
    }
    fn iter_indexed(&self) -> impl Iterator<Item = (usize, &Value)> {
        let base = self.base_items();
        (0..base.len()).map(|i| (i, self.overrides.get(&i).unwrap_or(&base[i])))
    }
    fn clusters(&self) -> &Value {
        &self.base["clusters"]
    }
}

fn item_vid_opt(e: &Value, is_v3: bool) -> Option<&str> {
    if is_v3 {
        e["vid"].as_str()
    } else {
        e["task"]["vid"].as_str()
    }
}
fn item_vid(e: &Value, is_v3: bool) -> &str {
    item_vid_opt(e, is_v3).unwrap_or("")
}
fn item_path(e: &Value, is_v3: bool) -> &str {
    if is_v3 {
        e["path"].as_str().unwrap_or("")
    } else {
        e["task"]["path"].as_str().unwrap_or("")
    }
}
fn item_name(e: &Value, is_v3: bool) -> &str {
    if is_v3 {
        e["name"].as_str().unwrap_or("")
    } else {
        e["task"]["name"].as_str().unwrap_or("")
    }
}
fn item_manual(e: &Value, is_v3: bool) -> &Value {
    if is_v3 {
        &e["manual"]
    } else {
        &e["task"]["manual"]
    }
}
fn item_deferred(e: &Value, is_v3: bool) -> &Value {
    if is_v3 {
        &e["deferred"]
    } else {
        &e["task"]["deferred"]
    }
}
fn item_bucket(e: &Value, is_v3: bool) -> &str {
    if review::number(item_manual(e, is_v3)).unwrap_or(0) != 0 {
        "manual"
    } else if is_v3 {
        e["kind"].as_str().unwrap_or("none")
    } else {
        e["task"]["data"]["kind"].as_str().unwrap_or("none")
    }
}
fn item_suggested(e: &Value, is_v3: bool) -> &Value {
    if is_v3 {
        &e["suggested"]
    } else {
        &e["task"]["data"]["suggested"]
    }
}
fn item_candidates(e: &Value, is_v3: bool) -> &Value {
    if is_v3 {
        &e["candidates"]
    } else {
        &e["task"]["data"]["candidates"]
    }
}

// Only this session's durable, completed operations amend its presentation. Evidence is never replaced by latest model output.
fn amended(config: &Config, id: &str, snapshot: Arc<Value>) -> Result<AmendedSnapshot> {
    let conn = schema_readonly(config)?;
    let ops = db::rows(
        &conn,
        "SELECT plan FROM operations WHERE status='complete' AND json_extract(plan,'$.request.payload.review_session')=? ORDER BY rowid",
        [id],
    )?;
    if ops.is_empty() {
        return Ok(AmendedSnapshot {
            base: snapshot,
            overrides: BTreeMap::new(),
        });
    }
    let is_v3 = snapshot["schema_version"] == 3;
    let base_items = snapshot[if is_v3 { "items" } else { "evidence" }]
        .as_array()
        .ok_or_else(expired)?;
    let positions: BTreeMap<&str, usize> = base_items
        .iter()
        .enumerate()
        .filter_map(|(i, e)| item_vid_opt(e, is_v3).map(|vid| (vid, i)))
        .collect();
    let mut overrides: BTreeMap<usize, Value> = BTreeMap::new();
    let mut next_pending = base_items.len() as u64;
    let mut next_deferred = next_pending;
    for row in ops {
        let plan: Value = serde_json::from_str(row["plan"].as_str().unwrap())?;
        let body = &plan["request"]["payload"];
        let action = plan["action"].as_str().unwrap_or("");
        let selected: BTreeSet<usize> = body["vids"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str().and_then(|id| positions.get(id)).copied())
            .collect();
        for index in selected {
            let e = overrides
                .entry(index)
                .or_insert_with(|| base_items[index].clone());
            e["disposition"] = plan["id"].clone();
            match action {
                "queue-confirm" | "review-delete" => e["completed"] = json!(true),
                "queue-undo" => {
                    e["completed"] = json!(false);
                    let restored:Option<String>=conn.query_row("SELECT plan FROM operations WHERE status='complete' AND json_extract(plan,'$.action')='queue-restore' AND json_extract(plan,'$.request.payload.parent_id')=?",[plan["id"].as_str().unwrap_or("")],|r|r.get(0)).optional()?;
                    if let Some(raw) = restored {
                        let child: Value = serde_json::from_str(&raw)?;
                        let target_vid = item_vid(e, is_v3);
                        if child["execution"]["data"]["job_updates"]
                            .as_array()
                            .is_some_and(|rows| {
                                rows.iter().any(|r| {
                                    r["vid"].as_str() == Some(target_vid)
                                        && review::number(&r["deferred"]) == Some(0)
                                })
                            })
                        {
                            if e["pending_order"].is_null() {
                                e["pending_order"] = json!(next_pending);
                                next_pending += 1;
                            }
                            e["pending_seen"] = json!(true);
                            if is_v3 {
                                e["deferred"] = json!(0);
                            } else {
                                set_manual(e, "deferred", json!(0));
                            }
                        }
                    }
                }
                "review-note" => {
                    if is_v3 {
                        e["note"] = body["note"].clone();
                    } else {
                        set_manual(e, "note", body["note"].clone());
                    }
                }
                "review-defer" => {
                    if e["deferred_order"].is_null() {
                        e["deferred_order"] = json!(next_deferred);
                        next_deferred += 1;
                    }
                    e["deferred_seen"] = json!(true);
                    if is_v3 {
                        e["deferred"] = json!(1);
                    } else {
                        set_manual(e, "deferred", json!(1));
                    }
                }
                "review-restore" => {
                    if e["pending_order"].is_null() {
                        e["pending_order"] = json!(next_pending);
                        next_pending += 1;
                    }
                    e["pending_seen"] = json!(true);
                    if is_v3 {
                        e["deferred"] = json!(0);
                    } else {
                        set_manual(e, "deferred", json!(0));
                    }
                }
                _ => {}
            }
        }
    }
    Ok(AmendedSnapshot {
        base: snapshot,
        overrides,
    })
}
fn apply_v3_amendment(ev: &mut Value, skel: &Value) {
    if !skel["disposition"].is_null() {
        ev["disposition"] = skel["disposition"].clone();
    }
    if let Some(completed) = skel.get("completed").filter(|v| !v.is_null()) {
        ev["completed"] = completed.clone();
    }
    if !skel["pending_seen"].is_null() {
        ev["pending_seen"] = skel["pending_seen"].clone();
    }
    if !skel["deferred_seen"].is_null() {
        ev["deferred_seen"] = skel["deferred_seen"].clone();
    }
    if !skel["pending_order"].is_null() {
        ev["pending_order"] = skel["pending_order"].clone();
    }
    if !skel["deferred_order"].is_null() {
        ev["deferred_order"] = skel["deferred_order"].clone();
    }
    if ev["task"]["note"] != skel["note"] {
        set_manual(ev, "note", skel["note"].clone());
    }
    let deferred = review::number(&skel["deferred"]).unwrap_or(0);
    if review::number(&ev["task"]["deferred"]).unwrap_or(0) != deferred {
        set_manual(ev, "deferred", json!(deferred));
    }
}
fn copy_v3_order_flags(ev: &mut Value, base_skel: &Value) {
    ev["pending_seen"] = base_skel["pending_seen"].clone();
    ev["deferred_seen"] = base_skel["deferred_seen"].clone();
    if !base_skel["pending_order"].is_null() {
        ev["pending_order"] = base_skel["pending_order"].clone();
    }
    if !base_skel["deferred_order"].is_null() {
        ev["deferred_order"] = base_skel["deferred_order"].clone();
    }
}
fn materialize_v3_items(
    config: &Config,
    miss_items: &[(&str, &Value)],
    health: &HealthReader,
) -> Result<BTreeMap<String, Value>> {
    if miss_items.is_empty() {
        return Ok(BTreeMap::new());
    }
    let miss_vids: BTreeSet<String> = miss_items.iter().map(|(v, _)| (*v).to_owned()).collect();
    let material = shared_metadata::MaterialReader::new_for_vids(config, Some(&miss_vids))?;
    let disps = dispositions(config)?;
    let path = config.base.join("processing/processing.db");
    let mut raw_by_vid: BTreeMap<String, Value> = BTreeMap::new();
    if path.exists() {
        let conn = db::open_readonly(&path)?;
        let vid_list: Vec<&str> = miss_vids.iter().map(String::as_str).collect();
        for chunk in vid_list.chunks(250) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!("SELECT * FROM tasks WHERE vid IN ({placeholders})");
            for row in db::rows(
                &conn,
                &sql,
                rusqlite::params_from_iter(chunk.iter().copied()),
            )? {
                if let Some(vid) = row["vid"].as_str().map(str::to_owned) {
                    raw_by_vid.insert(vid, row);
                }
            }
        }
    }
    let mut out = BTreeMap::new();
    for (vid, base_skel) in miss_items {
        let mut ev = if let Some(row) = raw_by_vid.remove(*vid) {
            let mut t = public(row)?;
            t["__review_raw_people"] = json!(candidate_names(&t["data"]));
            canonical_fields(&mut t["data"], "", material.aliases())?;
            crate::processing_result::re_resolve_task_data(&mut t["data"]);
            evidence(config, &t, &material, health, &disps)?
        } else {
            json!({
                "task": {
                    "vid": base_skel["vid"],
                    "name": base_skel["name"],
                    "path": base_skel["path"],
                    "size": base_skel["size"],
                    "mtime_ns": base_skel["mtime_ns"],
                    "manual": base_skel["manual"],
                    "deferred": base_skel["deferred"],
                    "note": base_skel["note"],
                    "data": {
                        "kind": base_skel["kind"],
                        "suggested": base_skel["suggested"],
                        "candidates": base_skel["candidates"],
                    }
                },
                "group": base_skel["group"],
                "scope": {"vids": [vid], "people": [], "watermark_keys": []},
                "material_hash": "",
                "manual_tasks": [],
                "health": {"hidden": false},
                "disposition": null,
            })
        };
        if !base_skel["group"].is_null() {
            ev["group"] = base_skel["group"].clone();
        }
        copy_v3_order_flags(&mut ev, base_skel);
        out.insert((*vid).to_owned(), ev);
    }
    Ok(out)
}
fn load_v3_session_items(
    conn: &rusqlite::Connection,
    session: &str,
    vids: &[&str],
) -> Result<BTreeMap<String, Value>> {
    let mut out = BTreeMap::new();
    if vids.is_empty() || !sidecar_has_items_table(conn) {
        return Ok(out);
    }
    for chunk in vids.chunks(250) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT vid, evidence FROM session_items WHERE session=? AND vid IN ({placeholders})"
        );
        let mut stmt = conn.prepare(&sql)?;
        let params_iter = std::iter::once(session).chain(chunk.iter().copied());
        let mut rows = stmt.query(rusqlite::params_from_iter(params_iter))?;
        while let Some(row) = rows.next()? {
            let vid: String = row.get(0)?;
            let raw: String = row.get(1)?;
            out.insert(vid, serde_json::from_str(&raw)?);
        }
    }
    Ok(out)
}
fn set_manual_rows(rows: &mut [Value], key: &str, value: &Value) {
    for r in rows {
        r[key] = if key == "note" {
            value.clone()
        } else {
            review::integer(value.as_i64().unwrap_or(0))
        };
    }
}
fn set_manual(e: &mut Value, key: &str, value: Value) {
    e["task"][key] = value.clone();
    if let Some(rows) = e.get_mut("manual_tasks").and_then(Value::as_array_mut) {
        set_manual_rows(rows, key, &value);
    }
    if let Some(rows) = e
        .get_mut("material")
        .and_then(|m| m.get_mut("manual_tasks"))
        .and_then(Value::as_array_mut)
    {
        set_manual_rows(rows, key, &value);
    }
}
pub fn validate(config: &Config, body: &Value) -> Result<Vec<Value>> {
    let session = body["review_session"].as_str().ok_or_else(expired)?;
    read(config, session)?;
    let ids = review::vids(&body["vids"])?;
    let id_set: BTreeSet<String> = ids.iter().cloned().collect();
    let conn = schema_readonly(config)?;
    let sidecar_ro = if config.review_sessions_db().exists() {
        let ro = sidecar_readonly(config)?;
        sidecar_has_deliveries_table(&ro).then_some(ro)
    } else {
        None
    };
    let mut rows = vec![];
    let material = shared_metadata::MaterialReader::new_for_vids(config, Some(&id_set))?;
    let current = tasks_filtered(config, material.aliases(), Some(&id_set), false)?;
    let health = HealthReader::new(config)?;
    let dispositions = dispositions(config)?;
    for vid in ids {
        let token = body["review_tokens"][&vid].as_str().ok_or_else(|| {
            Error::new(
                409,
                "review_not_viewed",
                "所选视频缺少本次已展示证据，请重新打开该页",
            )
        })?;
        let mut raw: Option<String> = if let Some(ref sro) = sidecar_ro {
            sro.query_row(
                "SELECT evidence FROM review_deliveries WHERE token=? AND session=? AND vid=?",
                params![token, session, vid],
                |r| r.get(0),
            )
            .optional()?
        } else {
            None
        };
        if raw.is_none() {
            raw = conn
                .query_row(
                    "SELECT evidence FROM review_deliveries WHERE token=? AND session=? AND vid=?",
                    params![token, session, vid],
                    |r| r.get(0),
                )
                .optional()?;
        }
        let e: Value = serde_json::from_str(
            &raw.ok_or_else(|| Error::new(409, "review_not_viewed", "视频未在本次会话展示"))?,
        )?;
        if let Some(scope) = body.get("review_group").filter(|v| !v.is_null())
            && (scope["key"] != e["group"]["key"]
                || scope["deferred"]
                    != json!(review::number(&e["task"]["deferred"]).unwrap_or(0) != 0))
        {
            return Err(Error::conflict("所选视频不属于本次已展示核对组"));
        }
        let t = current
            .iter()
            .find(|t| t["vid"] == vid)
            .ok_or_else(changed)?;
        // Worker stages (including exhausted inference retries) do not change
        // already delivered human evidence. Material and disposition guards do.
        if ["path", "size", "mtime_ns"]
            .iter()
            .any(|key| t[*key] != e["task"][*key])
            || !text_matches(t, &e)
            || health.state(config, t)? != e["health"]
            || dispositions.get(&vid).cloned().unwrap_or(Value::Null) != e["disposition"]
            || !material_matches(material.material_only(config, &e["scope"])?, &e)
        {
            return Err(changed());
        }
        rows.push(e["task"].clone());
    }
    Ok(rows)
}
pub fn execute(engine: &mut Engine, command: &str, body: Value) -> Result<Value> {
    if engine.store.blocked()? {
        return Err(Error::new(
            503,
            "recovery_required",
            "操作需要恢复核对，暂不能建立核对快照",
        ));
    }
    let config = &engine.config;
    let id = body["session"]
        .as_str()
        .filter(|v| !v.is_empty())
        .map(str::to_owned);
    if command == "review-status" {
        let id = id.ok_or_else(expired)?;
        let (base, _in_sidecar) = read_session(config, &id)?;
        let snapshot = amended(config, &id, base)?;
        let mut changed_count = 0usize;
        let (waiting, refreshing) = if snapshot.is_v3() {
            let active_vids: BTreeSet<&str> = snapshot
                .iter_indexed()
                .filter(|(_, e)| e["completed"] != true)
                .filter_map(|(_, e)| item_vid_opt(e, true))
                .collect();
            let mut mat_rows: BTreeMap<String, String> = BTreeMap::new();
            if !active_vids.is_empty() && config.review_sessions_db().exists() {
                let sro = sidecar_readonly(config)?;
                if sidecar_has_items_table(&sro) {
                    let mut stmt =
                        sro.prepare("SELECT vid, evidence FROM session_items WHERE session=?")?;
                    let mut rows = stmt.query([&id])?;
                    while let Some(row) = rows.next()? {
                        let vid: String = row.get(0)?;
                        if active_vids.contains(vid.as_str()) {
                            let raw: String = row.get(1)?;
                            mat_rows.insert(vid, raw);
                        }
                    }
                }
            }
            let mat_vids: BTreeSet<String> = mat_rows.keys().cloned().collect();
            let mat_reader = if !mat_vids.is_empty() {
                Some(shared_metadata::MaterialReader::new_for_vids(
                    config,
                    Some(&mat_vids),
                )?)
            } else {
                None
            };
            let owned_aliases;
            let live_aliases = if let Some(ref mr) = mat_reader {
                mr.aliases()
            } else {
                owned_aliases = review::read(config, "person-renames.json", json!({}))?.1;
                &owned_aliases
            };
            let health = HealthReader::new(config)?;
            let scan = load_lightweight_tasks(config, live_aliases, &health)?;
            let visible_by_id: BTreeMap<&str, &Value> = snapshot
                .iter_indexed()
                .filter(|(_, e)| e["completed"] != true)
                .filter_map(|(_, e)| item_vid_opt(e, true).map(|vid| (vid, e)))
                .collect();
            for t in &scan.items {
                if !visible_by_id
                    .get(t["vid"].as_str().unwrap_or(""))
                    .is_some_and(|v| {
                        v["data_digest"] == t["data_digest"]
                            && v["path"] == t["path"]
                            && v["size"] == t["size"]
                            && v["mtime_ns"] == t["mtime_ns"]
                            && v["deferred"] == t["deferred"]
                            && v["note"] == t["note"]
                    })
                {
                    changed_count += 1;
                }
            }
            for (idx, skel) in snapshot.iter_indexed().filter(|(_, e)| e["completed"] != true) {
                let vid = item_vid(skel, true);
                let path = item_path(skel, true);
                if !scan.live_ids.contains(vid) || health.is_hidden(config, vid, path)? {
                    changed_count += 1;
                    continue;
                }
                if let (Some(raw_ev), Some(mr)) = (mat_rows.get(vid), &mat_reader) {
                    let mut ev: Value = serde_json::from_str(raw_ev)?;
                    if snapshot.is_amended(idx) {
                        apply_v3_amendment(&mut ev, skel);
                    }
                    if !material_matches(mr.material_only(config, &ev["scope"])?, &ev) {
                        changed_count += 1;
                    }
                }
            }
            (scan.waiting, scan.refreshing)
        } else {
            let material = shared_metadata::MaterialReader::new(config)?;
            let live = tasks_for_review(config, material.aliases())?;
            let health = HealthReader::new(config)?;
            let visible_by_id: BTreeMap<&str, &Value> = snapshot
                .iter_indexed()
                .filter(|(_, e)| e["completed"] != true)
                .filter_map(|(_, e)| e["task"]["vid"].as_str().map(|vid| (vid, &e["task"])))
                .collect();
            let live_ids: BTreeSet<_> = live
                .iter()
                .map(|t| t["vid"].as_str().unwrap_or(""))
                .collect();
            for t in &live {
                if available(t)
                    && health.state(config, t)?["hidden"] != true
                    && !visible_by_id
                        .get(t["vid"].as_str().unwrap_or(""))
                        .is_some_and(|v| {
                            v["data"] == t["data"]
                                && v["path"] == t["path"]
                                && v["size"] == t["size"]
                                && v["mtime_ns"] == t["mtime_ns"]
                                && v["deferred"] == t["deferred"]
                                && v["note"] == t["note"]
                        })
                {
                    changed_count += 1
                }
            }
            for (_, e) in snapshot.iter_indexed().filter(|(_, e)| e["completed"] != true) {
                if !live_ids.contains(e["task"]["vid"].as_str().unwrap_or(""))
                    || !material_matches(material.material_only(config, &e["scope"])?, e)
                    || health.state(config, &e["task"])? != e["health"]
                {
                    changed_count += 1
                }
            }
            let waiting = live
                .iter()
                .filter(|t| t["stage"] != "review" && review::number(&t["manual"]).unwrap_or(0) == 0)
                .count();
            let refreshing = live
                .iter()
                .filter(|t| {
                    available(t)
                        && t["stage"] != "review"
                        && review::number(&t["manual"]).unwrap_or(0) == 0
                })
                .count();
            (waiting, refreshing)
        };
        crate::model::trim_allocator();
        return Ok(
            json!({"session":id,"has_updates":changed_count>0,"changed":changed_count,"waiting":waiting,"refreshing":refreshing}),
        );
    }

    let mut created_ctx: Option<(String, HealthReader)> = None;
    let (id, snapshot, in_sidecar, waiting) = match id {
        Some(id) => {
            let (base, in_sidecar) = read_session(config, &id)?;
            let snap = amended(config, &id, base)?;
            let waiting = waiting_count(config)?;
            (id, snap, in_sidecar, waiting)
        }
        None => {
            let aliases = review::read(config, "person-renames.json", json!({}))?.1;
            let health = HealthReader::new(config)?;
            let scan = load_lightweight_tasks(config, &aliases, &health)?;
            let snapshot_val = json!({"schema_version":3,"items":scan.items});
            let raw = serde_json::to_string(&snapshot_val)?;
            if raw.len() > 64 * 1024 * 1024 {
                return Err(Error::new(
                    503,
                    "review_scope_limit",
                    "核对记录过多，请缩小处理范围",
                ));
            }
            let id = uuid::Uuid::new_v4().to_string();
            let arc = Arc::new(snapshot_val);
            created_ctx = Some((raw, health));
            (
                id,
                AmendedSnapshot {
                    base: arc,
                    overrides: BTreeMap::new(),
                },
                true,
                scan.waiting,
            )
        }
    };
    let is_v3 = snapshot.is_v3();
    let s = |key: &str| body[key].as_str().unwrap_or("");
    let view = if s("view").is_empty() {
        "all"
    } else {
        s("view")
    };
    let deferred = view == "deferred";
    let person = s("person");
    let cluster = s("cluster");
    let cluster_vids: Option<BTreeSet<String>> = if cluster.is_empty() {
        None
    } else {
        let clusters_val;
        let clusters_ref: &Value = if is_v3 {
            clusters_val = cached_clusters(config)?;
            &clusters_val
        } else {
            snapshot.clusters()
        };
        Some(
            clusters_ref
                .as_array()
                .into_iter()
                .flatten()
                .find(|c| c["cid"] == cluster)
                .and_then(|c| c["vids"].as_array())
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str().or_else(|| v["vid"].as_str()).map(str::to_owned))
                .collect(),
        )
    };
    let mut counts = json!({"all":0,"suggested":0,"conflict":0,"none":0,"manual":0,"deferred":0});
    for (_, e) in snapshot
        .iter_indexed()
        .filter(|(_, e)| e["completed"] != true)
    {
        if review::number(item_deferred(e, is_v3)).unwrap_or(0) != 0 {
            counts["deferred"] = json!(counts["deferred"].as_u64().unwrap() + 1)
        } else {
            for k in ["all", item_bucket(e, is_v3)] {
                counts[k] = json!(counts[k].as_u64().unwrap_or(0) + 1)
            }
        }
    }
    let mut scope: Vec<(usize, &Value)> = snapshot
        .iter_indexed()
        .filter(|(_, e)| {
            let lane = review::number(item_deferred(e, is_v3)).unwrap_or(0) != 0;
            let ever = e[if deferred {
                "deferred_seen"
            } else {
                "pending_seen"
            }] == true;
            (lane == deferred || ever)
                && (deferred || view == "all" || item_bucket(e, is_v3) == view)
                && (person.is_empty()
                    || item_candidates(e, is_v3)
                        .as_array()
                        .is_some_and(|cs| cs.iter().any(|c| c["name"] == person))
                    || item_suggested(e, is_v3) == person)
                && cluster_vids
                    .as_ref()
                    .is_none_or(|vids| vids.contains(item_vid(e, is_v3)))
        })
        .collect();
    scope.sort_by_key(|(_, e)| {
        e[if deferred {
            "deferred_order"
        } else {
            "pending_order"
        }]
        .as_u64()
        .unwrap_or(u64::MAX)
    });
    let is_remaining = |e: &Value| {
        e["completed"] != true
            && (review::number(item_deferred(e, is_v3)).unwrap_or(0) != 0) == deferred
    };
    let mut grouped: BTreeMap<String, Value> = BTreeMap::new();
    let mut totals: BTreeMap<String, usize> = BTreeMap::new();
    for (_, e) in snapshot.iter_indexed() {
        *totals
            .entry(e["group"]["key"].as_str().unwrap_or("").into())
            .or_default() += 1;
    }
    for &(_, e) in &scope {
        let key = e["group"]["key"].as_str().unwrap_or("").to_owned();
        let group = grouped.entry(key).or_insert_with(|| {
            let mut g = e["group"].clone();
            g["count"] = json!(0);
            g
        });
        if is_remaining(e) {
            group["count"] = json!(group["count"].as_u64().unwrap_or(0) + 1);
        }
    }
    let mut groups: Vec<Value> = grouped.into_values().collect();
    // Original snapshot counts determine order; receipt counts never reorder groups.
    let group_tier = |g: &Value| -> u8 {
        let kind = g["kind"].as_str().unwrap_or("");
        let key = g["key"].as_str().unwrap_or("");
        if kind == "suggested" {
            0
        } else if kind == "conflict" {
            1
        } else if key.starts_with("face:") {
            2
        } else if key.starts_with("folder:") {
            3
        } else if key == "none" {
            4
        } else {
            5
        }
    };
    groups.sort_by(|a, b| {
        group_tier(a)
            .cmp(&group_tier(b))
            .then_with(|| {
                totals
                    .get(b["key"].as_str().unwrap_or(""))
                    .cmp(&totals.get(a["key"].as_str().unwrap_or("")))
            })
            .then_with(|| a["label"].as_str().cmp(&b["label"].as_str()))
    });
    let focus = scope
        .iter()
        .find(|(_, e)| item_vid(e, is_v3) == s("vid"))
        .map(|(_, e)| *e);
    let focus_missing = !s("vid").is_empty() && focus.is_none();
    let requested = focus
        .map(|e| e["group"]["key"].as_str().unwrap_or(""))
        .unwrap_or(s("group"));
    let selected = if requested.is_empty() {
        groups.first().cloned()
    } else {
        groups.iter().find(|g| g["key"] == requested).cloned()
    };
    let query = s("q").to_lowercase();
    let folder = s("folder");
    let rows: Vec<(usize, &Value)> = scope
        .into_iter()
        .filter(|(_, e)| {
            selected
                .as_ref()
                .is_some_and(|g| e["group"]["key"] == g["key"])
                && (folder.is_empty()
                    || Path::new(item_path(e, is_v3))
                        .parent()
                        .is_some_and(|p| p == Path::new(folder)))
                && (query.is_empty()
                    || format!(
                        "{} {} {}",
                        item_name(e, is_v3),
                        item_path(e, is_v3),
                        item_candidates(e, is_v3)
                    )
                    .to_lowercase()
                    .contains(&query))
        })
        .collect();
    let limit = 24usize;
    let page = rows
        .iter()
        .position(|(_, e)| item_vid(e, is_v3) == s("vid"))
        .map(|n| n / limit)
        .unwrap_or_else(|| s("page").parse::<usize>().unwrap_or(0))
        .min(rows.len().saturating_sub(1) / limit);
    let (delivered, current_epoch) = if is_v3 {
        let page_items: Vec<(usize, &Value)> = rows
            .iter()
            .skip(page * limit)
            .take(limit)
            .copied()
            .filter(|(_, e)| is_remaining(e))
            .collect();
        if let Some((snapshot_raw, health)) = created_ctx {
            let miss_items: Vec<(&str, &Value)> = page_items
                .iter()
                .map(|(idx, _)| {
                    let base = snapshot.base_item(*idx);
                    (item_vid(base, true), base)
                })
                .collect();
            let mut materialized = materialize_v3_items(config, &miss_items, &health)?;
            let mut delivered = vec![];
            let mut new_items: Vec<(String, String)> = vec![];
            let mut missing_deliveries: Vec<(String, String, String)> = vec![];
            for (_, skel) in &page_items {
                let vid = item_vid(skel, true);
                let ev = materialized
                    .remove(vid)
                    .ok_or_else(|| Error::conflict("核对证据生成失败"))?;
                let raw = serde_json::to_string(&ev)?;
                new_items.push((vid.to_owned(), raw.clone()));
                let token = uuid::Uuid::new_v4().to_string();
                missing_deliveries.push((token.clone(), vid.to_owned(), raw));
                let mut task = ev["task"].clone();
                task["review_token"] = json!(token);
                delivered.push(task);
            }
            let replace_session = body["replace_session"]
                .as_str()
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_owned);
            let (expires, current_epoch) = {
                let _guard = session_write_lock()
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                let mut sidecar = sidecar_write(config)?;
                let epoch = now(&sidecar)?;
                let expires = epoch + TTL;
                let deleted = {
                    let stx = sidecar
                        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
                    let mut deleted = stx.execute(
                        "DELETE FROM session_snapshots WHERE expires<=unixepoch()",
                        [],
                    )?;
                    if let Some(old_id) = &replace_session
                        && old_id != &id
                    {
                        deleted += stx.execute(
                            "DELETE FROM session_snapshots WHERE id=?",
                            [old_id],
                        )?;
                    }
                    deleted += stx.execute(
                        "DELETE FROM session_snapshots WHERE id NOT IN (SELECT id FROM session_snapshots ORDER BY expires DESC, rowid DESC LIMIT ?1)",
                        [MAX_ACTIVE_SESSIONS - 1],
                    )?;
                    stx.execute(
                        "INSERT OR REPLACE INTO session_snapshots VALUES(?,?,?)",
                        params![&id, expires, &snapshot_raw],
                    )?;
                    if !new_items.is_empty() {
                        let mut item_insert =
                            stx.prepare("INSERT OR IGNORE INTO session_items VALUES(?,?,?)")?;
                        for (vid, raw) in &new_items {
                            item_insert.execute(params![&id, vid, raw])?;
                        }
                    }
                    if !missing_deliveries.is_empty() {
                        let mut deliv_insert =
                            stx.prepare("INSERT OR IGNORE INTO review_deliveries VALUES(?,?,?,?)")?;
                        for (token, vid, raw) in &missing_deliveries {
                            deliv_insert.execute(params![token, &id, vid, raw])?;
                        }
                    }
                    stx.commit()?;
                    deleted
                };
                if deleted > 0 {
                    maybe_incremental_vacuum(&sidecar, 16_384, 256);
                }
                (expires, epoch)
            };
            session_cache_put(
                &config.review_sessions_db(),
                &id,
                expires,
                snapshot.base.clone(),
            );
            crate::model::trim_allocator();
            (delivered, current_epoch)
        } else {
            let ro = sidecar_readonly(config)?;
            let current_epoch = now(&ro)?;
            let page_vids: Vec<&str> = page_items.iter().map(|(_, e)| item_vid(e, true)).collect();
            let mut stored_items = load_v3_session_items(&ro, &id, &page_vids)?;
            let miss_items: Vec<(&str, &Value)> = page_items
                .iter()
                .filter_map(|(idx, skel)| {
                    let vid = item_vid(skel, true);
                    (!stored_items.contains_key(vid)).then(|| (vid, snapshot.base_item(*idx)))
                })
                .collect();
            let mut materialized = if miss_items.is_empty() {
                BTreeMap::new()
            } else {
                let health = HealthReader::new(config)?;
                materialize_v3_items(config, &miss_items, &health)?
            };
            let has_deliveries = sidecar_has_deliveries_table(&ro);
            let mut stmt = if has_deliveries {
                Some(ro.prepare(
                    "SELECT token FROM review_deliveries WHERE session=? AND vid=? AND evidence=? LIMIT 1",
                )?)
            } else {
                None
            };
            let mut delivered = vec![];
            let mut new_items: Vec<(String, String)> = vec![];
            let mut missing_deliveries: Vec<(String, String, String)> = vec![];
            for (idx, skel) in &page_items {
                let vid = item_vid(skel, true);
                let mut ev = if let Some(existing) = stored_items.remove(vid) {
                    existing
                } else {
                    let fresh = materialized
                        .remove(vid)
                        .ok_or_else(|| Error::conflict("核对证据生成失败"))?;
                    new_items.push((vid.to_owned(), serde_json::to_string(&fresh)?));
                    fresh
                };
                if snapshot.is_amended(*idx) {
                    apply_v3_amendment(&mut ev, skel);
                }
                let raw = serde_json::to_string(&ev)?;
                let token = if let Some(ref mut s) = stmt {
                    s.query_row(params![&id, vid, &raw], |r| r.get::<_, String>(0))
                        .optional()?
                } else {
                    None
                };
                let token = match token {
                    Some(existing) => existing,
                    None => {
                        let token = uuid::Uuid::new_v4().to_string();
                        missing_deliveries.push((token.clone(), vid.to_owned(), raw));
                        token
                    }
                };
                let mut task = ev["task"].clone();
                task["review_token"] = json!(token);
                delivered.push(task);
            }
            drop(stmt);
            drop(ro);
            if !new_items.is_empty() || !missing_deliveries.is_empty() {
                let _guard = session_write_lock()
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                let mut rw = sidecar_write(config)?;
                let tx = rw.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
                if !new_items.is_empty() {
                    let mut item_insert =
                        tx.prepare("INSERT OR IGNORE INTO session_items VALUES(?,?,?)")?;
                    for (vid, raw) in &new_items {
                        item_insert.execute(params![&id, vid, raw])?;
                    }
                }
                if !missing_deliveries.is_empty() {
                    let mut deliv_insert =
                        tx.prepare("INSERT OR IGNORE INTO review_deliveries VALUES(?,?,?,?)")?;
                    for (token, vid, raw) in &missing_deliveries {
                        deliv_insert.execute(params![token, &id, vid, raw])?;
                    }
                }
                tx.commit()?;
            }
            (delivered, current_epoch)
        }
    } else if in_sidecar {
        let ro = sidecar_readonly(config)?;
        let has_deliveries = sidecar_has_deliveries_table(&ro);
        let mut stmt = if has_deliveries {
            Some(ro.prepare(
                "SELECT token FROM review_deliveries WHERE session=? AND vid=? AND evidence=? LIMIT 1",
            )?)
        } else {
            None
        };
        let mut delivered = vec![];
        let mut missing: Vec<(String, Option<String>, String)> = vec![];
        for (_, e) in rows
            .iter()
            .skip(page * limit)
            .take(limit)
            .filter(|(_, e)| is_remaining(e))
        {
            let raw = serde_json::to_string(e)?;
            let vid = e["task"]["vid"].as_str();
            let token = if let Some(ref mut s) = stmt {
                s.query_row(params![&id, vid, &raw], |r| r.get::<_, String>(0))
                    .optional()?
            } else {
                None
            };
            let token = match token {
                Some(existing) => existing,
                None => {
                    let token = uuid::Uuid::new_v4().to_string();
                    missing.push((token.clone(), vid.map(str::to_owned), raw));
                    token
                }
            };
            let mut task = e["task"].clone();
            task["review_token"] = json!(token);
            delivered.push(task);
        }
        let current_epoch = now(&ro)?;
        drop(stmt);
        drop(ro);
        if !missing.is_empty() {
            let _guard = session_write_lock()
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let mut rw = sidecar_write(config)?;
            let tx = rw.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            {
                let mut insert =
                    tx.prepare("INSERT OR IGNORE INTO review_deliveries VALUES(?,?,?,?)")?;
                for (token, vid, raw) in &missing {
                    insert.execute(params![token, &id, vid.as_deref(), raw])?;
                }
            }
            tx.commit()?;
        }
        (delivered, current_epoch)
    } else {
        let ro = schema_readonly(config)?;
        let mut stmt = ro.prepare(
            "SELECT token FROM review_deliveries WHERE session=? AND vid=? AND evidence=? LIMIT 1",
        )?;
        let mut delivered = vec![];
        let mut missing: Vec<(String, Option<String>, String)> = vec![];
        for (_, e) in rows
            .iter()
            .skip(page * limit)
            .take(limit)
            .filter(|(_, e)| is_remaining(e))
        {
            let raw = serde_json::to_string(e)?;
            let vid = e["task"]["vid"].as_str();
            let token = stmt
                .query_row(params![&id, vid, &raw], |r| r.get::<_, String>(0))
                .optional()?;
            let token = match token {
                Some(existing) => existing,
                None => {
                    let token = uuid::Uuid::new_v4().to_string();
                    missing.push((token.clone(), vid.map(str::to_owned), raw));
                    token
                }
            };
            let mut task = e["task"].clone();
            task["review_token"] = json!(token);
            delivered.push(task);
        }
        let current_epoch = now(&ro)?;
        drop(stmt);
        drop(ro);
        if !missing.is_empty() {
            let mut rw = schema(config)?;
            let tx = rw.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            {
                let mut insert =
                    tx.prepare("INSERT OR IGNORE INTO review_deliveries VALUES(?,?,?,?)")?;
                for (token, vid, raw) in &missing {
                    insert.execute(params![token, &id, vid.as_deref(), raw])?;
                }
            }
            tx.commit()?;
        }
        (delivered, current_epoch)
    };
    let pages = rows.len().div_ceil(limit);
    let has = |p: usize| {
        rows.iter()
            .skip(p * limit)
            .take(limit)
            .any(|(_, e)| is_remaining(e))
    };
    let next = (page + 1..pages).find(|p| has(*p));
    let previous = (0..page).rev().find(|p| has(*p));
    let folders: BTreeSet<_> = snapshot
        .iter_indexed()
        .filter_map(|(_, e)| {
            let path = item_path(e, is_v3);
            (!path.is_empty())
                .then(|| Path::new(path).parent().map(|p| p.display().to_string()))
                .flatten()
        })
        .collect();
    let heartbeat = review::read(config, "processing/heartbeat.json", json!({}))?.1;
    let age = current_epoch as f64 - heartbeat["at"].as_f64().unwrap_or(0.0);
    Ok(
        json!({"session":id,"counts":counts,"groups":groups,"group":selected,"rows":delivered,"total":rows.len(),"remaining":rows.iter().filter(|(_, e)| is_remaining(e)).count(),"page":page,"limit":limit,"next_page":next,"previous_page":previous,"waiting":waiting,"online":(-30.0..90.0).contains(&age),"folders":folders,"focus_missing":focus_missing,"has_updates":false}),
    )
}

#[cfg(test)]
mod alias_snapshot_tests {
    use super::*;
    use std::fs;

    #[test]
    fn task_names_and_material_share_one_alias_snapshot() {
        let root = fs::canonicalize("/tmp")
            .unwrap()
            .join(format!("face-review-alias-{}", uuid::Uuid::new_v4()));
        let base = root.join("base");
        let media = root.join("media");
        fs::create_dir_all(base.join("processing")).unwrap();
        fs::create_dir_all(&media).unwrap();
        let config = Config {
            base: base.clone(),
            media,
            ledger: root.join("ledger.db"),
            bind: "127.0.0.1:0".into(),
            token: None,
            fixture: None,
            parity_root: None,
        };
        let faces = rusqlite::Connection::open(base.join("faces.db")).unwrap();
        faces
            .execute_batch("CREATE TABLE videos(vid TEXT PRIMARY KEY,path TEXT);")
            .unwrap();
        let jobs = rusqlite::Connection::open(base.join("processing/processing.db")).unwrap();
        jobs.execute_batch("CREATE TABLE tasks(vid TEXT PRIMARY KEY,path TEXT,stage TEXT,priority INTEGER,is_new INTEGER,created_at REAL,data TEXT,manual INTEGER,deferred INTEGER,note TEXT);").unwrap();
        jobs.execute(
            "INSERT INTO tasks VALUES('v','/fixture/video.mp4','review',0,0,0,?,0,0,'')",
            [json!({"ready":true,"candidates":[{"name":"Alias"}]}).to_string()],
        )
        .unwrap();
        fs::write(
            base.join("person-renames.json"),
            json!({"Alias":"OldName"}).to_string(),
        )
        .unwrap();
        let material = shared_metadata::MaterialReader::new(&config).unwrap();
        fs::write(
            base.join("person-renames.json"),
            json!({"Alias":"NewName"}).to_string(),
        )
        .unwrap();
        let task = tasks(&config, material.aliases()).unwrap().remove(0);
        assert_eq!(task["data"]["candidates"][0]["name"], "OldName");
        assert_eq!(task["__review_raw_people"][0], "Alias");
        let scope =
            json!({"people":["Alias"],"alias_inputs":["Alias"],"vids":[],"watermark_keys":[]});
        let frozen = material.state(&config, &scope).unwrap();
        assert_eq!(frozen["material"]["names"][0][1], "OldName");
        let changed = shared_metadata::MaterialReader::new(&config)
            .unwrap()
            .state(&config, &scope)
            .unwrap();
        assert_ne!(frozen["version"], changed["version"]);
        drop(jobs);
        drop(faces);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn amended_overlay_mutates_only_affected_items_without_cloning_base() {
        let root = fs::canonicalize("/tmp")
            .unwrap()
            .join(format!("face-review-overlay-{}", uuid::Uuid::new_v4()));
        let base = root.join("base");
        let media = root.join("media");
        fs::create_dir_all(base.join("file-operations-rust")).unwrap();
        fs::create_dir_all(&media).unwrap();
        let config = Config {
            base,
            media,
            ledger: root.join("ledger.db"),
            bind: "127.0.0.1:0".into(),
            token: None,
            fixture: None,
            parity_root: None,
        };
        let store = crate::store::Store::start(&config).unwrap();
        let session_id = "sess-1";
        let plan = json!({
            "id": "op-1",
            "action": "review-note",
            "request": {
                "payload": {
                    "review_session": session_id,
                    "vids": ["v1"],
                    "note": "updated note"
                }
            }
        });
        store
            .db
            .execute(
                "INSERT INTO operations(id,request_hash,plan,status,phase) VALUES('op-1','h',?,'complete','complete')",
                [plan.to_string()],
            )
            .unwrap();
        drop(store);

        let base_snap = Arc::new(json!({
            "schema_version": 1,
            "clusters": [],
            "evidence": [
                {"task": {"vid": "v1", "note": ""}, "material": {"manual_tasks": [{"vid": "v1", "note": ""}]}},
                {"task": {"vid": "v2", "note": "keep"}, "material": {"manual_tasks": []}}
            ]
        }));
        let overlay = amended(&config, session_id, base_snap.clone()).unwrap();
        assert_eq!(overlay.overrides.len(), 1);
        let items: Vec<&Value> = overlay.iter().collect();
        assert_eq!(items[0]["task"]["note"], "updated note");
        assert_eq!(items[0]["disposition"], "op-1");
        assert_eq!(items[1]["task"]["note"], "keep");
        assert!(std::ptr::eq(items[1], &base_snap["evidence"][1]));
        assert_eq!(base_snap["evidence"][0]["task"]["note"], "");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn review_deliveries_reuses_tokens_without_re_writing() {
        let root = fs::canonicalize("/tmp")
            .unwrap()
            .join(format!("face-review-deliveries-{}", uuid::Uuid::new_v4()));
        let base = root.join("base");
        let media = root.join("media");
        fs::create_dir_all(base.join("file-operations-rust")).unwrap();
        fs::create_dir_all(base.join("processing")).unwrap();
        fs::create_dir_all(&media).unwrap();
        let config = Config {
            base: base.clone(),
            media,
            ledger: root.join("ledger.db"),
            bind: "127.0.0.1:0".into(),
            token: None,
            fixture: None,
            parity_root: None,
        };
        drop(crate::store::Store::start(&config).unwrap());
        let faces = rusqlite::Connection::open(base.join("faces.db")).unwrap();
        faces
            .execute_batch("CREATE TABLE videos(vid TEXT PRIMARY KEY,path TEXT);")
            .unwrap();
        let jobs = rusqlite::Connection::open(base.join("processing/processing.db")).unwrap();
        jobs.execute_batch("CREATE TABLE tasks(vid TEXT PRIMARY KEY,path TEXT,stage TEXT,priority INTEGER,is_new INTEGER,created_at REAL,data TEXT,manual INTEGER,deferred INTEGER,note TEXT);").unwrap();
        for (vid, file) in [("v1", "/fixture/a.mp4"), ("v2", "/fixture/b.mp4")] {
            jobs.execute(
                "INSERT INTO tasks VALUES(?,?,'review',0,0,0,?,0,0,'')",
                [vid, file, &json!({"ready":true,"candidates":[]}).to_string()],
            )
            .unwrap();
        }

        let op_observer = db::open_readonly(&config.state_db()).unwrap();
        let op_ver_before: i64 = op_observer
            .pragma_query_value(None, "data_version", |r| r.get(0))
            .unwrap();

        let mut engine = Engine::open_preview(config.clone()).unwrap();
        let first = execute(&mut engine, "review-page", json!({})).unwrap();
        let session_id = first["session"].as_str().unwrap().to_owned();
        let first_tokens: Vec<_> = first["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["review_token"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(first_tokens.len(), 2);

        let op_ver_after_first: i64 = op_observer
            .pragma_query_value(None, "data_version", |r| r.get(0))
            .unwrap();
        assert_eq!(
            op_ver_before, op_ver_after_first,
            "creating a review session and delivering tokens must not write operations.sqlite3"
        );

        let sc_observer = sidecar_readonly(&config).unwrap();
        let sc_ver_after_first: i64 = sc_observer
            .pragma_query_value(None, "data_version", |r| r.get(0))
            .unwrap();

        let second = execute(&mut engine, "review-page", json!({"session": session_id})).unwrap();
        let second_tokens: Vec<_> = second["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["review_token"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(first_tokens, second_tokens);

        let op_ver_after_second: i64 = op_observer
            .pragma_query_value(None, "data_version", |r| r.get(0))
            .unwrap();
        let sc_ver_after_second: i64 = sc_observer
            .pragma_query_value(None, "data_version", |r| r.get(0))
            .unwrap();
        assert_eq!(
            op_ver_before, op_ver_after_second,
            "revisiting a rendered review page must not write operations.sqlite3"
        );
        assert_eq!(
            sc_ver_after_first, sc_ver_after_second,
            "revisiting a rendered review page must not write review-sessions.sqlite3"
        );
        drop(sc_observer);
        drop(op_observer);
        drop(engine);
        drop(jobs);
        drop(faces);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn review_sessions_sidecar_isolation_replace_and_hard_cap() {
        let root = fs::canonicalize("/tmp")
            .unwrap()
            .join(format!("face-review-sidecar-{}", uuid::Uuid::new_v4()));
        let base = root.join("base");
        let media = root.join("media");
        fs::create_dir_all(base.join("file-operations-rust")).unwrap();
        fs::create_dir_all(base.join("processing")).unwrap();
        fs::create_dir_all(&media).unwrap();
        let config = Config {
            base: base.clone(),
            media,
            ledger: root.join("ledger.db"),
            bind: "127.0.0.1:0".into(),
            token: None,
            fixture: None,
            parity_root: None,
        };
        drop(crate::store::Store::start(&config).unwrap());
        let faces = rusqlite::Connection::open(base.join("faces.db")).unwrap();
        faces
            .execute_batch("CREATE TABLE videos(vid TEXT PRIMARY KEY,path TEXT);")
            .unwrap();
        let jobs = rusqlite::Connection::open(base.join("processing/processing.db")).unwrap();
        jobs.execute_batch("CREATE TABLE tasks(vid TEXT PRIMARY KEY,path TEXT,stage TEXT,priority INTEGER,is_new INTEGER,created_at REAL,data TEXT,manual INTEGER,deferred INTEGER,note TEXT);").unwrap();
        jobs.execute(
            "INSERT INTO tasks VALUES('v1','/fixture/v1.mp4','review',0,0,0,?,0,0,'')",
            [json!({"ready":true,"watermark":{"status":"done","pipeline":"ocr-v1","candidates":[]},"candidates":[]}).to_string()],
        )
        .unwrap();
        fs::write(
            base.join("clusters.json"),
            json!([{"cid":"c1","total_gb":9.9,"vids":[{"vid":"v1","extra":"drop"}]}]).to_string(),
        )
        .unwrap();

        let mut engine = Engine::open_preview(config.clone()).unwrap();
        let first = execute(&mut engine, "review-page", json!({})).unwrap();
        let first_id = first["session"].as_str().unwrap().to_owned();
        let replaced =
            execute(&mut engine, "review-page", json!({"replace_session": first_id})).unwrap();
        let replaced_id = replaced["session"].as_str().unwrap().to_owned();
        assert_ne!(first_id, replaced_id);
        assert!(read(&config, &first_id).is_err());

        for _ in 0..20 {
            execute(&mut engine, "review-page", json!({})).unwrap();
        }
        let op_conn = db::open_readonly(&config.state_db()).unwrap();
        let op_count: i64 = op_conn
            .query_row("SELECT COUNT(*) FROM review_sessions", [], |r| r.get(0))
            .unwrap();
        let op_deliv_count: i64 = op_conn
            .query_row("SELECT COUNT(*) FROM review_deliveries", [], |r| r.get(0))
            .unwrap();
        assert_eq!(op_count, 0);
        assert_eq!(op_deliv_count, 0);

        let sc_conn = sidecar_readonly(&config).unwrap();
        let sc_count: i64 = sc_conn
            .query_row("SELECT COUNT(*) FROM session_snapshots", [], |r| r.get(0))
            .unwrap();
        let sc_del_sessions: i64 = sc_conn
            .query_row(
                "SELECT COUNT(DISTINCT session) FROM review_deliveries",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let (sc_sid, sc_raw): (String, String) = sc_conn
            .query_row(
                "SELECT id, snapshot FROM session_snapshots ORDER BY rowid DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(sc_count, MAX_ACTIVE_SESSIONS);
        assert!(sc_del_sessions <= MAX_ACTIVE_SESSIONS);
        let sc_val: Value = serde_json::from_str(&sc_raw).unwrap();
        assert_eq!(sc_val["schema_version"], 3);
        assert!(sc_val.get("clusters").is_none());
        assert!(sc_val.get("evidence").is_none());
        assert!(sc_val["items"][0]["data_digest"].is_string());
        assert!(sc_val["items"][0].get("face").is_none());
        assert!(sc_val["items"][0].get("watermark").is_none());

        let item_raw: String = sc_conn
            .query_row(
                "SELECT evidence FROM session_items WHERE session=? AND vid='v1'",
                [&sc_sid],
                |r| r.get(0),
            )
            .unwrap();
        let item_val: Value = serde_json::from_str(&item_raw).unwrap();
        assert!(item_val.get("material").is_none());
        assert!(item_val.get("text").is_none());
        assert!(item_val["material_hash"].is_string());

        let hit_cluster = execute(
            &mut engine,
            "review-page",
            json!({"session": sc_sid, "cluster": "c1"}),
        )
        .unwrap();
        assert_eq!(hit_cluster["rows"].as_array().unwrap().len(), 1);
        let miss_cluster = execute(
            &mut engine,
            "review-page",
            json!({"session": sc_sid, "cluster": "missing"}),
        )
        .unwrap();
        assert_eq!(miss_cluster["rows"].as_array().unwrap().len(), 0);

        drop(sc_conn);
        drop(op_conn);
        drop(engine);
        drop(jobs);
        drop(faces);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn schema_v3_lazy_materialization_and_aba_freeze() {
        let root = fs::canonicalize("/tmp")
            .unwrap()
            .join(format!("face-review-v3-lazy-{}", uuid::Uuid::new_v4()));
        let base = root.join("base");
        let media = root.join("media");
        fs::create_dir_all(base.join("file-operations-rust")).unwrap();
        fs::create_dir_all(base.join("processing")).unwrap();
        fs::create_dir_all(&media).unwrap();
        let config = Config {
            base: base.clone(),
            media,
            ledger: root.join("ledger.db"),
            bind: "127.0.0.1:0".into(),
            token: None,
            fixture: None,
            parity_root: None,
        };
        drop(crate::store::Store::start(&config).unwrap());
        let faces = rusqlite::Connection::open(base.join("faces.db")).unwrap();
        faces
            .execute_batch("CREATE TABLE videos(vid TEXT PRIMARY KEY,path TEXT);")
            .unwrap();
        let jobs = rusqlite::Connection::open(base.join("processing/processing.db")).unwrap();
        jobs.execute_batch("CREATE TABLE tasks(vid TEXT PRIMARY KEY,path TEXT,stage TEXT,priority INTEGER,is_new INTEGER,created_at REAL,data TEXT,manual INTEGER,deferred INTEGER,note TEXT);").unwrap();
        let orig_data = json!({
            "ready": true,
            "candidates": [],
            "face": {"unknown_group": "", "bulky": "FACE_BLOB_MARKER"},
            "watermark": {"status": "done", "pipeline": "ocr-v1", "candidates": [], "bulky": "WM_BLOB_MARKER"}
        });
        for i in 0..30 {
            let vid = format!("v{i:02}");
            let file = format!("/fixture/{vid}.mp4");
            jobs.execute(
                "INSERT INTO tasks VALUES(?,?,'review',0,?,?,?,0,0,'')",
                params![&vid, &file, 100 - i as i64, 0, orig_data.to_string()],
            )
            .unwrap();
        }

        let op_obs = db::open_readonly(&config.state_db()).unwrap();
        let op_ver0: i64 = op_obs
            .pragma_query_value(None, "data_version", |r| r.get(0))
            .unwrap();

        let mut engine = Engine::open_preview(config.clone()).unwrap();
        let page0 = execute(&mut engine, "review-page", json!({})).unwrap();
        let sid = page0["session"].as_str().unwrap().to_owned();
        assert_eq!(page0["total"], 30);
        assert_eq!(page0["rows"].as_array().unwrap().len(), 24);
        let first_vid = page0["rows"][0]["vid"].as_str().unwrap().to_owned();
        let first_token = page0["rows"][0]["review_token"]
            .as_str()
            .unwrap()
            .to_owned();

        let sc_obs = sidecar_readonly(&config).unwrap();
        let snap_raw: String = sc_obs
            .query_row(
                "SELECT snapshot FROM session_snapshots WHERE id=?",
                [&sid],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!snap_raw.contains("FACE_BLOB_MARKER"));
        assert!(!snap_raw.contains("WM_BLOB_MARKER"));
        let item_count_0: i64 = sc_obs
            .query_row(
                "SELECT COUNT(*) FROM session_items WHERE session=?",
                [&sid],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(item_count_0, 24);

        let sc_ver0: i64 = sc_obs
            .pragma_query_value(None, "data_version", |r| r.get(0))
            .unwrap();
        let page0_revisit = execute(&mut engine, "review-page", json!({"session": sid})).unwrap();
        assert_eq!(page0_revisit["rows"][0]["review_token"], first_token);
        let op_ver1: i64 = op_obs
            .pragma_query_value(None, "data_version", |r| r.get(0))
            .unwrap();
        let sc_ver1: i64 = sc_obs
            .pragma_query_value(None, "data_version", |r| r.get(0))
            .unwrap();
        assert_eq!(op_ver0, op_ver1);
        assert_eq!(sc_ver0, sc_ver1);

        let mut mutated = orig_data.clone();
        mutated["kind"] = json!("suggested");
        mutated["suggested"] = json!("Mutated");
        jobs.execute(
            "UPDATE tasks SET data=? WHERE vid=?",
            params![mutated.to_string(), &first_vid],
        )
        .unwrap();

        let status = execute(&mut engine, "review-status", json!({"session": sid})).unwrap();
        assert_eq!(status["has_updates"], true);
        assert!(status["changed"].as_u64().unwrap() >= 1);

        let page0_after_edit =
            execute(&mut engine, "review-page", json!({"session": sid})).unwrap();
        assert_eq!(page0_after_edit["rows"][0]["data"], orig_data);
        assert_eq!(page0_after_edit["rows"][0]["review_token"], first_token);

        let page1 = execute(
            &mut engine,
            "review-page",
            json!({"session": sid, "page": "1"}),
        )
        .unwrap();
        assert_eq!(page1["rows"].as_array().unwrap().len(), 6);
        let item_count_1: i64 = sc_obs
            .query_row(
                "SELECT COUNT(*) FROM session_items WHERE session=?",
                [&sid],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(item_count_1, 30);

        drop(sc_obs);
        drop(op_obs);
        drop(engine);
        drop(jobs);
        drop(faces);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dispositions_uses_partial_index_under_skew_and_handles_vids_and_scalar_vid() {
        let root = fs::canonicalize("/tmp")
            .unwrap()
            .join(format!("face-review-disp-{}", uuid::Uuid::new_v4()));
        let base = root.join("base");
        let media = root.join("media");
        fs::create_dir_all(base.join("file-operations-rust")).unwrap();
        fs::create_dir_all(&media).unwrap();
        let config = Config {
            base,
            media,
            ledger: root.join("ledger.db"),
            bind: "127.0.0.1:0".into(),
            token: None,
            fixture: None,
            parity_root: None,
        };
        let store = crate::store::Store::start(&config).unwrap();
        for i in 0..500 {
            store
                .db
                .execute(
                    "INSERT INTO operations(id,request_hash,plan,status,phase) VALUES(?1,'h','{\"action\":\"processing-result\"}','complete','complete')",
                    [format!("op-pr-{i}")],
                )
                .unwrap();
        }
        let samples = [
            ("op-1", "queue-confirm", json!({"vids": ["v1", 99, "v2"]})),
            ("op-2", "review-note", json!({"vid": "v3", "vids": "not-array"})),
            ("op-3", "review-defer", json!({"vids": ["v1"]})),
            ("op-4", "review-delete", json!({"vids": ["v4"]})),
            ("op-5", "claim-decide", json!({"vid": 123})),
            ("op-6", "archive", json!({"vids": ["v1"]})),
            ("op-7", "auto-archive", json!({"vids": ["v2"]})),
            ("op-8", "merge-person", json!({"vids": ["v3"]})),
            ("op-9", "person-batch", json!({"vids": ["v4"]})),
            ("op-10", "metadata-write", json!({"vids": ["v5"]})),
        ];
        for (id, action, payload) in samples {
            store
                .db
                .execute(
                    "INSERT INTO operations(id,request_hash,plan,status,phase) VALUES(?1,'h',?2,'complete','complete')",
                    params![
                        id,
                        json!({"action": action, "request": {"payload": payload}}).to_string()
                    ],
                )
                .unwrap();
        }
        store
            .db
            .execute(
                "INSERT INTO operations(id,request_hash,plan,status,phase) VALUES('op-pending','h',?,'running','file_moving')",
                [json!({"action": "queue-confirm", "request": {"payload": {"vids": ["v2"]}}}).to_string()],
            )
            .unwrap();
        store.db.execute_batch("ANALYZE;").unwrap();

        let eqp: Vec<String> = store
            .db
            .prepare(
                "EXPLAIN QUERY PLAN SELECT id, \
                 CASE WHEN json_type(plan,'$.request.payload.vids')='array' THEN json_extract(plan,'$.request.payload.vids') END, \
                 CASE WHEN json_type(plan,'$.request.payload.vid')='text' THEN json_extract(plan,'$.request.payload.vid') END \
                 FROM operations INDEXED BY idx_operations_complete_action \
                 WHERE status='complete' \
                   AND json_extract(plan,'$.action') IN ('queue-confirm','queue-undo','review-note','review-defer','review-restore','review-delete','claim-decide') \
                 ORDER BY rowid",
            )
            .unwrap()
            .query_map([], |r| r.get::<_, String>(3))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert!(
            eqp.iter()
                .any(|line| line.contains("idx_operations_complete_action")),
            "expected idx_operations_complete_action in {eqp:?}"
        );
        drop(store);

        let map = dispositions(&config).unwrap();
        assert_eq!(map.get("v1"), Some(&json!("op-3")));
        assert_eq!(map.get("v2"), Some(&json!("op-1")));
        assert_eq!(map.get("v3"), Some(&json!("op-2")));
        assert_eq!(map.get("v4"), Some(&json!("op-4")));
        assert!(!map.contains_key("v5"));
        assert_eq!(map.len(), 4);

        let map_hit = dispositions(&config).unwrap();
        assert!(Arc::ptr_eq(&map, &map_hit));

        let rw = db::open(&config.state_db()).unwrap();
        rw.execute(
            "INSERT INTO operations(id,request_hash,plan,status,phase) VALUES('op-11','h',?,'complete','complete')",
            [json!({"action": "review-note", "request": {"payload": {"vids": ["v6"]}}}).to_string()],
        )
        .unwrap();
        let map_new = dispositions(&config).unwrap();
        assert!(!Arc::ptr_eq(&map, &map_new));
        assert_eq!(map_new.get("v6"), Some(&json!("op-11")));

        rw.execute(
            "UPDATE operations SET status='complete', phase='complete' WHERE id='op-pending'",
            [],
        )
        .unwrap();
        let map_ooo = dispositions(&config).unwrap();
        assert!(!Arc::ptr_eq(&map_new, &map_ooo));
        assert_eq!(map_ooo.get("v2"), Some(&json!("op-pending")));

        drop(rw);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_inline_session_and_legacy_deliveries_remain_validatable() {
        let root = fs::canonicalize("/tmp")
            .unwrap()
            .join(format!("face-review-legacy-{}", uuid::Uuid::new_v4()));
        let base = root.join("base");
        let media = root.join("media");
        let video_path = media.join("pikpak/legacy.mp4");
        fs::create_dir_all(base.join("file-operations-rust")).unwrap();
        fs::create_dir_all(base.join("processing")).unwrap();
        fs::create_dir_all(video_path.parent().unwrap()).unwrap();
        fs::write(&video_path, b"legacy-video").unwrap();
        let stat = fs::metadata(&video_path).unwrap();
        let size = stat.len();
        let mtime_ns = crate::files::mtime_ns(&stat);
        let config = Config {
            base: base.clone(),
            media,
            ledger: root.join("ledger.db"),
            bind: "127.0.0.1:0".into(),
            token: None,
            fixture: None,
            parity_root: None,
        };
        let store = crate::store::Store::start(&config).unwrap();
        let faces = rusqlite::Connection::open(base.join("faces.db")).unwrap();
        faces
            .execute_batch("CREATE TABLE videos(vid TEXT PRIMARY KEY,path TEXT);")
            .unwrap();
        faces
            .execute(
                "INSERT INTO videos VALUES('v-leg', ?)",
                [video_path.to_str().unwrap()],
            )
            .unwrap();
        let jobs = rusqlite::Connection::open(base.join("processing/processing.db")).unwrap();
        jobs.execute_batch("CREATE TABLE tasks(vid TEXT PRIMARY KEY,path TEXT,stage TEXT,priority INTEGER,is_new INTEGER,created_at REAL,data TEXT,manual INTEGER,deferred INTEGER,note TEXT);").unwrap();
        jobs.execute(
            "INSERT INTO tasks VALUES('v-leg', ?, 'review', 0, 0, 0, ?, 0, 0, '')",
            [
                video_path.to_str().unwrap(),
                &json!({"ready":true,"candidates":[]}).to_string(),
            ],
        )
        .unwrap();

        // Case 1: Inline session stored directly in operations.sqlite3.
        let inline_id = "sess-inline";
        let inline_snap = json!({
            "schema_version": 1,
            "clusters": [],
            "evidence": [{
                "task": {"vid": "v-leg", "name": "legacy.mp4", "path": video_path.to_str().unwrap(), "size": size, "mtime_ns": mtime_ns, "deferred": 0, "manual": 0, "note": "", "data": {"kind": "none", "ready": true, "candidates": []}},
                "group": {"key": "none", "label": "无候选人物", "kind": "none"},
                "scope": {"vids": ["v-leg"], "people": [], "watermark_keys": []},
                "material": {},
                "text": {},
                "health": {"hidden": false},
                "disposition": null,
                "pending_seen": true,
                "deferred_seen": false,
                "pending_order": 0
            }]
        });
        store
            .db
            .execute(
                "INSERT INTO review_sessions VALUES(?, unixepoch() + 3600, ?)",
                params![inline_id, inline_snap.to_string()],
            )
            .unwrap();

        let mut engine = Engine::open_preview(config.clone()).unwrap();
        let inline_page =
            execute(&mut engine, "review-page", json!({"session": inline_id})).unwrap();
        assert_eq!(inline_page["rows"].as_array().unwrap().len(), 1);

        // Case 2: Sidecar session whose delivery token resides in operations.sqlite3.review_deliveries.
        let sidecar_page = execute(&mut engine, "review-page", json!({})).unwrap();
        let sc_sid = sidecar_page["session"].as_str().unwrap().to_owned();
        let legacy_token = "tok-in-operations-db";
        let material = shared_metadata::MaterialReader::new(&config).unwrap();
        let health = HealthReader::new(&config).unwrap();
        let disp = dispositions(&config).unwrap();
        let live = tasks_for_review(&config, material.aliases()).unwrap();
        let ev = evidence(&config, &live[0], &material, &health, &disp).unwrap();
        store
            .db
            .execute(
                "INSERT INTO review_sessions VALUES(?, unixepoch() + 3600, '{\"schema_version\":2,\"sidecar\":true}')",
                [&sc_sid],
            )
            .unwrap();
        store
            .db
            .execute(
                "INSERT INTO review_deliveries VALUES(?, ?, 'v-leg', ?)",
                params![legacy_token, &sc_sid, ev.to_string()],
            )
            .unwrap();

        let validated = validate(
            &config,
            &json!({
                "review_session": sc_sid,
                "vids": ["v-leg"],
                "review_tokens": {"v-leg": legacy_token}
            }),
        )
        .unwrap();
        assert_eq!(validated.len(), 1);
        assert_eq!(validated[0]["vid"], "v-leg");

        // Case 3: Schema v2 snapshot stored directly in review-sessions.sqlite3.
        let v2_sid = "sess-v2-sidecar";
        let v2_snap = json!({
            "schema_version": 2,
            "clusters": [],
            "evidence": [ev]
        });
        let sw = sidecar_write(&config).unwrap();
        sw.execute(
            "INSERT INTO session_snapshots VALUES(?, unixepoch() + 3600, ?)",
            params![v2_sid, v2_snap.to_string()],
        )
        .unwrap();
        drop(sw);
        let v2_page = execute(&mut engine, "review-page", json!({"session": v2_sid})).unwrap();
        assert_eq!(v2_page["rows"].as_array().unwrap().len(), 1);
        assert_eq!(v2_page["rows"][0]["vid"], "v-leg");
        let v2_status = execute(&mut engine, "review-status", json!({"session": v2_sid})).unwrap();
        assert_eq!(v2_status["has_updates"], false);

        drop(engine);
        drop(store);
        drop(jobs);
        drop(faces);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn none_bucket_groups_multi_video_subfolders_and_keeps_ready_reocr_reviewable() {
        let root = fs::canonicalize("/tmp")
            .unwrap()
            .join(format!("face-review-folders-{}", uuid::Uuid::new_v4()));
        let base = root.join("base");
        let media = root.join("media");
        fs::create_dir_all(base.join("processing")).unwrap();
        fs::create_dir_all(&media).unwrap();
        let config = Config {
            base: base.clone(),
            media: media.clone(),
            ledger: root.join("ledger.db"),
            bind: "127.0.0.1:0".into(),
            token: None,
            fixture: None,
            parity_root: None,
        };
        let faces = rusqlite::Connection::open(base.join("faces.db")).unwrap();
        faces
            .execute_batch("CREATE TABLE videos(vid TEXT PRIMARY KEY,path TEXT);")
            .unwrap();
        let proc_db = rusqlite::Connection::open(base.join("processing/processing.db")).unwrap();
        proc_db
            .execute_batch(
                "CREATE TABLE tasks(vid TEXT PRIMARY KEY,path TEXT,name TEXT,size INTEGER,mtime_ns TEXT,stage TEXT,retry_stage TEXT,manual INTEGER,deferred INTEGER,note TEXT,priority INTEGER,is_new INTEGER,created_at REAL,batch_id TEXT,data TEXT);",
            )
            .unwrap();
        let make_row = |vid: &str, rel_path: &str, name: &str, stage: &str| {
            let full = media.join(rel_path).to_string_lossy().into_owned();
            let data = json!({
                "kind": "none",
                "ready": true,
                "candidates": [],
                "face": {"matches": []},
                "watermark": {"pipeline": "v2", "status": "done", "candidates": []}
            });
            proc_db
                .execute(
                    "INSERT INTO tasks VALUES(?,?,?,?,?,?,?,0,0,'',0,0,1.0,'b1',?)",
                    params![vid, full, name, 100i64, "1", stage, "text", data.to_string()],
                )
                .unwrap();
        };
        // Two multi-video folders sharing the leaf name "sub" -> disambiguated labels
        make_row("v1", "pikpak/My Telegram/PackA/sub/a1.mp4", "a1.mp4", "review");
        make_row("v2", "pikpak/My Telegram/PackA/sub/a2.mp4", "a2.mp4", "queued");
        make_row("v3", "pikpak/My Telegram/PackB/sub/b1.mp4", "b1.mp4", "review");
        make_row("v4", "pikpak/My Telegram/PackB/sub/b2.mp4", "b2.mp4", "text");
        // Unique leaf folder >=2 -> leaf label only
        make_row("v5", "pikpak/My Telegram/GroupC/c1.mp4", "c1.mp4", "review");
        make_row("v6", "pikpak/My Telegram/GroupC/c2.mp4", "c2.mp4", "review");
        // Single-video subfolder -> stays in catch-all "none"
        make_row("v7", "pikpak/My Telegram/Lonely/d1.mp4", "d1.mp4", "review");
        // Broad root My Telegram -> stays in catch-all "none"
        make_row("v8", "pikpak/My Telegram/root1.mp4", "root1.mp4", "review");
        make_row("v9", "pikpak/My Telegram/root2.mp4", "root2.mp4", "review");
        drop(proc_db);

        let mut engine = Engine::new(config.clone()).unwrap();
        let page = execute(&mut engine, "review-page", json!({"view": "none"})).unwrap();
        let groups = page["groups"].as_array().unwrap();
        let keys: BTreeSet<&str> = groups.iter().filter_map(|g| g["key"].as_str()).collect();
        assert!(keys.contains("folder:My Telegram/PackA/sub"));
        assert!(keys.contains("folder:My Telegram/PackB/sub"));
        assert!(keys.contains("folder:My Telegram/GroupC"));
        assert!(keys.contains("none"));
        assert!(!keys.contains("folder:My Telegram/Lonely"));
        assert!(!keys.contains("folder:My Telegram"));

        let label_of = |k: &str| {
            groups
                .iter()
                .find(|g| g["key"] == k)
                .unwrap()["label"]
                .as_str()
                .unwrap()
                .to_owned()
        };
        assert_eq!(label_of("folder:My Telegram/PackA/sub"), "PackA/sub");
        assert_eq!(label_of("folder:My Telegram/PackB/sub"), "PackB/sub");
        assert_eq!(label_of("folder:My Telegram/GroupC"), "GroupC");
        assert_eq!(groups.iter().find(|g| g["key"] == "none").unwrap()["count"], 3);

        // Verify scope validation & ready re-OCR availability
        let folder_page = execute(
            &mut engine,
            "review-page",
            json!({"session": page["session"], "view": "none", "group": "folder:My Telegram/PackA/sub"}),
        )
        .unwrap();
        assert_eq!(folder_page["rows"].as_array().unwrap().len(), 2);

        let v2_path = media.join("pikpak/My Telegram/PackA/sub/a2.mp4").to_string_lossy().into_owned();
        let v2_task = json!({"vid": "v2", "path": v2_path, "manual": 0, "stage": "queued", "retry_stage": "text", "data": {"kind": "none", "ready": true}});
        assert!(available(&v2_task));
        assert!(review::matches_group_scope(
            &config,
            &v2_task,
            &v2_task["data"],
            "folder:My Telegram/PackA/sub"
        ));
        assert!(!review::matches_group_scope(
            &config,
            &v2_task,
            &v2_task["data"],
            "folder:My Telegram/GroupC"
        ));
        assert!(!available(&json!({"stage": "error", "retry_stage": "text", "manual": 0, "data": {"ready": true}})));

        drop(engine);
        fs::remove_dir_all(root).unwrap();
    }
}
