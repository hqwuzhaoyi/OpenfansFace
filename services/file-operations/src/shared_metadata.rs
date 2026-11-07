//! Shared business metadata: closed commands, native admission and durable document CAS.
use crate::{
    config::Config,
    db,
    engine::Engine,
    files,
    metadata::{Document, Snapshot},
    model::{self, AffectedStores, Error, ExecutionIntent, FailurePolicy, Plan, Request, Result},
    native::DomainSnapshot,
    person_registration, review,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeSet, fs, path::Path};

const BUSINESS: &[(&str, bool)] = &[
    ("person-renames.json", false),
    ("template-overrides.json", false),
    ("watermarks/mappings.json", false),
    ("people-manual.json", true),
    ("people-deleted.json", false),
    ("person-rejections.json", false),
    ("external-profiles/manual-links.json", false),
    ("external-profiles/reviewed-links.json", false),
    ("integration-queue.json", true),
    ("review-decisions.json", false),
    ("archive-history.json", true),
    ("video-path-overrides.json", false),
];
fn bad(message: &str) -> Error {
    Error::new(400, "invalid_request", message)
}
fn changed() -> Error {
    Error::new(
        409,
        "material_changed",
        "相关文件、规则或人工资料已变化，请重新核对",
    )
}
fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value[key]
        .as_str()
        .filter(|s| !s.is_empty() && !s.contains('\0'))
        .ok_or_else(|| bad("缺少有效参数"))
}
fn read(config: &Config, name: &str, array: bool) -> Result<Value> {
    Ok(review::read(config, name, if array { json!([]) } else { json!({}) })?.1)
}
fn names(value: &Value) -> Result<Vec<String>> {
    if value.is_null() {
        return Ok(vec![]);
    }
    let rows = value.as_array().ok_or_else(|| bad("范围必须是列表"))?;
    if rows.len() > 5000 {
        return Err(bad("范围过大"));
    }
    rows.iter()
        .map(|v| {
            v.as_str()
                .filter(|s| !s.is_empty() && !s.contains('\0'))
                .map(str::to_owned)
                .ok_or_else(|| bad("范围编号无效"))
        })
        .collect()
}
fn canonical(config: &Config, name: &str) -> Result<String> {
    review::creator(config, &json!(name))
}
fn relevant(
    config: &Config,
    row: &Value,
    people: &BTreeSet<String>,
    vids: &BTreeSet<String>,
) -> Result<bool> {
    for key in ["creator", "person", "name"] {
        if let Some(name) = row[key].as_str()
            && people.contains(&canonical(config, name)?)
        {
            return Ok(true);
        }
    }
    Ok(row["vid"].as_str().is_some_and(|v| vids.contains(v)))
}
fn file_guard(config: &Config, vid: &str) -> Result<Value> {
    let sources = review::sources(config)?;
    file_guard_from(config, vid, &sources)
}
fn file_guard_from(
    config: &Config,
    vid: &str,
    sources: &std::collections::BTreeMap<String, String>,
) -> Result<Value> {
    let Some(path) = review::source(config, vid, sources) else {
        return Ok(json!({"vid":vid,"missing":true}));
    };
    let p = Path::new(&path);
    if !p.starts_with(config.media.join("pikpak")) {
        return Err(changed());
    }
    match fs::metadata(p) {
        Ok(stat) if stat.is_file() => {
            if fs::canonicalize(p)? != p {
                return Err(changed());
            }
            Ok(json!({"vid":vid,"path":path,"fingerprint":files::fingerprint(&path,&stat)}))
        }
        Ok(_) => Err(changed()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Ok(json!({"vid":vid,"path":path,"missing":true}))
        }
        Err(e) => Err(e.into()),
    }
}
/// #18 calls this under its admission gate. Analysis ranks, stages and retries excluded.
pub fn material_state(config: &Config, scope: &Value) -> Result<Value> {
    if !scope.is_object() {
        return Err(bad("缺少核对范围"));
    }
    let raw_people = names(&scope["people"])?;
    let vids: BTreeSet<_> = names(&scope["vids"])?.into_iter().collect();
    let keys: BTreeSet<_> = names(&scope["watermark_keys"])?.into_iter().collect();
    let people: BTreeSet<_> = raw_people
        .iter()
        .map(|s| canonical(config, s))
        .collect::<Result<_>>()?;
    let sources = review::sources(config)?;
    let mut material = json!({"names":raw_people.iter().map(|s|Ok(json!([s,canonical(config,s)?]))).collect::<Result<Vec<_>>>()?,"files":vids.iter().map(|id|file_guard_from(config,id,&sources)).collect::<Result<Vec<_>>>()?});
    for (name, array) in BUSINESS {
        if [
            "person-renames.json",
            "video-path-overrides.json",
            "people-manual.json",
        ]
        .contains(name)
        {
            continue;
        }
        let value = read(config, name, *array)?;
        if name.starts_with("external-profiles/")
            && !value.as_object().unwrap().is_empty()
            && !value["links"].is_array()
        {
            return Err(Error::new(
                503,
                "metadata_contract",
                "账号核对资料无法读取，请恢复后重新核对",
            ));
        }
        let selected = if *array {
            json!(
                value
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|r| r["vid"].as_str().is_some_and(|id| vids.contains(id)))
                    .cloned()
                    .collect::<Vec<_>>()
            )
        } else if name.starts_with("external-profiles/") {
            json!(
                value["links"]
                    .as_array()
                    .unwrap_or(&vec![])
                    .iter()
                    .filter_map(|r| match relevant(config, r, &people, &vids) {
                        Ok(true) => Some(Ok(r.clone())),
                        Ok(false) => None,
                        Err(e) => Some(Err(e)),
                    })
                    .collect::<Result<Vec<_>>>()?
            )
        } else if *name == "review-decisions.json" {
            json!({"approved":value["approved"].as_array().unwrap_or(&vec![]).iter().filter(|v|v.as_str().is_some_and(|id|vids.contains(id))).collect::<Vec<_>>(),"rejected":value["rejected"].as_array().unwrap_or(&vec![]).iter().filter(|v|v.as_str().is_some_and(|id|vids.contains(id))).collect::<Vec<_>>()})
        } else {
            let mut out = serde_json::Map::new();
            for (k, v) in value.as_object().unwrap() {
                let keep = if *name == "watermarks/mappings.json" {
                    keys.contains(k) || relevant(config, v, &people, &vids)?
                } else if *name == "person-rejections.json" {
                    v["vid"].as_str().is_some_and(|id| vids.contains(id))
                        && relevant(config, v, &people, &vids)?
                } else {
                    people.contains(&canonical(config, k)?)
                };
                if keep {
                    out.insert(k.clone(), v.clone());
                }
            }
            Value::Object(out)
        };
        material[*name] = selected;
    }
    let task_path = config.base.join("processing/processing.db");
    if task_path.exists() {
        let conn = db::open(&task_path)?;
        let columns = db::columns(&conn, "tasks")?;
        let selected: Vec<_> = ["vid", "manual", "deferred", "note"]
            .into_iter()
            .filter(|key| columns.iter().any(|c| c == key))
            .collect();
        let mut tasks = vec![];
        for vid in &vids {
            tasks.extend(db::rows(
                &conn,
                &format!("SELECT {} FROM tasks WHERE vid=?", selected.join(",")),
                [vid],
            )?);
        }
        material["manual_tasks"] = json!(tasks);
    }
    Ok(json!({"version":model::hash(&material),"scope":scope,"material":material}))
}
pub fn check_state(config: &Config, scope: &Value, version: &str) -> Result<Value> {
    let state = material_state(config, scope)?;
    if state["version"] != version {
        return Err(changed());
    }
    Ok(state)
}
pub fn analysis_state(config: &Config) -> Result<Value> {
    let mut input = serde_json::Map::new();
    for (name, array) in BUSINESS {
        input.insert((*name).into(), read(config, name, *array)?);
    }
    for name in ["report.json", "claims-detail.json", "promoted.json"] {
        let raw = files::read_optional(&config.base.join(name))?;
        input.insert(name.into(), json!(raw));
    }
    let path = config.base.join("faces.db");
    if path.exists() {
        let conn = db::open(&path)?;
        let tx = conn.unchecked_transaction()?;
        input.insert(
            "videos".into(),
            json!(table_digest(&tx, "SELECT * FROM videos ORDER BY vid")?),
        );
        input.insert(
            "faces".into(),
            json!(table_digest(&tx, "SELECT * FROM faces ORDER BY rowid")?),
        );
        tx.commit()?;
    }
    let sources = review::sources(config)?;
    input.insert(
        "files".into(),
        json!(
            sources
                .keys()
                .map(|vid| file_guard_from(config, vid, &sources))
                .collect::<Result<Vec<_>>>()?
        ),
    );
    Ok(json!({"version":model::hash(&input)}))
}
fn table_digest(conn: &rusqlite::Connection, sql: &str) -> Result<String> {
    use sha2::{Digest, Sha256};
    let mut statement = conn.prepare(sql)?;
    let count = statement.column_count();
    let mut rows = statement.query([])?;
    let mut digest = Sha256::new();
    while let Some(row) = rows.next()? {
        for index in 0..count {
            let value = format!("{:?}", row.get_ref(index)?);
            digest.update(value.len().to_be_bytes());
            digest.update(value.as_bytes());
        }
        digest.update(b"row");
    }
    Ok(format!("{:x}", digest.finalize()))
}
fn recommendation_exists(config: &Config, name: &str, vid: &str) -> Result<bool> {
    for file in ["integration-queue.json", "archive-history.json"] {
        if read(config, file, true)?
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["vid"] == vid)
        {
            return Ok(false);
        }
    }
    let report = read(config, "report.json", false)?;
    for person in report["identities"].as_array().into_iter().flatten() {
        if person["name"]
            .as_str()
            .is_some_and(|n| canonical(config, n).ok().as_deref() == Some(name))
            && person["vids"]
                .as_array()
                .is_some_and(|rows| rows.iter().any(|r| r["vid"] == vid))
        {
            return Ok(true);
        }
    }
    let claims = files::read_optional(&config.base.join("claims-detail.json"))?
        .map(|raw| serde_json::from_str::<Value>(&raw))
        .transpose()?
        .unwrap_or(json!([]));
    if claims.as_array().is_some_and(|rows| {
        rows.iter().any(|r| {
            r["vid"] == vid
                && r["name"]
                    .as_str()
                    .is_some_and(|n| canonical(config, n).ok().as_deref() == Some(name))
        })
    }) {
        return Ok(true);
    }
    let records = read(config, "watermarks/results.json", false)?;
    let mappings = read(config, "watermarks/mappings.json", false)?;
    let record = &records[vid];
    Ok(record["status"] == "done"
        && record["candidates"].as_array().is_some_and(|rows| {
            rows.iter().any(|r| {
                r["frames"].as_u64().is_some_and(|n| n >= 2)
                    && r["key"].as_str().is_some_and(|key| {
                        mappings[key]["kind"] == "person"
                            && mappings[key]["creator"]
                                .as_str()
                                .is_some_and(|n| canonical(config, n).ok().as_deref() == Some(name))
                    })
            })
        }))
}
#[derive(Clone, Serialize, Deserialize)]
struct Prepared {
    documents: Vec<Document>,
    guards: Value,
    result: Value,
}
fn prepare(config: &Config, body: &Value, id: &str) -> Result<Prepared> {
    let command = string(body, "command")?;
    let at = string(body, "at")?;
    review::timestamp(at).map_err(|_| bad("保存时间无效"))?;
    if let Some(expected) = body.get("expected_version") {
        let version = expected
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| bad("资料版本无效"))?;
        check_state(config, &body["scope"], version)?;
    }
    let mut out = Prepared {
        documents: vec![],
        guards: json!({}),
        result: json!({"ok":true}),
    };
    let mut add = |name: &str, before: Option<String>, after: Value| -> Result<()> {
        out.documents.push(Document {
            name: name.into(),
            after: Some(serde_json::to_string_pretty(&after)?),
            before,
        });
        Ok(())
    };
    if command == "analysis-publish" {
        let state = analysis_state(config)?;
        if state["version"] != string(body, "version")? {
            return Err(changed());
        }
        out.guards = state;
        let outputs = body["outputs"]
            .as_object()
            .ok_or_else(|| bad("缺少分析输出"))?;
        if !outputs.contains_key("report.json")
            || outputs.keys().any(|k| {
                !["report.json", "claims-detail.json", "promoted.json"].contains(&k.as_str())
            })
        {
            return Err(bad("分析输出范围无效"));
        }
        for (name, value) in outputs {
            if (name == "report.json" && (!value.is_object() || !value["identities"].is_array()))
                || (name != "report.json" && !value.is_array())
            {
                return Err(bad("分析输出格式无效"));
            }
            add(
                name,
                files::read_optional(&config.base.join(name))?,
                value.clone(),
            )?;
        }
        out.result["published"] = json!(outputs.keys().collect::<Vec<_>>());
        return Ok(out);
    }
    let name = if command == "watermark-set" && body["kind"] != "person" {
        String::new()
    } else {
        canonical(config, string(body, "name")?)?
    };
    out.guards = json!({"name":name});
    match command {
        "account-link" | "account-unlink" => {
            if !person_registration::known_person(config, &name)? {
                return Err(Error::new(
                    404,
                    "missing_person",
                    "人物不存在，请刷新人物列表",
                ));
            }
            let (before, mut value) = review::read(
                config,
                "external-profiles/manual-links.json",
                json!({"version":1,"source":"nv-pu-sa","links":[]}),
            )?;
            if value["version"] != 1 || value["source"] != "nv-pu-sa" || !value["links"].is_array()
            {
                return Err(Error::new(503, "metadata_contract", "手动关联记录格式无效"));
            }
            let links = value["links"].as_array_mut().unwrap();
            let mut pairs = BTreeSet::new();
            let mut ids = BTreeSet::new();
            for link in links.iter_mut() {
                let creator = canonical(config, string(link, "creator")?)?;
                let profile = string(&link["profile"], "id")?;
                let handle = string(&link["profile"], "handle")?;
                if !profile.bytes().all(|b| b.is_ascii_digit())
                    || handle.len() > 15
                    || !handle
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_')
                    || !link["profile"]["name"].is_string()
                    || !link["profile"]["description"].is_string()
                    || ["linked_at", "source_fetched_at"].iter().any(|key| {
                        link[*key]
                            .as_str()
                            .is_none_or(|date| review::timestamp(date).is_err())
                    })
                {
                    return Err(Error::new(
                        503,
                        "metadata_contract",
                        "手动关联记录损坏，请核对后重试",
                    ));
                }
                if !pairs.insert((creator.clone(), profile.to_owned()))
                    || !ids.insert(string(link, "id")?.to_owned())
                {
                    return Err(Error::new(503, "metadata_contract", "手动关联记录重复"));
                }
                link["creator"] = json!(creator);
            }
            if command == "account-unlink" {
                let link_id = string(body, "link_id")?;
                let old = links.len();
                links.retain(|l| !(l["creator"] == name && l["id"] == link_id));
                if links.len() == old {
                    return Err(changed());
                }
            } else {
                let profile_id = string(body, "profile_id")?;
                let handle = string(body, "handle")?;
                let cache = read(config, "external-profiles/nv-pu-sa.json", false)?;
                let rows = cache["data"]
                    .as_array()
                    .ok_or_else(|| Error::new(503, "profile_unavailable", "请重新搜索账号资料"))?;
                let matched: Vec<_> = rows
                    .iter()
                    .filter(|r| {
                        r["id"] == profile_id
                            || r["screen_name"]
                                .as_str()
                                .is_some_and(|h| h.eq_ignore_ascii_case(handle))
                    })
                    .collect();
                if matched.len() != 1
                    || matched[0]["id"] != profile_id
                    || !matched[0]["screen_name"]
                        .as_str()
                        .is_some_and(|h| h.eq_ignore_ascii_case(handle))
                    || !profile_id.bytes().all(|b| b.is_ascii_digit())
                    || handle.len() > 15
                    || !handle
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_')
                {
                    return Err(changed());
                }
                let r = matched[0];
                review::timestamp(string(&cache, "fetched_at")?).map_err(|_| {
                    Error::new(503, "profile_unavailable", "账号资料时间无效，请重新搜索")
                })?;
                let followers = r["followers_count"]
                    .as_u64()
                    .filter(|n| *n <= 9_007_199_254_740_991);
                let synced = r["last_synced_at"]
                    .as_str()
                    .filter(|s| review::timestamp(s).is_ok());
                let profile = json!({"id":profile_id,"handle":r["screen_name"],"name":r["name"].as_str().unwrap_or(handle).chars().take(200).collect::<String>(),"description":r["description"].as_str().unwrap_or("").chars().take(2000).collect::<String>(),"followers":followers,"synced_at":synced});
                let link=links.iter().find(|l|l["creator"]==name&&l["profile"]["id"]==profile_id).cloned().unwrap_or_else(||json!({"id":id,"creator":name,"profile":profile,"linked_at":at,"source_fetched_at":cache["fetched_at"]}));
                if !links.contains(&link) {
                    links.push(link.clone())
                }
                out.result["link"] = link;
            }
            add("external-profiles/manual-links.json", before, value)?;
        }
        "template-set" => {
            if !person_registration::known_person(config, &name)? {
                return Err(Error::new(404, "missing_person", "人物不存在"));
            }
            let groups = body["groups"]
                .as_array()
                .ok_or_else(|| bad("模板分组格式无效"))?;
            if groups.len() > 1000 || groups.iter().any(|v| v.as_u64().is_none()) {
                return Err(bad("模板分组格式无效"));
            }
            let (before, mut value) = review::read(config, "template-overrides.json", json!({}))?;
            value[&name] = json!({"groups":groups,"at":at});
            add("template-overrides.json", before, value)?;
        }
        "person-remember" => {
            let (before, mut value) = review::read(config, "people-manual.json", json!([]))?;
            let rows = value.as_array_mut().unwrap();
            let existing = rows.iter().position(|v| v["name"] == name);
            let mut row = existing
                .map(|i| rows.remove(i))
                .unwrap_or_else(|| json!({"name":name,"created_at":at,"rep_thumb":null}));
            row["last_used_at"] = json!(at);
            if row["rep_thumb"].as_str().is_none_or(str::is_empty)
                && let Some(thumb) = body["thumb"].as_str()
            {
                row["rep_thumb"] = json!(thumb)
            }
            rows.push(row);
            add("people-manual.json", before, value)?;
            let (before, mut value) = review::read(config, "people-deleted.json", json!({}))?;
            value.as_object_mut().unwrap().remove(&name);
            add("people-deleted.json", before, value)?;
        }
        "recommendation-reject" | "recommendation-undo" => {
            let vid = string(body, "vid")?;
            let key = serde_json::to_string(&json!([name, vid]))?;
            let (before, mut value) = review::read(config, "person-rejections.json", json!({}))?;
            if command == "recommendation-undo" {
                if value[&key]["id"] != string(body, "rejection_id")? {
                    return Err(changed());
                }
                value.as_object_mut().unwrap().remove(&key);
            } else {
                let guard = file_guard(config, vid)?;
                let path = string(&guard, "path")?;
                let stat = review::safe_source(config, path)?;
                if !recommendation_exists(config, &name, vid)? {
                    return Err(changed());
                }
                if body["path"] != path
                    || body["size"].as_u64() != Some(stat.len())
                    || body["mtime_ns"] != files::mtime_ns(&stat)
                {
                    return Err(changed());
                }
                out.guards["file"] = guard.clone();
                value[&key] = json!({"id":id,"name":name,"vid":vid,"path":path,"size":stat.len(),"mtime_ns":files::mtime_ns(&stat),"at":at});
            }
            add("person-rejections.json", before, value)?;
        }
        "watermark-set" => {
            let key = string(body, "key")?;
            if key.len() != 64
                || !key
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return Err(bad("水印映射参数无效"));
            }
            let kind = string(body, "kind")?;
            let (before, mut value) = review::read(config, "watermarks/mappings.json", json!({}))?;
            if kind == "remove" {
                value.as_object_mut().unwrap().remove(key);
            } else {
                if !["person", "source"].contains(&kind) {
                    return Err(bad("映射类别无效"));
                }
                let candidate = &body["candidate"];
                if candidate["key"] != key {
                    return Err(changed());
                }
                let vid = string(body, "evidence_vid")?;
                let guard = file_guard(config, vid)?;
                let source_path = string(&guard, "path")?;
                let stat = review::safe_source(config, source_path)?;
                let matches_file = |record: &Value| {
                    record["path"] == source_path
                        && review::number(&record["size"]) == Some(stat.len() as i64)
                        && record["mtime_ns"] == files::mtime_ns(&stat)
                };
                let records = read(config, "watermarks/results.json", false)?;
                let record = &records[vid];
                let mut found = matches_file(record)
                    && record["candidates"]
                        .as_array()
                        .is_some_and(|rows| rows.contains(candidate));
                let path = config.base.join("processing/processing.db");
                if !found && path.exists() {
                    for task in
                        db::rows(&db::open(&path)?, "SELECT * FROM tasks WHERE vid=?", [vid])?
                    {
                        let data = review::task_data(&task)?;
                        if matches_file(&task)
                            && data["watermark"]["candidates"]
                                .as_array()
                                .is_some_and(|rows| rows.contains(candidate))
                        {
                            found = true;
                        }
                    }
                }
                if !found {
                    return Err(changed());
                }
                out.guards["evidence"] = guard;
                value[key] = json!({"key":key,"text":candidate["text"],"token":candidate["token"],"platform":candidate["platform"],"kind":kind,"confirmed_at":at});
                if kind == "person" {
                    value[key]["creator"] = json!(name)
                }
            }
            add("watermarks/mappings.json", before, value)?;
        }
        _ => return Err(bad("未知资料保存命令")),
    }
    Ok(out)
}
pub fn plan(config: &Config, request: Request) -> Result<Plan> {
    let body = request
        .payload
        .as_ref()
        .ok_or_else(|| bad("缺少资料内容"))?;
    let prepared = prepare(config, body, &request.id)?;
    Ok(Plan {
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
                jobs: false,
                ledger: false,
            },
            data: serde_json::to_value(&prepared)?,
            result: prepared.result,
        }),
    })
}
pub fn revalidate(config: &Config, plan: &Plan) -> Result<()> {
    let prepared = prepare(config, plan.request.payload.as_ref().unwrap(), &plan.id)?;
    if serde_json::to_value(prepared)? != plan.execution.as_ref().unwrap().data {
        return Err(changed());
    }
    Ok(())
}
pub fn snapshot(_config: &Config, plan: &Plan, successful: &[usize]) -> Result<DomainSnapshot> {
    if !successful.is_empty() {
        return Err(bad("资料保存不能移动文件"));
    }
    let p: Prepared = serde_json::from_value(plan.execution.as_ref().unwrap().data.clone())?;
    Ok(DomainSnapshot {
        metadata: Snapshot {
            documents: p.documents,
            videos: vec![],
            jobs: vec![],
            faces_delta: None,
            jobs_delta: None,
        },
        ledger: None,
    })
}
pub fn execute(engine: &mut Engine, command: &str, mut body: Value) -> Result<Value> {
    if !body.is_object() {
        return Err(bad("资料请求必须是对象"));
    }
    if ["state", "check-state", "analysis-state"].contains(&command) {
        if engine.store.blocked()? {
            return Err(Error::new(
                503,
                "recovery_required",
                "资料操作需要恢复核对，暂不能建立新快照",
            ));
        }
        let check_gate = || -> Result<()> {
            match fs::symlink_metadata(engine.config.base.join(".review-write.lock")) {
                Ok(_) => Err(Error::new(
                    409,
                    "writer_busy",
                    "正在保存资料，请稍后读取快照",
                )),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e.into()),
            }
        };
        check_gate()?;
        let result = match command {
            "state" => material_state(&engine.config, &body["scope"]),
            "check-state" => check_state(&engine.config, &body["scope"], string(&body, "version")?),
            _ => analysis_state(&engine.config),
        };
        check_gate()?;
        return result;
    }
    let id = body
        .get("operation_id")
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .ok_or_else(|| bad("操作编号无效"))
        })
        .transpose()?
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    model::validate_id(&id)?;
    body.as_object_mut().unwrap().remove("operation_id");
    body.as_object_mut().unwrap().remove("at");
    body["command"] = json!(command);
    let plan = if let Some(record) = engine.store.get(&id)? {
        let mut old = record.plan.request.payload.clone().unwrap_or(Value::Null);
        if let Some(o) = old.as_object_mut() {
            o.remove("at");
        }
        if record.plan.action != "metadata-write" || old != body {
            return Err(Error::conflict("同一操作编号不能更换资料内容"));
        }
        record.plan
    } else {
        body["at"] = json!(engine.store.db.query_row(
            "SELECT strftime('%Y-%m-%dT%H:%M:%fZ','now')",
            [],
            |r| r.get::<_, String>(0)
        )?);
        serde_json::from_value(engine.preview(Request {
            id,
            action: "metadata-write".into(),
            person: String::new(),
            files: vec![],
            filename: String::new(),
            target_person: None,
            target_folder: None,
            payload: Some(body),
        })?)?
    };
    engine.apply(plan.request, plan.version)
}
