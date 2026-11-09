//! Public compatibility presentation; all writes run on the existing Engine worker.
use crate::{
    archive,
    config::Config,
    db,
    engine::Engine,
    model::{self, Error, Request, Result},
    store,
};
use rusqlite::OptionalExtension;
use serde_json::{Value, json};

fn request(id: String, action: &str, payload: Value) -> Request {
    Request {
        id,
        action: action.into(),
        person: String::new(),
        files: vec![],
        filename: String::new(),
        target_person: None,
        target_folder: None,
        payload: Some(payload),
    }
}
pub fn execute(engine: &mut Engine, command: &str, mut body: Value) -> Result<Value> {
    if !body.is_object() {
        return Err(Error::new(400, "invalid_request", "操作请求格式无效"));
    }
    if ["review-page", "review-status"].contains(&command) {
        return crate::review_sessions::execute(engine, command, body);
    }
    if command == "archive-preview" {
        return archive::preview(&engine.config);
    }
    if command == "archive-reconcile" {
        let id = body["id"]
            .as_str()
            .ok_or_else(|| Error::new(400, "invalid_request", "缺少操作编号"))?
            .to_owned();
        model::validate_id(&id)?;
        let (request, version) = match engine.store.get(&id)? {
            Some(record) => {
                if record.plan.action != "archive" {
                    return Err(Error::conflict("编号属于其他操作"));
                }
                (record.plan.request, Some(record.plan.version))
            }
            None => (request(id.clone(), "archive", json!({})), None),
        };
        engine.reconcile_request(request, version)?;
        return progress(&engine.config, Some(&id));
    }
    let action = match command {
        "archive-apply" => "archive",
        "archive-resolve" => "archive-resolve",
        "queue-confirm" => "queue-confirm",
        "review-note" => "review-note",
        "review-defer" => "review-defer",
        "review-restore" => "review-restore",
        "queue-undo" => "queue-undo",
        "claim-decide" => "claim-decide",
        _ => return Err(Error::new(400, "invalid_request", "未知人工确认操作")),
    };
    let id = match body.get("operation_id").or(body.get("id")) {
        Some(value) => value
            .as_str()
            .ok_or_else(|| Error::new(400, "invalid_request", "操作编号无效"))?
            .to_owned(),
        None => uuid::Uuid::new_v4().to_string(),
    };
    model::validate_id(&id)?;
    let obj = body.as_object_mut().unwrap();
    obj.remove("operation_id");
    obj.remove("id");
    obj.remove("at");
    if let Some(ids) = body.get_mut("vids") {
        let mut values = crate::review::vids(ids)?;
        if action == "archive" {
            values.sort();
        }
        *ids = json!(values);
    }
    let plan = if let Some(record) = engine.store.get(&id)? {
        let mut previous = record.plan.request.payload.clone().unwrap_or(Value::Null);
        if let Some(obj) = previous.as_object_mut() {
            obj.remove("at");
        }
        if record.plan.action != action || previous != body {
            return Err(Error::conflict("同一操作编号不能更换范围或人工决定"));
        }
        record.plan
    } else {
        body["at"] = json!(engine.store.db.query_row(
            "SELECT strftime('%Y-%m-%dT%H:%M:%fZ','now')",
            [],
            |r| r.get::<_, String>(0)
        )?);
        serde_json::from_value(engine.preview(request(id.clone(), action, body))?)?
    };
    match engine.apply(plan.request.clone(), plan.version.clone()) {
        Ok(result) => {
            if ["queue-confirm", "queue-undo"].contains(&action) {
                optional_rule(engine, &plan, result)
            } else {
                Ok(result)
            }
        }
        Err(error) if action == "archive" => {
            if let Some(record) = engine.store.get(&id)?
                && ["rolled_back", "recovery_needed", "failed"].contains(&record.status.as_str())
                && record.status != "not_started"
            {
                let cause = record
                    .error
                    .as_ref()
                    .map(|e| e.error.as_str())
                    .unwrap_or(&error.error);
                let result = failure_result(
                    &record.plan,
                    &record.status,
                    cause,
                    &engine.store.failures(&id)?,
                    &retained_indexes(&engine.store.db, &id)?,
                );
                // Keep the terminal operation state; only its compatibility receipt is published.
                // An uncertain/recovery-needed operation must never gain a final result that
                // could short-circuit the common executor's reconciliation.
                if ["rolled_back", "failed"].contains(&record.status.as_str()) {
                    let changed=engine.store.db.execute("UPDATE operations SET result=?,revision=revision+1,updated_at=CURRENT_TIMESTAMP WHERE id=? AND status=? AND result IS NULL",rusqlite::params![serde_json::to_string(&result)?,id,record.status])?;
                    if changed != 1 {
                        return Err(Error::new(503, "outcome_unknown", "归档回执发布结果待核对"));
                    }
                }
                return Ok(result);
            }
            Err(error)
        }
        Err(error) => Err(error),
    }
}
fn failure_result(
    plan: &crate::model::Plan,
    status: &str,
    error: &str,
    skipped: &[Value],
    retained: &[Value],
) -> Value {
    // All accepted files are still unfinished after an aggregate failure. Keep the
    // original per-file skip reason instead of replacing it with the ledger error.
    let mut result = archive::result(plan, &[], skipped);
    let mut failures = skipped.to_vec();
    for (index, file) in plan.files.iter().enumerate() {
        if !skipped
            .iter()
            .any(|failure| failure["file_index"].as_u64() == Some(index as u64))
        {
            let message = if status == "rolled_back" {
                format!("本轮已执行的变更已回退：{error}")
            } else if status == "recovery_needed" {
                format!("结果待核对，暂不能重试：{error}")
            } else {
                error.to_owned()
            };
            failures.push(json!({"file_index":index,"src":file.src,"dst":file.dst,"error":{"error":message,"code":if status=="rolled_back"{"archive_rolled_back"}else{"archive_incomplete"},"step":status}}));
        }
    }
    result["failures"] = archive::result(plan, &[], &failures)["failures"].clone();
    let mut seen = std::collections::BTreeSet::new();
    if let Some(rows) = result["failures"].as_array_mut() {
        rows.retain(|row| seen.insert(row["vid"].as_str().unwrap_or("").to_owned()));
        for row in rows {
            row["retryable"] = json!(status != "recovery_needed" && retained.is_empty());
        }
    }
    let errors = result["errors"].as_array_mut().unwrap();
    if !errors.iter().any(|message| message == error) {
        errors.push(json!(error));
    }
    result["ok"] = json!(false);
    result["rolled_back"] = json!(status == "rolled_back");
    result["recovery_needed"] = json!(status == "recovery_needed");
    if !retained.is_empty() {
        result["external_index_conflicts"] = json!(retained.iter().map(|row| json!({
            "asset_id":row["id"]["$integer"], "path":row["path"], "disposition":"retained_unchanged"
        })).collect::<Vec<_>>());
        result["own_changes_rolled_back"] = json!(status == "rolled_back");
    }
    result
}
fn retained_indexes(db: &rusqlite::Connection, id: &str) -> Result<Vec<Value>> {
    let raw: Option<String> = db.query_row("SELECT evidence FROM steps WHERE operation_id=? AND phase='archive_external_indexes_retained' ORDER BY revision DESC LIMIT 1", [id], |r|r.get(0)).optional()?;
    let Some(raw) = raw else {
        return Ok(vec![]);
    };
    let value: Value = serde_json::from_str(&raw)?;
    serde_json::from_value(value["rows"].clone()).map_err(Into::into)
}
fn journal_failures(db: &rusqlite::Connection, id: &str) -> Result<Vec<Value>> {
    db::rows(
        db,
        "SELECT evidence FROM steps WHERE operation_id=? AND phase='file_failed' ORDER BY revision",
        [id],
    )?
    .into_iter()
    .map(|row| {
        Ok(serde_json::from_str(
            row["evidence"]
                .as_str()
                .ok_or_else(|| Error::conflict("文件失败回执格式无效"))?,
        )?)
    })
    .collect()
}
pub fn progress(config: &Config, id: Option<&str>) -> Result<Value> {
    let db = db::open(&config.state_db())?;
    let id=match id {Some(id)=>{model::validate_id(id)?;Some(id.to_owned())},None=>db.query_row("SELECT id FROM operations WHERE json_extract(plan,'$.action')='archive' ORDER BY created_at DESC,rowid DESC LIMIT 1",[],|r|r.get::<_,String>(0)).optional()?};
    let Some(id) = id else {
        return Ok(json!({"job":null}));
    };
    let status = match store::status(&config.state_db(), &id) {
        Ok(status) => status,
        Err(error) if error.status == 404 => return Ok(json!({"job":null})),
        Err(error) => return Err(error),
    };
    let (plan, created, updated): (Option<String>, Option<f64>, Option<f64>) = db
        .query_row(
            "SELECT o.plan,unixepoch(o.created_at),unixepoch(COALESCE((SELECT s.at FROM steps s WHERE s.operation_id=o.id ORDER BY s.revision DESC LIMIT 1),o.updated_at)) FROM operations o WHERE o.id=?",
            [&id],
            |r| Ok((Some(r.get(0)?), r.get(1)?, r.get(2)?)),
        )
        .optional()?
        .unwrap_or((None, None, None));
    let plan = plan
        .map(|p| serde_json::from_str::<crate::model::Plan>(&p))
        .transpose()?;
    if plan.as_ref().is_some_and(|p| p.action != "archive") {
        return Err(Error::conflict("编号属于其他操作"));
    }
    let state = status["status"].as_str().unwrap_or("unknown");
    let phase = status["phase"].as_str().unwrap_or("unknown");
    let terminal = ["complete", "failed", "rolled_back", "cancelled"].contains(&state);
    let moving = phase.starts_with("file_")
        || phase.starts_with("destination_")
        || phase.starts_with("source_");
    let partial = state == "complete" && status["result"]["ok"] == false;
    let current = if phase == "duplicate_verification_intent" {
        db.query_row("SELECT json_extract(evidence,'$.src') FROM steps WHERE operation_id=? AND phase='duplicate_verification_intent' ORDER BY revision DESC LIMIT 1",[&id],|r|r.get::<_,String>(0)).optional()?.map(Value::String).unwrap_or_else(|| status["current_file"].clone())
    } else {
        status["current_file"].clone()
    };
    let title = if phase.starts_with("duplicate_") {
        "校验重复文件"
    } else if partial {
        "部分归档完成"
    } else if state == "complete" {
        "归档完成"
    } else if state == "rolled_back" {
        "本轮已移动的文件已恢复"
    } else if state == "recovery_needed" {
        "需要核对恢复"
    } else if phase.contains("ledger") {
        "同步 Peach 账本"
    } else if phase.contains("json") || phase.contains("faces") || phase.contains("jobs") {
        "保存归档记录"
    } else if moving {
        "移动文件"
    } else if state == "cancelled" {
        "未开始，已停止受理"
    } else {
        "核对归档"
    };
    let mut out = json!({"job":{"id":id,"kind":"archive","title":"归档视频","status":if state=="complete"&&!partial{"complete"}else if terminal||state=="recovery_needed"{"error"}else{"running"},"stage":title,"current":current,"done":if moving{status["completed"].clone()}else{Value::Null},"total":if moving{status["total"].clone()}else{Value::Null},"unit":if moving{"个文件"}else{""},"failed":status["failed"],"started_at":created,"updated_at":updated,"heartbeat_at":updated,"href":"/integration","error":status["error"]}});
    if let Some(result) = status.get("result") {
        out["result"] = result.clone()
    } else if let Some(plan) = &plan
        && ["rolled_back", "failed", "recovery_needed"].contains(&state)
    {
        out["result"] = failure_result(
            plan,
            state,
            status["error"].as_str().unwrap_or("归档未完成"),
            &journal_failures(&db, &id)?,
            &retained_indexes(&db, &id)?,
        );
    }
    if state == "cancelled" {
        out["result"] = json!({"ok":false,"moved":0,"remaining":crate::review::read(config,"integration-queue.json",json!([]))?.1.as_array().unwrap().len(),"errors":["请求未开始，已停止受理"]})
    }
    Ok(out)
}

fn optional_rule(
    engine: &mut Engine,
    parent: &crate::model::Plan,
    mut result: Value,
) -> Result<Value> {
    let (field, warning, action, prefix) = if parent.action == "queue-undo" {
        (
            "optional_restore",
            "warning",
            "queue-restore",
            "归名已撤销，分析状态未确认恢复",
        )
    } else {
        (
            "optional_rule",
            "rule_warning",
            "queue-rule",
            "归名已保存，账号规则未确认保存",
        )
    };
    let Some(payload) = parent
        .execution
        .as_ref()
        .and_then(|e| e.data.get(field))
        .filter(|v| !v.is_null())
    else {
        return Ok(result);
    };
    if result.get(warning).is_none() {
        return Ok(result);
    }
    crate::fault::checkpoint(&engine.config, "optional_parent_committed")?;
    let digest = model::hash(&json!([parent.id, action]));
    let mut bytes = [0_u8; 16];
    for (i, b) in bytes.iter_mut().enumerate() {
        *b = u8::from_str_radix(&digest[i * 2..i * 2 + 2], 16).unwrap()
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let id = uuid::Uuid::from_bytes(bytes).to_string();
    let requested = request(id.clone(), action, payload.clone());
    if engine.store.fenced(&id)?
        && result[warning]
            .as_str()
            .is_some_and(|v| v.starts_with(&format!("{prefix}：")))
    {
        return Ok(result);
    }
    let attempt = (|| -> Result<Value> {
        let plan = match engine.store.get(&id)? {
            Some(record) => record.plan,
            None => match engine.preview(requested.clone()) {
                Ok(plan) => serde_json::from_value(plan)?,
                Err(error) => {
                    engine.store.fence(&requested, None)?;
                    return Err(error);
                }
            },
        };
        engine.apply(plan.request.clone(), plan.version)
    })();
    match attempt {
        Ok(_) => {
            crate::fault::checkpoint(&engine.config, "optional_rule_committed")?;
            result.as_object_mut().unwrap().remove(warning);
        }
        Err(error) => {
            if let Some(record) = engine.store.get(&id)?
                && record.status == "not_started"
            {
                engine
                    .store
                    .fence(&record.plan.request, Some(&record.plan.version))?;
            }
            result[warning] = json!(format!("{prefix}：{}", error.error));
        }
    }
    engine.store.complete(parent, &result)?;
    Ok(result)
}
