use crate::{
    config::Config,
    db,
    files::sync_parent,
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
        if schema > 1 {
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
          CREATE TABLE IF NOT EXISTS steps (
            operation_id TEXT NOT NULL REFERENCES operations(id), revision INTEGER NOT NULL,
            phase TEXT NOT NULL, evidence TEXT NOT NULL, at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            PRIMARY KEY(operation_id,revision)
          ); PRAGMA user_version=1;")?;
        sync_parent(&config.state_db())?;
        // Ticket #3 reconciles these records. Never auto-replay or discard an old marker here.
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
    pub fn blocked(&self) -> Result<bool> {
        Ok(self.db.query_row(
            "SELECT EXISTS(SELECT 1 FROM operations WHERE status='recovery_needed')",
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
        self.db.execute("UPDATE operations SET status='complete',phase='complete',result=?,revision=revision+1,updated_at=CURRENT_TIMESTAMP WHERE id=?",params![serde_json::to_string(result)?,plan.id])?;
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
        return Err(Error::new(404, "not_found", "找不到操作记录"));
    };
    let mut result = json!({"id":id,"status":record.status,"phase":record.phase,"revision":record.revision,"current_file":record.plan.file().src,"completed":if record.status=="complete"{1}else{0},"total":1,"failed":if record.error.is_some(){1}else{0}});
    if let Some(value) = record.result {
        result["result"] = value;
    }
    if let Some(value) = record.error {
        result["error"] = json!(value.error);
        result["failure"] = json!(value);
    }
    Ok(result)
}
