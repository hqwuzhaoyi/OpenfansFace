use crate::{
    config::Config,
    files::{self, PinnedMove, WriteGate},
    ledger, metadata,
    model::{self, Error, FailurePolicy, FileMove, Plan, Request, Result, hash},
    native,
    native::DomainSnapshot,
    store::Store,
};
use rusqlite::OptionalExtension;
use serde_json::{Value, json};
use std::{path::Path, sync::Arc};

pub struct Engine {
    pub config: Config,
    pub store: Store,
    pub auto_archive_wake: Option<Arc<crate::auto_archive::AutoArchiveWake>>,
}
impl Engine {
    pub fn new(config: Config) -> Result<Self> {
        let store = Store::start(&config)?;
        let mut engine = Self {
            config,
            store,
            auto_archive_wake: None,
        };
        if engine.config.parity_root.is_none() {
            engine.recover_startup()?;
            crate::person_batch::recover(&mut engine)?;
        }
        Ok(engine)
    }
    pub fn open_preview(config: Config) -> Result<Self> {
        let store = Store::open_preview(&config)?;
        Ok(Self {
            config,
            store,
            auto_archive_wake: None,
        })
    }
    pub fn set_auto_archive_wake(&mut self, wake: Arc<crate::auto_archive::AutoArchiveWake>) {
        self.auto_archive_wake = Some(wake);
    }
    pub fn notify_auto_archive(&self, vid: &str) {
        if let Some(wake) = &self.auto_archive_wake {
            wake.notify_vid(vid);
        }
    }
    pub fn preview(&mut self, request: Request) -> Result<Value> {
        crate::person_batch::reject_child(&self.store, &request.id)?;
        validate(&request)?;
        files::claim_owner_for(&self.config, &request.id, &request.action)?;
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
            if record.status == "not_started" && record.plan.execution.is_none() {
                files::PinnedMove::new(
                    &self.config,
                    &record.plan.person,
                    &record.plan.file().src,
                    &record.plan.file().dst,
                )?;
            }
            return Ok(serde_json::to_value(record.plan)?);
        }
        let plan = self.plan(request)?;
        self.store.preview(&plan)?;
        Ok(serde_json::to_value(plan)?)
    }
    fn plan(&self, request: Request) -> Result<Plan> {
        if crate::staging_delete::action(&request.action) {
            return crate::staging_delete::plan(&self.config, request);
        }
        if request.action != "rename-file" {
            let mut plan = native::plan(&self.config, request)?;
            seal_plan(&self.config, &mut plan)?;
            return Ok(plan);
        }
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
        if ids.is_empty() {
            return native::plan(&self.config, request);
        }
        if !ids.contains(&item.vid) {
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
            execution: None,
        };
        plan.metadata_version = metadata::version(&metadata::snapshot(&self.config, &plan)?);
        plan.ledger_version = ledger::snapshot(&self.config, &plan)
            .map(|v| hash(&v))
            .unwrap_or_default();
        plan.version = hash(&plan);
        Ok(plan)
    }
    pub fn apply(&mut self, request: Request, version: String) -> Result<Value> {
        self.config.require_writer()?;
        crate::person_batch::reject_child(&self.store, &request.id)?;
        validate(&request)?;
        files::claim_owner_for(&self.config, &request.id, &request.action)?;
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
        self.apply_under_gate(record.plan, version, gate)
    }
    /// The batch coordinator owns this exact child gate, never a parent gate.
    pub(crate) fn apply_under_gate(
        &mut self,
        stored: Plan,
        version: String,
        gate: WriteGate,
    ) -> Result<Value> {
        let request = stored.request.clone();
        self.store.remember_gate(&request.id, &gate.evidence()?)?;
        crate::fault::checkpoint(&self.config, "gate_acquired")?;
        if stored.action == "processing-result" {
            return crate::processing_result::apply(self, &stored, gate);
        }
        if crate::staging_delete::action(&stored.action) {
            return crate::staging_delete::apply(self, &stored, gate);
        }
        let prepared = (|| {
            let current = if stored.execution.is_some() {
                validate_plan(&self.config, &stored)?;
                native::revalidate(&self.config, &stored)?;
                stored.clone()
            } else {
                let mut current = self.plan(request.clone())?;
                if current.metadata_version == stored.metadata_version
                    && current.files == stored.files
                {
                    current.ledger_version = stored.ledger_version.clone();
                    current.version = String::new();
                    current.version = hash(&current);
                }
                current
            };
            if current.version != version {
                return Err(Error::conflict("文件或相关记录已变化，请重新预览"));
            }
            let domain = operation_snapshot(
                &self.config,
                &current,
                &(0..current.files.len()).collect::<Vec<_>>(),
            )?;
            if metadata::version(&domain.metadata) != current.metadata_version {
                return Err(Error::conflict("预检期间元数据发生变化"));
            }
            let mut identities = vec![];
            for (index, file) in current.files.iter().enumerate() {
                let stat = if let Some(execution) = &current.execution {
                    files::check_move(&self.config, &execution.scopes[index], file)?
                } else {
                    PinnedMove::new(&self.config, &current.person, &file.src, &file.dst)?.stat()?
                };
                identities.push(json!({"file_index":index,"src":file.src,"dst":file.dst,"source_identity":files::identity(&stat),"move_evidence":files::MoveEvidence{source_parent:files::parent_identity(&file.src)?,destination_parent:if Path::new(&file.dst).parent().unwrap().exists(){files::parent_identity(&file.dst)?}else{Value::Null},copy:None,restored:None}}));
            }
            let evidence = if current.execution.is_some() {
                json!({"schema":2,"domain":domain,"files":identities,"gate":gate.evidence()?})
            } else {
                json!({"metadata":domain.metadata,"ledger":ledger::snapshot(&self.config,&current).unwrap_or(Value::Null),"source_identity":identities[0]["source_identity"],"gate":gate.evidence()?})
            };
            Ok((current, domain, evidence))
        })();
        let (plan, domain, evidence) = match prepared {
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
        let executed = self.execute(&plan, &domain);
        match executed {
            Ok(result) => {
                crate::archive_cancel::release_scanner_lease(&self.config, &plan.id, "committed");
                gate.release()?;
                crate::fault::checkpoint(&self.config, "complete")?;
                Ok(result)
            }
            Err(error) => {
                // A failed call is not a failed transaction. Inspect all domains before deciding.
                match self.reconcile(&plan, &domain, true, &error) {
                    Ok(Some(result)) => {
                        crate::archive_cancel::release_scanner_lease(
                            &self.config,
                            &plan.id,
                            "committed",
                        );
                        gate.release()?;
                        Ok(result)
                    }
                    Ok(None) => {
                        crate::archive_cancel::release_scanner_lease(
                            &self.config,
                            &plan.id,
                            "rolled_back",
                        );
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
        crate::archive_cancel::release_scanner_lease(&self.config, &plan.id, "recovery_needed");
        if error.code == "recovery_required" && error.mutation_outcome.is_some() {
            return self.store.fail(&plan.id, error, true);
        }
        let failure = Error::new(
            503,
            "recovery_required",
            format!("操作 {} 需要核对恢复，已暂停写入：{}", plan.id, error.error),
        )
        .at(error.path.clone().unwrap_or_else(|| {
            plan.files
                .first()
                .map(|f| f.dst.clone())
                .unwrap_or_default()
        }))
        .during(error.step.clone().unwrap_or_else(|| "execution".into()));
        let failure = if error.mutation_outcome.is_some() {
            failure.ambiguous_mutation()
        } else {
            failure
        };
        self.store.fail(&plan.id, &failure, true)
    }
    /// Explicit reconciliation fences an unreceived/unstarted request before any late POST.
    /// Already-admitted I/O is never reported cancelled merely because a waiter disappeared.
    pub fn reconcile_request(
        &mut self,
        request: Request,
        version: Option<String>,
    ) -> Result<Value> {
        self.config.require_writer()?;
        crate::person_batch::reject_child(&self.store, &request.id)?;
        validate(&request)?;
        files::claim_owner_for(&self.config, &request.id, &request.action)?;
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
        let active_ids: std::collections::BTreeSet<String> =
            self.store.recovery_ids()?.into_iter().collect();
        crate::archive_cancel::recover_scanner_leases(&self.config, &active_ids);
        for id in self.store.recovery_ids()? {
            self.recover_one(&id);
        }
        // Final receipt may be durable while marker removal was interrupted. Only this
        // authority's exact admitted gate may be removed. Legacy/foreign markers survive.
        let gate_path = self.config.base.join(".review-write.lock");
        if let Ok(Some(raw)) = crate::control::read(&gate_path, crate::control::MARKER_LIMIT)
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
    pub(crate) fn recover_one(&mut self, id: &str) {
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
            if record.plan.action == "processing-result" {
                return crate::processing_result::recover(self, &record.plan, gate);
            }
            if crate::staging_delete::action(&record.plan.action) {
                return crate::staging_delete::recover(self, &record.plan, gate);
            }
            let domain = if record.plan.execution.is_some() {
                if evidence["schema"] != 2 {
                    return Err(Error::conflict("未知执行证据版本"));
                }
                serde_json::from_value::<DomainSnapshot>(evidence["domain"].clone())?
            } else {
                DomainSnapshot {
                    metadata: serde_json::from_value(evidence["metadata"].clone())?,
                    ledger: None,
                }
            };
            if metadata::version(&domain.metadata) != record.plan.metadata_version {
                return Err(Error::conflict("恢复证据与原计划不一致"));
            }
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
            if let Err(error) = self.reconcile(&record.plan, &domain, rollback, &cause) {
                eprintln!(
                    "operation {} recovery attempt failed (rollback={}): {} {}",
                    id,
                    rollback,
                    error.code,
                    error.error
                );
                self.reconcile(&record.plan, &domain, true, &error)
                    .inspect_err(|rollback_error| {
                        eprintln!(
                            "operation {} recovery rollback failed: {} {}",
                            id, rollback_error.code, rollback_error.error
                        )
                    })?;
            }
            gate.release()
        })();
        if let Err(error) = outcome
            && let Ok(Some(record)) = self.store.get(id)
        {
            let _ = self.recovery_failure(&record.plan, &error);
        }
    }
    pub(crate) fn require_resolved_mutation(&self, id: &str, cause: Option<&Error>) -> Result<()> {
        let phase: Option<String> = self.store.db.query_row(
            "SELECT phase FROM steps WHERE operation_id=? AND phase IN ('filesystem_mutation_intent','filesystem_mutation_resolved') ORDER BY revision DESC LIMIT 1",
            [id], |r| r.get(0)).optional()?;
        let stored = self.store.get(id)?.and_then(|record| record.error);
        if cause.is_some_and(|error| error.mutation_outcome.is_some())
            || phase.as_deref() == Some("filesystem_mutation_intent")
            || stored
                .as_ref()
                .is_some_and(|error| error.mutation_outcome.is_some())
        {
            let cause = cause
                .filter(|error| error.mutation_outcome.is_some())
                .or(stored.as_ref())
                .or(cause);
            if let Some(error) = cause.filter(|error| {
                error.code == "recovery_required" && error.mutation_outcome.is_some()
            }) {
                return Err(error.clone());
            }
            return Err(Error::new(503, "recovery_required",
                format!("文件系统变更结果不确定，可能仍有远端延迟写入；保留写锁，需人工确认存储端静止后恢复，路径观察或超时不能解除保护：{}", cause.map(|e| e.error.as_str()).unwrap_or("缺少系统调用结果回执")))
                .at(cause.and_then(|e| e.path.clone()).unwrap_or_default())
                .during(cause.and_then(|e| e.step.clone()).unwrap_or_else(|| "filesystem_mutation".into()))
                .ambiguous_mutation());
        }
        Ok(())
    }
    fn reconcile(
        &mut self,
        plan: &Plan,
        accepted: &DomainSnapshot,
        rollback: bool,
        cause: &Error,
    ) -> Result<Option<Value>> {
        self.require_resolved_mutation(&plan.id, Some(cause))?;
        let (domain, selected) = self.commit_scope(plan, accepted)?;
        let snapshot = &domain.metadata;
        let has_retained_step = self
            .store
            .latest_step(&plan.id, "archive_external_indexes_retained")?
            .is_some();
        let has_ledger_reserved = self
            .store
            .latest_step(&plan.id, "ledger_reserved")?
            .is_some();
        let allow_unreserved = !has_ledger_reserved
            && match self.verify_published_archive_destinations(plan, &selected) {
                Ok(()) => true,
                Err(published) => {
                    if crate::archive_cancel::supports_scanner_reconciliation(plan) {
                        eprintln!(
                            "operation {} archive destination verification failed: {} ({})",
                            plan.id,
                            published.error,
                            published.path.as_deref().unwrap_or("")
                        );
                    }
                    false
                }
            };
        let mut retained_indexes = vec![];
        let ledger_committed = self
            .store
            .latest_step(&plan.id, "ledger_committed")?
            .is_some();
        let ledger_state = if ledger_committed {
            "committed".to_owned()
        } else {
            match self.ledger_state(plan, &domain) {
                Ok(state) => state,
                Err(_)
                    if crate::archive_cancel::supports_scanner_reconciliation(plan)
                        && domain.ledger.is_some() =>
                {
                    if let Ok(rows) = crate::archive_cancel::inspect(
                        &self.config,
                        plan,
                        domain.ledger.as_ref().unwrap(),
                        allow_unreserved,
                    ) {
                        retained_indexes = rows;
                        "prepared".into()
                    } else {
                        "absent".into()
                    }
                }
                Err(_) => "absent".into(),
            }
        };
        if let Some(saved) = self
            .store
            .latest_step(&plan.id, "archive_external_indexes_retained")?
        {
            if !crate::archive_cancel::supports_scanner_reconciliation(plan) {
                return Err(Error::conflict("取消索引依据不属于归档操作"));
            }
            if retained_indexes.is_empty()
                && let Some(delta) = domain.ledger.as_ref()
                && let Ok(rows) =
                    crate::archive_cancel::inspect(&self.config, plan, delta, allow_unreserved)
            {
                retained_indexes = rows;
            }
            let _ = saved;
        }
        if writes_faces(plan) {
            metadata::inspect_domain(&self.config, plan, snapshot, false)?;
        }
        if writes_jobs(plan) {
            metadata::inspect_domain(&self.config, plan, snapshot, true)?;
        }
        metadata::inspect_documents(&self.config, snapshot)?;
        for path in &plan.directories {
            if let Some(proof) = self.store.directory_evidence(&plan.id, path)? {
                files::inspect_owned_directory(&proof)?;
            }
        }
        let failures = self.store.failures(&plan.id)?;
        // A crash after the durable pre-effect failure must not turn an all-or-
        // nothing move into an archive-style partial commit on startup.
        let failed_rollback = plan
            .execution
            .as_ref()
            .is_some_and(|e| e.failure_policy == FailurePolicy::Rollback)
            && !failures.is_empty();
        let persisted_cause = if failed_rollback {
            failures
                .first()
                .map(|f| serde_json::from_value::<Error>(f["error"].clone()))
                .transpose()?
        } else {
            None
        };
        let cause = persisted_cause.as_ref().unwrap_or(cause);
        let rollback = rollback || failed_rollback || has_retained_step;
        let external_cause = Error::new(
            409,
            "archive_external_index_retained",
            "发现外部新增目标索引；仅回退本次文件与队列变更，外部索引原样保留，请核对冲突后刷新预览",
        );
        let cause = if retained_indexes.is_empty() {
            cause
        } else {
            &external_cause
        };
        let directory_cleanup = self
            .store
            .latest_step(&plan.id, "source_directory_cleanup_intent")?
            .is_some();
        if directory_cleanup {
            if ledger_state != "committed" {
                return Err(Error::conflict("来源目录清理缺少已提交账本"));
            }
            for (index, file) in plan.files.iter().enumerate() {
                files::inspect_committed_destination(
                    file,
                    &self.store.file_evidence(&plan.id, index)?,
                )?;
            }
        }
        for (index, file) in plan.files.iter().enumerate() {
            if directory_cleanup {
                continue;
            }
            if failures.iter().any(|f| f["file_index"] == index) {
                continue;
            }
            // Before any file journal boundary the original source can be checked without
            // creating a missing directory. Never infer ownership from path existence.
            if let Some(execution) = &plan.execution
                && !Path::new(&file.dst).parent().unwrap().exists()
            {
                if ledger_state == "committed" {
                    return Err(Error::conflict("账本已提交但目标目录不存在"));
                }
                files::check_move(&self.config, &execution.scopes[index], file)?;
                continue;
            }
            let pinned = self.pin(plan, index)?;
            let position = pinned.position(&file.version)?;
            if ledger_state == "committed" && selected.contains(&index) && position != (false, true)
            {
                return Err(Error::conflict("账本已提交但文件位置不一致"));
            }
        }
        if (rollback || ledger_state == "cancelled") && ledger_state != "committed" {
            self.store.step(
                &plan.id,
                "rolling_back",
                "compensation_intent",
                json!({"cause":cause}),
            )?;
            crate::fault::checkpoint(&self.config, "compensation_intent")?;
            if retained_indexes.is_empty() {
                let _ = self.ledger_cancel(plan, &domain);
            } else {
                self.store.step(
                    &plan.id,
                    "rolling_back",
                    "archive_external_indexes_retained",
                    json!({"rows":retained_indexes}),
                )?;
                if let Some(delta) = domain.ledger.as_ref() {
                    let _ = crate::archive_cancel::cancel(
                        &self.config,
                        plan,
                        delta,
                        &retained_indexes,
                        allow_unreserved,
                    );
                }
            }
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
            if writes_jobs(plan) {
                metadata::compensate_domain(&self.config, plan, snapshot, true)?;
            }
            crate::fault::checkpoint(&self.config, "compensation_jobs_restored")?;
            if writes_faces(plan) {
                metadata::compensate_domain(&self.config, plan, snapshot, false)?;
            }
            crate::fault::checkpoint(&self.config, "compensation_faces_restored")?;
            for (index, file) in plan.files.iter().enumerate().rev() {
                if failures.iter().any(|f| f["file_index"] == index) {
                    continue;
                }
                if plan.execution.is_some() && !Path::new(&file.dst).parent().unwrap().exists() {
                    continue;
                }
                let mut pinned = self.pin(plan, index)?;
                if plan.execution.is_some() {
                    let config = self.config.clone();
                    pinned.compensate_recoverable(
                        &config,
                        &file.version,
                        &mut |phase, evidence| self.file_checkpoint(plan, index, phase, evidence),
                    )?;
                } else {
                    let config = self.config.clone();
                    pinned.compensate_recoverable(
                        &config,
                        &file.version,
                        &mut |phase, proof| self.file_checkpoint(plan, index, phase, proof),
                    )?;
                }
                crate::fault::checkpoint(&self.config, "compensation_file_restored")?;
            }
            for path in plan.directories.iter().rev() {
                if let Some(proof) = self.store.directory_evidence(&plan.id, path)? {
                    files::remove_owned_directory(&proof, &mut |phase, proof| {
                        self.checkpoint(&plan.id, phase, json!({"directory":proof}))
                    })?;
                }
            }
            self.store.rolled_back(&plan.id, cause)?;
            crate::archive_cancel::release_scanner_lease(&self.config, &plan.id, "rolled_back");
            Ok(None)
        } else {
            let result = self.execute(plan, &domain)?;
            crate::archive_cancel::release_scanner_lease(&self.config, &plan.id, "committed");
            Ok(Some(result))
        }
    }
    fn verify_published_archive_destinations(
        &self,
        plan: &Plan,
        successful: &[usize],
    ) -> Result<()> {
        if !crate::archive_cancel::supports_scanner_reconciliation(plan)
            || (successful.is_empty() && !plan.files.is_empty())
        {
            return Err(Error::conflict("缺少已发布的归档目标文件依据"));
        }
        for &index in successful {
            let file = plan
                .files
                .get(index)
                .ok_or_else(|| Error::conflict("归档文件索引越界"))?;
            let pinned = self.pin(plan, index)?;
            if pinned.position(&file.version)? != (false, true) {
                return Err(Error::conflict("归档目标文件位置与已发布记录不一致").at(&file.dst));
            }
            let evidence = self.store.file_evidence(&plan.id, index)?;
            files::inspect_committed_destination(file, &evidence)?;
        }
        Ok(())
    }
    fn pin(&self, plan: &Plan, index: usize) -> Result<PinnedMove> {
        if let Some(execution) = &plan.execution {
            files::recover_move(
                &self.config,
                &execution.scopes[index],
                &plan.files[index],
                self.store.file_evidence(&plan.id, index)?,
            )
        } else {
            PinnedMove::recover(&self.config, plan)
        }
    }
    fn file_checkpoint(
        &mut self,
        plan: &Plan,
        index: usize,
        phase: &str,
        evidence: &files::MoveEvidence,
    ) -> Result<()> {
        let file = &plan.files[index];
        self.checkpoint(
            &plan.id,
            phase,
            json!({"file_index":index,"src":file.src,"dst":file.dst,"move_evidence":evidence}),
        )?;
        crate::fault::checkpoint(&self.config, &format!("{phase}:{index}"))
    }
    fn execute_files_concurrent(&mut self, plan: &Plan, failures: &mut Vec<Value>) -> Result<()> {
        enum Msg {
            Checkpoint {
                phase: String,
                evidence: Value,
                ack: std::sync::mpsc::SyncSender<Result<()>>,
            },
            FileCheckpoint {
                index: usize,
                phase: String,
                evidence: files::MoveEvidence,
                ack: std::sync::mpsc::SyncSender<Result<()>>,
            },
            PreEffectFailure {
                failure: Value,
                ack: std::sync::mpsc::SyncSender<Result<()>>,
            },
            Done(Result<()>),
        }
        let id = &plan.id;
        let mut pending = Vec::new();
        for (index, file) in plan.files.iter().enumerate() {
            if failures.iter().any(|f| f["file_index"] == index) {
                continue;
            }
            let started = self
                .store
                .latest_file_step(id, index)?
                .unwrap_or(Value::Null);
            let initial_evidence = self.store.file_evidence(id, index)?;
            let is_complete_step = self
                .store
                .db
                .query_row(
                    "SELECT phase FROM steps WHERE operation_id=?1 AND json_extract(evidence,'$.file_index')=?2 ORDER BY revision DESC LIMIT 1",
                    rusqlite::params![id, index as i64],
                    |r| r.get::<_, String>(0),
                )
                .optional()?
                .as_deref()
                == Some("source_removed");
            if is_complete_step
                && let Some(execution) = &plan.execution
                && let Ok(pinned) = files::recover_move(
                    &self.config,
                    &execution.scopes[index],
                    file,
                    initial_evidence.clone(),
                )
                && pinned.position(&file.version).ok() == Some((false, true))
                && crate::archive::verify_duplicate(&self.config, plan, index, false).is_ok()
            {
                continue;
            }
            pending.push((index, started, initial_evidence));
        }
        if pending.is_empty() {
            return Ok(());
        }
        let owned_rt = if tokio::runtime::Handle::try_current().is_err() {
            Some(
                tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(4)
                    .enable_all()
                    .build()
                    .map_err(|e| {
                        Error::new(
                            503,
                            "internal_error",
                            format!("Tokio 运行时初始化失败：{e}"),
                        )
                    })?,
            )
        } else {
            None
        };
        let handle = match &owned_rt {
            Some(rt) => rt.handle().clone(),
            None => tokio::runtime::Handle::current(),
        };
        let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(4));
        let (tx, rx) = std::sync::mpsc::channel::<Msg>();
        let plan_arc = std::sync::Arc::new(plan.clone());
        let aborted = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        for (index, started, initial_evidence) in pending {
            let sem = semaphore.clone();
            let tx = tx.clone();
            let config = self.config.clone();
            let plan = plan_arc.clone();
            let aborted = aborted.clone();
            handle.spawn(async move {
                let _permit = sem.acquire_owned().await.ok();
                if aborted.load(std::sync::atomic::Ordering::Relaxed) {
                    let _ = tx.send(Msg::Done(Ok(())));
                    return;
                }
                let tx_inner = tx.clone();
                let res = tokio::task::spawn_blocking(move || -> Result<()> {
                    let send_checkpoint = |phase: &str, evidence: Value| -> Result<()> {
                        let (ack_tx, ack_rx) = std::sync::mpsc::sync_channel(1);
                        tx_inner
                            .send(Msg::Checkpoint {
                                phase: phase.to_owned(),
                                evidence,
                                ack: ack_tx,
                            })
                            .map_err(|_| Error::new(503, "internal_error", "检查点通道已关闭"))?;
                        ack_rx
                            .recv()
                            .map_err(|_| Error::new(503, "internal_error", "检查点回执丢失"))?
                    };
                    let send_file_checkpoint =
                        |phase: &str, evidence: &files::MoveEvidence| -> Result<()> {
                            let (ack_tx, ack_rx) = std::sync::mpsc::sync_channel(1);
                            tx_inner
                                .send(Msg::FileCheckpoint {
                                    index,
                                    phase: phase.to_owned(),
                                    evidence: evidence.clone(),
                                    ack: ack_tx,
                                })
                                .map_err(|_| {
                                    Error::new(503, "internal_error", "文件检查点通道已关闭")
                                })?;
                            ack_rx
                                .recv()
                                .map_err(|_| Error::new(503, "internal_error", "文件检查点回执丢失"))?
                        };
                    let file = &plan.files[index];
                    let execution = plan
                        .execution
                        .as_ref()
                        .ok_or_else(|| Error::conflict("缺少并发执行范围"))?;
                    let pinned = if started.is_null() {
                        if plan.action == "archive"
                            && execution.data["rows"][index]["operation"] == "stage-duplicate"
                        {
                            send_checkpoint(
                                "duplicate_verification_intent",
                                json!({"index":index,"src":file.src}),
                            )?;
                        }
                        crate::archive::verify_duplicate(&config, &plan, index, true)
                            .and_then(|_| {
                                files::prepare_move(&config, &execution.scopes[index], file)
                            })
                            .and_then(|_| {
                                files::recover_move(
                                    &config,
                                    &execution.scopes[index],
                                    file,
                                    initial_evidence.clone(),
                                )
                            })
                    } else {
                        files::recover_move(
                            &config,
                            &execution.scopes[index],
                            file,
                            initial_evidence.clone(),
                        )
                    };
                    let mut pinned = match pinned {
                        Ok(p) => p,
                        Err(error) => {
                            if started.is_null() {
                                let failure = json!({"file_index":index,"src":file.src,"dst":file.dst,"error":error,"unchanged":true});
                                let (ack_tx, ack_rx) = std::sync::mpsc::sync_channel(1);
                                tx_inner
                                    .send(Msg::PreEffectFailure {
                                        failure,
                                        ack: ack_tx,
                                    })
                                    .map_err(|_| {
                                        Error::new(503, "internal_error", "失败记录通道已关闭")
                                    })?;
                                ack_rx
                                    .recv()
                                    .map_err(|_| {
                                        Error::new(503, "internal_error", "失败记录回执丢失")
                                    })??;
                                if execution.failure_policy == FailurePolicy::ContinueUnchanged {
                                    return Ok(());
                                }
                            }
                            return Err(error);
                        }
                    };
                    send_file_checkpoint("destination_publish_intent", &pinned.evidence)?;
                    if !pinned.position(&file.version)?.1 {
                        pinned.publish_recoverable(
                            &config,
                            &file.version,
                            &mut |phase, proof| send_file_checkpoint(phase, proof),
                        )?;
                    }
                    send_file_checkpoint("destination_published", &pinned.evidence)?;
                    crate::archive::verify_duplicate(&config, &plan, index, false)?;
                    send_file_checkpoint("source_remove_intent", &pinned.evidence)?;
                    if pinned.position(&file.version)?.0 {
                        pinned.remove_source_recorded(&file.version, &mut |phase, proof| {
                            send_file_checkpoint(phase, proof)
                        })?;
                    }
                    send_file_checkpoint("source_removed", &pinned.evidence)?;
                    Ok(())
                })
                .await
                .unwrap_or_else(|_| Err(Error::new(503, "internal_error", "并发文件任务中断")));
                let _ = tx.send(Msg::Done(res));
            });
        }
        drop(tx);
        let mut first_err: Option<Error> = None;
        while let Ok(msg) = rx.recv() {
            match msg {
                Msg::Checkpoint {
                    phase,
                    evidence,
                    ack,
                } => {
                    let res = self.checkpoint(&plan.id, &phase, evidence);
                    if let Err(ref e) = res
                        && first_err.is_none()
                    {
                        aborted.store(true, std::sync::atomic::Ordering::Relaxed);
                        first_err = Some(e.clone());
                    }
                    let _ = ack.send(res);
                }
                Msg::FileCheckpoint {
                    index,
                    phase,
                    evidence,
                    ack,
                } => {
                    let res = self.file_checkpoint(plan, index, &phase, &evidence);
                    if let Err(ref e) = res
                        && first_err.is_none()
                    {
                        aborted.store(true, std::sync::atomic::Ordering::Relaxed);
                        first_err = Some(e.clone());
                    }
                    let _ = ack.send(res);
                }
                Msg::PreEffectFailure { failure, ack } => {
                    let res = self.checkpoint(&plan.id, "file_failed", failure.clone());
                    if res.is_ok() {
                        failures.push(failure);
                    } else if let Err(ref e) = res
                        && first_err.is_none()
                    {
                        aborted.store(true, std::sync::atomic::Ordering::Relaxed);
                        first_err = Some(e.clone());
                    }
                    let _ = ack.send(res);
                }
                Msg::Done(Err(e)) => {
                    if first_err.is_none() {
                        aborted.store(true, std::sync::atomic::Ordering::Relaxed);
                        first_err = Some(e);
                    }
                }
                Msg::Done(Ok(())) => {}
            }
        }
        if let Some(err) = first_err {
            return Err(err);
        }
        Ok(())
    }
    fn commit_scope(
        &self,
        plan: &Plan,
        accepted: &DomainSnapshot,
    ) -> Result<(DomainSnapshot, Vec<usize>)> {
        if let Some(scope) = self.store.latest_step(&plan.id, "commit_scope")? {
            let indexes: Vec<usize> =
                serde_json::from_value(scope["successful_file_indexes"].clone())?;
            if indexes.iter().any(|i| *i >= plan.files.len())
                || indexes.windows(2).any(|w| w[0] >= w[1])
            {
                return Err(Error::conflict("持久化提交范围无效"));
            }
            let domain: DomainSnapshot = serde_json::from_value(scope["domain"].clone())?;
            return Ok((domain, indexes));
        }
        Ok((accepted.clone(), (0..plan.files.len()).collect()))
    }
    fn ledger_state(&self, plan: &Plan, domain: &DomainSnapshot) -> Result<String> {
        if let Some(execution) = &plan.execution {
            if !execution.stores.ledger {
                return Ok("absent".into());
            }
            let Some(delta) = domain.ledger.as_ref() else {
                return Err(Error::conflict("缺少账本意图"));
            };
            ledger::inspect_delta(&self.config, plan, delta)
        } else {
            ledger::inspect(&self.config, plan)
        }
    }
    fn ledger_reserve(&self, plan: &Plan, domain: &DomainSnapshot) -> Result<()> {
        if let Some(execution) = &plan.execution {
            if !execution.stores.ledger {
                return Ok(());
            }
            let Some(delta) = domain.ledger.as_ref() else {
                return Err(Error::conflict("缺少账本意图"));
            };
            ledger::reserve_delta(&self.config, plan, delta)
        } else {
            ledger::reserve(&self.config, plan)
        }
    }
    fn ledger_cancel(&self, plan: &Plan, domain: &DomainSnapshot) -> Result<()> {
        if let Some(execution) = &plan.execution {
            if !execution.stores.ledger {
                return Ok(());
            }
            let Some(delta) = domain.ledger.as_ref() else {
                return Ok(());
            };
            ledger::cancel_delta(&self.config, plan, delta)
        } else {
            ledger::cancel(&self.config, plan)
        }
    }
    fn ledger_apply(&self, plan: &Plan, domain: &DomainSnapshot) -> Result<Value> {
        if let Some(execution) = &plan.execution {
            if !execution.stores.ledger {
                return Ok(json!({"skipped":true}));
            }
            let Some(delta) = domain.ledger.as_ref() else {
                return Err(Error::conflict("缺少账本意图"));
            };
            ledger::apply_delta(&self.config, plan, delta)
        } else {
            ledger::apply(&self.config, plan)
        }
    }
    fn checkpoint(&mut self, id: &str, phase: &str, evidence: Value) -> Result<()> {
        self.store.step(id, "running", phase, evidence)?;
        crate::fault::checkpoint(&self.config, phase)
    }
    fn execute(&mut self, plan: &Plan, accepted: &DomainSnapshot) -> Result<Value> {
        let id = &plan.id;
        self.require_resolved_mutation(id, None)?;
        if crate::archive_cancel::supports_scanner_reconciliation(plan) {
            let _ = crate::archive_cancel::acquire_scanner_lease(&self.config, plan);
        }
        if plan.execution.is_none() {
            self.checkpoint(
                id,
                "ledger_reserve_intent",
                json!({"signature":plan.version}),
            )?;
            let reserve_state = match self.ledger_state(plan, accepted) {
                Ok(state) => {
                    if state == "absent"
                        && let Err(err) = self.ledger_reserve(plan, accepted)
                    {
                        json!({"state":"skipped","warning":err.error})
                    } else {
                        json!({"state":"prepared"})
                    }
                }
                Err(err) => json!({"state":"skipped","warning":err.error}),
            };
            self.checkpoint(id, "ledger_reserved", reserve_state)?;
        }
        let already_selected = self.store.latest_step(id, "commit_scope")?.is_some();
        if !already_selected {
            let fixed_subset = self.store.latest_step(id, "successful_file_subset")?;
            if fixed_subset.is_none() {
                for path in &plan.directories {
                    let previous = self.store.directory_evidence(id, path)?;
                    self.checkpoint(id, "directory_create_intent", json!({"path":path}))?;
                    files::create_directory(path, previous, &mut |phase, proof| {
                        self.checkpoint(id, phase, json!({"directory":proof}))
                    })?;
                }
                let mut failures = self.store.failures(id)?;
                let use_tokio_concurrent = self.config.fixture.is_none()
                    && plan.files.len() > 1
                    && plan
                        .execution
                        .as_ref()
                        .is_some_and(|e| e.failure_policy == FailurePolicy::ContinueUnchanged);
                if use_tokio_concurrent {
                    self.execute_files_concurrent(plan, &mut failures)?;
                } else {
                    for (index, file) in plan.files.iter().enumerate() {
                        if failures.iter().any(|f| f["file_index"] == index) {
                            continue;
                        }
                        let started: Value = self
                            .store
                            .latest_file_step(id, index)?
                            .unwrap_or(Value::Null);
                        let pinned = if let Some(execution) = &plan.execution
                            && started.is_null()
                        {
                            if plan.action == "archive"
                                && execution.data["rows"][index]["operation"] == "stage-duplicate"
                            {
                                self.checkpoint(
                                    id,
                                    "duplicate_verification_intent",
                                    json!({"index":index,"src":file.src}),
                                )?;
                            }
                            crate::archive::verify_duplicate(&self.config, plan, index, true)
                                .and_then(|_| {
                                    files::prepare_move(
                                        &self.config,
                                        &execution.scopes[index],
                                        file,
                                    )
                                })
                                .and_then(|_| self.pin(plan, index))
                        } else {
                            self.pin(plan, index)
                        };
                        let mut pinned = match pinned {
                            Ok(p) => p,
                            Err(error) => {
                                if started.is_null()
                                    && let Some(execution) = &plan.execution
                                {
                                    // prepare_move / pin are read-only, and no before-effect
                                    // file intent exists. This proves only this file untouched;
                                    // separately journaled directory effects still need cleanup.
                                    let failure = json!({"file_index":index,"src":file.src,"dst":file.dst,"error":error,"unchanged":true});
                                    self.checkpoint(id, "file_failed", failure.clone())?;
                                    failures.push(failure);
                                    if execution.failure_policy == FailurePolicy::ContinueUnchanged
                                    {
                                        continue;
                                    }
                                }
                                return Err(error);
                            }
                        };
                        self.file_checkpoint(
                            plan,
                            index,
                            "destination_publish_intent",
                            &pinned.evidence,
                        )?;
                        if !pinned.position(&file.version)?.1 {
                            if plan.execution.is_some() {
                                let config = self.config.clone();
                                pinned.publish_recoverable(
                                    &config,
                                    &file.version,
                                    &mut |phase, proof| {
                                        self.file_checkpoint(plan, index, phase, proof)
                                    },
                                )?;
                            } else {
                                pinned.publish_recorded(&file.version, &mut |phase, proof| {
                                    self.file_checkpoint(plan, index, phase, proof)
                                })?;
                            }
                        }
                        crate::fault::checkpoint(&self.config, "destination_published_unrecorded")?;
                        crate::fault::checkpoint(
                            &self.config,
                            &format!("destination_published_unrecorded:{index}"),
                        )?;
                        self.file_checkpoint(
                            plan,
                            index,
                            "destination_published",
                            &pinned.evidence,
                        )?;
                        crate::archive::verify_duplicate(&self.config, plan, index, false)?;
                        self.file_checkpoint(
                            plan,
                            index,
                            "source_remove_intent",
                            &pinned.evidence,
                        )?;
                        if pinned.position(&file.version)?.0 {
                            pinned.remove_source_recorded(&file.version, &mut |phase, proof| {
                                self.file_checkpoint(plan, index, phase, proof)
                            })?;
                        }
                        crate::fault::checkpoint(&self.config, "source_removed_unrecorded")?;
                        crate::fault::checkpoint(
                            &self.config,
                            &format!("source_removed_unrecorded:{index}"),
                        )?;
                        self.file_checkpoint(plan, index, "source_removed", &pinned.evidence)?;
                    }
                }
            }
            if plan.execution.is_some() {
                let failures = self.store.failures(id)?;
                let successful: Vec<usize> = if let Some(fixed) = &fixed_subset {
                    serde_json::from_value(fixed["successful_file_indexes"].clone())?
                } else {
                    (0..plan.files.len())
                        .filter(|index| !failures.iter().any(|f| f["file_index"] == *index))
                        .collect()
                };
                if successful.iter().any(|i| *i >= plan.files.len())
                    || successful.windows(2).any(|w| w[0] >= w[1])
                {
                    return Err(Error::conflict("成功文件子集无效"));
                }
                // Fix subset durably BEFORE constructing domain-specific deltas.
                if fixed_subset.is_none() {
                    self.checkpoint(
                        id,
                        "successful_file_subset",
                        json!({"successful_file_indexes":successful,"failures":failures}),
                    )?;
                }
                let domain = if successful.len() == plan.files.len() {
                    accepted.clone()
                } else {
                    native::snapshot(&self.config, plan, &successful)?
                };
                validate_domain(plan, &domain)?;
                self.checkpoint(
                    id,
                    "commit_scope",
                    json!({"successful_file_indexes":successful,"domain":domain}),
                )?;
            }
        }
        let (domain, successful) = self.commit_scope(plan, accepted)?;
        let snapshot = &domain.metadata;
        for &index in &successful {
            crate::archive::verify_duplicate(&self.config, plan, index, false)?;
        }
        let strict_ledger = matches!(plan.action.as_str(), "auto-archive" | "archive-unarchive");
        if plan.execution.is_some() {
            let has_ledger_reserved = self.store.latest_step(id, "ledger_reserved")?.is_some();
            self.checkpoint(
                id,
                "ledger_reserve_intent",
                json!({"signature":plan.version,"successful_file_indexes":successful}),
            )?;
            let reserve_state = match self.ledger_state(plan, &domain) {
                Ok(state) => {
                    if state == "absent" {
                        match self.ledger_reserve(plan, &domain) {
                            Ok(()) => json!({"state":"prepared"}),
                            Err(err) if strict_ledger => return Err(err),
                            Err(err) => json!({"state":"skipped","warning":err.error}),
                        }
                    } else {
                        json!({"state":state})
                    }
                }
                Err(error) if crate::archive_cancel::supports_scanner_reconciliation(plan) => {
                    let fallback = self
                        .verify_published_archive_destinations(plan, &successful)
                        .and_then(|()| {
                            let delta = domain
                                .ledger
                                .as_ref()
                                .ok_or_else(|| Error::conflict("缺少账本意图"))?;
                            crate::archive_cancel::reserve_with_scanner_index(
                                &self.config,
                                plan,
                                delta,
                                !has_ledger_reserved,
                            )
                        });
                    match fallback {
                        Ok(_) => json!({"state":"prepared"}),
                        Err(_) if strict_ledger => return Err(error),
                        Err(err) => {
                            json!({"state":"skipped","warning":format!("{}; {}", error.error, err.error)})
                        }
                    }
                }
                Err(error) if strict_ledger => return Err(error),
                Err(error) => json!({"state":"skipped","warning":error.error}),
            };
            self.checkpoint(id, "ledger_reserved", reserve_state)?;
        }
        self.checkpoint(id, "faces_commit_intent", json!({"before":snapshot.videos}))?;
        let evidence = if !writes_faces(plan) {
            json!({"skipped":true})
        } else if metadata::inspect_domain(&self.config, plan, snapshot, false)? {
            json!({"reconciled":true})
        } else {
            metadata::apply_faces(&self.config, plan, snapshot)?
        };
        crate::fault::checkpoint(&self.config, "faces_committed_unrecorded")?;
        self.checkpoint(id, "faces_committed", evidence)?;
        self.checkpoint(id, "jobs_commit_intent", json!({"before":snapshot.jobs}))?;
        let evidence = if !writes_jobs(plan) {
            json!({"skipped":true})
        } else if metadata::inspect_domain(&self.config, plan, snapshot, true)? {
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
        let mut reconciled_scanner_assets: Vec<i64> = vec![];
        let mut ledger_warning: Option<String> = None;
        for &index in &successful {
            crate::archive::verify_duplicate(&self.config, plan, index, false)?;
        }
        let primary_ledger = match self.ledger_state(plan, &domain) {
            Ok(state) if state == "committed" => {
                if crate::archive_cancel::supports_scanner_reconciliation(plan) {
                    reconciled_scanner_assets = crate::archive_cancel::committed_reconciled_assets(
                        &self.config,
                        plan,
                    )
                    .unwrap_or_default();
                }
                Ok(json!({"reconciled":true}))
            }
            Ok(_) => match self.ledger_apply(plan, &domain) {
                Ok(applied) => Ok(applied),
                Err(error) if crate::archive_cancel::supports_scanner_reconciliation(plan) => self
                    .apply_archive_with_scanner_reconciliation(plan, &domain, &successful, error)
                    .map(|(applied, reconciled)| {
                        reconciled_scanner_assets = reconciled;
                        applied
                    }),
                Err(error) => Err(error),
            },
            Err(error) if crate::archive_cancel::supports_scanner_reconciliation(plan) => self
                .apply_archive_with_scanner_reconciliation(plan, &domain, &successful, error)
                .map(|(applied, reconciled)| {
                    reconciled_scanner_assets = reconciled;
                    applied
                }),
            Err(error) => Err(error),
        };
        let evidence = match primary_ledger {
            Ok(applied) => applied,
            Err(primary_err) if strict_ledger => return Err(primary_err),
            Err(primary_err) => match self.best_effort_sync_peach(plan, &successful) {
                Ok(fallback_ev) => {
                    if let Some(ids) = fallback_ev["reconciled_scanner_assets"].as_array() {
                        reconciled_scanner_assets =
                            ids.iter().filter_map(Value::as_i64).collect();
                    }
                    fallback_ev
                }
                Err(fallback_err) => {
                    let msg = format!(
                        "Peach 旁路同步已跳过（主文件与本地记录已完成）：{}；{}",
                        primary_err.error, fallback_err.error
                    );
                    eprintln!("operation {} {}", plan.id, msg);
                    ledger_warning = Some(msg.clone());
                    json!({"skipped":true,"warning":msg,"code":fallback_err.code})
                }
            },
        };
        if !reconciled_scanner_assets.is_empty() {
            self.checkpoint(
                id,
                "archive_scanner_indexes_reconciled",
                json!({"asset_ids":reconciled_scanner_assets}),
            )?;
        }
        crate::fault::checkpoint(&self.config, "ledger_committed_unrecorded")?;
        self.checkpoint(id, "ledger_committed", evidence)?;
        let mut directory_warning = None;
        if let Some(inventory) = crate::person_directory::intent(plan)? {
            self.checkpoint(
                id,
                "source_directory_cleanup_intent",
                json!({"directories":inventory.directories}),
            )?;
            for (index, file) in plan.files.iter().enumerate() {
                files::inspect_committed_destination(
                    file,
                    &self.store.file_evidence(&plan.id, index)?,
                )?;
            }
            for directory in inventory.directories.iter().rev() {
                self.checkpoint(
                    id,
                    "source_directory_remove_intent",
                    json!({"directory":directory}),
                )?;
                let config = self.config.clone();
                if let Err(error) =
                    files::remove_source_directory(&config, directory, &mut |phase| {
                        self.checkpoint(id, phase, json!({"directory":directory}))
                    })
                {
                    self.require_resolved_mutation(id, Some(&error))?;
                    self.checkpoint(
                        id,
                        "source_directory_cleanup_warning",
                        json!({"directory":directory,"error":error}),
                    )?;
                    directory_warning = Some("操作已完成，旧目录中还有新文件或空目录待核对");
                    break;
                }
                crate::fault::checkpoint(&self.config, "source_directory_removed_unrecorded")?;
                self.checkpoint(
                    id,
                    "source_directory_removed",
                    json!({"directory":directory}),
                )?;
            }
        }
        let mut result = if plan.execution.is_some() {
            native::result(plan, &successful, &self.store.failures(id)?)
        } else {
            model::final_result(plan)
        };
        if !reconciled_scanner_assets.is_empty() {
            result["reconciled_scanner_assets"] = json!(reconciled_scanner_assets);
        }
        if let Some(warning) = ledger_warning {
            result["ledger_warning"] = json!(warning);
        }
        if let Some(warning) = directory_warning {
            result["warning"] = json!(warning);
        }
        self.checkpoint(id, "final_receipt_intent", json!({"result":result}))?;
        self.store.complete(plan, &result)?;
        crate::archive_cancel::release_scanner_lease(&self.config, &plan.id, "committed");
        crate::fault::checkpoint(&self.config, "final_receipt_unreleased")?;
        Ok(result)
    }

    fn best_effort_sync_peach(&self, plan: &Plan, successful: &[usize]) -> Result<Value> {
        if plan
            .execution
            .as_ref()
            .is_some_and(|e| !e.stores.ledger)
        {
            return Ok(json!({"skipped":true}));
        }
        let mut conn = crate::db::open(&self.config.ledger)?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS _face_admin_person_ops (
                id TEXT PRIMARY KEY,
                signature TEXT NOT NULL,
                state TEXT NOT NULL,
                data TEXT NOT NULL
            )",
        )?;
        if let Some((state, data)) = tx
            .query_row(
                "SELECT state,data FROM _face_admin_person_ops WHERE id=?1",
                [&plan.id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()?
            && state == "committed"
            && let Ok(parsed) = serde_json::from_str::<Value>(&data)
        {
            return Ok(parsed);
        }
        let ensure_creator = |tx: &rusqlite::Connection, name: &str, at: &str| -> Result<Option<i64>> {
            let trimmed = name.trim();
            if trimmed.is_empty() {
                return Ok(None);
            }
            let normalized = trimmed.to_lowercase();
            if let Some(id) = tx
                .query_row(
                    "SELECT id FROM entity WHERE kind='creator' AND (canonical_name=?1 OR normalized_name=?2) ORDER BY id LIMIT 1",
                    rusqlite::params![trimmed, normalized],
                    |r| r.get::<_, i64>(0),
                )
                .optional()?
            {
                return Ok(Some(id));
            }
            let cols = crate::db::rows(tx, "PRAGMA table_info(entity)", [])?;
            let col_names: Vec<&str> = cols.iter().filter_map(|c| c["name"].as_str()).collect();
            if col_names.contains(&"created_at") && col_names.contains(&"updated_at") {
                tx.execute(
                    "INSERT INTO entity(kind,canonical_name,normalized_name,created_at,updated_at) VALUES('creator',?1,?2,?3,?3)",
                    rusqlite::params![trimmed, normalized, at],
                )?;
            } else {
                tx.execute(
                    "INSERT INTO entity(kind,canonical_name,normalized_name) VALUES('creator',?1,?2)",
                    rusqlite::params![trimmed, normalized],
                )?;
            }
            Ok(Some(tx.last_insert_rowid()))
        };
        let upsert_link = |tx: &rusqlite::Connection,
                           asset_id: i64,
                           entity_id: i64,
                           source: &str,
                           confidence: f64,
                           at: &str|
         -> Result<()> {
            let exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM asset_entity WHERE asset_id=?1 AND entity_id=?2 AND role='creator')",
                rusqlite::params![asset_id, entity_id],
                |r| r.get(0),
            )?;
            if exists {
                return Ok(());
            }
            let cols = crate::db::rows(tx, "PRAGMA table_info(asset_entity)", [])?;
            let col_names: Vec<&str> = cols.iter().filter_map(|c| c["name"].as_str()).collect();
            if col_names.contains(&"first_seen_at") && col_names.contains(&"last_seen_at") {
                tx.execute(
                    "INSERT OR IGNORE INTO asset_entity(asset_id,entity_id,role,source,confidence,metadata_json,first_seen_at,last_seen_at) VALUES(?1,?2,'creator',?3,?4,'{}',?5,?5)",
                    rusqlite::params![asset_id, entity_id, source, confidence, at],
                )?;
            } else {
                tx.execute(
                    "INSERT OR IGNORE INTO asset_entity(asset_id,entity_id,role,source) VALUES(?1,?2,'creator',?3)",
                    rusqlite::params![asset_id, entity_id, source],
                )?;
            }
            Ok(())
        };
        let mut reconciled_stubs: Vec<i64> = vec![];
        let mut clean_target = |tx: &rusqlite::Connection, win_dst: &str, keep_id: Option<i64>| -> Result<()> {
            let existing = crate::db::rows(
                tx,
                "SELECT id FROM asset WHERE path=?1 ORDER BY id",
                [win_dst],
            )?;
            for row in existing {
                let stub_id = crate::db::integer(&row["id"])?;
                if Some(stub_id) == keep_id {
                    continue;
                }
                let _ = tx.execute("DELETE FROM asset_entity WHERE asset_id=?1", [stub_id]);
                if tx.execute("DELETE FROM asset WHERE id=?1", [stub_id]).is_ok() {
                    reconciled_stubs.push(stub_id);
                }
            }
            Ok(())
        };
        let mut updated_assets = 0usize;
        match plan.action.as_str() {
            "archive" => {
                let data = &plan
                    .execution
                    .as_ref()
                    .ok_or_else(|| Error::conflict("归档意图缺失"))?
                    .data;
                let at = data["at"].as_str().unwrap_or("1970-01-01T00:00:00Z");
                for &index in successful {
                    let file = &plan.files[index];
                    let row = &data["rows"][index];
                    let creator = row["creator"].as_str().unwrap_or("");
                    let eid = ensure_creator(&tx, creator, at)?;
                    let win_src = metadata::win(&self.config, &file.src);
                    let win_dst = metadata::win(&self.config, &file.dst);
                    let dst_name = Path::new(&file.dst)
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("");
                    let src_id: Option<i64> = tx
                        .query_row(
                            "SELECT id FROM asset WHERE path=?1 ORDER BY CASE WHEN disposal IS NULL THEN 0 ELSE 1 END, id LIMIT 1",
                            [&win_src],
                            |r| r.get(0),
                        )
                        .optional()?;
                    if let Some(aid) = src_id {
                        clean_target(&tx, &win_dst, Some(aid))?;
                        tx.execute(
                            "UPDATE asset SET path=?1, name=CASE WHEN ?2='' THEN name ELSE ?2 END WHERE id=?3",
                            rusqlite::params![win_dst, dst_name, aid],
                        )?;
                        updated_assets += 1;
                        if let Some(eid) = eid {
                            upsert_link(&tx, aid, eid, "face-curation-8919", 0.95, at)?;
                        }
                    } else if let Some(dst_id) = tx
                        .query_row(
                            "SELECT id FROM asset WHERE path=?1 ORDER BY id LIMIT 1",
                            [&win_dst],
                            |r| r.get::<_, i64>(0),
                        )
                        .optional()?
                        && let Some(eid) = eid
                    {
                        upsert_link(&tx, dst_id, eid, "face-curation-8919", 0.95, at)?;
                    }
                    if row["operation"] == "stage-duplicate"
                        && let Some(canon_path) = row["dst"].as_str()
                        && let Some(eid) = eid
                    {
                        let win_canon = metadata::win(&self.config, canon_path);
                        if let Some(canon_id) = tx
                            .query_row(
                                "SELECT id FROM asset WHERE path=?1 ORDER BY id LIMIT 1",
                                [&win_canon],
                                |r| r.get::<_, i64>(0),
                            )
                            .optional()?
                        {
                            upsert_link(&tx, canon_id, eid, "face-curation-8919", 0.95, at)?;
                        }
                    }
                }
            }
            "rename-person" | "delete-person" => {
                let deleting = plan.action == "delete-person";
                let (source_root, target_root) = crate::person_directory::roots(&self.config, plan);
                let old_prefix = format!(
                    "{}\\",
                    metadata::win(&self.config, &source_root.display().to_string())
                );
                let new_prefix = format!(
                    "{}\\",
                    metadata::win(&self.config, &target_root.display().to_string())
                );
                let rows = crate::db::rows(
                    &tx,
                    "SELECT id, path FROM asset WHERE substr(path,1,length(?1))=?1 ORDER BY id",
                    [&old_prefix],
                )?;
                for row in rows {
                    let aid = crate::db::integer(&row["id"])?;
                    let old_path = row["path"].as_str().unwrap_or("");
                    let new_path = format!("{}{}", new_prefix, &old_path[old_prefix.len()..]);
                    clean_target(&tx, &new_path, Some(aid))?;
                    tx.execute(
                        "UPDATE asset SET path=?1 WHERE id=?2",
                        rusqlite::params![new_path, aid],
                    )?;
                    updated_assets += 1;
                }
                if !deleting && !plan.target_person.is_empty() {
                    let target_exists: bool = tx.query_row(
                        "SELECT EXISTS(SELECT 1 FROM entity WHERE kind='creator' AND (canonical_name=?1 OR normalized_name=?2))",
                        rusqlite::params![plan.target_person, plan.target_person.to_lowercase()],
                        |r| r.get(0),
                    )?;
                    if !target_exists {
                        let _ = tx.execute(
                            "UPDATE entity SET canonical_name=?1, normalized_name=?2 WHERE kind='creator' AND (canonical_name=?3 OR normalized_name=?4)",
                            rusqlite::params![
                                plan.target_person,
                                plan.target_person.to_lowercase(),
                                plan.person,
                                plan.person.to_lowercase()
                            ],
                        );
                    }
                }
            }
            "merge-person" => {
                let at = plan
                    .execution
                    .as_ref()
                    .and_then(|e| e.data["timestamp"].as_str())
                    .unwrap_or("1970-01-01T00:00:00Z");
                let src_eid: Option<i64> = tx
                    .query_row(
                        "SELECT id FROM entity WHERE kind='creator' AND (canonical_name=?1 OR normalized_name=?2) ORDER BY id LIMIT 1",
                        rusqlite::params![plan.person, plan.person.to_lowercase()],
                        |r| r.get(0),
                    )
                    .optional()?;
                let dst_eid = ensure_creator(&tx, &plan.target_person, at)?;
                for &index in successful {
                    let file = &plan.files[index];
                    let win_src = metadata::win(&self.config, &file.src);
                    let win_dst = metadata::win(&self.config, &file.dst);
                    let dst_name = Path::new(&file.dst)
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("");
                    if let Some(aid) = tx
                        .query_row(
                            "SELECT id FROM asset WHERE path=?1 ORDER BY id LIMIT 1",
                            [&win_src],
                            |r| r.get::<_, i64>(0),
                        )
                        .optional()?
                    {
                        clean_target(&tx, &win_dst, Some(aid))?;
                        tx.execute(
                            "UPDATE asset SET path=?1, name=CASE WHEN ?2='' THEN name ELSE ?2 END WHERE id=?3",
                            rusqlite::params![win_dst, dst_name, aid],
                        )?;
                        updated_assets += 1;
                        if let Some(eid) = dst_eid {
                            upsert_link(&tx, aid, eid, "face-persons", 1.0, at)?;
                        }
                    }
                }
                if let (Some(s_id), Some(t_id)) = (src_eid, dst_eid)
                    && s_id != t_id
                {
                    let _ = tx.execute(
                        "UPDATE OR IGNORE asset_entity SET entity_id=?1 WHERE entity_id=?2",
                        rusqlite::params![t_id, s_id],
                    );
                    let _ = tx.execute("DELETE FROM asset_entity WHERE entity_id=?1", [s_id]);
                    let _ = tx.execute(
                        "UPDATE OR IGNORE entity_alias SET entity_id=?1 WHERE entity_id=?2",
                        rusqlite::params![t_id, s_id],
                    );
                    let _ = tx.execute("DELETE FROM entity_alias WHERE entity_id=?1", [s_id]);
                    let _ = tx.execute("DELETE FROM entity WHERE id=?1", [s_id]);
                }
            }
            _ => {
                let at = plan
                    .execution
                    .as_ref()
                    .and_then(|e| e.data["timestamp"].as_str())
                    .unwrap_or("1970-01-01T00:00:00Z");
                let deleting = plan.action == "delete-files";
                let changed_owner =
                    !deleting && !plan.target_person.is_empty() && plan.person != plan.target_person;
                let src_eid: Option<i64> = if changed_owner && !plan.person.is_empty() {
                    tx.query_row(
                        "SELECT id FROM entity WHERE kind='creator' AND (canonical_name=?1 OR normalized_name=?2) ORDER BY id LIMIT 1",
                        rusqlite::params![plan.person, plan.person.to_lowercase()],
                        |r| r.get(0),
                    )
                    .optional()?
                } else {
                    None
                };
                let dst_eid = if changed_owner {
                    ensure_creator(&tx, &plan.target_person, at)?
                } else {
                    None
                };
                for &index in successful {
                    let file = &plan.files[index];
                    let win_src = metadata::win(&self.config, &file.src);
                    let win_dst = metadata::win(&self.config, &file.dst);
                    let dst_name = Path::new(&file.dst)
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("");
                    if let Some(aid) = tx
                        .query_row(
                            "SELECT id FROM asset WHERE path=?1 ORDER BY id LIMIT 1",
                            [&win_src],
                            |r| r.get::<_, i64>(0),
                        )
                        .optional()?
                    {
                        clean_target(&tx, &win_dst, Some(aid))?;
                        tx.execute(
                            "UPDATE asset SET path=?1, name=CASE WHEN ?2='' THEN name ELSE ?2 END WHERE id=?3",
                            rusqlite::params![win_dst, dst_name, aid],
                        )?;
                        updated_assets += 1;
                        if changed_owner {
                            if let Some(s_id) = src_eid {
                                let _ = tx.execute(
                                    "DELETE FROM asset_entity WHERE asset_id=?1 AND entity_id=?2 AND role='creator'",
                                    rusqlite::params![aid, s_id],
                                );
                            }
                            if let Some(t_id) = dst_eid {
                                upsert_link(&tx, aid, t_id, "face-persons", 1.0, at)?;
                            }
                        }
                    }
                }
            }
        }
        let evidence = json!({
            "backend": "rust",
            "mode": "best_effort",
            "kind": plan.action,
            "updated_assets": updated_assets,
            "reconciled_scanner_assets": reconciled_stubs,
        });
        tx.execute(
            "INSERT INTO _face_admin_person_ops(id,signature,state,data) VALUES(?1,?2,'committed',?3)
             ON CONFLICT(id) DO UPDATE SET signature=excluded.signature,state='committed',data=excluded.data",
            rusqlite::params![plan.id, plan.version, serde_json::to_string(&evidence)?],
        )?;
        tx.commit()?;
        Ok(evidence)
    }

    fn apply_archive_with_scanner_reconciliation(
        &self,
        plan: &Plan,
        domain: &DomainSnapshot,
        successful: &[usize],
        error: Error,
    ) -> Result<(Value, Vec<i64>)> {
        self.verify_published_archive_destinations(plan, successful)
            .map_err(|published| with_reconciliation_cause(error.clone(), "归档目标核对", &published))?;
        let delta = domain
            .ledger
            .as_ref()
            .ok_or_else(|| Error::conflict("缺少账本意图"))?;
        let applied =
            crate::archive_cancel::apply_with_scanner_reconciliation(&self.config, plan, delta)
                .map_err(|observed| with_reconciliation_cause(error, "归档扫描提交", &observed))?;
        let reconciled = applied["reconciled_scanner_assets"]
            .as_array()
            .map(|ids| ids.iter().filter_map(Value::as_i64).collect())
            .unwrap_or_default();
        Ok((applied, reconciled))
    }
}
/// Keep the primary ledger conflict for callers while preserving why the
/// archive-specific fallback did not apply. A swallowed fallback cause leaves
/// recovery_needed without any actionable evidence.
fn with_reconciliation_cause(mut primary: Error, stage: &str, cause: &Error) -> Error {
    primary.error = match cause.path.as_deref() {
        Some(path) if !path.is_empty() => {
            format!("{}；{}：{}（{}）", primary.error, stage, cause.error, path)
        }
        _ => format!("{}；{}：{}", primary.error, stage, cause.error),
    };
    primary
}
fn validate(request: &Request) -> Result<()> {
    model::validate_id(&request.id)?;
    if request.action != "rename-file" {
        return native::validate(request);
    }
    if request.files.len() != 1 {
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

fn writes_faces(plan: &Plan) -> bool {
    plan.execution.as_ref().is_none_or(|e| e.stores.faces)
}
fn writes_jobs(plan: &Plan) -> bool {
    plan.execution.as_ref().is_none_or(|e| e.stores.jobs)
}
fn operation_snapshot(
    config: &Config,
    plan: &Plan,
    successful: &[usize],
) -> Result<DomainSnapshot> {
    if plan.execution.is_some() {
        let domain = native::snapshot(config, plan, successful)?;
        validate_domain(plan, &domain)?;
        Ok(domain)
    } else {
        Ok(DomainSnapshot {
            metadata: metadata::snapshot(config, plan)?,
            ledger: None,
        })
    }
}
fn ledger_snapshot_hash(config: &Config, plan: &Plan, domain: &DomainSnapshot) -> Result<String> {
    if plan.execution.is_some() {
        Ok(hash(&domain.ledger))
    } else {
        Ok(ledger::snapshot(config, plan)
            .map(|v| hash(&v))
            .unwrap_or_default())
    }
}
pub fn seal_plan(config: &Config, plan: &mut Plan) -> Result<()> {
    validate_plan(config, plan)?;
    let domain = operation_snapshot(config, plan, &(0..plan.files.len()).collect::<Vec<_>>())?;
    plan.metadata_version = metadata::version(&domain.metadata);
    plan.ledger_version = ledger_snapshot_hash(config, plan, &domain)?;
    plan.version = String::new();
    plan.version = hash(plan);
    Ok(())
}
fn validate_domain(plan: &Plan, domain: &DomainSnapshot) -> Result<()> {
    for document in &domain.metadata.documents {
        let path = Path::new(&document.name);
        if document.name.is_empty()
            || !path
                .components()
                .all(|p| matches!(p, std::path::Component::Normal(_)))
        {
            return Err(Error::conflict("元数据文件名超出本地状态目录"));
        }
    }
    if serde_json::to_vec(domain)?.len() > 64 * 1024 * 1024 {
        return Err(Error::new(413, "scope_too_large", "操作快照超过 64 MiB"));
    }
    if let Some(execution) = &plan.execution
        && ((!execution.stores.ledger && domain.ledger.is_some())
            || (!execution.stores.faces
                && (domain.metadata.faces_delta.is_some() || !domain.metadata.videos.is_empty()))
            || (!execution.stores.jobs
                && (domain.metadata.jobs_delta.is_some() || !domain.metadata.jobs.is_empty())))
    {
        return Err(Error::conflict("元数据快照与声明的存储范围不一致"));
    }
    Ok(())
}
fn validate_plan(config: &Config, plan: &Plan) -> Result<()> {
    if plan.files.len() > 5000 {
        return Err(Error::new(
            413,
            "scope_too_large",
            "一次最多处理 5000 个文件",
        ));
    }
    if let Some(execution) = &plan.execution {
        if execution.scopes.len() != plan.files.len() {
            return Err(Error::conflict("文件范围数量不一致"));
        }
        let mut identities = std::collections::HashSet::new();
        let mut sources = std::collections::HashSet::new();
        let mut destinations = std::collections::HashSet::new();
        for (index, file) in plan.files.iter().enumerate() {
            if !sources.insert(&file.src) || !destinations.insert(&file.dst) {
                return Err(Error::conflict("一个物理文件不能重复移动"));
            }
            let stat = files::check_move(config, &execution.scopes[index], file)?;
            let identity = files::identity(&stat);
            if !identities.insert((
                identity["dev"].clone().to_string(),
                identity["ino"].clone().to_string(),
            )) && !["rename-person", "merge-person", "delete-person"]
                .contains(&plan.action.as_str())
            {
                return Err(Error::conflict("同一物理文件不能重复移动"));
            }
        }
        if sources.iter().any(|s| destinations.contains(s)) {
            return Err(Error::conflict("源路径与目标路径不能相互覆盖"));
        }
        for directory in &plan.directories {
            if !crate::person_directory::approved(config, plan, directory)
                && !plan.files.iter().enumerate().any(|(index, file)| {
                    Path::new(&file.dst)
                        .parent()
                        .is_some_and(|p| p.starts_with(directory))
                        && files::approved_directory(
                            config,
                            &execution.scopes[index],
                            Path::new(directory),
                        )
                        .unwrap_or(false)
                })
            {
                return Err(Error::conflict("创建目录不在文件目标范围内"));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod shared_tests {
    use super::*;
    use crate::model::{AffectedStores, ExecutionIntent, MoveScope, ScopeRoot};
    use std::fs;
    fn fixture(count: usize) -> (Config, Plan, DomainSnapshot) {
        let root = fs::canonicalize("/tmp").unwrap().join(format!(
            "face-admin-person-fileops-shared-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(root.join("base")).unwrap();
        fs::write(root.join(".fileops-fixture"), "person file operations").unwrap();
        let config = Config {
            base: root.join("base"),
            media: root.join("media"),
            ledger: root.join("ledger.db"),
            bind: "127.0.0.1:0".into(),
            token: None,
            parity_root: None,
            fixture: Some(root.clone()),
        };
        fs::create_dir_all(config.creators().join("甲")).unwrap();
        fs::create_dir_all(config.creators().join("乙")).unwrap();
        let mut files = vec![];
        let mut scopes = vec![];
        for index in 0..count {
            let src = config.creators().join(format!("甲/{index}.mp4"));
            let dst = config.creators().join(format!("乙/{index}.mp4"));
            fs::write(&src, format!("isolated bytes {index}")).unwrap();
            let stat = fs::metadata(&src).unwrap();
            let src = src.display().to_string();
            files.push(FileMove {
                src: src.clone(),
                dst: dst.display().to_string(),
                vids: vec![format!("id-{index}")],
                size: stat.len(),
                version: files::fingerprint(&src, &stat),
            });
            scopes.push(MoveScope {
                source: ScopeRoot::Person("甲".into()),
                destination: ScopeRoot::Person("乙".into()),
            });
        }
        let id = uuid::Uuid::new_v4().to_string();
        let domain = DomainSnapshot {
            metadata: metadata::Snapshot {
                documents: vec![metadata::Document {
                    name: "fixture.json".into(),
                    before: None,
                    after: Some("{\"saved\":true}".into()),
                }],
                videos: vec![],
                jobs: vec![],
                faces_delta: None,
                jobs_delta: None,
            },
            ledger: None,
        };
        let request = Request {
            id: id.clone(),
            action: "fixture-shared".into(),
            person: "甲".into(),
            files: vec![],
            filename: String::new(),
            target_person: None,
            target_folder: None,
            payload: None,
        };
        let mut plan = Plan {
            id,
            request,
            version: String::new(),
            action: "fixture-shared".into(),
            person: "甲".into(),
            target_person: "乙".into(),
            total_bytes: files.iter().map(|f| f.size).sum(),
            files,
            directories: vec![],
            ledger_version: hash(&domain.ledger),
            metadata_version: metadata::version(&domain.metadata),
            execution: Some(ExecutionIntent {
                scopes,
                failure_policy: FailurePolicy::Rollback,
                stores: AffectedStores {
                    faces: false,
                    jobs: false,
                    ledger: false,
                },
                data: Value::Null,
                result: json!({"ok":true}),
            }),
        };
        plan.version = hash(&plan);
        (config, plan, domain)
    }
    fn admitted(config: &Config, plan: &Plan, domain: &DomainSnapshot) -> (Engine, WriteGate) {
        let mut engine = Engine::new(config.clone()).unwrap();
        engine.store.preview(plan).unwrap();
        let gate = WriteGate::acquire(config, &plan.id).unwrap();
        engine
            .store
            .remember_gate(&plan.id, &gate.evidence().unwrap())
            .unwrap();
        engine
            .store
            .admit(
                plan,
                json!({"schema":2,"domain":domain,"files":[],"gate":gate.evidence().unwrap()}),
            )
            .unwrap();
        (engine, gate)
    }
    #[test]
    fn two_files_commit_one_snapshot_and_real_progress() {
        let (config, plan, domain) = fixture(2);
        let (mut engine, gate) = admitted(&config, &plan, &domain);
        let result = engine.execute(&plan, &domain).unwrap();
        assert_eq!(result["changed"], 2);
        assert_eq!(result["successful_file_indexes"], json!([0, 1]));
        for (index, file) in plan.files.iter().enumerate() {
            assert!(!Path::new(&file.src).exists());
            assert_eq!(
                fs::read_to_string(&file.dst).unwrap(),
                format!("isolated bytes {index}")
            );
        }
        let status = crate::store::status(&config.state_db(), &plan.id).unwrap();
        assert_eq!(status["completed"], 2);
        assert_eq!(status["total"], 2);
        let scope = engine
            .store
            .latest_step(&plan.id, "commit_scope")
            .unwrap()
            .unwrap();
        assert_eq!(scope["successful_file_indexes"], json!([0, 1]));
        gate.release().unwrap();
    }
    #[test]
    fn zero_file_local_operation_does_not_open_unaffected_databases() {
        let (config, plan, domain) = fixture(0);
        assert!(!config.ledger.exists());
        assert!(!config.base.join("faces.db").exists());
        let (mut engine, gate) = admitted(&config, &plan, &domain);
        engine.execute(&plan, &domain).unwrap();
        gate.release().unwrap();
        assert_eq!(
            fs::read_to_string(config.base.join("fixture.json")).unwrap(),
            "{\"saved\":true}"
        );
        assert!(!config.ledger.exists());
        assert!(!config.base.join("faces.db").exists());
        let status = crate::store::status(&config.state_db(), &plan.id).unwrap();
        assert_eq!(status["total"], 0);
    }
    #[test]
    fn second_file_failure_compensates_the_entire_operation() {
        let (config, plan, domain) = fixture(2);
        let (mut engine, gate) = admitted(&config, &plan, &domain);
        fs::write(
            config.fixture.as_ref().unwrap().join("rust-fault.json"),
            r#"{"phase":"destination_published:1","mode":"error"}"#,
        )
        .unwrap();
        let error = engine.execute(&plan, &domain).unwrap_err();
        fs::remove_file(config.fixture.as_ref().unwrap().join("rust-fault.json")).unwrap();
        assert!(
            engine
                .reconcile(&plan, &domain, true, &error)
                .unwrap()
                .is_none()
        );
        gate.release().unwrap();
        for (index, file) in plan.files.iter().enumerate() {
            assert_eq!(
                fs::read_to_string(&file.src).unwrap(),
                format!("isolated bytes {index}")
            );
            assert!(!Path::new(&file.dst).exists());
        }
        assert!(!config.base.join("fixture.json").exists());
        assert_eq!(
            engine.store.get(&plan.id).unwrap().unwrap().status,
            "rolled_back"
        );
    }
    fn assert_pre_effect_second_failure_rolls_back(external_source_change: bool, restart: bool) {
        let (config, mut plan, domain) = fixture(2);
        let owned_dir = config.creators().join("乙/owned-new");
        plan.files[0].dst = owned_dir.join("0.mp4").display().to_string();
        plan.directories = vec![owned_dir.display().to_string()];
        plan.version = String::new();
        plan.version = hash(&plan);
        let (mut engine, gate) = admitted(&config, &plan, &domain);
        let external = if external_source_change {
            &plan.files[1].src
        } else {
            &plan.files[1].dst
        };
        fs::write(external, b"external late bytes").unwrap();
        let error = engine.execute(&plan, &domain).unwrap_err();
        assert!(!Path::new(&plan.files[0].src).exists());
        let failures = engine.store.failures(&plan.id).unwrap();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0]["file_index"], 1);
        assert_eq!(failures[0]["unchanged"], true);
        if restart {
            drop(engine);
            drop(gate);
            engine = Engine::new(config.clone()).unwrap();
        } else {
            assert!(
                engine
                    .reconcile(&plan, &domain, true, &error)
                    .unwrap()
                    .is_none()
            );
            gate.release().unwrap();
        }
        assert_eq!(
            engine.store.get(&plan.id).unwrap().unwrap().status,
            "rolled_back"
        );
        assert_eq!(
            fs::read_to_string(&plan.files[0].src).unwrap(),
            "isolated bytes 0"
        );
        assert!(!Path::new(&plan.files[0].dst).exists());
        assert!(!owned_dir.exists());
        assert_eq!(fs::read(external).unwrap(), b"external late bytes");
        if !external_source_change {
            assert_eq!(
                fs::read_to_string(&plan.files[1].src).unwrap(),
                "isolated bytes 1"
            );
        }
        assert!(!config.base.join("fixture.json").exists());
        assert!(!config.base.join(".review-write.lock").exists());
        assert!(
            engine
                .store
                .latest_step(&plan.id, "commit_scope")
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn late_second_target_preserves_external_bytes_and_cleans_owned_directory() {
        assert_pre_effect_second_failure_rolls_back(false, false);
    }
    #[test]
    fn changed_second_source_does_not_block_first_file_compensation() {
        assert_pre_effect_second_failure_rolls_back(true, false);
    }
    #[test]
    fn restart_after_pre_effect_failure_keeps_rollback_policy() {
        assert_pre_effect_second_failure_rolls_back(false, true);
    }
    #[test]
    fn restart_after_second_publication_preserves_file_indexes() {
        let (config, plan, domain) = fixture(2);
        let (mut engine, gate) = admitted(&config, &plan, &domain);
        fs::write(
            config.fixture.as_ref().unwrap().join("rust-fault.json"),
            r#"{"phase":"destination_published:1","mode":"error"}"#,
        )
        .unwrap();
        assert!(engine.execute(&plan, &domain).is_err());
        drop(engine);
        drop(gate);
        fs::remove_file(config.fixture.as_ref().unwrap().join("rust-fault.json")).unwrap();
        let engine = Engine::new(config.clone()).unwrap();
        let record = engine.store.get(&plan.id).unwrap().unwrap();
        assert_eq!(record.status, "complete");
        assert_eq!(record.result.unwrap()["changed"], 2);
        for file in &plan.files {
            assert!(!Path::new(&file.src).exists());
            assert!(Path::new(&file.dst).exists());
        }
        assert!(!config.base.join(".review-write.lock").exists());
    }
    #[test]
    fn shared_mutation_unknown_stops_remaining_files_and_survives_restart() {
        for step in ["destination_publish", "source_remove"] {
            let (config, plan, domain) = fixture(2);
            let (mut engine, gate) = admitted(&config, &plan, &domain);
            let fault = config.fixture.as_ref().unwrap().join("rust-fault.json");
            fs::write(
                &fault,
                serde_json::to_vec(&json!({"phase":format!("mutation:{step}"),
                "mode":"mutation_error_after","errno":"EIO"}))
                .unwrap(),
            )
            .unwrap();
            let error = engine.execute(&plan, &domain).unwrap_err();
            assert_eq!(error.mutation_outcome.as_deref(), Some("unknown"));
            let unknown = engine.reconcile(&plan, &domain, true, &error).unwrap_err();
            engine.recovery_failure(&plan, &unknown).unwrap();
            fs::remove_file(fault).unwrap();
            let starts: i64 = engine.store.db.query_row(
                "SELECT COUNT(*) FROM steps WHERE operation_id=? AND phase='filesystem_mutation_intent'",
                [&plan.id], |r| r.get(0)).unwrap();
            drop(engine);
            drop(gate);
            let mut engine = Engine::new(config.clone()).unwrap();
            engine.recover_one(&plan.id);
            assert_eq!(
                engine.store.get(&plan.id).unwrap().unwrap().status,
                "recovery_needed"
            );
            assert!(config.base.join(".review-write.lock").exists());
            assert!(Path::new(&plan.files[0].dst).exists());
            assert_eq!(
                Path::new(&plan.files[0].src).exists(),
                step != "source_remove"
            );
            assert!(Path::new(&plan.files[1].src).exists());
            assert!(!Path::new(&plan.files[1].dst).exists());
            assert!(!config.base.join("fixture.json").exists());
            assert!(
                engine
                    .store
                    .latest_step(&plan.id, "compensation_intent")
                    .unwrap()
                    .is_none()
            );
            let after: i64 = engine.store.db.query_row(
                "SELECT COUNT(*) FROM steps WHERE operation_id=? AND phase='filesystem_mutation_intent'",
                [&plan.id], |r| r.get(0)).unwrap();
            assert_eq!(starts, after);
        }
    }
    #[test]
    fn old_request_and_plan_serialization_do_not_gain_optional_fields() {
        let request = r#"{"id":"35392923-e779-48f0-b3cd-2d97a72943d2","action":"rename-file","person":"甲","files":[],"filename":"b.mp4"}"#;
        let decoded: Request = serde_json::from_str(request).unwrap();
        assert_eq!(serde_json::to_string(&decoded).unwrap(), request);
        let (_, mut plan, _) = fixture(1);
        plan.execution = None;
        plan.request = decoded;
        let serialized = serde_json::to_value(plan).unwrap();
        assert!(serialized.get("execution").is_none());
        assert!(serialized["request"].get("target_person").is_none());
    }
    #[test]
    fn tokio_concurrent_multi_file_execution_commits_and_isolates_unchanged_failures() {
        let (mut config, mut plan, domain) = fixture(6);
        config.fixture = None;
        plan.execution.as_mut().unwrap().failure_policy = FailurePolicy::ContinueUnchanged;
        plan.version = hash(&plan);
        let (mut engine, gate) = admitted(&config, &plan, &domain);
        let result = engine.execute(&plan, &domain).unwrap();
        gate.release().unwrap();
        assert_eq!(result["ok"], true);
        for index in 0..6 {
            assert!(!Path::new(&plan.files[index].src).exists());
            assert!(Path::new(&plan.files[index].dst).exists());
        }
        let status = crate::store::status(&config.state_db(), &plan.id).unwrap();
        assert_eq!(status["completed"], 6);
        assert_eq!(status["status"], "complete");
    }
    #[test]
    fn peach_ledger_conflict_or_missing_db_does_not_block_or_rollback_local_commit() {
        let (config, mut plan, mut domain) = fixture(1);
        plan.execution.as_mut().unwrap().stores.ledger = true;
        // Domain has no ledger delta (simulating missing or conflicting Peach ledger at plan time)
        // and config.ledger does not exist on disk.
        domain.ledger = None;
        plan.version = hash(&plan);
        let (mut engine, gate) = admitted(&config, &plan, &domain);
        let result = engine.execute(&plan, &domain).unwrap();
        gate.release().unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(result["changed"], 1);
        assert!(!Path::new(&plan.files[0].src).exists());
        assert!(Path::new(&plan.files[0].dst).exists());
        assert_eq!(
            fs::read_to_string(config.base.join("fixture.json")).unwrap(),
            "{\"saved\":true}"
        );
        let status = crate::store::status(&config.state_db(), &plan.id).unwrap();
        assert_eq!(status["status"], "complete");
        assert!(!config.base.join(".review-write.lock").exists());
    }
}
