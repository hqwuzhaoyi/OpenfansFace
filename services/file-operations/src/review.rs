//! Human confirmation is a local-only operation. This module prepares immutable deltas;
//! admission, receipts, publication and compensation belong to the common executor.
use crate::{
    config::Config,
    db, files,
    metadata::Document,
    model::{Error, Result},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path},
};

#[derive(Clone, Serialize, Deserialize)]
pub struct Prepared {
    pub documents: Vec<Document>,
    pub jobs: Vec<Value>,
    pub job_updates: Vec<Value>,
    pub settings_before: Vec<Value>,
    pub settings_after: Vec<Value>,
    pub guards: Value,
    pub result: Value,
    pub optional_rule: Option<Value>,
    pub optional_restore: Option<Value>,
}

pub fn read(config: &Config, name: &str, fallback: Value) -> Result<(Option<String>, Value)> {
    let path = config.base.join(name);
    if path
        .symlink_metadata()
        .is_ok_and(|m| m.file_type().is_symlink())
    {
        return Err(Error::conflict("元数据不能是符号链接").at(path.display().to_string()));
    }
    let raw = files::read_optional(&path)?;
    let value = raw
        .as_deref()
        .map(serde_json::from_str)
        .transpose()?
        .unwrap_or(fallback.clone());
    if value.is_array() != fallback.is_array() || value.is_object() != fallback.is_object() {
        return Err(
            Error::new(503, "metadata_contract", "元数据格式不符").at(path.display().to_string())
        );
    }
    Ok((raw, value))
}
pub fn document(config: &Config, name: &str, fallback: Value, after: Value) -> Result<Document> {
    let (before, value) = read(config, name, fallback)?;
    Ok(Document {
        name: name.into(),
        after: if value == after {
            before.clone()
        } else {
            Some(serde_json::to_string_pretty(&after)?)
        },
        before,
    })
}
pub fn vids(value: &Value) -> Result<Vec<String>> {
    let rows = value
        .as_array()
        .ok_or_else(|| Error::new(400, "invalid_request", "请选择要处理的视频"))?;
    if rows.is_empty() || rows.len() > 5000 {
        return Err(Error::new(
            400,
            "invalid_request",
            "请选择 1 至 5000 个视频",
        ));
    }
    let mut seen = BTreeSet::new();
    let mut out = vec![];
    for row in rows {
        let id = row
            .as_str()
            .filter(|v| !v.is_empty() && !v.contains('\0'))
            .ok_or_else(|| Error::new(400, "invalid_request", "视频编号无效"))?;
        if seen.insert(id) {
            out.push(id.to_owned())
        }
    }
    Ok(out)
}
pub fn creator(config: &Config, value: &Value) -> Result<String> {
    creator_with_aliases(value, &read(config, "person-renames.json", json!({}))?.1)
}
pub fn creator_with_aliases(value: &Value, aliases: &Value) -> Result<String> {
    let name = value
        .as_str()
        .map(str::trim)
        .filter(|s| {
            !s.is_empty()
                && s.chars().count() <= 120
                && !s.chars().any(|c| c == '/' || c == '\\' || c < ' ')
                && !matches!(*s, "." | "..")
        })
        .ok_or_else(|| {
            Error::new(
                400,
                "invalid_request",
                "请输入有效人物名，不能包含路径分隔符",
            )
        })?;
    let mut name = name.to_owned();
    let mut seen = BTreeSet::new();
    while let Some(next) = aliases[&name].as_str() {
        if !seen.insert(name.clone()) {
            return Err(Error::conflict("人物别名包含循环"));
        }
        name = next.into()
    }
    files::validate_name(&name)?;
    Ok(name)
}
pub fn sources(config: &Config) -> Result<BTreeMap<String, String>> {
    let mut out = indexed_sources(config)?;
    let path = config.base.join("processing/processing.db");
    if path.exists() {
        for row in db::rows(
            &db::open(&path)?,
            "SELECT vid,path FROM tasks ORDER BY vid",
            [],
        )? {
            if let (Some(id), Some(path)) = (row["vid"].as_str(), row["path"].as_str()) {
                out.entry(id.into()).or_insert_with(|| path.into());
            }
        }
    }
    Ok(out)
}
/// Index/explicit overrides outrank analysis discovery; whole-report inputs exclude task-only paths.
pub fn indexed_sources(config: &Config) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for row in db::rows(
        &db::open(&config.base.join("faces.db"))?,
        "SELECT vid,path FROM videos ORDER BY vid",
        [],
    )? {
        if let (Some(id), Some(path)) = (row["vid"].as_str(), row["path"].as_str()) {
            out.insert(id.into(), path.into());
        }
    }
    for (id, path) in read(config, "video-path-overrides.json", json!({}))?
        .1
        .as_object()
        .unwrap()
    {
        if let Some(path) = path.as_str() {
            out.insert(id.clone(), path.into());
        }
    }
    Ok(out)
}
pub fn source(config: &Config, id: &str, sources: &BTreeMap<String, String>) -> Option<String> {
    let mut candidates = vec![];
    for value in sources
        .get(id)
        .map(String::as_str)
        .into_iter()
        .chain(std::iter::once(id))
    {
        let normalized = value.replace('\\', "/");
        if normalized.split('/').any(|s| s == "..") {
            continue;
        }
        let path = if normalized.starts_with(&format!("{}/", config.media.display())) {
            Some(normalized.clone())
        } else {
            normalized
                .split_once("/media/pikpak/")
                .map(|(_, rest)| format!("{}/pikpak/{rest}", config.media.display()))
        };
        if let Some(path) = path
            && !candidates.contains(&path)
        {
            candidates.push(path)
        }
    }
    candidates
        .iter()
        .find(|p| Path::new(p).is_file())
        .cloned()
        .or_else(|| candidates.first().cloned())
}
pub fn safe_source(config: &Config, path: &str) -> Result<fs::Metadata> {
    let p = Path::new(path);
    if !p.starts_with(config.media.join("pikpak"))
        || p.starts_with(config.creators())
        || p.starts_with(config.media.join("pikpak/.face-cleanup-staging"))
        || p.components().any(|c| matches!(c, Component::ParentDir))
    {
        return Err(Error::conflict("视频不在允许归名的未整理目录").at(path));
    }
    if fs::canonicalize(p)? != p {
        return Err(Error::conflict("源路径包含符号链接").at(path));
    }
    let stat = fs::metadata(p)?;
    if !stat.is_file() {
        return Err(Error::conflict("源视频不是文件").at(path));
    }
    Ok(stat)
}
pub fn percent(value: &str) -> String {
    value
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
fn provenance(source: &str, id: &str) -> Result<Value> {
    if source == "cluster"
        && !(id.starts_with('p') && id.len() > 1 && id[1..].bytes().all(|b| b.is_ascii_digit()))
    {
        return Err(Error::new(400, "invalid_request", "相似组编号无效"));
    }
    let href = match source {
        "review" => "/review".into(),
        "watermark" => "/watermarks".into(),
        "all" => "/unclassified".into(),
        "approval" => {
            if id.is_empty() {
                "/approvals".into()
            } else {
                format!("/approvals?vid={}", percent(id))
            }
        }
        "person" => format!("/persons/{}", percent(id)),
        "cluster" => format!("/clusters?cid={}", percent(id)),
        _ => "/clusters".into(),
    };
    Ok(json!({"source":source,"source_id":id,"return_href":href}))
}
fn group(task: &Value, data: &Value) -> String {
    if number(&task["manual"]).unwrap_or(0) != 0 {
        return "manual".into();
    }
    let bucket = data["kind"].as_str().unwrap_or("none");
    if bucket == "suggested"
        && let Some(s) = data["suggested"].as_str().filter(|s| !s.is_empty())
    {
        return format!("person:{s}");
    }
    if bucket == "none"
        && let Some(s) = data["face"]["unknown_group"]
            .as_str()
            .filter(|s| !s.is_empty())
    {
        return format!("face:{s}");
    }
    bucket.into()
}
pub fn health_hidden(config: &Config, vid: &str, path: &str, stat: &fs::Metadata) -> Result<bool> {
    crate::video_health::effective_hidden(config, vid, path, stat)
}

pub fn timestamp(at: &str) -> Result<f64> {
    Ok(rusqlite::Connection::open_in_memory()?.query_row(
        "SELECT unixepoch(?, 'subsec')",
        [at],
        |r| r.get(0),
    )?)
}
pub fn number(v: &Value) -> Option<i64> {
    v.as_i64()
        .or_else(|| v["$integer"].as_str().and_then(|s| s.parse().ok()))
}
pub fn integer(v: i64) -> Value {
    json!({"$integer":v.to_string()})
}
pub fn task_data(task: &Value) -> Result<Value> {
    serde_json::from_str(
        task["data"]
            .as_str()
            .ok_or_else(|| Error::conflict("任务数据无效"))?,
    )
    .map_err(Into::into)
}

/// `at` and `id` come from the persisted request, so revalidation is deterministic.
pub fn prepare(
    config: &Config,
    action: &str,
    body: &Value,
    id: &str,
    at: &str,
) -> Result<Prepared> {
    let (_, queue_value) = read(config, "integration-queue.json", json!([]))?;
    let mut queue = queue_value.as_array().unwrap().clone();
    let (_, history) = read(config, "archive-history.json", json!([]))?;
    let mut decisions = if action == "queue-confirm" {
        json!({"approved":[],"rejected":[]})
    } else {
        read(
            config,
            "review-decisions.json",
            json!({"approved":[],"rejected":[]}),
        )?
        .1
    };
    for key in ["approved", "rejected"] {
        if !decisions[key].is_array() {
            return Err(Error::conflict("人工确认记录格式无效"));
        }
    }
    let claim = if action == "claim-decide" {
        let (_, claims) = read(config, "claims-detail.json", json!([]))?;
        Some(
            claims
                .as_array()
                .unwrap()
                .iter()
                .find(|c| c["vid"] == body["vid"])
                .cloned()
                .ok_or_else(|| Error::new(404, "not_found", "找不到这条识别推荐，请刷新"))?,
        )
    } else {
        None
    };
    let ids = vids(&if claim.is_some() {
        json!([body["vid"]])
    } else {
        body["vids"].clone()
    })?;
    if ids
        .iter()
        .any(|id| history.as_array().unwrap().iter().any(|r| r["vid"] == *id))
    {
        return Err(Error::conflict("部分视频已归档，请刷新后检查完成记录"));
    }
    let tasks = if action == "queue-confirm" && body["source"] == "review" {
        crate::review_sessions::validate(config, body)?
            .into_iter()
            .map(|mut task| {
                task["data"] = json!(serde_json::to_string(&task["data"]).unwrap());
                task
            })
            .collect()
    } else {
        vec![]
    };
    let mut out = Prepared {
        documents: vec![],
        jobs: vec![],
        job_updates: vec![],
        settings_before: vec![],
        settings_after: vec![],
        guards: json!({}),
        result: Value::Null,
        optional_rule: None,
        optional_restore: None,
    };
    let undo = action == "queue-undo";
    let approve = if claim.is_some() {
        body["approve"]
            .as_bool()
            .ok_or_else(|| Error::new(400, "invalid_request", "请选择视频并确认操作"))?
    } else {
        true
    };
    if undo {
        crate::review_sessions::validate_undo(config, body)?;
        let mut removed = queue
            .iter()
            .filter(|r| ids.iter().any(|id| r["vid"] == *id))
            .cloned()
            .collect::<Vec<_>>();
        for row in &mut removed {
            if row["return_href"].is_null() {
                let (_, clusters) = read(config, "clusters.json", json!([]))?;
                let cid = clusters
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|c| {
                        c["vids"]
                            .as_array()
                            .is_some_and(|vs| vs.iter().any(|v| v["vid"] == row["vid"]))
                    })
                    .and_then(|c| c["cid"].as_str())
                    .unwrap_or("")
                    .to_owned();
                let source = row["source"].as_str().unwrap_or("legacy").to_owned();
                row["source"] = json!(source);
                row["source_id"] = json!(cid);
                row["return_href"] = json!(if cid.is_empty() {
                    "/clusters".into()
                } else {
                    format!("/clusters?cid={}", percent(&cid))
                });
            }
        }
        queue.retain(|r| !ids.iter().any(|id| r["vid"] == *id));
        for key in ["approved", "rejected"] {
            decisions[key]
                .as_array_mut()
                .unwrap()
                .retain(|v| !removed.iter().any(|r| r["vid"] == *v));
        }
        out.result = json!({"ok":true,"removed":removed,"queue_len":queue.len(),"warning":"归名已撤销，分析状态恢复尚未确认"});
        out.optional_restore = Some(
            json!({"vids":removed.iter().map(|r|r["vid"].clone()).collect::<Vec<_>>(),"at":at,"parent_id":id}),
        );
    } else if !approve {
        if queue.iter().any(|r| r["vid"] == ids[0]) {
            return Err(Error::conflict("视频已在待归档，请先撤销"));
        }
        decisions["approved"]
            .as_array_mut()
            .unwrap()
            .retain(|v| v != &ids[0]);
        if !decisions["rejected"]
            .as_array()
            .unwrap()
            .contains(&json!(ids[0]))
        {
            decisions["rejected"]
                .as_array_mut()
                .unwrap()
                .push(json!(ids[0]));
        }
        out.result = json!({"ok":true,"bucket":"rejected","queued":false});
    } else {
        let name = creator(
            config,
            body.get("creator")
                .or_else(|| claim.as_ref().and_then(|c| c.get("name")))
                .unwrap_or(&Value::Null),
        )?;
        let src = if claim.is_some() {
            "approval"
        } else {
            body["source"]
                .as_str()
                .filter(|s| {
                    [
                        "cluster",
                        "person",
                        "approval",
                        "all",
                        "watermark",
                        "review",
                    ]
                    .contains(s)
                })
                .unwrap_or("legacy")
        };
        let source_id = if claim.is_some() {
            ids[0].as_str()
        } else {
            body["source_id"].as_str().unwrap_or("")
        };
        let origin = provenance(src, source_id)?;
        let sources = sources(config)?;
        let mut selected_paths = BTreeSet::new();
        let mut guards = vec![];
        let mut rule = None;
        let mut added = 0;
        for vid in &ids {
            let path = source(config, vid, &sources)
                .ok_or_else(|| Error::conflict("源视频无法读取，请核对路径"))?;
            let stat = safe_source(config, &path)?;
            if health_hidden(config, vid, &path, &stat)? {
                return Err(Error::conflict("视频体检连续确认异常，请到回收站查看记录"));
            }
            if !selected_paths.insert(path.clone())
                || queue.iter().any(|r| {
                    r["vid"] != *vid
                        && r["vid"]
                            .as_str()
                            .and_then(|id| source(config, id, &sources))
                            .as_deref()
                            == Some(path.as_str())
                })
            {
                return Err(Error::conflict("同一个源文件已在本次选择或待归档中"));
            }
            let task = tasks.iter().find(|t| t["vid"] == *vid);
            if src == "review" {
                let task = task.ok_or_else(|| Error::conflict("视频尚未就绪或状态已变化"))?;
                let data = task_data(task)?;
                let manual = number(&task["manual"]).unwrap_or(0) != 0;
                if task["stage"] != "review"
                    && !manual
                    && !(data["ready"] == true
                        && (task["stage"] == "face"
                            || task["stage"] == "face_queued"
                            || (task["stage"] == "retry" || task["stage"] == "error")
                                && task["retry_stage"] == "face"))
                {
                    return Err(Error::conflict("视频尚未就绪或状态已变化"));
                }
                if let Some(scope) = body.get("review_group").filter(|s| !s.is_null())
                    && (scope["key"].as_str() != Some(group(task, &data).as_str())
                        || scope["deferred"].as_bool()
                            != Some(number(&task["deferred"]).unwrap_or(0) != 0))
                {
                    return Err(Error::conflict("确认组或暂缓状态已变化，请重新核对"));
                }
                for expected in [&body["fingerprints"][vid], task] {
                    if expected["path"] != path
                        || number(&expected["size"]) != Some(stat.len() as i64)
                        || expected["mtime_ns"].as_str() != Some(files::mtime_ns(&stat).as_str())
                    {
                        return Err(Error::conflict("视频文件已变化，请重新分析后核对"));
                    }
                }
                if let Some(key) = body["watermark_key"].as_str().filter(|s| !s.is_empty()) {
                    let candidate = data["watermark"]["candidates"]
                        .as_array()
                        .and_then(|rows| {
                            rows.iter().find(|r| {
                                r["key"] == key
                                    && r["kind"] == "account"
                                    && r["platform"] != "Telegram"
                            })
                        })
                        .ok_or_else(|| Error::conflict("选中视频没有共同的作者账号证据"))?;
                    rule = Some(candidate.clone());
                }
                // Confirmation reads readiness/evidence but does not own analysis progress.
                // Only the explicit optional rule/restoration commands write task rows.
            }
            guards
                .push(json!({"vid":vid,"path":path,"fingerprint":files::fingerprint(&path,&stat)}));
            if let Some(existing) = queue.iter().find(|r| r["vid"] == *vid) {
                if creator(config, &existing["creator"])? != name {
                    return Err(Error::conflict("视频已归给其他人物，请先到待归档撤销"));
                }
                continue;
            }
            let mut row = json!({"vid":vid,"creator":name,"at":at,"batch_id":id});
            for (k, v) in origin.as_object().unwrap() {
                row[k] = v.clone()
            }
            if let Some(note) = task
                .and_then(|t| t["note"].as_str())
                .filter(|n| !n.is_empty())
            {
                row["note"] = json!(note)
            }
            queue.push(row);
            added += 1;
        }
        out.guards = json!({"files":guards,"claim":claim});
        let (_, mut people) = read(config, "people-manual.json", json!([]))?;
        let old = people
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == name)
            .cloned();
        people.as_array_mut().unwrap().retain(|p| p["name"] != name);
        people.as_array_mut().unwrap().push(json!({"name":name,"created_at":old.as_ref().and_then(|v|v["created_at"].as_str()).unwrap_or(at),"last_used_at":at,"rep_thumb":old.as_ref().map(|v|v["rep_thumb"].clone()).unwrap_or(Value::Null)}));
        out.documents
            .push(document(config, "people-manual.json", json!([]), people)?);
        let (_, mut deleted) = read(config, "people-deleted.json", json!({}))?;
        deleted.as_object_mut().unwrap().remove(&name);
        out.documents
            .push(document(config, "people-deleted.json", json!({}), deleted)?);
        out.result = json!({"ok":true,"added":added,"queue_len":queue.len(),"vids":ids});
        if let Some(mark) = rule {
            out.optional_rule = Some(
                json!({"creator":name,"mark":mark,"vids":ids,"queue_rows":queue.iter().filter(|row|ids.iter().any(|id|row["vid"]==*id)).cloned().collect::<Vec<_>>(),"guards":out.guards,"at":at,"parent_id":id}),
            );
            out.result["rule_warning"] = json!("归名已保存，账号规则尚未确认保存");
        }
        if claim.is_some() {
            decisions["rejected"]
                .as_array_mut()
                .unwrap()
                .retain(|v| v != &ids[0]);
            if !decisions["approved"]
                .as_array()
                .unwrap()
                .contains(&json!(ids[0]))
            {
                decisions["approved"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!(ids[0]));
            }
            out.result = json!({"ok":true,"bucket":"approved","queued":true});
        }
    }
    out.documents.push(document(
        config,
        "integration-queue.json",
        json!([]),
        json!(queue),
    )?);
    if undo || claim.is_some() {
        out.documents.push(document(
            config,
            "review-decisions.json",
            json!({"approved":[],"rejected":[]}),
            decisions,
        )?);
    }
    Ok(out)
}

pub fn plan(config: &Config, request: crate::model::Request) -> Result<crate::model::Plan> {
    use crate::model::{AffectedStores, ExecutionIntent, FailurePolicy, Plan};
    let body = request
        .payload
        .as_ref()
        .ok_or_else(|| Error::new(400, "invalid_request", "缺少人工确认内容"))?;
    let at = body["at"]
        .as_str()
        .ok_or_else(|| Error::new(400, "invalid_request", "缺少持久化操作时间"))?;
    let prepared = if request.action.starts_with("review-") {
        prepare_edit(config, &request.action, body)?
    } else if request.action == "archive-resolve" {
        crate::archive_resolution::prepare(config, body)?
    } else if request.action == "queue-rule" {
        prepare_rule(config, body)?
    } else if request.action == "queue-restore" {
        prepare_restore(config, body)?
    } else {
        prepare(config, &request.action, body, &request.id, at)?
    };
    let plan = Plan {
        id: request.id.clone(),
        action: request.action.clone(),
        person: String::new(),
        target_person: String::new(),
        request,
        version: String::new(),
        files: vec![],
        total_bytes: 0,
        directories: vec![],
        ledger_version: String::new(),
        metadata_version: String::new(),
        execution: Some(ExecutionIntent {
            scopes: vec![],
            failure_policy: FailurePolicy::Rollback,
            stores: AffectedStores {
                faces: false,
                jobs: !prepared.jobs.is_empty() || !prepared.settings_after.is_empty(),
                ledger: false,
            },
            data: serde_json::to_value(&prepared)?,
            result: prepared.result.clone(),
        }),
    };
    Ok(plan)
}
pub fn revalidate(config: &Config, plan: &crate::model::Plan) -> Result<()> {
    let body = plan
        .request
        .payload
        .as_ref()
        .ok_or_else(|| Error::conflict("人工确认内容缺失"))?;
    let prepared = if plan.action.starts_with("review-") {
        prepare_edit(config, &plan.action, body)?
    } else if plan.action == "archive-resolve" {
        crate::archive_resolution::prepare(config, body)?
    } else if plan.action == "queue-rule" {
        prepare_rule(config, body)?
    } else if plan.action == "queue-restore" {
        prepare_restore(config, body)?
    } else {
        prepare(
            config,
            &plan.action,
            body,
            &plan.id,
            body["at"].as_str().unwrap_or(""),
        )?
    };
    if serde_json::to_value(&prepared)? != plan.execution.as_ref().unwrap().data {
        return Err(Error::conflict("文件、确认组或人工决定已变化，请重新核对"));
    }
    Ok(())
}
pub fn snapshot(
    _config: &Config,
    plan: &crate::model::Plan,
    successful: &[usize],
) -> Result<crate::native::DomainSnapshot> {
    if !successful.is_empty() {
        return Err(Error::conflict("归名确认不能移动文件"));
    }
    let prepared: Prepared = serde_json::from_value(
        plan.execution
            .as_ref()
            .ok_or_else(|| Error::conflict("人工确认意图缺失"))?
            .data
            .clone(),
    )?;
    let mut tables = vec![];
    if !prepared.jobs.is_empty() {
        tables.push(crate::archive_ledger::table(
            db::Table::Tasks,
            vec!["vid".into()],
            prepared.jobs,
            prepared.job_updates,
        ));
    }
    if !prepared.settings_after.is_empty() {
        tables.push(crate::archive_ledger::table(
            db::Table::Settings,
            vec!["key".into()],
            prepared.settings_before,
            prepared.settings_after,
        ));
    }
    Ok(crate::native::DomainSnapshot {
        metadata: crate::metadata::Snapshot {
            documents: prepared.documents,
            videos: vec![],
            jobs: vec![],
            faces_delta: None,
            jobs_delta: if plan.execution.as_ref().unwrap().stores.jobs {
                Some(db::Delta {
                    guards: vec![],
                    tables,
                })
            } else {
                None
            },
        },
        ledger: None,
    })
}

/// Deliberately optional, separate local-only transaction after the confirmation receipt.
fn prepare_rule(config: &Config, body: &Value) -> Result<Prepared> {
    let parent = body["parent_id"]
        .as_str()
        .ok_or_else(|| Error::conflict("缺少原归名回执"))?;
    let db = db::open(&config.state_db())?;
    let (state, raw): (String, String) = db.query_row(
        "SELECT status,plan FROM operations WHERE id=?",
        [parent],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let saved: Value = serde_json::from_str(&raw)?;
    if state != "complete" || saved["execution"]["data"]["optional_rule"] != *body {
        return Err(Error::conflict("账号规则必须绑定已完成的归名回执"));
    }

    let name = creator(config, &body["creator"])?;
    let ids = vids(&body["vids"])?;
    let (_, queue) = read(config, "integration-queue.json", json!([]))?;
    if ids.iter().any(|id| {
        !queue
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["vid"] == *id && r["creator"] == name)
    }) {
        return Err(Error::conflict("归名已撤销或变化，账号规则未保存"));
    }
    if !body["queue_rows"].as_array().is_some_and(|rows| {
        rows.iter()
            .all(|saved| queue.as_array().unwrap().contains(saved))
    }) {
        return Err(Error::conflict(
            "人工归名已被重新处置，旧账号规则请求未保存",
        ));
    }
    let guards = body["guards"]["files"]
        .as_array()
        .ok_or_else(|| Error::conflict("账号证据缺失"))?;
    for guard in guards {
        let path = guard["path"]
            .as_str()
            .ok_or_else(|| Error::conflict("账号证据路径缺失"))?;
        let stat = safe_source(config, path)?;
        if files::fingerprint(path, &stat) != guard["fingerprint"] {
            return Err(Error::conflict("视频证据已变化，账号规则未保存"));
        }
    }
    let mark = &body["mark"];
    let key = mark["key"]
        .as_str()
        .ok_or_else(|| Error::conflict("账号规则编号缺失"))?;
    if mark["kind"] != "account" || mark["platform"] == "Telegram" {
        return Err(Error::conflict("所选文字不是作者账号证据"));
    }
    let at = body["at"]
        .as_str()
        .ok_or_else(|| Error::conflict("账号确认时间缺失"))?;
    let (_, mut mappings) = read(config, "watermarks/mappings.json", json!({}))?;
    mappings[key] = json!({"key":key,"text":mark["text"],"token":mark["token"],"platform":mark["platform"],"kind":"person","creator":name,"confirmed_at":at});
    let mut out = Prepared {
        documents: vec![document(
            config,
            "watermarks/mappings.json",
            json!({}),
            mappings,
        )?],
        jobs: vec![],
        job_updates: vec![],
        settings_before: vec![],
        settings_after: vec![],
        guards: body["guards"].clone(),
        result: json!({"ok":true}),
        optional_rule: None,
        optional_restore: None,
    };
    (out.jobs, out.job_updates) = rule_task_updates(config, key, at)?;
    Ok(out)
}

/// Rule edits invalidate interpretation even between text completion and the next face claim.
/// Preserve visible evidence/manual fields; only the affected analysis attempts are fenced.
pub fn rule_task_updates(config: &Config, key: &str, at: &str) -> Result<(Vec<Value>, Vec<Value>)> {
    let path = config.base.join("processing/processing.db");
    let mut before = vec![];
    let mut after = vec![];
    if !path.exists() {
        return Ok((before, after));
    }
    for task in db::rows(&db::open(&path)?, "SELECT * FROM tasks ORDER BY vid", [])? {
        if number(&task["manual"]).unwrap_or(0) != 0
            || !matches!(
                task["stage"].as_str(),
                Some("queued" | "text" | "face" | "face_queued" | "review" | "retry" | "error")
            )
        {
            continue;
        }
        let data = task_data(&task)?;
        if !data["watermark"]["candidates"]
            .as_array()
            .is_some_and(|rows| rows.iter().any(|r| r["key"] == key))
        {
            continue;
        }
        let mut next = task.clone();
        next["stage"] = json!("queued");
        if task.get("retry_stage").is_some() {
            next["retry_stage"] = json!("text");
        }
        for field in [
            "text_attempts",
            "face_attempts",
            "next_retry",
            "lease_expires_at",
        ] {
            if task.get(field).is_some() {
                next[field] = if task[field].is_number() {
                    json!(0.0)
                } else {
                    integer(0)
                };
            }
        }
        if task.get("run_id").is_some() {
            next["run_id"] = json!("");
        }
        if task.get("priority").is_some() {
            next["priority"] = integer(number(&task["priority"]).unwrap_or(0).max(1));
        }
        next["updated_at"] = json!(timestamp(at)?);
        before.push(task);
        after.push(next);
    }
    Ok((before, after))
}

fn prepare_restore(config: &Config, body: &Value) -> Result<Prepared> {
    let parent = body["parent_id"]
        .as_str()
        .ok_or_else(|| Error::conflict("缺少原撤销回执"))?;
    let db = db::open(&config.state_db())?;
    let (state, raw): (String, String) = db.query_row(
        "SELECT status,plan FROM operations WHERE id=?",
        [parent],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let saved: Value = serde_json::from_str(&raw)?;
    if state != "complete" || saved["execution"]["data"]["optional_restore"] != *body {
        return Err(Error::conflict("恢复必须绑定已完成的撤销回执"));
    }
    let ids = body["vids"]
        .as_array()
        .ok_or_else(|| Error::conflict("撤销范围缺失"))?;
    let (_, queue) = read(config, "integration-queue.json", json!([]))?;
    if queue
        .as_array()
        .unwrap()
        .iter()
        .any(|r| ids.contains(&r["vid"]))
    {
        return Err(Error::conflict("视频已有新的人工归名，跳过旧恢复请求"));
    }
    let at = body["at"]
        .as_str()
        .ok_or_else(|| Error::conflict("撤销时间缺失"))?;
    let mut out = Prepared {
        documents: vec![],
        jobs: vec![],
        job_updates: vec![],
        settings_before: vec![],
        settings_after: vec![],
        guards: json!({}),
        result: json!({"ok":true}),
        optional_rule: None,
        optional_restore: None,
    };
    let path = config.base.join("processing/processing.db");
    if !path.exists() {
        return Ok(out);
    }
    let db = db::open(&path)?;
    for task in db::rows(&db, "SELECT * FROM tasks ORDER BY vid", [])? {
        if !ids.contains(&task["vid"])
            || task["updated_at"]
                .as_f64()
                .is_some_and(|updated| updated > timestamp(at).unwrap_or(0.0))
        {
            continue;
        }
        let Some(path) = task["path"].as_str() else {
            continue;
        };
        let Ok(stat) = fs::metadata(path) else {
            continue;
        };
        if number(&task["size"]) != Some(stat.len() as i64)
            || task["mtime_ns"].as_str() != Some(files::mtime_ns(&stat).as_str())
        {
            continue;
        }
        let data = task_data(&task)?;
        let mut next = task.clone();
        next["stage"] = json!(if number(&task["manual"]).unwrap_or(0) != 0 {
            "error"
        } else if data["ready"] == true {
            "review"
        } else {
            "queued"
        });
        next["deferred"] = integer(0);
        next["updated_at"] = json!(timestamp(at)?);
        out.jobs.push(task);
        out.job_updates.push(next);
    }
    out.settings_before = db::rows(
        &db,
        "SELECT * FROM settings WHERE key='discovery_requested'",
        [],
    )?;
    out.settings_after =
        vec![json!({"key":"discovery_requested","value":serde_json::to_string(&timestamp(at)?)?})];
    Ok(out)
}

/// Explicit human task fields only; analysis data, leases and stage remain worker-owned.
fn prepare_edit(config: &Config, action: &str, body: &Value) -> Result<Prepared> {
    crate::review_sessions::validate(config, body)?;
    let ids = vids(&body["vids"])?;
    if action == "review-note"
        && body["note"]
            .as_str()
            .is_none_or(|s| s.chars().count() > 1000)
    {
        return Err(Error::new(400, "invalid_request", "备注最多 1000 字"));
    }
    let conn = db::open(&config.base.join("processing/processing.db"))?;
    let mut before = vec![];
    let mut after = vec![];
    for id in &ids {
        let row = db::rows(&conn, "SELECT * FROM tasks WHERE vid=?", [id])?
            .into_iter()
            .next()
            .ok_or_else(|| Error::conflict("视频已移出核对范围"))?;
        let mut next = row.clone();
        if action == "review-note" {
            next["note"] = body["note"].clone()
        } else {
            next["deferred"] = integer(if action == "review-defer" { 1 } else { 0 })
        }
        before.push(row);
        after.push(next);
    }
    Ok(Prepared {
        documents: vec![],
        jobs: before,
        job_updates: after,
        settings_before: vec![],
        settings_after: vec![],
        guards: json!({}),
        result: json!({"ok":true,"vids":ids}),
        optional_rule: None,
        optional_restore: None,
    })
}
