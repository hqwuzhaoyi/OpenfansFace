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
    fs::{self, File, OpenOptions},
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
        let ownership = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;
        flock(&ownership, FlockOperation::NonBlockingLockExclusive)
            .map_err(|_| Error::new(503, "writer_busy", "另一个 Rust 执行器正在使用此状态目录"))?;
        let conn = Connection::open(config.state_db())?;
        let schema: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if schema > 2 {
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
          ); PRAGMA user_version=2;")?;
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
    let mut result = json!({"id":id,"status":record.status,"phase":record.phase,"revision":record.revision,"io_state":if ["complete","rolled_back","failed","cancelled","not_started"].contains(&record.status.as_str()){"idle"}else if record.status=="recovery_needed"{"unknown"}else{"running"},"current_file":record.plan.file().src,"completed":if record.status=="complete"{1}else{0},"total":1,"failed":if record.error.is_some(){1}else{0}});
    if let Some(value) = record.result {
        result["result"] = value;
    }
    if let Some(value) = record.error {
        result["error"] = json!(value.error);
        result["failure"] = json!(value);
    }
    Ok(result)
}
