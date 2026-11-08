//! Durable, immutable review evidence. Model progress never becomes human authority.
use crate::{
    config::Config,
    db,
    engine::Engine,
    model::{Error, Result},
    review, shared_metadata,
};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};
const TTL: i64 = 8 * 60 * 60;
fn expired() -> Error {
    Error::new(
        409,
        "review_session_expired",
        "本次核对已过期或不存在，请明确更新列表；当前选择尚未提交",
    )
}
fn changed() -> Error {
    Error::new(
        409,
        "review_material_changed",
        "文件、文字账号规则、人工处置或体检状态已变化，请更新后重新核对",
    )
}
fn schema(config: &Config) -> Result<rusqlite::Connection> {
    db::open(&config.state_db())
}
fn now(conn: &rusqlite::Connection) -> Result<i64> {
    Ok(conn.query_row("SELECT unixepoch()", [], |r| r.get(0))?)
}
pub fn available(t: &Value) -> bool {
    review::number(&t["manual"]).unwrap_or(0) != 0
        || t["stage"] == "review"
        || (t["data"]["ready"] == true
            && (t["stage"] == "face"
                || t["stage"] == "face_queued"
                || (t["stage"] == "retry" || t["stage"] == "error") && t["retry_stage"] == "face"))
}
fn public(mut t: Value) -> Result<Value> {
    for v in t.as_object_mut().unwrap().values_mut() {
        if let Some(n) = review::number(v) {
            *v = json!(n)
        }
    }
    t["data"] = review::task_data(&t)?;
    Ok(t)
}
fn candidate_names(data: &Value) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for candidate in data["candidates"]
        .as_array()
        .into_iter()
        .flatten()
        .chain(data["folder"].as_array().into_iter().flatten())
    {
        if let Some(name) = candidate["name"].as_str().filter(|name| !name.is_empty()) {
            names.insert(name.to_owned());
        }
    }
    if let Some(name) = data["suggested"].as_str().filter(|name| !name.is_empty()) {
        names.insert(name.to_owned());
    }
    names
}
fn tasks(config: &Config) -> Result<Vec<Value>> {
    let path = config.base.join("processing/processing.db");
    if !path.exists() {
        return Ok(vec![]);
    }
    let queue = review::read(config, "integration-queue.json", json!([]))?.1;
    let history = review::read(config, "archive-history.json", json!([]))?.1;
    let aliases = review::read(config, "person-renames.json", json!({}))?.1;
    let sources = review::sources(config)?;
    let paths: BTreeSet<_> = queue
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| {
            r["vid"]
                .as_str()
                .and_then(|v| review::source(config, v, &sources))
        })
        .collect();
    let excluded_vids: BTreeSet<String> = queue
        .as_array()
        .unwrap()
        .iter()
        .chain(history.as_array().unwrap())
        .filter_map(|r| r["vid"].as_str().map(str::to_owned))
        .collect();
    let mut out = vec![];
    for row in db::rows(
        &db::open(&path)?,
        "SELECT * FROM tasks WHERE stage!='excluded' ORDER BY priority DESC,is_new DESC,created_at DESC,vid",
        [],
    )? {
        let id = row["vid"].as_str().unwrap_or("");
        if excluded_vids.contains(id) || paths.contains(row["path"].as_str().unwrap_or("")) {
            continue;
        }
        let mut t = public(row)?;
        t["__review_raw_people"] = json!(candidate_names(&t["data"]));
        canonical_fields(&mut t["data"], "", &aliases)?;
        out.push(t)
    }
    Ok(out)
}
fn canonical_fields(value: &mut Value, key: &str, aliases: &Value) -> Result<()> {
    if ["name", "creator", "suggested"].contains(&key) && value.is_string() {
        let mut name = value.as_str().unwrap().to_owned();
        let mut seen = BTreeSet::new();
        while let Some(next) = aliases[&name].as_str() {
            if !seen.insert(name.clone()) {
                return Err(Error::conflict("人物别名包含循环"));
            }
            name = next.to_owned()
        }
        *value = json!(name);
    } else if let Some(rows) = value.as_array_mut() {
        for row in rows {
            canonical_fields(row, "", aliases)?
        }
    } else if let Some(fields) = value.as_object_mut() {
        for (key, v) in fields {
            canonical_fields(v, key, aliases)?
        }
    }
    Ok(())
}
fn bucket(t: &Value) -> &str {
    if review::number(&t["manual"]).unwrap_or(0) != 0 {
        "manual"
    } else {
        t["data"]["kind"].as_str().unwrap_or("none")
    }
}
fn group(t: &Value) -> Value {
    let kind = bucket(t);
    let suggested = t["data"]["suggested"].as_str().unwrap_or("");
    let face = t["data"]["face"]["unknown_group"].as_str().unwrap_or("");
    let (key, label) = if kind == "suggested" && !suggested.is_empty() {
        (format!("person:{suggested}"), suggested.into())
    } else if kind == "none" && !face.is_empty() {
        (
            format!("face:{face}"),
            format!(
                "未命名相似组 {}",
                face.chars()
                    .rev()
                    .take(5)
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect::<String>()
            ),
        )
    } else {
        (
            kind.into(),
            match kind {
                "manual" => "人工接管",
                "conflict" => "证据冲突",
                _ => "无候选人物",
            }
            .into(),
        )
    };
    json!({"key":key,"label":label,"kind":kind})
}
fn text_evidence(t: &Value) -> Value {
    let w = &t["data"]["watermark"];
    json!({"folder":t["data"]["folder"],"watermark":if w.is_null(){Value::Null}else{json!({"pipeline":w["pipeline"],"status":w["status"],"candidates":w["candidates"]})}})
}
struct HealthReader {
    candidates: BTreeSet<String>,
}
impl HealthReader {
    fn new(config: &Config) -> Result<Self> {
        let report = review::read(config, "video-health.json", json!({}))?.1;
        if report.get("records").is_some_and(|rows| !rows.is_object()) {
            return Err(Error::new(503, "health_contract", "体检记录损坏，无法读取"));
        }
        Ok(Self {
            candidates: report["records"]
                .as_object()
                .map(|rows| {
                    rows.iter()
                        .filter(|(_, row)| row["hidden"] == true)
                        .map(|(vid, _)| vid.clone())
                        .collect()
                })
                .unwrap_or_default(),
        })
    }
    fn state(&self, config: &Config, t: &Value) -> Result<Value> {
        let id = t["vid"].as_str().unwrap_or("");
        let path = t["path"].as_str().unwrap_or("");
        let hidden = if self.candidates.contains(id) {
            std::fs::metadata(path)
                .ok()
                .map(|s| review::health_hidden(config, id, path, &s))
                .transpose()?
                .unwrap_or(false)
        } else {
            false
        };
        Ok(json!({"hidden":hidden}))
    }
}
fn dispositions(config: &Config) -> Result<BTreeMap<String, Value>> {
    let conn = schema(config)?;
    let mut result = BTreeMap::new();
    for row in db::rows(
        &conn,
        "SELECT id,plan FROM operations WHERE status='complete' AND json_extract(plan,'$.action') IN ('queue-confirm','queue-undo','review-note','review-defer','review-restore','claim-decide') ORDER BY rowid",
        [],
    )? {
        let plan: Value = serde_json::from_str(row["plan"].as_str().unwrap())?;
        let body = &plan["request"]["payload"];
        for vid in body["vids"]
            .as_array()
            .into_iter()
            .flatten()
            .chain(std::iter::once(&body["vid"]))
        {
            if let Some(vid) = vid.as_str() {
                result.insert(vid.into(), row["id"].clone());
            }
        }
    }
    Ok(result)
}
pub fn validate_undo(config: &Config, body: &Value) -> Result<()> {
    let Some(session) = body["review_session"].as_str() else {
        return Ok(());
    };
    read(config, session)?;
    let conn = schema(config)?;
    let queue = review::read(config, "integration-queue.json", json!([]))?.1;
    for vid in review::vids(&body["vids"])? {
        let batch = queue
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["vid"] == vid)
            .and_then(|r| r["batch_id"].as_str())
            .ok_or_else(changed)?;
        let owner:Option<String>=conn.query_row("SELECT json_extract(plan,'$.request.payload.review_session') FROM operations WHERE id=? AND status='complete'",[batch],|r|r.get(0)).optional()?.flatten();
        if owner.as_deref() != Some(session) {
            return Err(Error::conflict(
                "这次归名已被其他会话处置，请核对待归档记录",
            ));
        }
    }
    Ok(())
}
fn evidence(
    config: &Config,
    t: &Value,
    material: &shared_metadata::MaterialReader,
    health: &HealthReader,
    dispositions: &BTreeMap<String, Value>,
) -> Result<Value> {
    let mut names = BTreeSet::new();
    for c in t["data"]["candidates"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .chain(t["data"]["folder"].as_array().unwrap_or(&vec![]))
    {
        if let Some(n) = c["name"].as_str() {
            match material.canonical(n) {
                Ok(_) => {
                    names.insert(n.to_owned());
                }
                Err(e) if e.status == 400 => {}
                Err(e) => return Err(e),
            }
        }
    }
    if let Some(n) = t["data"]["suggested"].as_str() {
        match material.canonical(n) {
            Ok(_) => {
                names.insert(n.to_owned());
            }
            Err(e) if e.status == 400 => {}
            Err(e) => return Err(e),
        }
    }
    let keys: Vec<_> = t["data"]["watermark"]["candidates"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .filter_map(|c| c["key"].as_str().map(str::to_owned))
        .collect();
    let mut scope = json!({"vids":[t["vid"]],"people":names,"watermark_keys":keys});
    if t["__review_raw_people"]
        .as_array()
        .is_some_and(|rows| !rows.is_empty())
    {
        scope["alias_inputs"] = t["__review_raw_people"].clone();
    }
    let mut displayed = t.clone();
    displayed
        .as_object_mut()
        .unwrap()
        .remove("__review_raw_people");
    Ok(
        json!({"task":displayed,"group":group(t),"scope":scope,"material":material.state(config,&scope)?["material"],"text":text_evidence(t),"health":health.state(config,t)?,"disposition":dispositions.get(t["vid"].as_str().unwrap_or("")),"pending_seen":review::number(&t["deferred"]).unwrap_or(0)==0,"deferred_seen":review::number(&t["deferred"]).unwrap_or(0)!=0}),
    )
}
fn read(config: &Config, id: &str) -> Result<Value> {
    let conn = schema(config)?;
    let raw: Option<String> = conn
        .query_row(
            "SELECT snapshot FROM review_sessions WHERE id=? AND expires>unixepoch()",
            [id],
            |r| r.get(0),
        )
        .optional()?;
    let value: Value = serde_json::from_str(&raw.ok_or_else(expired)?)?;
    if value["schema_version"] != 1 {
        return Err(expired());
    }
    Ok(value)
}
// Only this session's durable, completed operations amend its presentation. Evidence is never replaced by latest model output.
fn amended(config: &Config, id: &str, mut snapshot: Value) -> Result<Value> {
    let conn = schema(config)?;
    let positions: BTreeMap<String, usize> = snapshot["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .filter_map(|(i, e)| e["task"]["vid"].as_str().map(|vid| (vid.into(), i)))
        .collect();
    let mut next_pending = snapshot["evidence"].as_array().unwrap().len() as u64;
    let mut next_deferred = next_pending;
    for row in db::rows(
        &conn,
        "SELECT plan FROM operations WHERE status='complete' AND json_extract(plan,'$.request.payload.review_session')=? ORDER BY rowid",
        [id],
    )? {
        let plan: Value = serde_json::from_str(row["plan"].as_str().unwrap())?;
        let body = &plan["request"]["payload"];
        let action = plan["action"].as_str().unwrap_or("");
        let selected: BTreeSet<usize> = body["vids"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str().and_then(|id| positions.get(id)).copied())
            .collect();
        for index in selected {
            let e = &mut snapshot["evidence"][index];
            e["disposition"] = plan["id"].clone();
            match action {
                "queue-confirm" => e["completed"] = json!(true),
                "queue-undo" => {
                    e["completed"] = json!(false);
                    let restored:Option<String>=conn.query_row("SELECT plan FROM operations WHERE status='complete' AND json_extract(plan,'$.action')='queue-restore' AND json_extract(plan,'$.request.payload.parent_id')=?",[plan["id"].as_str().unwrap_or("")],|r|r.get(0)).optional()?;
                    if let Some(raw) = restored {
                        let child: Value = serde_json::from_str(&raw)?;
                        if child["execution"]["data"]["job_updates"]
                            .as_array()
                            .is_some_and(|rows| {
                                rows.iter().any(|r| {
                                    r["vid"] == e["task"]["vid"]
                                        && review::number(&r["deferred"]) == Some(0)
                                })
                            })
                        {
                            if e["pending_order"].is_null() {
                                e["pending_order"] = json!(next_pending);
                                next_pending += 1;
                            }
                            e["pending_seen"] = json!(true);
                            set_manual(e, "deferred", json!(0));
                        }
                    }
                }
                "review-note" => set_manual(e, "note", body["note"].clone()),
                "review-defer" => {
                    if e["deferred_order"].is_null() {
                        e["deferred_order"] = json!(next_deferred);
                        next_deferred += 1;
                    }
                    e["deferred_seen"] = json!(true);
                    set_manual(e, "deferred", json!(1));
                }
                "review-restore" => {
                    if e["pending_order"].is_null() {
                        e["pending_order"] = json!(next_pending);
                        next_pending += 1;
                    }
                    e["pending_seen"] = json!(true);
                    set_manual(e, "deferred", json!(0));
                }
                _ => {}
            }
        }
    }
    Ok(snapshot)
}
fn set_manual(e: &mut Value, key: &str, value: Value) {
    e["task"][key] = value.clone();
    if let Some(rows) = e["material"]["manual_tasks"].as_array_mut() {
        for r in rows {
            r[key] = if key == "note" {
                value.clone()
            } else {
                review::integer(value.as_i64().unwrap_or(0))
            };
        }
    }
}
pub fn validate(config: &Config, body: &Value) -> Result<Vec<Value>> {
    let session = body["review_session"].as_str().ok_or_else(expired)?;
    read(config, session)?;
    let ids = review::vids(&body["vids"])?;
    let conn = schema(config)?;
    let mut rows = vec![];
    let current = tasks(config)?;
    let material = shared_metadata::MaterialReader::new(config)?;
    let health = HealthReader::new(config)?;
    let dispositions = dispositions(config)?;
    for vid in ids {
        let token = body["review_tokens"][&vid].as_str().ok_or_else(|| {
            Error::new(
                409,
                "review_not_viewed",
                "所选视频缺少本次已展示证据，请重新打开该页",
            )
        })?;
        let raw: Option<String> = conn
            .query_row(
                "SELECT evidence FROM review_deliveries WHERE token=? AND session=? AND vid=?",
                params![token, session, vid],
                |r| r.get(0),
            )
            .optional()?;
        let e: Value = serde_json::from_str(
            &raw.ok_or_else(|| Error::new(409, "review_not_viewed", "视频未在本次会话展示"))?,
        )?;
        if let Some(scope) = body.get("review_group").filter(|v| !v.is_null())
            && (scope["key"] != e["group"]["key"]
                || scope["deferred"]
                    != json!(review::number(&e["task"]["deferred"]).unwrap_or(0) != 0))
        {
            return Err(Error::conflict("所选视频不属于本次已展示核对组"));
        }
        let t = current
            .iter()
            .find(|t| t["vid"] == vid)
            .ok_or_else(changed)?;
        // Worker stages (including exhausted inference retries) do not change
        // already delivered human evidence. Material and disposition guards do.
        if ["path", "size", "mtime_ns"]
            .iter()
            .any(|key| t[*key] != e["task"][*key])
            || text_evidence(t) != e["text"]
            || health.state(config, t)? != e["health"]
            || dispositions.get(&vid).cloned().unwrap_or(Value::Null) != e["disposition"]
            || material.state(config, &e["scope"])?["material"] != e["material"]
        {
            return Err(changed());
        }
        rows.push(e["task"].clone());
    }
    Ok(rows)
}
pub fn execute(engine: &mut Engine, command: &str, body: Value) -> Result<Value> {
    if engine.store.blocked()? {
        return Err(Error::new(
            503,
            "recovery_required",
            "操作需要恢复核对，暂不能建立核对快照",
        ));
    }
    let config = &engine.config;
    let conn = schema(config)?;
    let id = body["session"]
        .as_str()
        .filter(|v| !v.is_empty())
        .map(str::to_owned);
    let live = tasks(config)?;
    let material = shared_metadata::MaterialReader::new(config)?;
    let health = HealthReader::new(config)?;
    let dispositions = dispositions(config)?;
    let created = id.is_none();
    let id = match id {
        Some(id) => id,
        None => {
            if command == "review-status" {
                return Err(expired());
            }
            let id = uuid::Uuid::new_v4().to_string();
            let mut evidence = live
                .iter()
                .filter(|t| available(t))
                .map(|t| evidence(config, t, &material, &health, &dispositions))
                .collect::<Result<Vec<_>>>()?;
            evidence.retain(|e| e["health"]["hidden"] != true);
            for (index, e) in evidence.iter_mut().enumerate() {
                let key = if e["pending_seen"] == true {
                    "pending_order"
                } else {
                    "deferred_order"
                };
                e[key] = json!(index);
            }
            let snapshot = json!({"schema_version":1,"evidence":evidence,"clusters":review::read(config,"clusters.json",json!([]))?.1});
            let raw = serde_json::to_string(&snapshot)?;
            if raw.len() > 64 * 1024 * 1024 {
                return Err(Error::new(
                    503,
                    "review_scope_limit",
                    "核对记录过多，请缩小处理范围",
                ));
            }
            conn.execute("DELETE FROM review_sessions WHERE expires<=unixepoch()", [])?;
            conn.execute(
                "INSERT INTO review_sessions VALUES(?,?,?)",
                params![id, now(&conn)? + TTL, raw],
            )?;
            id
        }
    };
    let snapshot = amended(config, &id, read(config, &id)?)?;
    let evidence = snapshot["evidence"].as_array().unwrap();
    let visible: Vec<_> = evidence
        .iter()
        .filter(|e| e["completed"] != true)
        .map(|e| e["task"].clone())
        .collect();
    let mut changed_count = 0;
    if !created {
        let visible_by_id: BTreeMap<_, _> = visible
            .iter()
            .map(|t| (t["vid"].as_str().unwrap_or(""), t))
            .collect();
        let live_ids: BTreeSet<_> = live
            .iter()
            .map(|t| t["vid"].as_str().unwrap_or(""))
            .collect();
        for t in &live {
            if available(t)
                && health.state(config, t)?["hidden"] != true
                && !visible_by_id
                    .get(t["vid"].as_str().unwrap_or(""))
                    .is_some_and(|v| {
                        v["data"] == t["data"]
                            && v["path"] == t["path"]
                            && v["size"] == t["size"]
                            && v["mtime_ns"] == t["mtime_ns"]
                            && v["deferred"] == t["deferred"]
                            && v["note"] == t["note"]
                    })
            {
                changed_count += 1
            }
        }
        for e in evidence.iter().filter(|e| e["completed"] != true) {
            if !live_ids.contains(e["task"]["vid"].as_str().unwrap_or(""))
                || material.state(config, &e["scope"])?["material"] != e["material"]
                || health.state(config, &e["task"])? != e["health"]
            {
                changed_count += 1
            }
        }
    }
    let waiting = live
        .iter()
        .filter(|t| t["stage"] != "review" && review::number(&t["manual"]).unwrap_or(0) == 0)
        .count();
    if command == "review-status" {
        return Ok(
            json!({"session":id,"has_updates":changed_count>0,"changed":changed_count,"waiting":waiting,"refreshing":live.iter().filter(|t|available(t)&&t["stage"]!="review"&&review::number(&t["manual"]).unwrap_or(0)==0).count()}),
        );
    }
    let s = |key: &str| body[key].as_str().unwrap_or("");
    let view = if s("view").is_empty() {
        "all"
    } else {
        s("view")
    };
    let deferred = view == "deferred";
    let person = s("person");
    let cluster = s("cluster");
    let mut counts = json!({"all":0,"suggested":0,"conflict":0,"none":0,"manual":0,"deferred":0});
    for t in &visible {
        if review::number(&t["deferred"]).unwrap_or(0) != 0 {
            counts["deferred"] = json!(counts["deferred"].as_u64().unwrap() + 1)
        } else {
            for k in ["all", bucket(t)] {
                counts[k] = json!(counts[k].as_u64().unwrap_or(0) + 1)
            }
        }
    }
    let mut scope: Vec<_> = evidence
        .iter()
        .filter(|e| {
            let t = &e["task"];
            let lane = review::number(&t["deferred"]).unwrap_or(0) != 0;
            let ever = e[if deferred {
                "deferred_seen"
            } else {
                "pending_seen"
            }] == true;
            (lane == deferred || ever)
                && (deferred || view == "all" || bucket(t) == view)
                && (person.is_empty()
                    || t["data"]["candidates"]
                        .as_array()
                        .is_some_and(|cs| cs.iter().any(|c| c["name"] == person))
                    || t["data"]["suggested"] == person)
                && (cluster.is_empty()
                    || snapshot["clusters"].as_array().is_some_and(|cs| {
                        cs.iter().any(|c| {
                            c["cid"] == cluster
                                && c["vids"]
                                    .as_array()
                                    .is_some_and(|vs| vs.iter().any(|v| v["vid"] == t["vid"]))
                        })
                    }))
        })
        .collect();
    scope.sort_by_key(|e| {
        e[if deferred {
            "deferred_order"
        } else {
            "pending_order"
        }]
        .as_u64()
        .unwrap_or(u64::MAX)
    });
    let is_remaining = |e: &&Value| {
        e["completed"] != true
            && (review::number(&e["task"]["deferred"]).unwrap_or(0) != 0) == deferred
    };
    let mut grouped: BTreeMap<String, Value> = BTreeMap::new();
    let mut totals: BTreeMap<String, usize> = BTreeMap::new();
    for e in evidence {
        *totals
            .entry(e["group"]["key"].as_str().unwrap_or("").into())
            .or_default() += 1;
    }
    for e in &scope {
        let key = e["group"]["key"].as_str().unwrap_or("").to_owned();
        let group = grouped.entry(key).or_insert_with(|| {
            let mut g = e["group"].clone();
            g["count"] = json!(0);
            g
        });
        if is_remaining(e) {
            group["count"] = json!(group["count"].as_u64().unwrap_or(0) + 1);
        }
    }
    let mut groups: Vec<Value> = grouped.into_values().collect();
    // Original snapshot counts determine order; receipt counts never reorder groups.
    groups.sort_by(|a, b| {
        (a["kind"] != "suggested")
            .cmp(&(b["kind"] != "suggested"))
            .then_with(|| {
                totals
                    .get(b["key"].as_str().unwrap_or(""))
                    .cmp(&totals.get(a["key"].as_str().unwrap_or("")))
            })
            .then_with(|| a["label"].as_str().cmp(&b["label"].as_str()))
    });
    let focus = scope.iter().find(|e| e["task"]["vid"] == s("vid"));
    let focus_missing = !s("vid").is_empty() && focus.is_none();
    let requested = focus
        .map(|e| e["group"]["key"].as_str().unwrap_or(""))
        .unwrap_or(s("group"));
    let selected = if requested.is_empty() {
        groups.first().cloned()
    } else {
        groups.iter().find(|g| g["key"] == requested).cloned()
    };
    let query = s("q").to_lowercase();
    let folder = s("folder");
    let rows: Vec<_> = scope
        .into_iter()
        .filter(|e| {
            selected
                .as_ref()
                .is_some_and(|g| e["group"]["key"] == g["key"])
                && (folder.is_empty()
                    || Path::new(e["task"]["path"].as_str().unwrap_or(""))
                        .parent()
                        .is_some_and(|p| p == Path::new(folder)))
                && (query.is_empty()
                    || format!(
                        "{} {} {}",
                        e["task"]["name"].as_str().unwrap_or(""),
                        e["task"]["path"].as_str().unwrap_or(""),
                        e["task"]["data"]["candidates"]
                    )
                    .to_lowercase()
                    .contains(&query))
        })
        .collect();
    let limit = 24usize;
    let page = rows
        .iter()
        .position(|e| e["task"]["vid"] == s("vid"))
        .map(|n| n / limit)
        .unwrap_or_else(|| s("page").parse::<usize>().unwrap_or(0))
        .min(rows.len().saturating_sub(1) / limit);
    let mut delivered = vec![];
    for e in rows
        .iter()
        .skip(page * limit)
        .take(limit)
        .filter(|e| is_remaining(e))
    {
        let raw = serde_json::to_string(e)?;
        let token=conn.query_row("SELECT token FROM review_deliveries WHERE session=? AND vid=? AND evidence=? LIMIT 1",params![id,e["task"]["vid"].as_str(),raw],|r|r.get::<_,String>(0)).optional()?.unwrap_or_else(||uuid::Uuid::new_v4().to_string());
        let mut task = e["task"].clone();
        task["review_token"] = json!(token);
        conn.execute(
            "INSERT OR IGNORE INTO review_deliveries VALUES(?,?,?,?)",
            params![token, id, task["vid"].as_str(), serde_json::to_string(e)?],
        )?;
        delivered.push(task);
    }
    let pages = rows.len().div_ceil(limit);
    let has = |p: usize| rows.iter().skip(p * limit).take(limit).any(&is_remaining);
    let next = (page + 1..pages).find(|p| has(*p));
    let previous = (0..page).rev().find(|p| has(*p));
    let folders: BTreeSet<_> = evidence
        .iter()
        .filter_map(|e| {
            Path::new(e["task"]["path"].as_str()?)
                .parent()
                .map(|p| p.display().to_string())
        })
        .collect();
    let heartbeat = review::read(config, "processing/heartbeat.json", json!({}))?.1;
    let age = now(&conn)? as f64 - heartbeat["at"].as_f64().unwrap_or(0.0);
    Ok(
        json!({"session":id,"counts":counts,"groups":groups,"group":selected,"rows":delivered,"total":rows.len(),"remaining":rows.iter().filter(|e|is_remaining(e)).count(),"page":page,"limit":limit,"next_page":next,"previous_page":previous,"waiting":waiting,"online":(-30.0..90.0).contains(&age),"folders":folders,"focus_missing":focus_missing,"has_updates":changed_count>0}),
    )
}
