//! A batch is durable orchestration over independent person transactions.
//! Parent intent, child intents, progress and receipts share the sole operation store.
use crate::{
    config::Config,
    engine::{self, Engine},
    files::{self, WriteGate},
    model::{AffectedStores, Error, ExecutionIntent, FailurePolicy, Plan, Request, Result, hash},
    native, person_directory, person_registration,
    store::{self, Store},
};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const ACTION: &str = "person-batch";
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Selection {
    action: String,
    people: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    target: Option<String>,
}
fn children(parent: &Plan) -> Result<Vec<Plan>> {
    if parent.action != ACTION {
        return Err(Error::conflict("操作编号不属于人物批次"));
    }
    Ok(serde_json::from_value(
        parent
            .execution
            .as_ref()
            .ok_or_else(|| Error::conflict("批次意图缺失"))?
            .data["plans"]
            .clone(),
    )?)
}
fn selection(parent: &Plan) -> Result<Selection> {
    Ok(serde_json::from_value(
        parent
            .request
            .payload
            .clone()
            .ok_or_else(|| Error::conflict("批次范围缺失"))?,
    )?)
}
/// Child IDs cannot be used through standalone preview/apply/reconcile routes.
/// The immutable parent records the exact original request, not only a display label.
pub fn reject_child(store: &Store, id: &str) -> Result<()> {
    let parent: Option<String> = store.db.query_row(
        "SELECT o.id FROM operations o, json_each(json_extract(o.plan,'$.execution.data.plans')) c WHERE json_extract(o.plan,'$.action')='person-batch' AND json_extract(c.value,'$.id')=? LIMIT 1",
        [id], |r| r.get(0)).optional()?;
    if parent.is_some() {
        return Err(Error::conflict("此子操作由原批次管理，请查询或提交原批次"));
    }
    Ok(())
}
fn canonical(config: &Config, input: Selection) -> Result<Selection> {
    if !["merge", "delete"].contains(&input.action.as_str())
        || input.people.is_empty()
        || input.people.len() > 50
    {
        return Err(Error::new(400, "invalid_request", "请选择 1–50 个人物"));
    }
    let people = input
        .people
        .iter()
        .map(|name| {
            files::validate_name(name)?;
            person_registration::selected(config, name)
        })
        .collect::<Result<Vec<_>>>()?;
    let unique = people.iter().collect::<std::collections::BTreeSet<_>>();
    if unique.len() != people.len() || (input.action == "merge" && people.len() < 2) {
        return Err(Error::new(
            400,
            "invalid_request",
            "合并至少选择两位不同人物，删除至少选择一位",
        ));
    }
    let target = if input.action == "merge" {
        let name = input
            .target
            .ok_or_else(|| Error::new(400, "invalid_request", "请选择保留人物"))?;
        files::validate_name(&name)?;
        let name = person_registration::canonical(config, &name)?;
        if !people.contains(&name) {
            return Err(Error::new(
                400,
                "invalid_request",
                "保留人物必须从已选人物中选择",
            ));
        }
        Some(name)
    } else {
        if input.target.is_some() {
            return Err(Error::new(400, "invalid_request", "删除不接受保留人物"));
        }
        None
    };
    Ok(Selection {
        action: input.action,
        people,
        target,
    })
}
fn presentation(config: &Config, plan: &Plan) -> Result<Value> {
    let plans = children(plan)?
        .iter()
        .map(|child| {
            let mut value = json!(child);
            value["target_directory"] = json!(person_directory::roots(config, child).1);
            value
        })
        .collect::<Vec<_>>();
    Ok(
        json!({"id":plan.id,"version":plan.version,"request":selection(plan)?,"plans":plans,"backend":"rust"}),
    )
}
fn preview(engine: &mut Engine, body: Value) -> Result<Value> {
    let id = body["id"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    crate::model::validate_id(&id)?;
    let input: Selection = serde_json::from_value(body["request"].clone())?;
    // A saved request remains retrievable after identities change as a result of its execution.
    if let Some(existing) = engine.store.get(&id)? {
        children(&existing.plan)?;
        if serde_json::to_value(input)? != existing.plan.request.payload.clone().unwrap() {
            return Err(Error::conflict("同一批次编号不能更换请求"));
        }
        return presentation(&engine.config, &existing.plan);
    }
    let input = canonical(&engine.config, input)?;
    let mut plans = vec![];
    for person in input
        .people
        .iter()
        .filter(|p| Some(*p) != input.target.as_ref())
    {
        let request = Request {
            id: uuid::Uuid::new_v4().to_string(),
            action: if input.action == "merge" {
                "merge-person"
            } else {
                "delete-person"
            }
            .into(),
            person: person.clone(),
            files: vec![],
            filename: String::new(),
            target_person: input.target.clone(),
            target_folder: None,
            payload: None,
        };
        plans.push(native::plan(&engine.config, request)?);
    }
    let request = Request {
        id: id.clone(),
        action: ACTION.into(),
        person: String::new(),
        files: vec![],
        filename: String::new(),
        target_person: input.target.clone(),
        target_folder: None,
        payload: Some(json!(input)),
    };
    let mut parent = Plan {
        id: id.clone(),
        request,
        version: String::new(),
        action: ACTION.into(),
        person: String::new(),
        target_person: input.target.unwrap_or_default(),
        files: vec![],
        total_bytes: plans.iter().map(|p| p.total_bytes).sum(),
        directories: vec![],
        ledger_version: String::new(),
        metadata_version: String::new(),
        execution: Some(ExecutionIntent {
            scopes: vec![],
            failure_policy: FailurePolicy::Rollback,
            stores: AffectedStores {
                faces: false,
                jobs: false,
                ledger: false,
            },
            data: json!({"plans":plans}),
            result: json!({}),
        }),
    };
    if serde_json::to_vec(&parent)?.len() > 64 * 1024 * 1024 {
        return Err(Error::new(413, "scope_too_large", "批次预览超过 64 MiB"));
    }
    parent.version = hash(&parent);
    files::claim_owner_for(&engine.config, &id, ACTION)?;
    for child in &plans {
        files::claim_owner_for(&engine.config, &child.id, &child.action)?;
    }
    // Persist all bindings atomically. No child can become independently executable.
    let tx = engine.store.db.transaction()?;
    for plan in std::iter::once(&parent).chain(plans.iter()) {
        tx.execute("INSERT INTO operations(id,request_hash,plan,status,phase) VALUES(?,?,?,'not_started','preview')",params![plan.id,hash(&plan.request),serde_json::to_string(plan)?])?;
    }
    tx.commit()?;
    presentation(&engine.config, &parent)
}
fn verify_original(engine: &Engine, plan: &Plan) -> Result<()> {
    native::revalidate(&engine.config, plan)?;
    let mut current = plan.clone();
    engine::seal_plan(&engine.config, &mut current)?;
    if current.version != plan.version {
        return Err(Error::conflict("人物、文件或账本已变化，请重新预览"));
    }
    Ok(())
}
/// Preserve the source scope and its identities while allowing earlier successful
/// people to change target collisions, aliases, and shared metadata.
fn source_proof(plan: &Plan) -> Result<Value> {
    let inventory =
        person_directory::intent(plan)?.ok_or_else(|| Error::conflict("来源目录证据缺失"))?;
    Ok(
        json!({"person":plan.person,"files":plan.files.iter().map(|f|json!([f.src,f.version,f.vids])).collect::<Vec<_>>(),"directories":inventory.directories.iter().map(|d|json!([d.src,d.identity,d.parent])).collect::<Vec<_>>() }),
    )
}
fn result(db: &rusqlite::Connection, parent: &Plan, error: Option<&str>) -> Result<Value> {
    let mut completed = vec![];
    let mut remaining = vec![];
    let mut recovery = false;
    let mut child_error = None;
    for original in children(parent)? {
        let child = store::record(db, &original.id)?
            .ok_or_else(|| Error::conflict("批次子操作记录缺失"))?;
        if child.request_hash != hash(&original.request) {
            return Err(Error::conflict("批次子操作请求不匹配"));
        }
        if child.status == "complete" {
            let receipt = child
                .result
                .ok_or_else(|| Error::conflict("已完成子操作缺少回执"))?;
            completed.push(json!({"person":original.person,"result":receipt}));
        } else {
            remaining.push(original.person);
            recovery |= ["accepted", "running", "recovery_needed", "rolling_back"]
                .contains(&child.status.as_str());
            if child_error.is_none() {
                child_error = child.error.map(|e| e.error);
            }
        }
    }
    let mut value = json!({"ok":remaining.is_empty(),"completed":completed,"remaining":remaining,"target":selection(parent)?.target});
    if !value["ok"].as_bool().unwrap() {
        if let Some(message) = child_error.as_deref().or(error) {
            value["error"] = json!(message);
        }
        if recovery {
            value["recovery_needed"] = json!(true);
        }
    }
    Ok(value)
}
fn finish(engine: &mut Engine, parent: &Plan, error: Option<&str>) -> Result<Value> {
    let mut value = result(&engine.store.db, parent, error)?;
    // A failure before child admission may still leave its gate durable. Never
    // present this as an ordinary compensated failure or silently clear that gate.
    match crate::control::read(
        &engine.config.base.join(".review-write.lock"),
        crate::control::MARKER_LIMIT,
    ) {
        Ok(Some(raw)) => match serde_json::from_str::<Value>(&raw) {
            Ok(marker) => {
                if children(parent)?
                    .iter()
                    .any(|p| marker["operation"] == p.id)
                {
                    value["recovery_needed"] = json!(true);
                }
            }
            Err(_) => {
                value["recovery_needed"] = json!(true);
            }
        },
        Err(_) => {
            value["recovery_needed"] = json!(true);
        }
        Ok(None) => (),
    }
    engine.store.complete(parent, &value)?;
    Ok(value)
}
fn apply(engine: &mut Engine, body: Value) -> Result<Value> {
    let id = body["id"]
        .as_str()
        .ok_or_else(|| Error::new(400, "invalid_request", "缺少批次编号"))?;
    crate::model::validate_id(id)?;
    let record = engine
        .store
        .get(id)?
        .ok_or_else(|| Error::new(404, "not_found", "找不到此批次"))?;
    let parent = record.plan;
    let plans = children(&parent)?;
    if body["version"] != parent.version {
        return Err(Error::conflict("预览已失效，请重新预览"));
    }
    if let Some(saved) = record.result {
        return Ok(saved);
    }
    if record.status != "not_started" {
        return Err(Error::new(
            503,
            "recovery_required",
            "此批次已受理，请查询原回执",
        ));
    }
    if engine.store.blocked()? {
        return Err(Error::new(
            503,
            "recovery_required",
            "存在未核对操作，已暂停新写入",
        ));
    }
    // Acquire the FIRST CHILD's gate. Pass it directly into the native executor;
    // never hold a parent gate while asking a child to acquire another one.
    let gate = WriteGate::acquire(&engine.config, &plans[0].id)?;
    engine
        .store
        .remember_gate(&plans[0].id, &gate.evidence()?)?;
    let checked = plans.iter().try_for_each(|p| verify_original(engine, p));
    if let Err(error) = checked {
        gate.release()?;
        return Err(error);
    }
    engine.store.step(
        id,
        "batch_running",
        "batch_validated",
        json!({"children":plans.iter().map(|p|&p.id).collect::<Vec<_>>()}),
    )?;
    crate::fault::checkpoint(&engine.config, "batch_validated")?;
    let mut first_gate = Some(gate);
    for (index, original) in plans.iter().enumerate() {
        engine.store.step(
            id,
            "batch_running",
            "batch_child",
            json!({"child":original.id,"active_person":original.person,"index":index}),
        )?;
        let gate = if let Some(gate) = first_gate.take() {
            gate
        } else {
            match WriteGate::acquire(&engine.config, &original.id) {
                Ok(gate) => gate,
                Err(e) => return finish(engine, &parent, Some(&e.error)),
            }
        };
        engine
            .store
            .remember_gate(&original.id, &gate.evidence()?)?;
        let prepared = (|| -> Result<Plan> {
            crate::fault::checkpoint(&engine.config, &format!("batch_before_child:{index}"))?;
            let current = native::plan(&engine.config, original.request.clone())?;
            if source_proof(original)? != source_proof(&current)? {
                return Err(Error::conflict(
                    "来源文件在批量操作期间变化，已停止剩余操作",
                ));
            }
            let changed=engine.store.db.execute("UPDATE operations SET plan=?,revision=revision+1 WHERE id=? AND status='not_started' AND request_hash=?",params![serde_json::to_string(&current)?,original.id,hash(&original.request)])?;
            if changed != 1 {
                return Err(Error::conflict("批次子操作已受理，不能重复执行"));
            }
            Ok(current)
        })();
        let child = match prepared {
            Ok(plan) => plan,
            Err(e) => {
                gate.release()?;
                return finish(engine, &parent, Some(&e.error));
            }
        };
        crate::fault::checkpoint(&engine.config, &format!("batch_child_prepared:{index}"))?;
        let outcome = engine.apply_under_gate(child.clone(), child.version, gate);
        // A child commit may be durable even if the parent did not receive it.
        crate::fault::checkpoint(&engine.config, &format!("batch_child_returned:{index}"))?;
        if let Err(e) = outcome {
            return finish(engine, &parent, Some(&e.error));
        }
        engine.store.step(
            id,
            "batch_running",
            "batch_child_complete",
            json!({"child":original.id,"person":original.person,"index":index}),
        )?;
        crate::fault::checkpoint(&engine.config, &format!("batch_child_recorded:{index}"))?;
    }
    finish(engine, &parent, None)
}
fn reconcile(engine: &mut Engine, body: Value) -> Result<Value> {
    let id = body["id"]
        .as_str()
        .ok_or_else(|| Error::new(400, "invalid_request", "缺少批次编号"))?;
    crate::model::validate_id(id)?;
    let record = engine
        .store
        .get(id)?
        .ok_or_else(|| Error::new(404, "not_found", "找不到此批次"))?;
    let plans = children(&record.plan)?;
    if body["version"] != record.plan.version {
        return Err(Error::conflict("核对范围与原批次不一致"));
    }
    if record.status == "not_started" {
        return Err(Error::conflict("批次尚未执行，请使用原预览确认"));
    }
    for original in plans {
        if let Some(child) = engine.store.get(&original.id)? {
            if child.request_hash != hash(&original.request) {
                return Err(Error::conflict("批次子操作请求不匹配"));
            }
            if ["recovery_needed", "rolling_back"].contains(&child.status.as_str()) {
                engine.recover_one(&original.id);
            }
        }
    }
    finish(engine, &record.plan, Some("未完成范围请重新预览"))
}
pub fn execute(engine: &mut Engine, body: Value) -> Result<Value> {
    match body["mode"].as_str() {
        Some("preview") => preview(engine, body),
        Some("apply") => apply(engine, body),
        Some("reconcile") => reconcile(engine, body),
        _ => Err(Error::new(400, "invalid_request", "请选择预览或执行操作")),
    }
}
pub fn progress(config: &Config, id: &str) -> Result<Value> {
    crate::model::validate_id(id)?;
    let db = crate::db::open(&config.state_db())?;
    let record =
        store::record(&db, id)?.ok_or_else(|| Error::new(404, "not_found", "找不到此批次"))?;
    let parent = record.plan;
    children(&parent)?;
    let value = if let Some(saved) = record.result {
        saved
    } else {
        result(&db, &parent, None)?
    };
    let active:Option<String>=db.query_row("SELECT json_extract(evidence,'$.active_person') FROM steps WHERE operation_id=? AND phase='batch_child' ORDER BY revision DESC LIMIT 1",[id],|r|r.get(0)).optional()?;
    let status = if record.status == "not_started" {
        "preview"
    } else if record.status == "complete" {
        "complete"
    } else {
        "running"
    };
    let mut response = json!({"id":id,"status":status,"completed":value["completed"].as_array().unwrap().iter().map(|v|v["person"].clone()).collect::<Vec<_>>(),"remaining":value["remaining"],"error":value["error"],"recovery_needed":value["recovery_needed"]==true});
    if status == "running" {
        response["active_person"] = json!(active);
    }
    if status == "complete" {
        response["result"] = value;
    }
    Ok(response)
}
/// Child startup recovery has already run under the lifetime executor lock.
/// Fold committed receipts, never redo successful people or auto-start remaining ones.
pub fn recover(engine: &mut Engine) -> Result<()> {
    let ids = {
        let mut statement=engine.store.db.prepare("SELECT id FROM operations WHERE json_extract(plan,'$.action')='person-batch' AND (status='batch_running' OR json_extract(result,'$.recovery_needed')=1)")?;
        statement
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?
    };
    for id in ids {
        let parent = engine.store.get(&id)?.unwrap().plan;
        finish(
            engine,
            &parent,
            Some("执行器重启，已保留完成回执；未完成范围请重新预览"),
        )?;
    }
    Ok(())
}
