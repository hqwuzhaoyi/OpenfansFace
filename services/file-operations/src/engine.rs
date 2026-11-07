use crate::{
    config::Config,
    files::{self, PinnedMove, WriteGate},
    ledger, metadata,
    model::{self, Error, FileMove, Plan, Request, Result, hash},
    store::Store,
};
use rusqlite::OptionalExtension;
use serde_json::{Value, json};
use std::path::Path;

pub struct Engine {
    pub config: Config,
    pub store: Store,
}
impl Engine {
    pub fn new(config: Config) -> Result<Self> {
        let store = Store::start(&config)?;
        let mut engine = Self { config, store };
        engine.recover_startup()?;
        Ok(engine)
    }
    pub fn preview(&mut self, request: Request) -> Result<Value> {
        validate(&request)?;
        files::claim_owner(&self.config, &request.id)?;
        if self.store.fenced(&request.id)? {
            return Err(Error::new(
                409,
                "cancelled",
                "此操作已停止受理，迟到提交被拒绝",
            ));
        }
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
        if self.store.fenced(&request.id)? {
            return Err(Error::new(
                409,
                "cancelled",
                "此操作已停止受理，迟到提交被拒绝",
            ));
        }
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
        self.store.remember_gate(&request.id, &gate.evidence()?)?;
        crate::fault::checkpoint(&self.config, "gate_acquired")?;
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
            let evidence = json!({"metadata":snapshot,"ledger":ledger::snapshot(&self.config,&current)?,"source_identity":files::identity(&pinned.stat()?),"gate":gate.evidence()?});
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
        crate::fault::checkpoint(&self.config, "admitted")?;
        let executed = self.execute(&plan, &snapshot, &pinned);
        match executed {
            Ok(result) => {
                gate.release()?;
                crate::fault::checkpoint(&self.config, "complete")?;
                Ok(result)
            }
            Err(error) => {
                // A failed call is not a failed transaction. Inspect all domains before deciding.
                match self.reconcile(&plan, &snapshot, &pinned, true, &error) {
                    Ok(Some(result)) => {
                        gate.release()?;
                        Ok(result)
                    }
                    Ok(None) => {
                        gate.release()?;
                        Err(error)
                    }
                    Err(unknown) => {
                        self.recovery_failure(&plan, &unknown)?;
                        Err(self.store.get(&plan.id)?.unwrap().error.unwrap())
                    }
                }
            }
        }
    }
    fn recovery_failure(&mut self, plan: &Plan, error: &Error) -> Result<()> {
        let failure = Error::new(
            503,
            "recovery_required",
            format!("操作 {} 需要核对恢复，已暂停写入：{}", plan.id, error.error),
        )
        .at(error
            .path
            .clone()
            .unwrap_or_else(|| plan.file().dst.clone()));
        self.store.fail(&plan.id, &failure, true)
    }
    /// Explicit reconciliation fences an unreceived/unstarted request before any late POST.
    /// Already-admitted I/O is never reported cancelled merely because a waiter disappeared.
    pub fn reconcile_request(
        &mut self,
        request: Request,
        version: Option<String>,
    ) -> Result<Value> {
        validate(&request)?;
        files::claim_owner(&self.config, &request.id)?;
        if let Some(record) = self.store.get(&request.id)? {
            if record.request_hash != hash(&request)
                || version.as_ref().is_some_and(|v| *v != record.plan.version)
            {
                return Err(Error::conflict("核对范围与原操作不一致"));
            }
            if record.status == "not_started" {
                self.store.fence(&request, version.as_deref())?;
            } else if record.status == "recovery_needed" || record.status == "rolling_back" {
                self.recover_one(&request.id);
            }
        } else {
            self.store.fence(&request, version.as_deref())?;
        }
        crate::store::status(&self.config.state_db(), &request.id)
    }
    fn recover_startup(&mut self) -> Result<()> {
        for id in self.store.recovery_ids()? {
            self.recover_one(&id);
        }
        // Final receipt may be durable while marker removal was interrupted. Only this
        // authority's exact admitted gate may be removed. Legacy/foreign markers survive.
        let gate_path = self.config.base.join(".review-write.lock");
        if let Ok(Some(raw)) = files::read_optional(&gate_path)
            && let Ok(marker) = serde_json::from_str::<Value>(&raw)
            && let Some(id) = marker["operation"].as_str()
            && let Some(record) = self.store.get(id)?
        {
            if ["complete", "rolled_back"].contains(&record.status.as_str()) {
                if let Some(evidence) = self.store.evidence(id)?
                    && let Ok(gate) = WriteGate::recover(&self.config, id, &evidence["gate"])
                {
                    gate.release()?;
                }
            } else if record.status == "not_started"
                && let Some(evidence) = self.store.gate_evidence(id)?
                && let Ok(gate) = WriteGate::recover(&self.config, id, &evidence)
            {
                // The gate was acquired but no durable admission exists. No worker can
                // still execute under the lifetime flock. Fence before releasing it.
                self.store
                    .fence(&record.plan.request, Some(&record.plan.version))?;
                gate.release()?;
            }
        }
        Ok(())
    }
    fn recover_one(&mut self, id: &str) {
        let outcome = (|| -> Result<()> {
            let record = self
                .store
                .get(id)?
                .ok_or_else(|| Error::conflict("恢复意图不存在"))?;
            let evidence = self
                .store
                .evidence(id)?
                .ok_or_else(|| Error::conflict("缺少持久化执行证据"))?;
            let gate = WriteGate::recover(&self.config, id, &evidence["gate"])?;
            let snapshot: metadata::Snapshot =
                serde_json::from_value(evidence["metadata"].clone())?;
            if metadata::version(&snapshot) != record.plan.metadata_version
                || hash(&evidence["ledger"]) != record.plan.ledger_version
            {
                return Err(Error::conflict("恢复证据与原计划不一致"));
            }
            let pinned = PinnedMove::recover(&self.config, &record.plan)?;
            let compensation: Option<String> = self.store.db.query_row("SELECT evidence FROM steps WHERE operation_id=? AND phase='compensation_intent' ORDER BY revision LIMIT 1",[id],|r|r.get(0)).optional()?;
            let original_cause = compensation
                .map(|raw| -> Result<Error> {
                    let data: Value = serde_json::from_str(&raw)?;
                    Ok(serde_json::from_value(data["cause"].clone())?)
                })
                .transpose()?;
            let cause = original_cause
                .or(record.error)
                .unwrap_or_else(|| Error::new(503, "interrupted", "执行中断后已回退"));
            let rollback = record.status == "rolling_back" || self.store.db.query_row("SELECT EXISTS(SELECT 1 FROM steps WHERE operation_id=? AND phase='compensation_intent')",[id],|r|r.get(0))?;
            if let Err(error) = self.reconcile(&record.plan, &snapshot, &pinned, rollback, &cause) {
                self.reconcile(&record.plan, &snapshot, &pinned, true, &error)?;
            }
            gate.release()
        })();
        if let Err(error) = outcome
            && let Ok(Some(record)) = self.store.get(id)
        {
            let _ = self.recovery_failure(&record.plan, &error);
        }
    }
    fn reconcile(
        &mut self,
        plan: &Plan,
        snapshot: &metadata::Snapshot,
        pinned: &PinnedMove,
        rollback: bool,
        cause: &Error,
    ) -> Result<Option<Value>> {
        // Complete preflight before any recovery write. Unknown/foreign state is never a
        // reason to undo a transaction or unlink a path.
        let ledger_state = ledger::inspect(&self.config, plan)?;
        metadata::inspect_domain(&self.config, plan, snapshot, false)?;
        metadata::inspect_domain(&self.config, plan, snapshot, true)?;
        metadata::inspect_documents(&self.config, snapshot)?;
        let position = pinned.position(&plan.file().version)?;
        if ledger_state == "committed" && position != (false, true) {
            return Err(Error::conflict("账本已提交但文件位置不一致"));
        }
        if (rollback || ledger_state == "cancelled") && ledger_state != "committed" {
            self.store.step(
                &plan.id,
                "rolling_back",
                "compensation_intent",
                json!({"cause":cause}),
            )?;
            crate::fault::checkpoint(&self.config, "compensation_intent")?;
            ledger::cancel(&self.config, plan)?;
            crate::fault::checkpoint(&self.config, "compensation_ledger_fenced")?;
            for doc in snapshot.documents.iter().rev() {
                if doc.before == doc.after {
                    continue;
                }
                crate::fault::checkpoint(
                    &self.config,
                    &format!("compensation_json_restore_intent:{}", doc.name),
                )?;
                metadata::compensate_document(&self.config, doc)?;
                crate::fault::checkpoint(
                    &self.config,
                    &format!("compensation_json_restored:{}", doc.name),
                )?;
            }
            crate::fault::checkpoint(&self.config, "compensation_documents_restored")?;
            metadata::compensate_domain(&self.config, plan, snapshot, true)?;
            crate::fault::checkpoint(&self.config, "compensation_jobs_restored")?;
            metadata::compensate_domain(&self.config, plan, snapshot, false)?;
            crate::fault::checkpoint(&self.config, "compensation_faces_restored")?;
            pinned.compensate(&self.config, &plan.file().version)?;
            crate::fault::checkpoint(&self.config, "compensation_file_restored")?;
            self.store.rolled_back(&plan.id, cause)?;
            Ok(None)
        } else {
            self.execute(plan, snapshot, pinned).map(Some)
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
        if ledger::inspect(&self.config, plan)? == "absent" {
            ledger::reserve(&self.config, plan)?;
        }
        self.checkpoint(id, "ledger_reserved", json!({"state":"prepared"}))?;
        self.checkpoint(id,"destination_publish_intent",json!({"src":plan.file().src,"dst":plan.file().dst,"identity":files::identity(&pinned.stat()?)}))?;
        if !pinned.position(&plan.file().version)?.1 {
            pinned.publish(&plan.file().version)?;
        }
        crate::fault::checkpoint(&self.config, "destination_published_unrecorded")?;
        self.checkpoint(
            id,
            "destination_published",
            files::identity(&pinned.stat()?),
        )?;
        self.checkpoint(id, "source_remove_intent", json!({"src":plan.file().src}))?;
        if pinned.position(&plan.file().version)?.0 {
            pinned.remove_source(&plan.file().version)?;
        }
        crate::fault::checkpoint(&self.config, "source_removed_unrecorded")?;
        self.checkpoint(id, "source_removed", files::identity(&pinned.stat()?))?;
        self.checkpoint(id, "faces_commit_intent", json!({"before":snapshot.videos}))?;
        let evidence = if metadata::inspect_domain(&self.config, plan, snapshot, false)? {
            json!({"reconciled":true})
        } else {
            metadata::apply_faces(&self.config, plan, snapshot)?
        };
        crate::fault::checkpoint(&self.config, "faces_committed_unrecorded")?;
        self.checkpoint(id, "faces_committed", evidence)?;
        self.checkpoint(id, "jobs_commit_intent", json!({"before":snapshot.jobs}))?;
        let evidence = if metadata::inspect_domain(&self.config, plan, snapshot, true)? {
            json!({"reconciled":true})
        } else {
            metadata::apply_jobs(&self.config, plan, snapshot)?
        };
        crate::fault::checkpoint(&self.config, "jobs_committed_unrecorded")?;
        self.checkpoint(id, "jobs_committed", evidence)?;
        for doc in &snapshot.documents {
            if doc.before == doc.after {
                continue;
            }
            self.checkpoint(id, "json_replace_intent", serde_json::to_value(doc)?)?;
            crate::fault::checkpoint(&self.config, &format!("json_replace_intent:{}", doc.name))?;
            if files::read_optional(&self.config.base.join(&doc.name))? != doc.after {
                metadata::apply_document(&self.config, doc)?;
            }
            crate::fault::checkpoint(
                &self.config,
                &format!("json_replaced_unrecorded:{}", doc.name),
            )?;
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
        let evidence = if ledger::inspect(&self.config, plan)? == "committed" {
            json!({"reconciled":true})
        } else {
            ledger::apply(&self.config, plan)?
        };
        crate::fault::checkpoint(&self.config, "ledger_committed_unrecorded")?;
        self.checkpoint(id, "ledger_committed", evidence)?;
        let result = model::final_result(plan);
        self.checkpoint(id, "final_receipt_intent", json!({"result":result}))?;
        self.store.complete(plan, &result)?;
        crate::fault::checkpoint(&self.config, "final_receipt_unreleased")?;
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
