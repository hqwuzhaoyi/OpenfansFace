//! Irreversible staged-file disposal under the shared Engine/Store/write authority.
//! Per-item snapshots and native store receipts are durable before unlink. Never compensate bytes.
use crate::{
    config::Config,
    db,
    engine::Engine,
    files::{self, PinnedDelete, WriteGate},
    ledger, metadata,
    model::{
        self, AffectedStores, Error, ExecutionIntent, FailurePolicy, FileMove, Plan, Request,
        Result, SelectedFile, hash,
    },
    native::DomainSnapshot,
    person_registration as registration, staging,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
};
#[derive(Clone, Serialize, Deserialize)]
struct Item {
    proof: Value,
    origin: Value,
    domain: DomainSnapshot,
}
pub fn action(value: &str) -> bool {
    matches!(value, "delete-staged" | "clear-staging")
}
pub fn confirmation(action: &str) -> &'static str {
    if action == "clear-staging" {
        "清空回收站"
    } else {
        "永久删除"
    }
}
pub fn validate(r: &Request) -> Result<()> {
    model::validate_id(&r.id)?;
    if !action(&r.action)
        || !r.person.is_empty()
        || !r.filename.is_empty()
        || r.target_person.is_some()
        || r.target_folder.is_some()
        || r.payload.is_some()
    {
        return Err(Error::new(
            400,
            "invalid_request",
            "永久删除不接受目标或自定义执行参数",
        ));
    }
    if (r.action == "clear-staging" && !r.files.is_empty())
        || (r.action == "delete-staged" && (r.files.is_empty() || r.files.len() > 500))
    {
        return Err(Error::new(
            400,
            "invalid_request",
            "请选择 1–500 个文件；清空范围由服务器完整扫描",
        ));
    }
    Ok(())
}
/// Strict scan is intentionally separate from paginated list; special/incomplete entries fail closed.
pub fn scan(config: &Config) -> Result<Vec<SelectedFile>> {
    let root = staging::root(config);
    let mut pending = vec![root.clone()];
    let mut rows = BTreeMap::new();
    let mut count = 0;
    while let Some(dir) = pending.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if dir == root && e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(Error::from(e).at(dir.display().to_string())),
        };
        files::parent_identity(&dir.join("probe").to_string_lossy())?;
        for entry in entries {
            let p = entry?.path();
            count += 1;
            if count > 100_000 {
                return Err(Error::new(
                    413,
                    "scan_limit",
                    "暂存区超出完整扫描限制，不能清空",
                ));
            }
            let stat = p.symlink_metadata()?;
            let path = p
                .to_str()
                .ok_or_else(|| Error::conflict("暂存区包含无法显示的文件名"))?
                .to_owned();
            if stat.is_symlink() || (!stat.is_dir() && !stat.is_file()) {
                return Err(Error::conflict("暂存区含符号链接或特殊条目，不能安全清空").at(path));
            }
            if stat.is_dir() {
                pending.push(p);
            } else {
                rows.insert(
                    path.clone(),
                    SelectedFile {
                        vid: path.clone(),
                        version: files::fingerprint(&path, &stat),
                        path,
                    },
                );
            }
        }
    }
    Ok(rows.into_values().collect())
}
fn legacy_guard(config: &Config) -> Result<()> {
    let directory = config.base.join("staging-operations");
    let entries = match fs::read_dir(&directory) {
        Ok(v) => v,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let Some(id) = name.to_str() else {
            return Err(Error::conflict("旧暂存回执目录无效"));
        };
        model::validate_id(id)?;
        let (raw, v) = registration::document(
            config,
            &format!("staging-operations/{id}/operation.json"),
            false,
        )?;
        if raw.is_none()
            || !matches!(v["state"].as_str(), Some("preview" | "finished"))
            || v["result"]["status"] == "recovery_needed"
        {
            return Err(Error::new(
                503,
                "legacy_recovery_required",
                "存在旧暂存操作未核对，禁止新永久删除",
            )
            .at(entry.path().display().to_string()));
        }
    }
    Ok(())
}
fn table_delta(
    table: db::Table,
    rows: Vec<Value>,
    after: Vec<Value>,
    paths: Vec<String>,
    vids: Vec<String>,
) -> db::Delta {
    db::Delta {
        tables: vec![db::TableDelta {
            table,
            key_columns: vec!["vid".into()],
            scope: rows.iter().map(|r| json!({"vid":r["vid"]})).collect(),
            before: rows.clone(),
            after: after.clone(),
        }],
        guards: vec![db::Guard::LocalFiles {
            table,
            paths,
            vids,
            before: rows,
            after,
        }],
    }
}

fn matches_history(config: &Config, file: &FileMove, row: &Value) -> bool {
    row["staged_path"] == file.src
        || row["dst"] == file.src
        || row["win_dst"] == metadata::win(config, &file.src)
}
/// Only affected records enter the preview hash. Background health/queue work on other
/// videos must neither invalidate a confirmation nor be overwritten by its saved snapshot.
fn projection(config: &Config, file: &FileMove, name: &str, value: &Value) -> Result<Value> {
    if name == "archive-history.json" {
        return Ok(Value::Array(
            value
                .as_array()
                .ok_or_else(|| Error::conflict("归档历史格式无效"))?
                .iter()
                .filter(|r| matches_history(config, file, r))
                .cloned()
                .collect(),
        ));
    }
    let map = value
        .as_object()
        .ok_or_else(|| Error::conflict("暂存元数据格式无效"))?;
    let mut selected = serde_json::Map::new();
    match name {
        "staging-origins.json" => {
            if let Some(v) = map.get(&file.src) {
                selected.insert(file.src.clone(), v.clone());
            }
        }
        "video-health.json" => {
            if let Some(records) = map.get("records") {
                for (id, row) in records
                    .as_object()
                    .ok_or_else(|| Error::conflict("体检记录格式无效"))?
                {
                    if row["path"] == file.src || row["path"] == metadata::win(config, &file.src) {
                        selected.insert(id.clone(), row.clone());
                    }
                }
            }
        }
        "video-path-overrides.json" => {
            for (id, path) in map {
                if file.vids.contains(id)
                    || *path == file.src
                    || *path == metadata::win(config, &file.src)
                {
                    selected.insert(id.clone(), path.clone());
                }
            }
        }
        _ => return Err(Error::conflict("未知暂存 JSON 记录")),
    }
    Ok(Value::Object(selected))
}
fn document_state(
    config: &Config,
    file: &FileMove,
    doc: &metadata::Document,
) -> Result<(Option<String>, Value, Value, Value)> {
    let (raw, value) =
        registration::document(config, &doc.name, doc.name == "archive-history.json")?;
    let before: Value = serde_json::from_str(
        doc.before
            .as_deref()
            .ok_or_else(|| Error::conflict("缺少 JSON 原值"))?,
    )?;
    let after: Value = serde_json::from_str(
        doc.after
            .as_deref()
            .ok_or_else(|| Error::conflict("缺少 JSON 结果"))?,
    )?;
    let current = projection(config, file, &doc.name, &value)?;
    if current != before && current != after {
        return Err(Error::conflict("相关暂存记录被修改，保留恢复状态").at(&doc.name));
    }
    Ok((raw, value, current, after))
}
fn inspect_document(config: &Config, file: &FileMove, doc: &metadata::Document) -> Result<()> {
    document_state(config, file, doc).map(|_| ())
}
fn apply_document(config: &Config, file: &FileMove, doc: &metadata::Document) -> Result<()> {
    let (raw, mut value, current, after) = document_state(config, file, doc)?;
    if current == after {
        return Ok(());
    }
    if doc.name == "archive-history.json" {
        let mut replacements = after
            .as_array()
            .ok_or_else(|| Error::conflict("归档记录恢复格式无效"))?
            .iter();
        for row in value.as_array_mut().unwrap() {
            if matches_history(config, file, row) {
                *row = replacements
                    .next()
                    .ok_or_else(|| Error::conflict("归档记录范围变化"))?
                    .clone();
            }
        }
        if replacements.next().is_some() {
            return Err(Error::conflict("归档记录数量变化"));
        }
    } else {
        let target = if doc.name == "video-health.json" {
            &mut value["records"]
        } else {
            &mut value
        };
        let map = target
            .as_object_mut()
            .ok_or_else(|| Error::conflict("暂存记录恢复格式无效"))?;
        for key in current.as_object().unwrap().keys() {
            map.remove(key);
        }
        map.extend(after.as_object().unwrap().clone());
    }
    metadata::apply_document(
        config,
        &metadata::Document {
            name: doc.name.clone(),
            before: raw,
            after: Some(serde_json::to_string_pretty(&value)?),
        },
    )
}
fn domain(
    config: &Config,
    file: &FileMove,
    operation: &str,
    virtual_docs: &mut BTreeMap<String, Option<String>>,
) -> Result<DomainSnapshot> {
    let paths = vec![file.src.clone(), metadata::win(config, &file.src)];
    let videos = db::rows(
        &db::open(&config.base.join("faces.db"))?,
        "SELECT * FROM videos ORDER BY vid",
        [],
    )?
    .into_iter()
    .filter(|r| file.vids.iter().any(|v| r["vid"] == *v) || paths.iter().any(|p| r["path"] == *p))
    .collect::<Vec<_>>();
    let faces_delta = table_delta(
        db::Table::Videos,
        videos.clone(),
        videos,
        paths.clone(),
        file.vids.clone(),
    );
    let jobs_delta = if config.base.join("processing/processing.db").exists() {
        let rows = db::rows(
            &db::open(&config.base.join("processing/processing.db"))?,
            "SELECT * FROM tasks ORDER BY vid",
            [],
        )?
        .into_iter()
        .filter(|r| {
            paths.iter().any(|p| r["path"] == *p) || file.vids.iter().any(|v| r["vid"] == *v)
        })
        .collect::<Vec<_>>();
        let after = rows
            .iter()
            .map(|r| {
                let mut r = r.clone();
                r["stage"] = json!("excluded");
                r["mtime_ns"] = json!("0");
                r
            })
            .collect();
        Some(table_delta(
            db::Table::Tasks,
            rows,
            after,
            paths.clone(),
            file.vids.clone(),
        ))
    } else {
        None
    };
    let mut documents = vec![];
    for name in [
        "archive-history.json",
        "video-health.json",
        "staging-origins.json",
        "video-path-overrides.json",
    ] {
        let before = if let Some(raw) = virtual_docs.get(name) {
            raw.clone()
        } else {
            registration::document(config, name, name == "archive-history.json")?.0
        };
        let mut v: Value = before
            .as_ref()
            .map(|s| serde_json::from_str(s))
            .transpose()?
            .unwrap_or_else(|| {
                if name == "archive-history.json" {
                    json!([])
                } else {
                    json!({})
                }
            });
        let old = v.clone();
        match name {
            "archive-history.json" => {
                for row in v
                    .as_array_mut()
                    .ok_or_else(|| Error::conflict("归档历史格式无效"))?
                {
                    if row["staged_path"] == file.src {
                        row["staged_state"] = json!("deleted");
                        row["last_staging_operation"] = json!(operation);
                    } else if paths
                        .iter()
                        .any(|p| row["dst"] == *p || row["win_dst"] == *p)
                    {
                        row["file_state"] = json!("deleted");
                        row["last_staging_operation"] = json!(operation);
                    }
                }
            }
            "video-health.json" => {
                if let Some(rows) = v.get_mut("records").and_then(Value::as_object_mut) {
                    for row in rows.values_mut() {
                        if paths.iter().any(|p| row["path"] == *p) {
                            row["status"] = json!("missing");
                            row["detail"] = json!("文件已从回收站永久删除。");
                            row["hidden"] = json!(false);
                        }
                    }
                }
            }
            "staging-origins.json" => {
                if v.get(&file.src).is_none() {
                    v[&file.src] = json!({"source":"unknown","original_path":null});
                }
                v[&file.src]["deleted_by"] = json!(operation);
            }
            _ => (),
        }
        virtual_docs.insert(name.into(), Some(serde_json::to_string(&v)?));
        documents.push(metadata::Document {
            name: name.into(),
            before: Some(serde_json::to_string(&projection(
                config, file, name, &old,
            )?)?),
            after: Some(serde_json::to_string(&projection(config, file, name, &v)?)?),
        });
    }
    let conn = db::open(&config.ledger)?;
    let path = metadata::win(config, &file.src);
    let assets = db::rows(
        &conn,
        "SELECT * FROM asset WHERE path=? ORDER BY id",
        [&path],
    )?;
    if assets.len() > 1 {
        return Err(Error::conflict("暂存路径对应多条 Peach 记录").at(&file.src));
    }
    let after: Vec<_> = assets
        .iter()
        .map(|a| {
            let mut a = a.clone();
            if a["disposal"].is_null() {
                a["disposal"] = json!("trash");
            }
            a
        })
        .collect();
    let mut guards = vec![db::Guard::AssetPath {
        path,
        before: assets.clone(),
        after: after.clone(),
    }];
    for asset in &assets {
        let links = db::rows(
            &conn,
            "SELECT * FROM asset_entity WHERE asset_id=? ORDER BY entity_id,role,source",
            [db::integer(&asset["id"])?],
        )?;
        guards.push(db::Guard::AssetLinks {
            asset_id: asset["id"].clone(),
            before: links.clone(),
            after: links,
        });
    }
    Ok(DomainSnapshot {
        metadata: metadata::Snapshot {
            documents,
            videos: vec![],
            jobs: vec![],
            faces_delta: Some(faces_delta),
            jobs_delta,
        },
        ledger: Some(ledger::Delta {
            changes: db::Delta {
                tables: vec![db::TableDelta {
                    table: db::Table::Asset,
                    key_columns: vec!["id".into()],
                    scope: assets.iter().map(|a| json!({"id":a["id"]})).collect(),
                    before: assets,
                    after,
                }],
                guards,
            },
            creators: vec![],
        }),
    })
}
pub fn plan(config: &Config, request: Request) -> Result<Plan> {
    validate(&request)?;
    legacy_guard(config)?;
    let selected = if request.action == "clear-staging" {
        scan(config)?
    } else {
        request.files.clone()
    };
    if selected.is_empty() {
        return Err(Error::conflict("暂存区没有可操作文件"));
    }
    let (_, origins) = registration::document(config, "staging-origins.json", false)?;
    let index = registration::SourceIndex::load(
        config,
        &selected.iter().map(|f| f.path.clone()).collect::<Vec<_>>(),
    )?;
    let mut items = vec![];
    let mut files = vec![];
    let mut seen = BTreeSet::new();
    let mut docs = BTreeMap::new();
    let mut total = 0u64;
    for chosen in selected {
        if !seen.insert(chosen.path.clone()) {
            return Err(Error::conflict("重复选择暂存文件"));
        }
        let pinned = PinnedDelete::open(config, &chosen.path, None)?;
        let proof = pinned.proof()?;
        pinned.verify(&proof)?;
        if proof["version"] != chosen.version {
            return Err(Error::conflict("暂存文件已变化").at(&chosen.path));
        }
        let stat = fs::symlink_metadata(&chosen.path)?;
        // Resolve only actual staged locations. Origin audit IDs may point at canonical duplicates.
        let file = FileMove {
            src: chosen.path.clone(),
            dst: String::new(),
            vids: index.ids_for_path(&chosen.path),
            version: chosen.version,
            size: stat.len(),
        };
        total = total
            .checked_add(file.size)
            .ok_or_else(|| Error::conflict("总大小溢出"))?;
        let item = Item {
            proof,
            origin: staging::origin(config, &file.src, &origins)?,
            domain: domain(config, &file, &request.id, &mut docs)?,
        };
        items.push(item);
        files.push(file);
    }
    let data = json!({"items":items,"confirmation":confirmation(&request.action)});
    if serde_json::to_vec(&data)?.len() > 64 * 1024 * 1024 {
        return Err(Error::new(
            413,
            "scope_too_large",
            "删除恢复材料超过 64 MiB，请分批处理",
        ));
    }
    let mut plan = Plan {
        id: request.id.clone(),
        action: request.action.clone(),
        person: String::new(),
        target_person: String::new(),
        request,
        files,
        total_bytes: total,
        directories: vec![],
        ledger_version: hash(&items.iter().map(|i| &i.domain.ledger).collect::<Vec<_>>()),
        metadata_version: hash(&items.iter().map(|i| &i.domain.metadata).collect::<Vec<_>>()),
        version: String::new(),
        execution: Some(ExecutionIntent {
            scopes: vec![],
            failure_policy: FailurePolicy::StopIrreversible,
            stores: AffectedStores {
                faces: true,
                jobs: config.base.join("processing/processing.db").exists(),
                ledger: true,
            },
            data,
            result: json!({}),
        }),
    };
    plan.version = hash(&plan);
    Ok(plan)
}
fn items(plan: &Plan) -> Result<Vec<Item>> {
    Ok(serde_json::from_value(
        plan.execution
            .as_ref()
            .ok_or_else(|| Error::conflict("缺少不可逆计划"))?
            .data["items"]
            .clone(),
    )?)
}
fn subplan(plan: &Plan, index: usize) -> Plan {
    let mut p = plan.clone();
    p.id = format!("{}:delete:{}", plan.id, index);
    p.files = vec![plan.files[index].clone()];
    p
}
fn event(engine: &mut Engine, plan: &Plan, index: usize, phase: &str, extra: Value) -> Result<()> {
    let mut v = json!({"file_index":index,"src":plan.files[index].src,"irreversible":true});
    v.as_object_mut()
        .unwrap()
        .extend(extra.as_object().cloned().unwrap_or_default());
    engine.store.step(&plan.id, "running", phase, v)
}
fn finished(engine: &mut Engine, plan: &Plan) -> Result<Value> {
    let mut rows = vec![];
    let mut done = 0;
    let mut failed = 0;
    for (index, file) in plan.files.iter().enumerate() {
        let step = engine.store.latest_file_step(&plan.id, index)?;
        let phase = step
            .as_ref()
            .and_then(|s| s["state"].as_str())
            .unwrap_or("not_started");
        if phase == "complete" {
            done += 1;
        }
        if phase == "failed" {
            failed += 1;
        }
        rows.push(json!({"file_index":index,"src":file.src,"status":phase,"error":step.as_ref().and_then(|s|s.get("error")),"irreversible":phase=="complete"}));
    }
    let result = json!({"id":plan.id,"ok":done==plan.files.len(),"action":plan.action,"status":if done==plan.files.len(){"complete"}else{"partial"},"changed":done,"completed":done,"failed":failed,"total":plan.files.len(),"remaining":plan.files.len()-done,"files":rows,"warning":if done>0{Some("已删除的文件内容不可恢复")}else{None}});
    engine.store.complete(plan, &result)?;
    Ok(result)
}
pub fn apply(engine: &mut Engine, plan: &Plan, gate: WriteGate) -> Result<Value> {
    let fresh = match self::plan(&engine.config, plan.request.clone()) {
        Ok(p) => p,
        Err(e) => {
            gate.release()?;
            return Err(e);
        }
    };
    if fresh.version != plan.version {
        gate.release()?;
        return Err(Error::conflict(
            "暂存范围、相关记录或权限已变化，请重新预览",
        ));
    }
    if let Err(e) = engine.store.admit(
        plan,
        json!({"schema":"staging-delete-v1","gate":gate.evidence()?,"items":items(plan)?}),
    ) {
        let failure = Error::new(
            503,
            "recovery_required",
            format!("删除意图持久化结果未知：{e}"),
        );
        let _ = engine.store.fail(&plan.id, &failure, true);
        return Err(failure);
    }
    let outcome = run(engine, plan, gate, false);
    if let Err(error) = &outcome {
        // Journal/receipt failures can occur outside the item loop. Do not strand a
        // finished worker in a misleading running state; keep its admitted gate.
        if engine
            .store
            .get(&plan.id)?
            .is_some_and(|record| record.status != "complete")
        {
            engine.store.fail(&plan.id, error, true)?;
        }
    }
    outcome
}
pub fn recover(engine: &mut Engine, plan: &Plan, gate: WriteGate) -> Result<()> {
    let evidence = engine
        .store
        .evidence(&plan.id)?
        .ok_or_else(|| Error::conflict("缺少删除恢复材料"))?;
    let mut signed = plan.clone();
    signed.version.clear();
    if hash(&signed) != plan.version
        || evidence["schema"] != "staging-delete-v1"
        || evidence["items"] != serde_json::to_value(items(plan)?)?
    {
        return Err(Error::conflict("不可逆删除恢复材料与原计划不一致"));
    }
    run(engine, plan, gate, true).map(|_| ())
}
fn run(engine: &mut Engine, plan: &Plan, gate: WriteGate, recovering: bool) -> Result<Value> {
    engine.require_resolved_mutation(&plan.id, None)?;
    let entries = items(plan)?;
    for (index, item) in entries.iter().enumerate() {
        let latest = engine.store.latest_file_step(&plan.id, index)?;
        if latest.as_ref().is_some_and(|s| s["state"] == "complete") {
            continue;
        }
        if latest.as_ref().is_some_and(|s| s["state"] == "failed") {
            let result = finished(engine, plan)?;
            gate.release()?;
            return Ok(result);
        }
        if recovering && latest.is_none() {
            // Never start a later item after interrupted batch.
            let result = finished(engine, plan)?;
            gate.release()?;
            return Ok(result);
        }
        if let Err(cause) = one(engine, plan, index, item, recovering) {
            let sub = subplan(plan, index);
            let receipt =
                ledger::inspect_delta(&engine.config, &sub, item.domain.ledger.as_ref().unwrap());
            let unlink = engine
                .store
                .latest_step(&plan.id, &format!("unlink_intent_{index}"))?;
            if unlink.is_none()
                && matches!(receipt.as_deref(), Ok("absent" | "prepared" | "cancelled"))
            {
                ledger::cancel_delta(&engine.config, &sub, item.domain.ledger.as_ref().unwrap())?;
                event(
                    engine,
                    plan,
                    index,
                    "file_failed",
                    json!({"state":"failed","error":cause,"bytes_preserved":true}),
                )?;
                let result = finished(engine, plan)?;
                gate.release()?;
                return Ok(result);
            }
            let error = Error::new(
                503,
                "recovery_required",
                format!(
                    "{}；不可逆删除结果需要核对，已删除内容不能回退",
                    cause.error
                ),
            )
            .at(&plan.files[index].src)
            .during(cause.step.unwrap_or_else(|| "staging_delete".into()));
            let error = if cause.mutation_outcome.is_some() {
                error.ambiguous_mutation()
            } else {
                error
            };
            engine.store.fail(&plan.id, &error, true)?;
            return Err(error);
        }
    }
    let result = finished(engine, plan)?;
    crate::fault::checkpoint(&engine.config, "delete_result_committed")?;
    gate.release()?;
    Ok(result)
}
fn one(
    engine: &mut Engine,
    plan: &Plan,
    index: usize,
    item: &Item,
    recovering: bool,
) -> Result<()> {
    let sub = subplan(plan, index);
    let file = &plan.files[index];
    let delta = item.domain.ledger.as_ref().unwrap();
    let unlink_intent = engine
        .store
        .latest_step(&plan.id, &format!("unlink_intent_{index}"))?
        .is_some();
    let removed = engine
        .store
        .latest_step(&plan.id, &format!("unlink_done_{index}"))?
        .is_some();
    let pinned = PinnedDelete::open(&engine.config, &file.src, Some(&item.proof["parent"]))?;
    if removed && pinned.present() {
        return Err(Error::conflict("已删除路径被重新占用，不能重删"));
    }
    if unlink_intent && !removed && pinned.present() {
        return Err(Error::new(
            503,
            "outcome_unknown",
            "删除系统调用结果未确认，禁止自动重放",
        ));
    }
    if !unlink_intent {
        pinned.verify(&item.proof)?;
    }
    let state = ledger::inspect_delta(&engine.config, &sub, delta)?;
    if state == "cancelled" {
        return Err(Error::conflict("此项账本意图已取消"));
    }
    if !unlink_intent {
        event(
            engine,
            plan,
            index,
            "delete_item_intent",
            json!({"state":"running","proof":item.proof,"domain":item.domain}),
        )?;
        crate::fault::checkpoint(&engine.config, "delete_item_intent")?;
    }
    if state == "absent" {
        ledger::reserve_delta(&engine.config, &sub, delta)?;
    }
    crate::fault::checkpoint(&engine.config, "delete_ledger_reserved")?;
    if state != "committed" {
        ledger::apply_delta(&engine.config, &sub, delta)?;
    }
    crate::fault::checkpoint(&engine.config, "delete_ledger_committed")?;
    // Check both stores before unlink. Their writes are forward-only after the byte boundary.
    metadata::inspect_domain(&engine.config, &sub, &item.domain.metadata, false)?;
    if item.domain.metadata.jobs_delta.is_some() {
        metadata::inspect_domain(&engine.config, &sub, &item.domain.metadata, true)?;
    }
    for doc in &item.domain.metadata.documents {
        inspect_document(&engine.config, file, doc)?;
    }
    if !unlink_intent {
        pinned.verify(&item.proof)?;
        engine.store.step(
            &plan.id,
            "running",
            &format!("unlink_intent_{index}"),
            json!({"file_index":index,"src":file.src,"state":"running","proof":item.proof}),
        )?;
        crate::fault::checkpoint(&engine.config, "delete_before_unlink")?;
        let config = engine.config.clone();
        pinned.unlink(&config, &item.proof, |phase| {
            event(
                engine,
                plan,
                index,
                phase,
                json!({"state":"running","step":"staging_unlink"}),
            )
        })?;
        crate::fault::checkpoint(&engine.config, "delete_after_unlink")?;
    } else if !removed && !recovering {
        return Err(Error::conflict("删除意图需要恢复核对"));
    }
    if !removed {
        engine.store.step(&plan.id,"running",&format!("unlink_done_{index}"),json!({"file_index":index,"src":file.src,"state":"deleted_pending_records","bytes_deleted":true}))?;
    }
    if !metadata::inspect_domain(&engine.config, &sub, &item.domain.metadata, false)? {
        metadata::apply_faces(&engine.config, &sub, &item.domain.metadata)?;
    }
    crate::fault::checkpoint(&engine.config, "delete_faces_committed")?;
    if item.domain.metadata.jobs_delta.is_some()
        && !metadata::inspect_domain(&engine.config, &sub, &item.domain.metadata, true)?
    {
        metadata::apply_jobs(&engine.config, &sub, &item.domain.metadata)?;
    }
    crate::fault::checkpoint(&engine.config, "delete_jobs_committed")?;
    for doc in &item.domain.metadata.documents {
        apply_document(&engine.config, file, doc)?;
        crate::fault::checkpoint(&engine.config, "delete_json_replaced")?;
    }
    ledger::inspect_delta(&engine.config, &sub, delta)?;
    event(
        engine,
        plan,
        index,
        "delete_item_complete",
        json!({"state":"complete","bytes_deleted":true}),
    )?;
    crate::fault::checkpoint(&engine.config, "delete_item_complete")?;
    Ok(())
}

pub fn status(connection: &rusqlite::Connection, record: &crate::store::Record) -> Result<Value> {
    use rusqlite::OptionalExtension;
    let mut rows = vec![];
    let mut done = 0;
    let mut failed = 0;
    for (index, file) in record.plan.files.iter().enumerate() {
        let raw:Option<String>=connection.query_row("SELECT evidence FROM steps WHERE operation_id=? AND json_extract(evidence,'$.file_index')=? ORDER BY revision DESC LIMIT 1",rusqlite::params![record.plan.id,index as i64],|r|r.get(0)).optional()?;
        let step: Value = raw
            .as_ref()
            .map(|s| serde_json::from_str(s))
            .transpose()?
            .unwrap_or(Value::Null);
        let state = step["state"].as_str().unwrap_or("not_started");
        if state == "complete" {
            done += 1;
        }
        if state == "failed" {
            failed += 1;
        }
        rows.push(json!({"file_index":index,"src":file.src,"status":if record.status=="recovery_needed" && !["complete","failed","not_started"].contains(&state){"recovery_needed"}else{state},"error":step["error"],"bytes_deleted":step["bytes_deleted"],"irreversible":true}));
    }
    Ok(
        json!({"id":record.plan.id,"action":record.plan.action,"status":record.status,"phase":record.phase,"revision":record.revision,"io_state":if record.status=="recovery_needed"{"unknown"}else if ["complete","not_started","cancelled"].contains(&record.status.as_str()){"idle"}else{"running"},"completed":done,"committed":done,"total":record.plan.files.len(),"failed":failed,"files":rows,"result":record.result,"error":record.error.as_ref().map(|e|&e.error),"failure":record.error,"irreversible":true}),
    )
}

/// Read old receipts with the same no-follow, bounded control reader; never replay a legacy writer.
pub fn historical_status(config: &Config, id: &str) -> Result<Value> {
    model::validate_id(id)?;
    let (raw, value) = registration::document(
        config,
        &format!("staging-operations/{id}/operation.json"),
        false,
    )?;
    if raw.is_some() {
        return Ok(
            json!({"id":id,"legacy":true,"status":value["result"]["status"].as_str().or(value["state"].as_str()),"result":value["result"],"total":value["plan"]["rows"].as_array().map(Vec::len)}),
        );
    }
    match crate::store::status(&config.state_db(), id) {
        Err(e) if e.status == 404 => {
            let (raw, old) = registration::document(
                config,
                &format!("person-operations/{id}/operation.json"),
                false,
            )?;
            if raw.is_none() {
                return Err(e);
            }
            Ok(json!({"id":id,"legacy":true,"status":old["status"],"result":old["result"]}))
        }
        result => result,
    }
}
