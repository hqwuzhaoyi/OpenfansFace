use crate::{
    config::Config,
    files::{self, PinnedMove, WriteGate},
    ledger, metadata,
    model::{self, Error, FileMove, Plan, Request, Result, hash},
    store::Store,
};
use serde_json::{Value, json};
use std::path::Path;

pub struct Engine {
    pub config: Config,
    pub store: Store,
}
impl Engine {
    pub fn new(config: Config) -> Result<Self> {
        let store = Store::start(&config)?;
        Ok(Self { config, store })
    }
    pub fn preview(&mut self, request: Request) -> Result<Value> {
        validate(&request)?;
        files::claim_owner(&self.config, &request.id)?;
        if let Some(record) = self.store.get(&request.id)? {
            if record.request_hash != hash(&request) {
                return Err(Error::conflict("同一操作编号不能更换请求"));
            }
            return Ok(serde_json::to_value(record.plan)?);
        }
        let plan = self.plan(request)?;
        self.store.preview(&plan)?;
        Ok(serde_json::to_value(plan)?)
    }
    fn plan(&self, request: Request) -> Result<Plan> {
        let item = &request.files[0];
        let src = &item.path;
        let dst = Path::new(src)
            .parent()
            .ok_or_else(|| Error::new(400, "invalid_request", "文件路径无效"))?
            .join(&request.filename)
            .to_string_lossy()
            .into_owned();
        if *src == dst {
            return Err(Error::new(400, "invalid_request", "源文件与目标路径相同"));
        }
        let extension = Path::new(src)
            .extension()
            .and_then(|v| v.to_str())
            .unwrap_or("")
            .to_lowercase();
        if ![
            "mp4", "m4v", "mov", "mkv", "webm", "avi", "wmv", "ts", "mts", "m2ts",
        ]
        .contains(&extension.as_str())
        {
            return Err(Error::new(
                400,
                "invalid_request",
                "只允许操作人物目录中的视频",
            ));
        }
        if Path::new(&request.filename)
            .extension()
            .and_then(|v| v.to_str())
            .unwrap_or("")
            .to_lowercase()
            != extension
        {
            return Err(Error::new(400, "invalid_request", "重命名需保留原扩展名"));
        }
        let pinned = PinnedMove::new(&self.config, &request.person, src, &dst)?;
        let stat = pinned.stat()?;
        pinned.verify(&item.version)?;
        let ids = metadata::registered_ids(&self.config, src)?;
        if ids.is_empty() || !ids.contains(&item.vid) {
            return Err(Error::new(
                409,
                "unsupported_scope",
                "当前 Rust 路径仅支持已登记且稳定编号匹配的视频",
            )
            .at(src));
        }
        let file = FileMove {
            vids: ids,
            src: src.clone(),
            dst,
            size: stat.len(),
            version: item.version.clone(),
        };
        let mut plan = Plan {
            id: request.id.clone(),
            action: request.action.clone(),
            person: request.person.clone(),
            target_person: request.person.clone(),
            request,
            version: String::new(),
            files: vec![file],
            total_bytes: stat.len(),
            directories: vec![],
            ledger_version: String::new(),
            metadata_version: String::new(),
        };
        plan.metadata_version = metadata::version(&metadata::snapshot(&self.config, &plan)?);
        plan.ledger_version = hash(&ledger::snapshot(&self.config, &plan)?);
        plan.version = hash(&plan);
        Ok(plan)
    }
    pub fn apply(&mut self, request: Request, version: String) -> Result<Value> {
        validate(&request)?;
        files::claim_owner(&self.config, &request.id)?;
        let record = self
            .store
            .get(&request.id)?
            .ok_or_else(|| Error::conflict("请先预览操作"))?;
        if record.request_hash != hash(&request) || record.plan.version != version {
            return Err(Error::conflict("操作请求或预览版本与原意图不一致"));
        }
        if let Some(result) = record.result {
            return Ok(result);
        }
        if let Some(error) = record.error {
            return Err(error);
        }
        if record.status != "not_started" {
            return Err(Error::new(
                503,
                "recovery_required",
                "操作已受理，需查询原结果",
            ));
        }
        if self.store.blocked()? {
            return Err(Error::new(
                503,
                "recovery_required",
                "存在未核对操作，已暂停新写入",
            ));
        }
        // Busy is retryable and MUST NOT consume the untouched preview.
        let gate = WriteGate::acquire(&self.config, &request.id)?;
        let prepared = (|| {
            let current = self.plan(request.clone())?;
            if current.version != version {
                return Err(Error::conflict("文件或相关记录已变化，请重新预览"));
            }
            let snapshot = metadata::snapshot(&self.config, &current)?;
            if metadata::version(&snapshot) != current.metadata_version {
                return Err(Error::conflict("预检期间元数据发生变化"));
            }
            let pinned = PinnedMove::new(
                &self.config,
                &current.person,
                &current.file().src,
                &current.file().dst,
            )?;
            pinned.verify(&current.file().version)?;
            let evidence = json!({"metadata":snapshot,"ledger":ledger::snapshot(&self.config,&current)?,"source_identity":files::identity(&pinned.stat()?)});
            Ok((current, snapshot, pinned, evidence))
        })();
        let (plan, snapshot, pinned, evidence) = match prepared {
            Ok(p) => p,
            Err(e) => {
                gate.release()?;
                return Err(e);
            }
        };
        // An admission COMMIT error is uncertain too: keep the marker even before media changes.
        if let Err(error) = self.store.admit(&plan, evidence) {
            let failure = Error::new(
                503,
                "recovery_required",
                format!("意图持久化结果未知，保留写锁：{error}"),
            );
            let _ = self.store.fail(&plan.id, &failure, true);
            return Err(failure);
        }
        let executed = self.execute(&plan, &snapshot, &pinned);
        match executed {
            Ok(result) => {
                gate.release()?;
                crate::fault::checkpoint(&self.config, "complete")?;
                Ok(result)
            }
            Err(error) => {
                // Admission and every external side effect are durable. No guessed compensation:
                // even a COMMIT error may have committed. #3 will reconcile exact before/after receipts.
                let failure = Error::new(
                    503,
                    "recovery_required",
                    format!("操作 {} 需要核对恢复，已暂停写入：{}", plan.id, error.error),
                )
                .at(error.path.unwrap_or_else(|| plan.file().dst.clone()));
                self.store.fail(&plan.id, &failure, true)?;
                Err(failure)
            }
        }
    }
    fn checkpoint(&mut self, id: &str, phase: &str, evidence: Value) -> Result<()> {
        self.store.step(id, "running", phase, evidence)?;
        crate::fault::checkpoint(&self.config, phase)
    }
    fn execute(
        &mut self,
        plan: &Plan,
        snapshot: &metadata::Snapshot,
        pinned: &PinnedMove,
    ) -> Result<Value> {
        let id = &plan.id;
        self.checkpoint(
            id,
            "ledger_reserve_intent",
            json!({"signature":plan.version}),
        )?;
        ledger::reserve(&self.config, plan)?;
        self.checkpoint(id, "ledger_reserved", json!({"state":"prepared"}))?;
        self.checkpoint(id,"destination_publish_intent",json!({"src":plan.file().src,"dst":plan.file().dst,"identity":files::identity(&pinned.stat()?)}))?;
        pinned.publish(&plan.file().version)?;
        crate::fault::checkpoint(&self.config, "destination_published_unrecorded")?;
        self.checkpoint(
            id,
            "destination_published",
            files::identity(&pinned.stat()?),
        )?;
        self.checkpoint(id, "source_remove_intent", json!({"src":plan.file().src}))?;
        pinned.remove_source(&plan.file().version)?;
        crate::fault::checkpoint(&self.config, "source_removed_unrecorded")?;
        self.checkpoint(id, "source_removed", files::identity(&pinned.stat()?))?;
        self.checkpoint(id, "faces_commit_intent", json!({"before":snapshot.videos}))?;
        let evidence = metadata::apply_faces(&self.config, plan, snapshot)?;
        crate::fault::checkpoint(&self.config, "faces_committed_unrecorded")?;
        self.checkpoint(id, "faces_committed", evidence)?;
        self.checkpoint(id, "jobs_commit_intent", json!({"before":snapshot.jobs}))?;
        let evidence = metadata::apply_jobs(&self.config, plan, snapshot)?;
        crate::fault::checkpoint(&self.config, "jobs_committed_unrecorded")?;
        self.checkpoint(id, "jobs_committed", evidence)?;
        for doc in &snapshot.documents {
            if doc.before == doc.after {
                continue;
            }
            self.checkpoint(id, "json_replace_intent", serde_json::to_value(doc)?)?;
            metadata::apply_document(&self.config, doc)?;
            crate::fault::checkpoint(&self.config, "json_replaced_unrecorded")?;
            self.checkpoint(
                id,
                "json_replaced",
                json!({"name":doc.name,"after_hash":hash(&doc.after)}),
            )?;
        }
        self.checkpoint(
            id,
            "ledger_commit_intent",
            json!({"signature":plan.version}),
        )?;
        let evidence = ledger::apply(&self.config, plan)?;
        crate::fault::checkpoint(&self.config, "ledger_committed_unrecorded")?;
        self.checkpoint(id, "ledger_committed", evidence)?;
        let result = model::final_result(plan);
        self.store.complete(plan, &result)?;
        Ok(result)
    }
}
fn validate(request: &Request) -> Result<()> {
    model::validate_id(&request.id)?;
    if request.action != "rename-file" || request.files.len() != 1 {
        return Err(Error::new(
            400,
            "unsupported_scope",
            "当前 Rust 接口每次只支持一个已登记视频改名",
        ));
    }
    files::validate_name(&request.person)?;
    files::validate_name(&request.filename)?;
    Ok(())
}
