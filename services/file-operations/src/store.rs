use crate::{
    config::Config,
    db,
    files::{identity, sync_parent},
    model::{Error, Plan, Result, hash},
};
use rusqlite::{Connection, OptionalExtension, params};
use rustix::fs::{FlockOperation, flock};
use serde_json::{Value, json};
use std::{
    fs::{self, File},
    path::Path,
};

pub struct Store {
    pub db: Connection,
    _ownership: File,
}
impl Store {
    pub fn start(config: &Config) -> Result<Self> {
        fs::create_dir_all(config.state_dir())?;
        let path = config.state_dir().join("executor.lock");
        let ownership = crate::control::open_executor(&path)?;
        flock(&ownership, FlockOperation::NonBlockingLockExclusive)
            .map_err(|_| Error::new(503, "writer_busy", "另一个 Rust 执行器正在使用此状态目录"))?;
        crate::control::verify_path(&ownership, &path)?;
        let conn = Connection::open(config.state_db())?;
        conn.busy_timeout(std::time::Duration::from_secs(30))?;
        let schema: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if schema > 9 {
            return Err(Error::new(
                503,
                "schema_version",
                "操作日志由更新版本创建，拒绝降级写入",
            ));
        }
        conn.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
          CREATE TABLE IF NOT EXISTS operations (
            id TEXT PRIMARY KEY, request_hash TEXT NOT NULL, plan TEXT NOT NULL,
            status TEXT NOT NULL, phase TEXT NOT NULL, revision INTEGER NOT NULL DEFAULT 0,
            evidence TEXT, result TEXT, error TEXT, created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
          );
          CREATE TABLE IF NOT EXISTS authority (singleton INTEGER PRIMARY KEY CHECK(singleton=1), identity TEXT NOT NULL);
          CREATE TABLE IF NOT EXISTS gates (id TEXT PRIMARY KEY, evidence TEXT NOT NULL);
          CREATE TABLE IF NOT EXISTS fences (id TEXT PRIMARY KEY, request_hash TEXT NOT NULL, version TEXT);
          CREATE TABLE IF NOT EXISTS steps (
            operation_id TEXT NOT NULL REFERENCES operations(id), revision INTEGER NOT NULL,
            phase TEXT NOT NULL, evidence TEXT NOT NULL, at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            PRIMARY KEY(operation_id,revision)
          );
          CREATE TABLE IF NOT EXISTS review_sessions(id TEXT PRIMARY KEY, expires INTEGER NOT NULL, snapshot TEXT NOT NULL);
          CREATE TABLE IF NOT EXISTS review_deliveries(token TEXT PRIMARY KEY, session TEXT NOT NULL, vid TEXT NOT NULL, evidence TEXT NOT NULL, FOREIGN KEY(session) REFERENCES review_sessions(id) ON DELETE CASCADE);
          CREATE INDEX IF NOT EXISTS review_delivery_session ON review_deliveries(session,vid);
          CREATE INDEX IF NOT EXISTS idx_operations_status ON operations(status,created_at,id);
          CREATE INDEX IF NOT EXISTS idx_operations_complete_action ON operations(json_extract(plan,'$.action')) WHERE status='complete';
          CREATE INDEX IF NOT EXISTS idx_operations_complete_review_session ON operations(json_extract(plan,'$.request.payload.review_session')) WHERE status='complete';
          CREATE INDEX IF NOT EXISTS idx_operations_archive_action ON operations(created_at DESC) WHERE json_extract(plan,'$.action') IN ('archive','auto-archive');
          PRAGMA user_version=9;")?;
        let has_stat1: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='sqlite_stat1')",
                [],
                |r| r.get(0),
            )
            .unwrap_or(false);
        let has_action_stat = has_stat1
            && conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_stat1 WHERE idx='idx_operations_complete_action')",
                    [],
                    |r| r.get(0),
                )
                .unwrap_or(false);
        let has_archive_stat = has_stat1
            && conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_stat1 WHERE idx='idx_operations_archive_action')",
                    [],
                    |r| r.get(0),
                )
                .unwrap_or(false);
        if !has_action_stat || !has_archive_stat {
            let _ = conn.execute_batch(
                "ANALYZE idx_operations_status; ANALYZE idx_operations_complete_action; ANALYZE idx_operations_complete_review_session; ANALYZE idx_operations_archive_action;",
            );
        }
        let _ = conn.execute(
            "DELETE FROM review_sessions WHERE expires <= unixepoch()",
            [],
        );
        let _ = conn.execute(
            "DELETE FROM review_sessions WHERE id NOT IN (SELECT id FROM review_sessions ORDER BY expires DESC, rowid DESC LIMIT ?1)",
            [crate::review_sessions::MAX_ACTIVE_SESSIONS],
        );
        let _ = crate::review_sessions::cleanup_sidecar(config);
        let _ = conn.execute(
            "DELETE FROM operations WHERE status='not_started' AND created_at < datetime('now','-2 hours') AND NOT EXISTS(SELECT 1 FROM steps WHERE operation_id=operations.id) AND NOT EXISTS(SELECT 1 FROM gates WHERE id=operations.id)",
            [],
        );
        let stat = identity(&ownership.metadata()?);
        let identity = serde_json::to_string(&json!({"dev":stat["dev"],"ino":stat["ino"]}))?;
        conn.execute("INSERT OR IGNORE INTO authority VALUES(1,?)", [&identity])?;
        let original: String = conn.query_row(
            "SELECT identity FROM authority WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        if original != identity {
            return Err(Error::new(
                503,
                "writer_authority_changed",
                "执行权威锁文件已被替换，拒绝恢复写入",
            ));
        }
        sync_parent(&config.state_db())?;
        // Reconciliation runs only after the process-lifetime lock has been acquired.
        let error = serde_json::to_string(&Error::new(
            503,
            "recovery_required",
            "执行器重启，未完成操作需要核对恢复",
        ))?;
        conn.execute("UPDATE operations SET status='recovery_needed',error=?,revision=MAX(revision,COALESCE((SELECT MAX(s.revision) FROM steps s WHERE s.operation_id=operations.id),0))+1 WHERE status IN ('accepted','running')",[error])?;
        Ok(Self {
            db: conn,
            _ownership: ownership,
        })
    }
    pub fn open_preview(config: &Config) -> Result<Self> {
        let path = config.state_dir().join("executor.lock");
        let ownership = crate::control::open_executor(&path)?;
        let conn = db::open(&config.state_db())?;
        Ok(Self {
            db: conn,
            _ownership: ownership,
        })
    }
    pub fn remember_gate(&self, id: &str, evidence: &Value) -> Result<()> {
        self.db.execute("INSERT INTO gates(id,evidence) VALUES(?,?) ON CONFLICT(id) DO UPDATE SET evidence=excluded.evidence",params![id,serde_json::to_string(evidence)?])?;
        Ok(())
    }
    pub fn gate_evidence(&self, id: &str) -> Result<Option<Value>> {
        let raw: Option<String> = self
            .db
            .query_row("SELECT evidence FROM gates WHERE id=?", [id], |r| r.get(0))
            .optional()?;
        Ok(raw.map(|v| serde_json::from_str(&v)).transpose()?)
    }
    pub fn evidence(&self, id: &str) -> Result<Option<Value>> {
        let raw: Option<String> =
            self.db
                .query_row("SELECT evidence FROM operations WHERE id=?", [id], |r| {
                    r.get(0)
                })?;
        Ok(raw.map(|v| serde_json::from_str(&v)).transpose()?)
    }
    pub fn latest_file_step(&self, id: &str, index: usize) -> Result<Option<Value>> {
        let raw:Option<String>=self.db.query_row("SELECT evidence FROM steps WHERE operation_id=? AND json_extract(evidence,'$.file_index')=? ORDER BY revision DESC LIMIT 1",params![id,index as i64],|r|r.get(0)).optional()?;
        Ok(raw.map(|s| serde_json::from_str(&s)).transpose()?)
    }
    pub fn latest_step(&self, id: &str, phase: &str) -> Result<Option<Value>> {
        read_latest_step(&self.db, id, phase)
    }
    pub fn file_evidence(&self, id: &str, index: usize) -> Result<crate::files::MoveEvidence> {
        read_file_evidence(&self.db, id, index)
    }

    pub fn directory_evidence(
        &self,
        id: &str,
        path: &str,
    ) -> Result<Option<crate::files::DirectoryProof>> {
        let raw:Option<String>=self.db.query_row("SELECT evidence FROM steps WHERE operation_id=? AND json_extract(evidence,'$.directory.path')=? ORDER BY revision DESC LIMIT 1",params![id,path],|r|r.get(0)).optional()?;
        raw.map(|s| {
            let v: Value = serde_json::from_str(&s)?;
            Ok(serde_json::from_value(v["directory"].clone())?)
        })
        .transpose()
    }
    pub fn failures(&self, id: &str) -> Result<Vec<Value>> {
        let mut query=self.db.prepare("SELECT evidence FROM steps WHERE operation_id=? AND phase='file_failed' ORDER BY revision")?;
        let raw = query
            .query_map([id], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        raw.into_iter()
            .map(|s| Ok(serde_json::from_str(&s)?))
            .collect()
    }
    pub fn recovery_ids(&self) -> Result<Vec<String>> {
        let mut query = self.db.prepare("SELECT id FROM operations WHERE status IN ('accepted','running','recovery_needed','rolling_back') ORDER BY created_at,id")?;
        Ok(query
            .query_map([], |r| r.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?)
    }
    pub fn fenced(&self, id: &str) -> Result<bool> {
        Ok(self.db.query_row(
            "SELECT EXISTS(SELECT 1 FROM fences WHERE id=?)",
            [id],
            |r| r.get(0),
        )?)
    }
    pub fn fence(&mut self, request: &crate::model::Request, version: Option<&str>) -> Result<()> {
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let existing: Option<(String, Option<String>)> = tx
            .query_row(
                "SELECT request_hash,version FROM fences WHERE id=?",
                [&request.id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((signature, stored_version)) = existing
            && (signature != hash(request) || stored_version.as_deref() != version)
        {
            return Err(Error::conflict("已停止受理的编号不能更换范围"));
        }
        tx.execute(
            "INSERT OR IGNORE INTO fences(id,request_hash,version) VALUES(?,?,?)",
            params![request.id, hash(request), version],
        )?;
        tx.execute("UPDATE operations SET status='cancelled',phase='cancelled',revision=revision+1 WHERE id=? AND status='not_started'", [&request.id])?;
        tx.commit()?;
        Ok(())
    }
    pub fn rolled_back(&mut self, id: &str, cause: &Error) -> Result<()> {
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute(
            "UPDATE operations SET status='rolled_back',phase='rolled_back',error=?,revision=MAX(revision,COALESCE((SELECT MAX(s.revision) FROM steps s WHERE s.operation_id=operations.id),0))+1,updated_at=CURRENT_TIMESTAMP WHERE id=?",
            params![serde_json::to_string(cause)?, id],
        )?;
        compact_terminal_blobs(&tx, id)?;
        tx.commit()?;
        Ok(())
    }
    pub fn blocked(&self) -> Result<bool> {
        Ok(self.db.query_row(
            "SELECT EXISTS(SELECT 1 FROM operations WHERE status IN ('recovery_needed','rolling_back'))",
            [],
            |r| r.get(0),
        )?)
    }
    pub fn get(&self, id: &str) -> Result<Option<Record>> {
        record(&self.db, id)
    }
    pub fn preview(&self, plan: &Plan) -> Result<()> {
        self.db.execute("INSERT INTO operations(id,request_hash,plan,status,phase) VALUES(?,?,?,'not_started','preview')",params![plan.id,hash(&plan.request),serde_json::to_string(plan)?])?;
        Ok(())
    }
    pub fn step(&mut self, id: &str, status: &str, phase: &str, evidence: Value) -> Result<()> {
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute("UPDATE operations SET status=?1,phase=?2,updated_at=CURRENT_TIMESTAMP WHERE id=?3 AND status!=?1",params![status,phase,id])?;
        tx.execute("INSERT INTO steps(operation_id,revision,phase,evidence) VALUES(?1,COALESCE((SELECT MAX(revision) FROM steps WHERE operation_id=?1),(SELECT revision FROM operations WHERE id=?1))+1,?2,?3)",params![id,phase,serde_json::to_string(&evidence)?])?;
        tx.commit()?;
        Ok(())
    }
    pub fn admit(&mut self, plan: &Plan, evidence: Value) -> Result<()> {
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute("UPDATE operations SET status='accepted',phase='accepted',revision=revision+1,evidence=?,updated_at=CURRENT_TIMESTAMP WHERE id=? AND status='not_started'",params![serde_json::to_string(&evidence)?,plan.id])?;
        tx.commit()?;
        Ok(())
    }
    pub fn complete(&mut self, plan: &Plan, result: &Value) -> Result<()> {
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute(
            "UPDATE operations SET status='complete',phase='complete',error=NULL,result=?,revision=MAX(revision,COALESCE((SELECT MAX(s.revision) FROM steps s WHERE s.operation_id=operations.id),0))+1,updated_at=CURRENT_TIMESTAMP WHERE id=?",
            params![serde_json::to_string(result)?, plan.id],
        )?;
        compact_terminal_blobs(&tx, &plan.id)?;
        tx.commit()?;
        Ok(())
    }
    pub fn fail(&mut self, id: &str, error: &Error, recovery: bool) -> Result<()> {
        self.db.execute("UPDATE operations SET status=?,error=?,revision=MAX(revision,COALESCE((SELECT MAX(s.revision) FROM steps s WHERE s.operation_id=operations.id),0))+1,updated_at=CURRENT_TIMESTAMP WHERE id=?",params![if recovery {"recovery_needed"}else{"failed"},serde_json::to_string(error)?,id])?;
        Ok(())
    }
}
/// Strip heavy rollback/snapshot mirrors after terminal completion while preserving
/// file move provenance (`files`, `source_identity`, `gate`) and commit scope indexes.
fn compact_terminal_blobs(tx: &rusqlite::Transaction, id: &str) -> Result<()> {
    let ev: Option<Option<String>> = tx
        .query_row("SELECT evidence FROM operations WHERE id=?", [id], |r| r.get(0))
        .optional()?;
    if let Some(Some(raw)) = ev
        && let Ok(mut value) = serde_json::from_str::<Value>(&raw)
        && let Some(obj) = value.as_object_mut()
    {
        let mut changed = false;
        for key in ["domain", "metadata", "ledger", "before", "after"] {
            if obj.remove(key).is_some() {
                changed = true;
            }
        }
        if changed {
            obj.insert("compacted".into(), json!(true));
            tx.execute(
                "UPDATE operations SET evidence=? WHERE id=?",
                params![serde_json::to_string(&value)?, id],
            )?;
        }
    }
    let plan_raw: Option<String> = tx
        .query_row("SELECT plan FROM operations WHERE id=?", [id], |r| r.get(0))
        .optional()?;
    if let Some(raw) = plan_raw
        && let Ok(mut plan) = serde_json::from_str::<Value>(&raw)
        && let Some(docs) = plan
            .get_mut("execution")
            .and_then(|e| e.get_mut("data"))
            .and_then(|d| d.get_mut("documents"))
            .and_then(Value::as_array_mut)
    {
        let mut changed = false;
        for doc in docs.iter_mut() {
            if doc
                .as_object()
                .is_some_and(|o| o.contains_key("before") || o.contains_key("after"))
            {
                *doc = json!({
                    "name": doc.get("name").cloned().unwrap_or(Value::Null),
                    "before_hash": doc.get("before").filter(|v| !v.is_null()).map(hash),
                    "after_hash": doc.get("after").filter(|v| !v.is_null()).map(hash),
                    "compacted": true,
                });
                changed = true;
            }
        }
        if changed {
            tx.execute(
                "UPDATE operations SET plan=? WHERE id=?",
                params![serde_json::to_string(&plan)?, id],
            )?;
        }
    }
    let scope_rows: Vec<(i64, String)> = {
        let mut stmt = tx.prepare(
            "SELECT revision, evidence FROM steps WHERE operation_id=? AND phase='commit_scope'",
        )?;
        let rows = stmt
            .query_map([id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows
    };
    for (rev, raw) in scope_rows {
        if let Ok(mut value) = serde_json::from_str::<Value>(&raw)
            && let Some(obj) = value.as_object_mut()
            && obj.remove("domain").is_some()
        {
            obj.insert("compacted".into(), json!(true));
            tx.execute(
                "UPDATE steps SET evidence=? WHERE operation_id=? AND revision=?",
                params![serde_json::to_string(&value)?, id, rev],
            )?;
        }
    }
    let json_rows: Vec<(i64, String)> = {
        let mut stmt = tx.prepare(
            "SELECT revision, evidence FROM steps WHERE operation_id=? AND phase='json_replace_intent'",
        )?;
        let rows = stmt
            .query_map([id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows
    };
    for (rev, raw) in json_rows {
        if let Ok(value) = serde_json::from_str::<Value>(&raw)
            && value
                .as_object()
                .is_some_and(|o| o.contains_key("before") || o.contains_key("after"))
        {
            let compact = json!({
                "name": value.get("name").cloned().unwrap_or(Value::Null),
                "before_hash": value.get("before").map(hash),
                "after_hash": value.get("after").map(hash),
                "compacted": true,
            });
            tx.execute(
                "UPDATE steps SET evidence=? WHERE operation_id=? AND revision=?",
                params![serde_json::to_string(&compact)?, id, rev],
            )?;
        }
    }
    Ok(())
}
pub struct Record {
    pub plan: Plan,
    pub request_hash: String,
    pub status: String,
    pub phase: String,
    pub revision: i64,
    pub result: Option<Value>,
    pub error: Option<Error>,
}
pub(crate) fn record(db: &Connection, id: &str) -> Result<Option<Record>> {
    type StoredRecord = (
        String,
        String,
        String,
        String,
        i64,
        Option<String>,
        Option<String>,
    );
    let row:Option<StoredRecord>=db.query_row("SELECT o.plan,o.request_hash,o.status,COALESCE((SELECT s.phase FROM steps s WHERE s.operation_id=o.id AND s.revision>o.revision ORDER BY s.revision DESC LIMIT 1),o.phase),MAX(o.revision,COALESCE((SELECT MAX(s.revision) FROM steps s WHERE s.operation_id=o.id),0)),o.result,o.error FROM operations o WHERE o.id=?",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?))).optional()?;
    row.map(
        |(plan, request_hash, status, phase, revision, result, error)| {
            Ok(Record {
                plan: serde_json::from_str(&plan)?,
                request_hash,
                status,
                phase,
                revision,
                result: result.map(|s| serde_json::from_str(&s)).transpose()?,
                error: error.map(|s| serde_json::from_str(&s)).transpose()?,
            })
        },
    )
    .transpose()
}
pub fn status(path: &Path, id: &str) -> Result<Value> {
    let db = db::open_readonly(path)?;
    let Some(record) = record(&db, id)? else {
        if db.query_row(
            "SELECT EXISTS(SELECT 1 FROM fences WHERE id=?)",
            [id],
            |r| r.get::<_, bool>(0),
        )? {
            return Ok(
                json!({"id":id,"status":"cancelled","phase":"cancelled","revision":0,"completed":0,"total":0,"failed":0}),
            );
        }
        return Err(Error::new(404, "not_found", "找不到操作记录"));
    };
    let total = record.plan.files.len();
    if crate::staging_delete::action(&record.plan.action) {
        return crate::staging_delete::status(&db, &record);
    }
    let done:i64=db.query_row("SELECT COUNT(DISTINCT COALESCE(json_extract(evidence,'$.file_index'),0)) FROM steps WHERE operation_id=? AND phase='source_removed'",[id],|r|r.get(0))?;
    let failures: i64 = db.query_row(
        "SELECT COUNT(*) FROM steps WHERE operation_id=? AND phase='file_failed'",
        [id],
        |r| r.get(0),
    )?;
    let current:Option<String>=db.query_row("SELECT json_extract(evidence,'$.src') FROM steps WHERE operation_id=? AND json_type(evidence,'$.file_index')='integer' ORDER BY revision DESC LIMIT 1",[id],|r|r.get(0)).optional()?.flatten();
    let mut result = json!({"id":id,"status":record.status,"phase":record.phase,"revision":record.revision,"io_state":if ["complete","rolled_back","failed","cancelled","not_started"].contains(&record.status.as_str()){"idle"}else if record.status=="recovery_needed"{"unknown"}else{"running"},"current_file":current.or_else(||record.plan.files.first().map(|f|f.src.clone())),"completed":if record.status=="rolled_back"{0}else{done},"total":total,"failed":if record.error.is_some(){failures.max(1)}else{failures}});
    let mut latest = std::collections::HashMap::<usize, (String, Value)>::new();
    let mut query=db.prepare("SELECT phase,evidence FROM steps WHERE operation_id=? AND (json_type(evidence,'$.file_index')='integer' OR phase IN ('destination_publish_intent','destination_published','source_remove_intent','source_removed')) ORDER BY revision")?;
    for row in query.query_map([id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })? {
        let (phase, raw) = row?;
        let evidence: Value = serde_json::from_str(&raw)?;
        let index = evidence["file_index"].as_u64().unwrap_or(0) as usize;
        latest.insert(index, (phase, evidence));
    }
    let items:Vec<Value>=record.plan.files.iter().enumerate().map(|(index,file)|{
        let step=latest.get(&index);let phase=step.map(|s|s.0.as_str()).unwrap_or("not_started");
        let state=if phase=="file_failed"{"failed"}else if record.status=="complete"{"complete"}else if record.status=="rolled_back"{"rolled_back"}else if record.status=="recovery_needed" && step.is_some(){"recovery_needed"}else if phase=="source_removed"{"moved_pending_commit"}else if step.is_some(){"moving"}else{"not_started"};
        let mut item=json!({"file_index":index,"src":file.src,"dst":file.dst,"vids":file.vids,"status":state,"phase":phase});
        if let Some((_,evidence))=step && let Some(error)=evidence.get("error"){item["error"]=error.clone();}
        item
    }).collect();
    result["files"] = json!(items);
    result["processed"] = json!(done);
    result["committed"] = json!(if record.status == "complete" { done } else { 0 });
    if let Some(value) = record.result {
        result["result"] = value;
    }
    if let Some(value) = record.error {
        result["error"] = json!(value.error);
        result["failure"] = json!(value);
    }
    Ok(result)
}

pub fn read_latest_step(db: &Connection, id: &str, phase: &str) -> Result<Option<Value>> {
    let raw:Option<String>=db.query_row("SELECT evidence FROM steps WHERE operation_id=? AND phase=? ORDER BY revision DESC LIMIT 1",params![id,phase],|r|r.get(0)).optional()?;
    Ok(raw.map(|s| serde_json::from_str(&s)).transpose()?)
}

pub fn read_file_evidence(
    db: &Connection,
    id: &str,
    index: usize,
) -> Result<crate::files::MoveEvidence> {
    // Project only the movement proof, never full domain/face snapshot evidence.
    let raw:Option<String>=db.query_row("SELECT json_extract(evidence,'$.move_evidence') FROM steps WHERE operation_id=? AND json_extract(evidence,'$.file_index')=? AND json_type(evidence,'$.move_evidence')='object' ORDER BY revision DESC LIMIT 1",params![id,index as i64],|r|r.get(0)).optional()?;
    if let Some(raw) = raw {
        return Ok(serde_json::from_str(&raw)?);
    }
    let raw: Option<String> = db.query_row(
        "SELECT json_extract(evidence,'$.files[' || ?2 || '].move_evidence') FROM operations WHERE id=?1", params![id,index as i64], |r| r.get(0))?;
    Ok(raw
        .map(|s| serde_json::from_str(&s))
        .transpose()?
        .unwrap_or_default())
}

pub fn read_committed_file(db: &Connection, id: &str, index: usize) -> Result<bool> {
    let raw: Option<Option<String>> = db.query_row(
        "SELECT json_extract(evidence,'$.successful_file_indexes') FROM steps WHERE operation_id=? AND phase='commit_scope' ORDER BY revision DESC LIMIT 1", [id], |r| r.get(0)).optional()?;
    match raw {
        None => Ok(true), // Legacy single-file operations have no subset step.
        Some(Some(raw)) => Ok(serde_json::from_str::<Vec<usize>>(&raw)?.contains(&index)),
        Some(None) => Err(Error::conflict("持久化提交范围无效")),
    }
}

#[cfg(test)]
mod perf_index_tests {
    use super::*;

    #[test]
    fn operations_partial_indexes_cover_review_queries_and_prune_expired_sessions() {
        let root = fs::canonicalize("/tmp")
            .unwrap()
            .join(format!("face-store-idx-{}", uuid::Uuid::new_v4()));
        let base = root.join("base");
        let media = root.join("media");
        fs::create_dir_all(&base).unwrap();
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
        let store = Store::start(&config).unwrap();
        for i in 0..100 {
            store
                .db
                .execute(
                    "INSERT INTO operations(id,request_hash,plan,status,phase) VALUES(?1,'h','{\"action\":\"processing-result\"}','complete','complete')",
                    [format!("op-pr-{i}")],
                )
                .unwrap();
        }
        store
            .db
            .execute(
                "INSERT INTO review_sessions VALUES('expired', 1, '{}'), ('active', unixepoch() + 3600, '{}')",
                [],
            )
            .unwrap();
        store
            .db
            .execute(
                "INSERT INTO review_deliveries VALUES('t-exp', 'expired', 'v1', '{}'), ('t-act', 'active', 'v1', '{}')",
                [],
            )
            .unwrap();
        drop(store);
        let reopened = Store::start(&config).unwrap();
        let action_plan: Vec<String> = reopened
            .db
            .prepare("EXPLAIN QUERY PLAN SELECT id,plan FROM operations WHERE status='complete' AND json_extract(plan,'$.action') IN ('queue-confirm','queue-undo') ORDER BY rowid")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(3))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert!(
            action_plan
                .iter()
                .any(|line| line.contains("idx_operations_complete_action")),
            "expected idx_operations_complete_action in {action_plan:?}"
        );
        let session_plan: Vec<String> = reopened
            .db
            .prepare("EXPLAIN QUERY PLAN SELECT plan FROM operations WHERE status='complete' AND json_extract(plan,'$.request.payload.review_session')='s1' ORDER BY rowid")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(3))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert!(
            session_plan
                .iter()
                .any(|line| line.contains("idx_operations_complete_review_session")),
            "expected idx_operations_complete_review_session in {session_plan:?}"
        );
        let sessions: i64 = reopened
            .db
            .query_row("SELECT COUNT(*) FROM review_sessions", [], |r| r.get(0))
            .unwrap();
        let deliveries: i64 = reopened
            .db
            .query_row("SELECT COUNT(*) FROM review_deliveries", [], |r| r.get(0))
            .unwrap();
        assert_eq!(sessions, 1);
        assert_eq!(deliveries, 1);
        drop(reopened);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn store_step_and_admit_succeed_under_concurrent_writer() {
        let root = fs::canonicalize("/tmp")
            .unwrap()
            .join(format!("face-store-lock-{}", uuid::Uuid::new_v4()));
        let base = root.join("base");
        let media = root.join("media");
        fs::create_dir_all(&base).unwrap();
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
        let mut store = Store::start(&config).unwrap();
        let plan = Plan {
            id: "op-concurrent-step".into(),
            request: crate::model::Request {
                id: "op-concurrent-step".into(),
                action: "delete-files".into(),
                person: "Alice".into(),
                files: vec![],
                filename: String::new(),
                target_person: None,
                target_folder: None,
                payload: None,
            },
            version: "v1".into(),
            action: "delete-files".into(),
            person: "Alice".into(),
            target_person: String::new(),
            files: vec![],
            total_bytes: 0,
            directories: vec![],
            ledger_version: String::new(),
            metadata_version: String::new(),
            execution: None,
        };
        store.preview(&plan).unwrap();
        store.admit(&plan, json!({"ok": true})).unwrap();
        store
            .step(
                &plan.id,
                "running",
                "destination_publish_intent",
                json!({"file_index": 0}),
            )
            .unwrap();

        let state_db = config.state_db();
        let writer = std::thread::spawn(move || {
            for i in 0..20 {
                let mut conn = db::open(&state_db).unwrap();
                let tx = conn
                    .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                    .unwrap();
                tx.execute(
                    "INSERT OR REPLACE INTO review_sessions VALUES(?, unixepoch() + 3600, '{}')",
                    [format!("s-{i}")],
                )
                .unwrap();
                std::thread::sleep(std::time::Duration::from_millis(2));
                tx.commit().unwrap();
            }
        });

        for i in 0..30 {
            store
                .step(
                    &plan.id,
                    "running",
                    "source_removed",
                    json!({"file_index": i}),
                )
                .unwrap();
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        writer.join().unwrap();
        let step_count: i64 = store
            .db
            .query_row(
                "SELECT COUNT(*) FROM steps WHERE operation_id=?",
                [&plan.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(step_count, 31);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn complete_compacts_recovery_blobs_while_preserving_relocation_and_commit_scope() {
        let root = fs::canonicalize("/tmp")
            .unwrap()
            .join(format!("face-store-compact-{}", uuid::Uuid::new_v4()));
        let base = root.join("base");
        let media = root.join("media");
        fs::create_dir_all(&base).unwrap();
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
        let mut store = Store::start(&config).unwrap();
        let plan = Plan {
            id: "op-compact".into(),
            request: crate::model::Request {
                id: "op-compact".into(),
                action: "move-files".into(),
                person: "Alice".into(),
                files: vec![],
                filename: String::new(),
                target_person: None,
                target_folder: None,
                payload: None,
            },
            version: "v1".into(),
            action: "move-files".into(),
            person: "Alice".into(),
            target_person: String::new(),
            files: vec![],
            total_bytes: 0,
            directories: vec![],
            ledger_version: String::new(),
            metadata_version: String::new(),
            execution: None,
        };
        store.preview(&plan).unwrap();
        store
            .admit(
                &plan,
                json!({
                    "schema": 2,
                    "gate": {"marker": {"token": "g1"}},
                    "files": [{"source_identity": {"dev": "1", "ino": "2", "size": "10", "mtime_ns": "3"}}],
                    "domain": {"metadata": {"documents": ["huge"]}},
                    "metadata": {"big": true},
                    "ledger": {"assets": [1, 2, 3]}
                }),
            )
            .unwrap();
        store
            .step(
                &plan.id,
                "running",
                "commit_scope",
                json!({"successful_file_indexes": [0], "domain": {"huge": "blob"}}),
            )
            .unwrap();
        store
            .step(
                &plan.id,
                "running",
                "json_replace_intent",
                json!({"name": "report.json", "before": "{\"a\":1}", "after": "{\"a\":2}"}),
            )
            .unwrap();
        store.complete(&plan, &json!({"ok": true})).unwrap();

        let ev: String = store
            .db
            .query_row(
                "SELECT evidence FROM operations WHERE id=?",
                [&plan.id],
                |r| r.get(0),
            )
            .unwrap();
        let ev: Value = serde_json::from_str(&ev).unwrap();
        assert!(ev.get("domain").is_none());
        assert!(ev.get("metadata").is_none());
        assert!(ev.get("ledger").is_none());
        assert_eq!(ev["files"][0]["source_identity"]["ino"], "2");
        assert_eq!(ev["gate"]["marker"]["token"], "g1");
        assert_eq!(ev["compacted"], true);

        assert_eq!(read_committed_file(&store.db, &plan.id, 0).unwrap(), true);
        let scope = store.latest_step(&plan.id, "commit_scope").unwrap().unwrap();
        assert!(scope.get("domain").is_none());

        let jri = store
            .latest_step(&plan.id, "json_replace_intent")
            .unwrap()
            .unwrap();
        assert!(jri.get("before").is_none());
        assert!(jri.get("after").is_none());
        assert_eq!(jri["name"], "report.json");
        assert!(jri["before_hash"].is_string());
        assert!(jri["after_hash"].is_string());

        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn archive_progress_uses_partial_index_under_skewed_distribution() {
        let root = fs::canonicalize("/tmp")
            .unwrap()
            .join(format!("face-store-archive-idx-{}", uuid::Uuid::new_v4()));
        let base = root.join("base");
        let media = root.join("media");
        fs::create_dir_all(&base).unwrap();
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
        let store = Store::start(&config).unwrap();
        for i in 0..250 {
            store
                .db
                .execute(
                    "INSERT INTO operations(id,request_hash,plan,status,phase,created_at) VALUES(?1,'h','{\"action\":\"processing-result\"}','complete','complete','2026-11-22 00:00:00')",
                    [format!("pr-{i}")],
                )
                .unwrap();
        }
        store
            .db
            .execute(
                "INSERT INTO operations(id,request_hash,plan,status,phase,created_at) VALUES('arch-old','h','{\"action\":\"archive\"}','complete','complete','2026-11-22 01:00:00')",
                [],
            )
            .unwrap();
        store
            .db
            .execute(
                "INSERT INTO operations(id,request_hash,plan,status,phase,created_at) VALUES('arch-new','h','{\"action\":\"auto-archive\"}','running','file_moving','2026-11-22 02:00:00')",
                [],
            )
            .unwrap();
        store.db.execute_batch("ANALYZE;").unwrap();
        let query = "SELECT id FROM operations INDEXED BY idx_operations_archive_action WHERE json_extract(plan,'$.action') IN ('archive','auto-archive') ORDER BY created_at DESC,rowid DESC LIMIT 1";
        let eqp: Vec<String> = store
            .db
            .prepare(&format!("EXPLAIN QUERY PLAN {query}"))
            .unwrap()
            .query_map([], |r| r.get::<_, String>(3))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert!(
            eqp.iter()
                .any(|line| line.contains("idx_operations_archive_action")),
            "expected idx_operations_archive_action in {eqp:?}"
        );
        let latest: String = store.db.query_row(query, [], |r| r.get(0)).unwrap();
        assert_eq!(latest, "arch-new");
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn complete_and_rollback_compact_plan_documents_while_preserving_receipt_fields() {
        let root = fs::canonicalize("/tmp")
            .unwrap()
            .join(format!("face-store-plan-compact-{}", uuid::Uuid::new_v4()));
        let base = root.join("base");
        let media = root.join("media");
        fs::create_dir_all(&base).unwrap();
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
        let mut store = Store::start(&config).unwrap();
        let make_plan = |id: &str, action: &str| Plan {
            id: id.into(),
            request: crate::model::Request {
                id: id.into(),
                action: action.into(),
                person: "Alice".into(),
                files: vec![],
                filename: String::new(),
                target_person: None,
                target_folder: None,
                payload: Some(json!({"vids": ["v1"]})),
            },
            version: format!("ver-{id}"),
            action: action.into(),
            person: "Alice".into(),
            target_person: String::new(),
            files: vec![crate::model::FileMove {
                src: "/tmp/a.mp4".into(),
                dst: "/tmp/b.mp4".into(),
                size: 42,
                vids: vec!["v1".into()],
                version: "fv1".into(),
            }],
            total_bytes: 42,
            directories: vec![],
            ledger_version: String::new(),
            metadata_version: String::new(),
            execution: Some(crate::model::ExecutionIntent {
                scopes: vec![],
                failure_policy: crate::model::FailurePolicy::Rollback,
                stores: crate::model::AffectedStores {
                    faces: false,
                    jobs: true,
                    ledger: false,
                },
                data: json!({
                    "documents": [
                        {"name": "watermarks/results.json", "before": "{\"huge\":1}", "after": "{\"huge\":2}"},
                        {"name": "new-doc.json", "before": null, "after": "{\"created\":true}"}
                    ],
                    "rows": [{"vid": "v1", "name": "a.mp4"}],
                    "optional_rule": {"parent_id": id, "key": "k1"},
                    "optional_restore": {"parent_id": id, "vids": ["v1"]},
                    "job_updates": [{"vid": "v1", "deferred": 0}]
                }),
                result: json!({"ok": true}),
            }),
        };

        let p_complete = make_plan("op-plan-complete", "queue-confirm");
        store.preview(&p_complete).unwrap();
        store.complete(&p_complete, &json!({"ok": true})).unwrap();
        // Idempotent second complete() call must keep compacted hashes intact.
        store.complete(&p_complete, &json!({"ok": true})).unwrap();

        let rec = store.get(&p_complete.id).unwrap().unwrap();
        assert_eq!(rec.plan.version, "ver-op-plan-complete");
        assert_eq!(rec.plan.files.len(), 1);
        let exec_data = &rec.plan.execution.as_ref().unwrap().data;
        assert_eq!(exec_data["rows"][0]["vid"], "v1");
        assert_eq!(exec_data["optional_rule"]["key"], "k1");
        assert_eq!(exec_data["optional_restore"]["vids"][0], "v1");
        assert_eq!(exec_data["job_updates"][0]["vid"], "v1");
        let docs = exec_data["documents"].as_array().unwrap();
        assert_eq!(docs.len(), 2);
        assert!(docs[0].get("before").is_none() && docs[0].get("after").is_none());
        assert_eq!(docs[0]["name"], "watermarks/results.json");
        assert_eq!(docs[0]["before_hash"], json!(hash(&json!("{\"huge\":1}"))));
        assert_eq!(docs[0]["after_hash"], json!(hash(&json!("{\"huge\":2}"))));
        assert_eq!(docs[0]["compacted"], true);
        assert!(docs[1]["before_hash"].is_null());
        assert_eq!(docs[1]["after_hash"], json!(hash(&json!("{\"created\":true}"))));

        let p_rb = make_plan("op-plan-rb", "archive");
        store.preview(&p_rb).unwrap();
        store
            .rolled_back(&p_rb.id, &Error::conflict("rolled back test"))
            .unwrap();
        let rb_rec = store.get(&p_rb.id).unwrap().unwrap();
        let rb_docs = rb_rec.plan.execution.unwrap().data["documents"]
            .as_array()
            .unwrap()
            .clone();
        assert!(rb_docs[0].get("before").is_none());
        assert!(rb_docs[0].get("after").is_none());
        assert_eq!(rb_docs[0]["compacted"], true);

        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
}
