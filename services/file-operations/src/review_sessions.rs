//! Durable, immutable review evidence. Model progress never becomes human authority.
use crate::{
    config::Config,
    db,
    engine::Engine,
    model::{Error, Result},
    review, shared_metadata,
};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
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
    let has_deliveries: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='review_deliveries')",
        [],
        |r| r.get(0),
    )?;
    if !has_snapshots {
        conn.execute_batch("PRAGMA auto_vacuum = INCREMENTAL; PRAGMA journal_mode = WAL;")?;
    }
    conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA synchronous = NORMAL;")?;
    if !has_snapshots || !has_deliveries {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS session_snapshots(id TEXT PRIMARY KEY, expires INTEGER NOT NULL, snapshot TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS review_deliveries(token TEXT PRIMARY KEY, session TEXT NOT NULL, vid TEXT NOT NULL, evidence TEXT NOT NULL, FOREIGN KEY(session) REFERENCES session_snapshots(id) ON DELETE CASCADE);
             CREATE INDEX IF NOT EXISTS review_delivery_session ON review_deliveries(session, vid);",
        )?;
    }
    Ok(conn)
}
fn sidecar_readonly(config: &Config) -> Result<rusqlite::Connection> {
    db::open_readonly(&config.review_sessions_db())
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
    review::number(&t["manual"]).unwrap_or(0) != 0
        || t["stage"] == "review"
        || (t["data"]["ready"] == true
            && (t["stage"] == "face"
                || t["stage"] == "face_queued"
                || (t["stage"] == "retry" || t["stage"] == "error") && t["retry_stage"] == "face"))
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
    let queue = review::read(config, "integration-queue.json", json!([]))?.1;
    let history = review::read(config, "archive-history.json", json!([]))?.1;
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
    let excluded_vids: BTreeSet<String> = queue
        .as_array()
        .unwrap()
        .iter()
        .chain(
            history
                .as_array()
                .unwrap()
                .iter()
                .filter(|r| r["undone"] != true),
        )
        .filter_map(|r| r["vid"].as_str().map(str::to_owned))
        .collect();
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
    let queue = review::read(config, "integration-queue.json", json!([]))?.1;
    let history = review::read(config, "archive-history.json", json!([]))?.1;
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
    let excluded_vids: BTreeSet<String> = queue
        .as_array()
        .unwrap()
        .iter()
        .chain(
            history
                .as_array()
                .unwrap()
                .iter()
                .filter(|r| r["undone"] != true),
        )
        .filter_map(|r| r["vid"].as_str().map(str::to_owned))
        .collect();
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
fn bucket(t: &Value) -> &str {
    if review::number(&t["manual"]).unwrap_or(0) != 0 {
        "manual"
    } else {
        t["data"]["kind"].as_str().unwrap_or("none")
    }
}
fn group(t: &Value) -> Value {
    let kind = bucket(t);
    let suggested = t["data"]["suggested"].as_str().unwrap_or("");
    let face = t["data"]["face"]["unknown_group"].as_str().unwrap_or("");
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
    fn state(&self, config: &Config, t: &Value) -> Result<Value> {
        let id = t["vid"].as_str().unwrap_or("");
        let path = t["path"].as_str().unwrap_or("");
        let hidden = if self.candidates.contains(id) {
            std::fs::metadata(path)
                .ok()
                .map(|s| review::health_hidden(config, id, path, &s))
                .transpose()?
                .unwrap_or(false)
        } else {
            false
        };
        Ok(json!({"hidden":hidden}))
    }
}
fn dispositions(config: &Config) -> Result<BTreeMap<String, Value>> {
    let conn = schema_readonly(config)?;
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
    Ok(result)
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
            if value["schema_version"] != 1 && value["schema_version"] != 2 {
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
    fn base_evidence(&self) -> &[Value] {
        self.base["evidence"].as_array().map_or(&[], Vec::as_slice)
    }
    fn iter(&self) -> impl Iterator<Item = &Value> {
        let base = self.base_evidence();
        (0..base.len()).map(|i| self.overrides.get(&i).unwrap_or(&base[i]))
    }
    fn clusters(&self) -> &Value {
        &self.base["clusters"]
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
    let base_evidence = snapshot["evidence"].as_array().unwrap();
    let positions: BTreeMap<&str, usize> = base_evidence
        .iter()
        .enumerate()
        .filter_map(|(i, e)| e["task"]["vid"].as_str().map(|vid| (vid, i)))
        .collect();
    let mut overrides: BTreeMap<usize, Value> = BTreeMap::new();
    let mut next_pending = base_evidence.len() as u64;
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
                .or_insert_with(|| base_evidence[index].clone());
            e["disposition"] = plan["id"].clone();
            match action {
                "queue-confirm" | "review-delete" => e["completed"] = json!(true),
                "queue-undo" => {
                    e["completed"] = json!(false);
                    let restored:Option<String>=conn.query_row("SELECT plan FROM operations WHERE status='complete' AND json_extract(plan,'$.action')='queue-restore' AND json_extract(plan,'$.request.payload.parent_id')=?",[plan["id"].as_str().unwrap_or("")],|r|r.get(0)).optional()?;
                    if let Some(raw) = restored {
                        let child: Value = serde_json::from_str(&raw)?;
                        if child["execution"]["data"]["job_updates"]
                            .as_array()
                            .is_some_and(|rows| {
                                rows.iter().any(|r| {
                                    r["vid"] == e["task"]["vid"]
                                        && review::number(&r["deferred"]) == Some(0)
                                })
                            })
                        {
                            if e["pending_order"].is_null() {
                                e["pending_order"] = json!(next_pending);
                                next_pending += 1;
                            }
                            e["pending_seen"] = json!(true);
                            set_manual(e, "deferred", json!(0));
                        }
                    }
                }
                "review-note" => set_manual(e, "note", body["note"].clone()),
                "review-defer" => {
                    if e["deferred_order"].is_null() {
                        e["deferred_order"] = json!(next_deferred);
                        next_deferred += 1;
                    }
                    e["deferred_seen"] = json!(true);
                    set_manual(e, "deferred", json!(1));
                }
                "review-restore" => {
                    if e["pending_order"].is_null() {
                        e["pending_order"] = json!(next_pending);
                        next_pending += 1;
                    }
                    e["pending_seen"] = json!(true);
                    set_manual(e, "deferred", json!(0));
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
    let material = shared_metadata::MaterialReader::new(config)?;
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
    let created = id.is_none();
    let need_live_diff = created || command == "review-status";
    let (id, snapshot, in_sidecar, changed_count, waiting, refreshing) = if !need_live_diff {
        let id = id.unwrap();
        let (base, in_sidecar) = read_session(config, &id)?;
        let snapshot = amended(config, &id, base)?;
        let waiting = waiting_count(config)?;
        (id, snapshot, in_sidecar, 0usize, waiting, 0usize)
    } else {
        let material = shared_metadata::MaterialReader::new(config)?;
        let live = tasks_for_review(config, material.aliases())?;
        let health = HealthReader::new(config)?;
        let (id, snapshot, in_sidecar) = match id {
            Some(id) => {
                let (base, in_sidecar) = read_session(config, &id)?;
                let snap = amended(config, &id, base)?;
                (id, snap, in_sidecar)
            }
            None => {
                if command == "review-status" {
                    return Err(expired());
                }
                let dispositions = dispositions(config)?;
                let id = uuid::Uuid::new_v4().to_string();
                let mut evidence = live
                    .iter()
                    .filter(|t| available(t))
                    .map(|t| evidence(config, t, &material, &health, &dispositions))
                    .collect::<Result<Vec<_>>>()?;
                evidence.retain(|e| e["health"]["hidden"] != true);
                evidence.sort_by_key(|e| !has_watermark_candidate(&e["task"]["data"]));
                for (index, e) in evidence.iter_mut().enumerate() {
                    let key = if e["pending_seen"] == true {
                        "pending_order"
                    } else {
                        "deferred_order"
                    };
                    e[key] = json!(index);
                }
                let snapshot = json!({"schema_version":2,"evidence":evidence,"clusters":compact_clusters(review::read(config,"clusters.json",json!([]))?.1)});
                let raw = serde_json::to_string(&snapshot)?;
                if raw.len() > 64 * 1024 * 1024 {
                    return Err(Error::new(
                        503,
                        "review_scope_limit",
                        "核对记录过多，请缩小处理范围",
                    ));
                }
                let replace_session = body["replace_session"]
                    .as_str()
                    .map(str::trim)
                    .filter(|v| !v.is_empty())
                    .map(str::to_owned);
                let expires = {
                    let _guard = session_write_lock()
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    let mut sidecar = sidecar_write(config)?;
                    let expires = now(&sidecar)? + TTL;
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
                            params![&id, expires, &raw],
                        )?;
                        stx.commit()?;
                        deleted
                    };
                    if deleted > 0 {
                        maybe_incremental_vacuum(&sidecar, 16_384, 256);
                    }
                    expires
                };
                let arc = Arc::new(snapshot);
                session_cache_put(&config.review_sessions_db(), &id, expires, arc.clone());
                (
                    id,
                    AmendedSnapshot {
                        base: arc,
                        overrides: BTreeMap::new(),
                    },
                    true,
                )
            }
        };
        let mut changed_count = 0;
        if !created {
            let visible_by_id: BTreeMap<&str, &Value> = snapshot
                .iter()
                .filter(|e| e["completed"] != true)
                .filter_map(|e| e["task"]["vid"].as_str().map(|vid| (vid, &e["task"])))
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
            for e in snapshot.iter().filter(|e| e["completed"] != true) {
                if !live_ids.contains(e["task"]["vid"].as_str().unwrap_or(""))
                    || !material_matches(material.material_only(config, &e["scope"])?, e)
                    || health.state(config, &e["task"])? != e["health"]
                {
                    changed_count += 1
                }
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
        (id, snapshot, in_sidecar, changed_count, waiting, refreshing)
    };
    if need_live_diff {
        crate::model::trim_allocator();
    }
    if command == "review-status" {
        return Ok(
            json!({"session":id,"has_updates":changed_count>0,"changed":changed_count,"waiting":waiting,"refreshing":refreshing}),
        );
    }
    let s = |key: &str| body[key].as_str().unwrap_or("");
    let view = if s("view").is_empty() {
        "all"
    } else {
        s("view")
    };
    let deferred = view == "deferred";
    let person = s("person");
    let cluster = s("cluster");
    let mut counts = json!({"all":0,"suggested":0,"conflict":0,"none":0,"manual":0,"deferred":0});
    for e in snapshot.iter().filter(|e| e["completed"] != true) {
        let t = &e["task"];
        if review::number(&t["deferred"]).unwrap_or(0) != 0 {
            counts["deferred"] = json!(counts["deferred"].as_u64().unwrap() + 1)
        } else {
            for k in ["all", bucket(t)] {
                counts[k] = json!(counts[k].as_u64().unwrap_or(0) + 1)
            }
        }
    }
    let mut scope: Vec<_> = snapshot
        .iter()
        .filter(|e| {
            let t = &e["task"];
            let lane = review::number(&t["deferred"]).unwrap_or(0) != 0;
            let ever = e[if deferred {
                "deferred_seen"
            } else {
                "pending_seen"
            }] == true;
            (lane == deferred || ever)
                && (deferred || view == "all" || bucket(t) == view)
                && (person.is_empty()
                    || t["data"]["candidates"]
                        .as_array()
                        .is_some_and(|cs| cs.iter().any(|c| c["name"] == person))
                    || t["data"]["suggested"] == person)
                && (cluster.is_empty()
                    || snapshot.clusters().as_array().is_some_and(|cs| {
                        cs.iter().any(|c| {
                            c["cid"] == cluster
                                && c["vids"]
                                    .as_array()
                                    .is_some_and(|vs| {
                                        vs.iter()
                                            .any(|v| v == &t["vid"] || v["vid"] == t["vid"])
                                    })
                        })
                    }))
        })
        .collect();
    scope.sort_by_key(|e| {
        e[if deferred {
            "deferred_order"
        } else {
            "pending_order"
        }]
        .as_u64()
        .unwrap_or(u64::MAX)
    });
    let is_remaining = |e: &&Value| {
        e["completed"] != true
            && (review::number(&e["task"]["deferred"]).unwrap_or(0) != 0) == deferred
    };
    let mut grouped: BTreeMap<String, Value> = BTreeMap::new();
    let mut totals: BTreeMap<String, usize> = BTreeMap::new();
    for e in snapshot.iter() {
        *totals
            .entry(e["group"]["key"].as_str().unwrap_or("").into())
            .or_default() += 1;
    }
    for e in &scope {
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
    groups.sort_by(|a, b| {
        (a["kind"] != "suggested")
            .cmp(&(b["kind"] != "suggested"))
            .then_with(|| {
                totals
                    .get(b["key"].as_str().unwrap_or(""))
                    .cmp(&totals.get(a["key"].as_str().unwrap_or("")))
            })
            .then_with(|| a["label"].as_str().cmp(&b["label"].as_str()))
    });
    let focus = scope.iter().find(|e| e["task"]["vid"] == s("vid"));
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
    let rows: Vec<_> = scope
        .into_iter()
        .filter(|e| {
            selected
                .as_ref()
                .is_some_and(|g| e["group"]["key"] == g["key"])
                && (folder.is_empty()
                    || Path::new(e["task"]["path"].as_str().unwrap_or(""))
                        .parent()
                        .is_some_and(|p| p == Path::new(folder)))
                && (query.is_empty()
                    || format!(
                        "{} {} {}",
                        e["task"]["name"].as_str().unwrap_or(""),
                        e["task"]["path"].as_str().unwrap_or(""),
                        e["task"]["data"]["candidates"]
                    )
                    .to_lowercase()
                    .contains(&query))
        })
        .collect();
    let limit = 24usize;
    let page = rows
        .iter()
        .position(|e| e["task"]["vid"] == s("vid"))
        .map(|n| n / limit)
        .unwrap_or_else(|| s("page").parse::<usize>().unwrap_or(0))
        .min(rows.len().saturating_sub(1) / limit);
    let (delivered, current_epoch) = if in_sidecar {
        let mut delivered = vec![];
        let mut missing: Vec<(String, Option<String>, String)> = vec![];
        if created {
            for e in rows
                .iter()
                .skip(page * limit)
                .take(limit)
                .filter(|e| is_remaining(e))
            {
                let raw = serde_json::to_string(e)?;
                let vid = e["task"]["vid"].as_str().map(str::to_owned);
                let token = uuid::Uuid::new_v4().to_string();
                missing.push((token.clone(), vid, raw));
                let mut task = e["task"].clone();
                task["review_token"] = json!(token);
                delivered.push(task);
            }
            let current_epoch = if !missing.is_empty() {
                let _guard = session_write_lock()
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                let mut rw = sidecar_write(config)?;
                let epoch = now(&rw)?;
                let tx = rw.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
                {
                    let mut insert =
                        tx.prepare("INSERT OR IGNORE INTO review_deliveries VALUES(?,?,?,?)")?;
                    for (token, vid, raw) in &missing {
                        insert.execute(params![token, &id, vid.as_deref(), raw])?;
                    }
                }
                tx.commit()?;
                epoch
            } else {
                let ro = sidecar_readonly(config)?;
                now(&ro)?
            };
            (delivered, current_epoch)
        } else {
            let ro = sidecar_readonly(config)?;
            let has_deliveries = sidecar_has_deliveries_table(&ro);
            let mut stmt = if has_deliveries {
                Some(ro.prepare(
                    "SELECT token FROM review_deliveries WHERE session=? AND vid=? AND evidence=? LIMIT 1",
                )?)
            } else {
                None
            };
            for e in rows
                .iter()
                .skip(page * limit)
                .take(limit)
                .filter(|e| is_remaining(e))
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
        }
    } else {
        let ro = schema_readonly(config)?;
        let mut stmt = ro.prepare(
            "SELECT token FROM review_deliveries WHERE session=? AND vid=? AND evidence=? LIMIT 1",
        )?;
        let mut delivered = vec![];
        let mut missing: Vec<(String, Option<String>, String)> = vec![];
        for e in rows
            .iter()
            .skip(page * limit)
            .take(limit)
            .filter(|e| is_remaining(e))
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
    let has = |p: usize| rows.iter().skip(p * limit).take(limit).any(&is_remaining);
    let next = (page + 1..pages).find(|p| has(*p));
    let previous = (0..page).rev().find(|p| has(*p));
    let folders: BTreeSet<_> = snapshot
        .iter()
        .filter_map(|e| {
            Path::new(e["task"]["path"].as_str()?)
                .parent()
                .map(|p| p.display().to_string())
        })
        .collect();
    let heartbeat = review::read(config, "processing/heartbeat.json", json!({}))?.1;
    let age = current_epoch as f64 - heartbeat["at"].as_f64().unwrap_or(0.0);
    Ok(
        json!({"session":id,"counts":counts,"groups":groups,"group":selected,"rows":delivered,"total":rows.len(),"remaining":rows.iter().filter(|e|is_remaining(e)).count(),"page":page,"limit":limit,"next_page":next,"previous_page":previous,"waiting":waiting,"online":(-30.0..90.0).contains(&age),"folders":folders,"focus_missing":focus_missing,"has_updates":changed_count>0}),
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
        let sc_raw: String = sc_conn
            .query_row(
                "SELECT snapshot FROM session_snapshots ORDER BY rowid DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(sc_count, MAX_ACTIVE_SESSIONS);
        assert!(sc_del_sessions <= MAX_ACTIVE_SESSIONS);
        let sc_val: Value = serde_json::from_str(&sc_raw).unwrap();
        assert_eq!(sc_val["schema_version"], 2);
        assert_eq!(sc_val["clusters"][0]["vids"][0], "v1");
        assert!(sc_val["evidence"][0].get("material").is_none());
        assert!(sc_val["evidence"][0].get("text").is_none());
        assert!(sc_val["evidence"][0]["material_hash"].is_string());

        drop(sc_conn);
        drop(op_conn);
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

        drop(engine);
        drop(store);
        drop(jobs);
        drop(faces);
        fs::remove_dir_all(root).unwrap();
    }
}
