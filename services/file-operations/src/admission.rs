//! One admission boundary for HTTP and internal publishers. Closing never cancels work.
use crate::{
    Command, Work,
    config::Config,
    model::{Error, Result},
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex, mpsc};

struct Gate {
    accepting: bool,
    pending: usize,
}
pub struct Completion(Arc<Mutex<Gate>>);
impl Completion {
    pub fn complete(&self) {
        self.0.lock().unwrap().pending -= 1;
    }
}
#[derive(Clone)]
pub struct Worker {
    sender: mpsc::SyncSender<Work>,
    gate: Arc<Mutex<Gate>>,
    config: Config,
}
impl Worker {
    pub fn new(config: Config, sender: mpsc::SyncSender<Work>) -> Result<Self> {
        let marker = config.state_dir().join("admission-closed");
        if std::env::var("FILE_OPERATIONS_ADMISSION")
            .is_ok_and(|v| !["open", "closed"].contains(&v.as_str()))
        {
            return Err(Error::new(
                503,
                "configuration",
                "准入配置必须为 open 或 closed",
            ));
        }
        if std::env::var("FILE_OPERATIONS_ADMISSION").as_deref() == Ok("closed") {
            crate::files::atomic_write(&marker, b"closed")?;
        }
        let accepting = !marker.exists();
        Ok(Self {
            sender,
            gate: Arc::new(Mutex::new(Gate {
                accepting,
                pending: 0,
            })),
            config,
        })
    }
    pub fn try_send(&self, work: Work) -> Result<()> {
        let mut gate = self.gate.lock().unwrap();
        if self.config.parity_root.is_some() {
            let preview = match &work.command {
                Command::Files(e) => e.mode == "preview" && e.request.action != "metadata-write",
                Command::Batch(b) => b["mode"] == "preview",
                Command::Archive { command, .. } => command == "archive-preview",
                Command::Metadata { .. } => false,
            };
            if !preview {
                return Err(Error::new(
                    403,
                    "read_only_parity",
                    "只读对照只允许预览，不允许执行、发布或恢复",
                ));
            }
        }
        if !gate.accepting {
            return Err(Error::new(
                503,
                "admission_closed",
                "已停止接收新操作；已受理操作仍可查询",
            ));
        }
        // This mutex covers enqueue and closing. Completion cannot pass enqueue accounting.
        self.sender
            .try_send(work)
            .map_err(|_| Error::new(503, "writer_busy", "执行队列已满或执行器不可用"))?;
        gate.pending += 1;
        Ok(())
    }
    pub fn send(&self, work: Work) -> Result<()> {
        self.try_send(work)
    }
    pub fn completion(&self) -> Completion {
        Completion(self.gate.clone())
    }
    pub fn set_accepting(&self, accepting: bool) -> Result<Value> {
        let mut gate = self.gate.lock().unwrap();
        if accepting && self.config.parity_root.is_some() {
            return Err(Error::new(
                403,
                "read_only_parity",
                "对照实例不能提升为执行器",
            ));
        }
        let marker = self.config.state_dir().join("admission-closed");
        if accepting {
            let state = self.snapshot(&gate)?;
            if state["unresolved"] != 0 || state["shared_gate_present"] == true {
                return Err(Error::new(
                    409,
                    "recovery_required",
                    "未完成操作或共享写锁尚未核对",
                ));
            }
            if marker.exists() {
                std::fs::remove_file(&marker)?;
                crate::files::sync_parent(&marker)?;
            }
        } else {
            crate::files::atomic_write(&marker, b"closed")?;
        }
        gate.accepting = accepting;
        self.snapshot(&gate)
    }
    pub fn readiness(&self) -> Result<Value> {
        self.snapshot(&self.gate.lock().unwrap())
    }
    fn snapshot(&self, gate: &Gate) -> Result<Value> {
        let db = crate::db::open(&self.config.state_db())?;
        let schema: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
        let unresolved: i64 = db.query_row("SELECT count(*) FROM operations WHERE status IN ('accepted','running','recovery_needed','rolling_back')", [], |r| r.get(0))?;
        let shared_gate_present = self.config.base.join(".review-write.lock").exists();
        let stat = crate::files::identity(&std::fs::metadata(
            self.config.state_dir().join("executor.lock"),
        )?);
        let authority: String = db.query_row(
            "SELECT identity FROM authority WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        let authority: Value = serde_json::from_str(&authority)?;
        let authority_valid = authority == json!({"dev":stat["dev"],"ino":stat["ino"]});
        let drained =
            gate.pending == 0 && unresolved == 0 && !shared_gate_present && authority_valid;
        Ok(
            json!({"service":"face-file-operations", "mode":if self.config.parity_root.is_some(){"parity"}else{"writer"},
            "accepting":gate.accepting,"pending":gate.pending,"unresolved":unresolved,"drained":drained,
            "ready":gate.accepting && drained && self.config.parity_root.is_none(),
            "schema":schema,"authority":authority,"authority_valid":authority_valid,"shared_gate_present":shared_gate_present}),
        )
    }
}
