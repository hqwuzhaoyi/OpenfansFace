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
    publishers: usize,
}
pub struct AcceptedPublisher(Worker);
impl AcceptedPublisher {
    pub fn send(&self, work: Work) -> Result<()> {
        self.0.enqueue(work, true)
    }
}
impl Drop for AcceptedPublisher {
    fn drop(&mut self) {
        self.0.gate.lock().unwrap().publishers -= 1;
    }
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
        let previous = crate::control::read(&marker, crate::control::MARKER_LIMIT)?;
        if std::env::var("FILE_OPERATIONS_ADMISSION").as_deref() == Ok("closed")
            && previous.is_none()
        {
            crate::files::atomic_write(&marker, b"closed")?;
        }
        let accepting = previous.is_none()
            && std::env::var("FILE_OPERATIONS_ADMISSION").as_deref() != Ok("closed");
        Ok(Self {
            sender,
            gate: Arc::new(Mutex::new(Gate {
                accepting,
                pending: 0,
                publishers: 0,
            })),
            config,
        })
    }
    pub fn begin_publisher(&self) -> Result<AcceptedPublisher> {
        self.config.require_writer()?;
        let mut gate = self.gate.lock().unwrap();
        if !gate.accepting {
            return Err(Error::new(503, "admission_closed", "准入已关闭"));
        }
        gate.publishers += 1;
        Ok(AcceptedPublisher(self.clone()))
    }
    pub fn try_send(&self, work: Work) -> Result<()> {
        self.enqueue(work, false)
    }
    fn enqueue(&self, work: Work, already_admitted: bool) -> Result<()> {
        let mut gate = self.gate.lock().unwrap();
        if self.config.parity_root.is_some() {
            let preview = match &work.command {
                Command::Files(e) => {
                    e.mode == "preview"
                        && [
                            "rename-file",
                            "move-files",
                            "delete-files",
                            "restore-files",
                            "delete-staged",
                            "clear-staging",
                            "rename-person",
                            "merge-person",
                            "delete-person",
                        ]
                        .contains(&e.request.action.as_str())
                }
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
        if !gate.accepting && !already_admitted {
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
    pub fn completion(&self) -> Completion {
        Completion(self.gate.clone())
    }
    pub fn has_pending(&self) -> bool {
        self.gate.lock().unwrap().pending > 0
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
            if state["drained"] != true {
                return Err(Error::new(
                    409,
                    "recovery_required",
                    "未完成操作或共享写锁尚未核对",
                ));
            }
            if crate::control::read(&marker, crate::control::MARKER_LIMIT)?.is_some() {
                std::fs::remove_file(&marker)?;
                crate::files::sync_parent(&marker)?;
            }
        } else {
            // Persistence failure must never leave this running process accepting work.
            gate.accepting = false;
            crate::control::read(&marker, crate::control::MARKER_LIMIT)?;
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
        let recovery_required: i64 = db.query_row(
            "SELECT count(*) FROM operations WHERE status IN ('recovery_needed','rolling_back')",
            [],
            |r| r.get(0),
        )?;
        let shared_gate = crate::control::read(
            &self.config.base.join(".review-write.lock"),
            crate::control::MARKER_LIMIT,
        )?;
        let shared_gate_present = shared_gate.is_some();
        let foreign_gate = if let Some(raw) = shared_gate {
            let marker: Value = serde_json::from_str(&raw)?;
            marker["backend"] != "rust" || marker["authority"] != json!(self.config.state_dir())
        } else {
            false
        };
        let stat = crate::files::identity(&std::fs::symlink_metadata(
            self.config.state_dir().join("executor.lock"),
        )?);
        let authority: String = db.query_row(
            "SELECT identity FROM authority WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        let authority: Value = serde_json::from_str(&authority)?;
        let authority_valid = authority == json!({"dev":stat["dev"],"ino":stat["ino"]});
        let drained = gate.pending == 0
            && gate.publishers == 0
            && unresolved == 0
            && !shared_gate_present
            && authority_valid;
        Ok(
            json!({"service":"face-file-operations", "mode":if self.config.parity_root.is_some(){"parity"}else{"writer"},
            "accepting":gate.accepting,"pending":gate.pending,"publishers":gate.publishers,"unresolved":unresolved,"drained":drained,
            "ready":gate.accepting && authority_valid && recovery_required == 0 && !foreign_gate && (unresolved == 0 || gate.pending > 0) && self.config.parity_root.is_none(),
            "recovery_required":recovery_required,
            "schema":schema,"authority":authority,"authority_valid":authority_valid,"shared_gate_present":shared_gate_present}),
        )
    }
}
