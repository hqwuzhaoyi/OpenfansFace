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
        plan.ledger_version = hash(&ledger::snapshot(&self.config, &plan)?);
        plan.version = hash(&plan);
        Ok(plan)
    }
    pub fn apply(&mut self, request: Request, version: String) -> Result<Value> {
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
        self.store.remember_gate(&request.id, &gate.evidence()?)?;
        crate::fault::checkpoint(&self.config, "gate_acquired")?;
        if crate::staging_delete::action(&record.plan.action) {
            return crate::staging_delete::apply(self, &record.plan, gate);
        }
        let prepared = (|| {
            let current = if record.plan.execution.is_some() {
                validate_plan(&self.config, &record.plan)?;
                native::revalidate(&self.config, &record.plan)?;
                record.plan.clone()
            } else {
                self.plan(request.clone())?
            };
            if current.version != version {
                return Err(Error::conflict("文件或相关记录已变化，请重新预览"));
            }
            let domain = operation_snapshot(
                &self.config,
                &current,
                &(0..current.files.len()).collect::<Vec<_>>(),
            )?;
            if metadata::version(&domain.metadata) != current.metadata_version
                || ledger_snapshot_hash(&self.config, &current, &domain)? != current.ledger_version
            {
                return Err(Error::conflict("预检期间元数据或账本发生变化"));
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
                json!({"metadata":domain.metadata,"ledger":ledger::snapshot(&self.config,&current)?,"source_identity":identities[0]["source_identity"],"gate":gate.evidence()?})
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
                gate.release()?;
                crate::fault::checkpoint(&self.config, "complete")?;
                Ok(result)
            }
            Err(error) => {
                // A failed call is not a failed transaction. Inspect all domains before deciding.
                match self.reconcile(&plan, &domain, true, &error) {
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
            if metadata::version(&domain.metadata) != record.plan.metadata_version
                || (if record.plan.execution.is_some() {
                    hash(&domain.ledger)
                } else {
                    hash(&evidence["ledger"])
                }) != record.plan.ledger_version
            {
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
                self.reconcile(&record.plan, &domain, true, &error)?;
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
        let ledger_state = self.ledger_state(plan, &domain)?;
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
        let rollback = rollback || failed_rollback;
        for (index, file) in plan.files.iter().enumerate() {
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
            self.ledger_cancel(plan, &domain)?;
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
            Ok(None)
        } else {
            self.execute(plan, &domain).map(Some)
        }
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
            ledger::inspect_delta(
                &self.config,
                plan,
                domain
                    .ledger
                    .as_ref()
                    .ok_or_else(|| Error::conflict("缺少账本意图"))?,
            )
        } else {
            ledger::inspect(&self.config, plan)
        }
    }
    fn ledger_reserve(&self, plan: &Plan, domain: &DomainSnapshot) -> Result<()> {
        if let Some(execution) = &plan.execution {
            if !execution.stores.ledger {
                return Ok(());
            }
            ledger::reserve_delta(
                &self.config,
                plan,
                domain
                    .ledger
                    .as_ref()
                    .ok_or_else(|| Error::conflict("缺少账本意图"))?,
            )
        } else {
            ledger::reserve(&self.config, plan)
        }
    }
    fn ledger_cancel(&self, plan: &Plan, domain: &DomainSnapshot) -> Result<()> {
        if let Some(execution) = &plan.execution {
            if !execution.stores.ledger {
                return Ok(());
            }
            ledger::cancel_delta(
                &self.config,
                plan,
                domain
                    .ledger
                    .as_ref()
                    .ok_or_else(|| Error::conflict("缺少账本意图"))?,
            )
        } else {
            ledger::cancel(&self.config, plan)
        }
    }
    fn ledger_apply(&self, plan: &Plan, domain: &DomainSnapshot) -> Result<Value> {
        if let Some(execution) = &plan.execution {
            if !execution.stores.ledger {
                return Ok(json!({"skipped":true}));
            }
            ledger::apply_delta(
                &self.config,
                plan,
                domain
                    .ledger
                    .as_ref()
                    .ok_or_else(|| Error::conflict("缺少账本意图"))?,
            )
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
        if plan.execution.is_none() {
            self.checkpoint(
                id,
                "ledger_reserve_intent",
                json!({"signature":plan.version}),
            )?;
            if self.ledger_state(plan, accepted)? == "absent" {
                self.ledger_reserve(plan, accepted)?;
            }
            self.checkpoint(id, "ledger_reserved", json!({"state":"prepared"}))?;
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
                        files::prepare_move(&self.config, &execution.scopes[index], file)
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
                                if execution.failure_policy == FailurePolicy::ContinueUnchanged {
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
                            pinned.publish_recoverable(&file.version, &mut |phase, proof| {
                                self.file_checkpoint(plan, index, phase, proof)
                            })?;
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
                    self.file_checkpoint(plan, index, "destination_published", &pinned.evidence)?;
                    self.file_checkpoint(plan, index, "source_remove_intent", &pinned.evidence)?;
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
        if plan.execution.is_some() {
            self.checkpoint(
                id,
                "ledger_reserve_intent",
                json!({"signature":plan.version,"successful_file_indexes":successful}),
            )?;
            if self.ledger_state(plan, &domain)? == "absent" {
                self.ledger_reserve(plan, &domain)?;
            }
            self.checkpoint(id, "ledger_reserved", json!({"state":"prepared"}))?;
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
        let evidence = if self.ledger_state(plan, &domain)? == "committed" {
            json!({"reconciled":true})
        } else {
            self.ledger_apply(plan, &domain)?
        };
        crate::fault::checkpoint(&self.config, "ledger_committed_unrecorded")?;
        self.checkpoint(id, "ledger_committed", evidence)?;
        let result = if plan.execution.is_some() {
            native::result(plan, &successful, &self.store.failures(id)?)
        } else {
            model::final_result(plan)
        };
        self.checkpoint(id, "final_receipt_intent", json!({"result":result}))?;
        self.store.complete(plan, &result)?;
        crate::fault::checkpoint(&self.config, "final_receipt_unreleased")?;
        Ok(result)
    }
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
        Ok(hash(&ledger::snapshot(config, plan)?))
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
        && (execution.stores.ledger != domain.ledger.is_some()
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
            )) {
                return Err(Error::conflict("同一物理文件不能重复移动"));
            }
        }
        if sources.iter().any(|s| destinations.contains(s)) {
            return Err(Error::conflict("源路径与目标路径不能相互覆盖"));
        }
        for directory in &plan.directories {
            if !plan.files.iter().enumerate().any(|(index, file)| {
                Path::new(&file.dst)
                    .parent()
                    .is_some_and(|p| p.starts_with(directory))
                    && files::approved_directory(
                        config,
                        &execution.scopes[index],
                        Path::new(directory),
                    )
                    .unwrap_or(false)
            }) {
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
}
