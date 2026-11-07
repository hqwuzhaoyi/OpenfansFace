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
        let schema: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if schema > 8 {
            return Err(Error::new(
                503,
                "schema_version",
                "操作日志由更新版本创建，拒绝降级写入",
            ));
        }
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
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
          ); PRAGMA user_version=8;")?;
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
        conn.execute("UPDATE operations SET status='recovery_needed',error=?,revision=revision+1 WHERE status IN ('accepted','running')",[error])?;
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
        let raw:Option<String>=self.db.query_row("SELECT evidence FROM steps WHERE operation_id=? AND phase=? ORDER BY revision DESC LIMIT 1",params![id,phase],|r|r.get(0)).optional()?;
        Ok(raw.map(|s| serde_json::from_str(&s)).transpose()?)
    }
    pub fn file_evidence(&self, id: &str, index: usize) -> Result<crate::files::MoveEvidence> {
        let raw:Option<String>=self.db.query_row("SELECT evidence FROM steps WHERE operation_id=? AND json_extract(evidence,'$.file_index')=? AND json_type(evidence,'$.move_evidence')='object' ORDER BY revision DESC LIMIT 1",params![id,index as i64],|r|r.get(0)).optional()?;
        if let Some(raw) = raw {
            let value: Value = serde_json::from_str(&raw)?;
            return Ok(serde_json::from_value(value["move_evidence"].clone())?);
        }
        let admitted = self.evidence(id)?;
        if let Some(evidence) = admitted
            .and_then(|v| v["files"].get(index).map(|f| f["move_evidence"].clone()))
            .filter(|v| !v.is_null())
        {
            return Ok(serde_json::from_value(evidence)?);
        }
        Ok(crate::files::MoveEvidence::default())
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
        self.db.execute("UPDATE operations SET status='rolled_back',phase='rolled_back',error=?,revision=revision+1,updated_at=CURRENT_TIMESTAMP WHERE id=?", params![serde_json::to_string(cause)?,id])?;
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
        let tx = self.db.transaction()?;
        tx.execute("UPDATE operations SET status=?,phase=?,revision=revision+1,updated_at=CURRENT_TIMESTAMP WHERE id=?",params![status,phase,id])?;
        tx.execute("INSERT INTO steps(operation_id,revision,phase,evidence) SELECT id,revision,?,? FROM operations WHERE id=?",params![phase,serde_json::to_string(&evidence)?,id])?;
        tx.commit()?;
        Ok(())
    }
    pub fn admit(&mut self, plan: &Plan, evidence: Value) -> Result<()> {
        let tx = self.db.transaction()?;
        tx.execute("UPDATE operations SET status='accepted',phase='accepted',revision=revision+1,evidence=?,updated_at=CURRENT_TIMESTAMP WHERE id=? AND status='not_started'",params![serde_json::to_string(&evidence)?,plan.id])?;
        tx.commit()?;
        Ok(())
    }
    pub fn complete(&mut self, plan: &Plan, result: &Value) -> Result<()> {
        self.db.execute("UPDATE operations SET status='complete',phase='complete',error=NULL,result=?,revision=revision+1,updated_at=CURRENT_TIMESTAMP WHERE id=?",params![serde_json::to_string(result)?,plan.id])?;
        Ok(())
    }
    pub fn fail(&mut self, id: &str, error: &Error, recovery: bool) -> Result<()> {
        self.db.execute("UPDATE operations SET status=?,error=?,revision=revision+1,updated_at=CURRENT_TIMESTAMP WHERE id=?",params![if recovery {"recovery_needed"}else{"failed"},serde_json::to_string(error)?,id])?;
        Ok(())
    }
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
fn record(db: &Connection, id: &str) -> Result<Option<Record>> {
    type StoredRecord = (
        String,
        String,
        String,
        String,
        i64,
        Option<String>,
        Option<String>,
    );
    let row:Option<StoredRecord>=db.query_row("SELECT plan,request_hash,status,phase,revision,result,error FROM operations WHERE id=?",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?))).optional()?;
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
    let db = db::open(path)?;
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
