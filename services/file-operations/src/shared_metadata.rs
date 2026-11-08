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
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

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
fn evidence_names(value: &Value) -> Result<Vec<String>> {
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
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| bad("范围编号无效"))
        })
        .collect()
}
fn canonical(config: &Config, name: &str) -> Result<String> {
    review::creator(config, &json!(name))
}
fn relevant_using(
    row: &Value,
    people: &BTreeSet<String>,
    vids: &BTreeSet<String>,
    canonical: impl Fn(&str) -> Result<String>,
) -> Result<bool> {
    for key in ["creator", "person", "name"] {
        if let Some(name) = row[key].as_str() {
            match canonical(name) {
                Ok(c) if people.contains(&c) => return Ok(true),
                Ok(_) => {}
                Err(e) if e.status == 400 => {}
                Err(e) => return Err(e),
            }
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
    let mut dir_cache = BTreeMap::new();
    file_guard_cached(config, vid, sources, &mut dir_cache)
}
fn file_guard_cached(
    config: &Config,
    vid: &str,
    sources: &std::collections::BTreeMap<String, String>,
    dir_cache: &mut BTreeMap<std::path::PathBuf, bool>,
) -> Result<Value> {
    let Some(path) = review::source(config, vid, sources) else {
        return Ok(json!({"vid":vid,"missing":true}));
    };
    let p = Path::new(&path);
    if !p.starts_with(config.media.join("pikpak")) {
        return Err(changed());
    }
    match fs::symlink_metadata(p) {
        Ok(stat) if stat.is_file() && !stat.file_type().is_symlink() => {
            let parent_ok = p.parent().is_some_and(|parent| {
                *dir_cache
                    .entry(parent.to_path_buf())
                    .or_insert_with(|| fs::canonicalize(parent).ok().as_deref() == Some(parent))
            });
            if !parent_ok {
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
#[derive(Default)]
struct ReferenceGuards {
    assigned: BTreeSet<String>,
    embedded: BTreeSet<String>,
    anchored: BTreeSet<String>,
}
fn reference_file_guard_from(
    config: &Config,
    vid: &str,
    sources: &BTreeMap<String, String>,
    scope: &ReferenceGuards,
    dir_cache: &mut BTreeMap<std::path::PathBuf, bool>,
) -> Result<Value> {
    let Some(path) = review::source(config, vid, sources) else {
        return Ok(json!({"vid":vid,"missing":true}));
    };
    let p = Path::new(&path);
    if !p.starts_with(config.media.join("pikpak")) {
        if !scope.assigned.contains(vid) && !scope.anchored.contains(vid) {
            return Ok(json!({"vid":vid,"path":path,"outside_scope":true}));
        }
        return Err(changed());
    }
    match fs::symlink_metadata(p) {
        Ok(stat) if stat.file_type().is_symlink() => Err(changed()),
        Ok(stat) if !stat.is_file() => {
            if scope.embedded.contains(vid)
                && (scope.assigned.contains(vid)
                    || scope.anchored.contains(vid)
                    || p.starts_with(config.creators()))
            {
                return Err(changed());
            }
            if fs::canonicalize(p)? != p {
                return Err(changed());
            }
            Ok(json!({"vid":vid,"path":path,"non_regular":true}))
        }
        _ => file_guard_cached(config, vid, sources, dir_cache),
    }
}
/// #18 calls this under its admission gate. Analysis ranks, stages and retries excluded.
/// Request-local reader: snapshot batches reuse source indexes and business inputs,
/// while each admission constructs a fresh reader. No process-wide material cache.
type VideoRows = BTreeMap<String, Vec<(usize, Value)>>;
pub struct MaterialReader {
    sources: BTreeMap<String, String>,
    aliases: Value,
    documents: BTreeMap<&'static str, Value>,
    arrays: BTreeMap<&'static str, VideoRows>,
    manual: Option<BTreeMap<String, Value>>,
    canonical_cache: std::cell::RefCell<BTreeMap<String, Result<String>>>,
    file_guards: std::cell::RefCell<BTreeMap<String, Result<Value>>>,
    canonical_dirs: std::cell::RefCell<BTreeMap<std::path::PathBuf, bool>>,
}
impl MaterialReader {
    pub fn new(config: &Config) -> Result<Self> {
        let mut documents = BTreeMap::new();
        let mut arrays = BTreeMap::new();
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
            if *array {
                let mut index: BTreeMap<String, Vec<(usize, Value)>> = BTreeMap::new();
                for (position, row) in value.as_array().unwrap().iter().enumerate() {
                    if let Some(vid) = row["vid"].as_str() {
                        index
                            .entry(vid.into())
                            .or_default()
                            .push((position, row.clone()));
                    }
                }
                arrays.insert(*name, index);
            }
            documents.insert(*name, value);
        }
        let path = config.base.join("processing/processing.db");
        let manual = if path.exists() {
            let conn = db::open(&path)?;
            let columns = db::columns(&conn, "tasks")?;
            let selected: Vec<_> = ["vid", "manual", "deferred", "note"]
                .into_iter()
                .filter(|key| columns.iter().any(|c| c == key))
                .collect();
            Some(
                db::rows(
                    &conn,
                    &format!("SELECT {} FROM tasks ORDER BY vid", selected.join(",")),
                    [],
                )?
                .into_iter()
                .filter_map(|row| row["vid"].as_str().map(|vid| (vid.to_owned(), row.clone())))
                .collect(),
            )
        } else {
            None
        };
        Ok(Self {
            sources: review::sources(config)?,
            aliases: read(config, "person-renames.json", false)?,
            documents,
            arrays,
            manual,
            canonical_cache: std::cell::RefCell::new(BTreeMap::new()),
            file_guards: std::cell::RefCell::new(BTreeMap::new()),
            canonical_dirs: std::cell::RefCell::new(BTreeMap::new()),
        })
    }
    pub(crate) fn canonical(&self, name: &str) -> Result<String> {
        if let Some(cached) = self.canonical_cache.borrow().get(name) {
            return cached.clone();
        }
        let result = review::creator_with_aliases(&json!(name), &self.aliases);
        self.canonical_cache
            .borrow_mut()
            .insert(name.to_owned(), result.clone());
        result
    }
    pub(crate) fn aliases(&self) -> &Value {
        &self.aliases
    }
    fn evidence_alias_chain(&self, name: &str) -> Result<Value> {
        let mut current = name.to_owned();
        let mut seen = BTreeSet::new();
        let mut chain = vec![];
        loop {
            if !seen.insert(current.clone()) {
                return Err(Error::conflict("人物别名包含循环"));
            }
            let next = self.aliases[&current].as_str();
            chain.push(json!([current, next]));
            match next {
                Some(value) => current = value.to_owned(),
                None => break,
            }
        }
        Ok(json!(chain))
    }
    fn file_guard(&self, config: &Config, vid: &str) -> Result<Value> {
        if let Some(cached) = self.file_guards.borrow().get(vid) {
            return cached.clone();
        }
        let result = file_guard_cached(
            config,
            vid,
            &self.sources,
            &mut self.canonical_dirs.borrow_mut(),
        );
        self.file_guards
            .borrow_mut()
            .insert(vid.to_owned(), result.clone());
        result
    }
    pub fn state(&self, config: &Config, scope: &Value) -> Result<Value> {
        if !scope.is_object() {
            return Err(bad("缺少核对范围"));
        }
        let raw_people = names(&scope["people"])?;
        let aliases: BTreeSet<_> = evidence_names(&scope["alias_inputs"])?
            .into_iter()
            .collect();
        let vids: BTreeSet<_> = names(&scope["vids"])?.into_iter().collect();
        let keys: BTreeSet<_> = names(&scope["watermark_keys"])?.into_iter().collect();
        let people: BTreeSet<_> = raw_people
            .iter()
            .map(|s| self.canonical(s))
            .collect::<Result<_>>()?;
        let mut material = json!({"names":raw_people.iter().map(|s|Ok(json!([s,self.canonical(s)?]))).collect::<Result<Vec<_>>>()?,"files":vids.iter().map(|id|self.file_guard(config,id)).collect::<Result<Vec<_>>>()?});
        if !aliases.is_empty() {
            material["candidate_alias_chains"] = json!(
                aliases
                    .iter()
                    .map(|name| self.evidence_alias_chain(name))
                    .collect::<Result<Vec<_>>>()?
            );
        }
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
            let value = &self.documents[name];
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
                let mut selected = vids
                    .iter()
                    .flat_map(|vid| self.arrays[name].get(vid).into_iter().flatten())
                    .collect::<Vec<_>>();
                selected.sort_by_key(|(index, _)| *index);
                json!(selected.into_iter().map(|(_, row)| row).collect::<Vec<_>>())
            } else if name.starts_with("external-profiles/") {
                json!(
                    value["links"]
                        .as_array()
                        .unwrap_or(&vec![])
                        .iter()
                        .filter_map(|r| {
                            match relevant_using(r, &people, &vids, |n| self.canonical(n)) {
                                Ok(true) => Some(Ok(r.clone())),
                                Ok(false) => None,
                                Err(e) => Some(Err(e)),
                            }
                        })
                        .collect::<Result<Vec<_>>>()?
                )
            } else if *name == "review-decisions.json" {
                json!({"approved":value["approved"].as_array().unwrap_or(&vec![]).iter().filter(|v|v.as_str().is_some_and(|id|vids.contains(id))).collect::<Vec<_>>(),"rejected":value["rejected"].as_array().unwrap_or(&vec![]).iter().filter(|v|v.as_str().is_some_and(|id|vids.contains(id))).collect::<Vec<_>>()})
            } else {
                let mut out = serde_json::Map::new();
                for (k, v) in value.as_object().unwrap() {
                    let keep = if *name == "watermarks/mappings.json" {
                        scope["all_watermark_rules"] == true
                            || keys.contains(k)
                            || relevant_using(v, &people, &vids, |n| self.canonical(n))?
                    } else if *name == "person-rejections.json" {
                        v["vid"].as_str().is_some_and(|id| vids.contains(id))
                            && relevant_using(v, &people, &vids, |n| self.canonical(n))?
                    } else {
                        match self.canonical(k) {
                            Ok(c) => people.contains(&c),
                            Err(e) if e.status == 400 => false,
                            Err(e) => return Err(e),
                        }
                    };
                    if keep {
                        out.insert(k.clone(), v.clone());
                    }
                }
                Value::Object(out)
            };
            material[*name] = selected;
        }
        if let Some(tasks) = &self.manual {
            material["manual_tasks"] = json!(
                vids.iter()
                    .filter_map(|vid| tasks.get(vid))
                    .collect::<Vec<_>>()
            );
        }
        Ok(json!({"version":model::hash(&material),"scope":scope,"material":material}))
    }
}
pub fn material_state(config: &Config, scope: &Value) -> Result<Value> {
    MaterialReader::new(config)?.state(config, scope)
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
    let mut reference = ReferenceGuards::default();
    for name in ["integration-queue.json", "archive-history.json"] {
        for row in input[name].as_array().into_iter().flatten() {
            if row["file_state"] != "staged"
                && let Some(vid) = row["vid"].as_str()
            {
                reference.assigned.insert(vid.to_owned());
            }
        }
    }
    let report = input["report.json"]
        .as_str()
        .map(serde_json::from_str::<Value>)
        .transpose()?
        .unwrap_or_else(|| json!({}));
    let thumbnails: BTreeSet<_> = report["identities"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| p["rep_thumb"].as_str().map(str::to_owned))
        .collect();
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
        let columns = db::columns(&tx, "faces")?;
        if columns.iter().any(|column| column == "emb") {
            let thumb = if columns.iter().any(|column| column == "thumb") {
                "thumb"
            } else {
                "NULL AS thumb"
            };
            for row in db::rows(
                &tx,
                &format!("SELECT DISTINCT vid,{thumb} FROM faces WHERE length(emb)=2048"),
                [],
            )? {
                if let Some(vid) = row["vid"].as_str() {
                    reference.embedded.insert(vid.to_owned());
                    if row["thumb"]
                        .as_str()
                        .is_some_and(|thumb| thumbnails.contains(thumb))
                    {
                        reference.anchored.insert(vid.to_owned());
                    }
                }
            }
        }
        tx.commit()?;
    }
    let sources = review::indexed_sources(config)?;
    let mut dir_cache = BTreeMap::new();
    input.insert(
        "files".into(),
        json!(
            sources
                .keys()
                .map(|vid| reference_file_guard_from(
                    config,
                    vid,
                    &sources,
                    &reference,
                    &mut dir_cache
                ))
                .collect::<Result<Vec<_>>>()?
        ),
    );
    input.insert(
        "assigned_feature_caches".into(),
        crate::processing_result::assigned_reference_inputs(config)?,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    jobs_delta: Option<db::Delta>,
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
    if command.starts_with("health-") {
        let (documents, guards, result) = crate::video_health::prepare(config, command, body, at)?;
        return Ok(Prepared {
            documents,
            jobs_delta: None,
            guards,
            result,
        });
    }
    let mut out = Prepared {
        documents: vec![],
        jobs_delta: None,
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
            let (jobs, updates) = review::rule_task_updates(config, key, at)?;
            if !jobs.is_empty() {
                out.jobs_delta = Some(db::Delta {
                    tables: vec![crate::archive_ledger::table(
                        db::Table::Tasks,
                        vec!["vid".into()],
                        jobs,
                        updates,
                    )],
                    guards: vec![],
                });
            }
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
                jobs: prepared.jobs_delta.is_some(),
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
            jobs_delta: p.jobs_delta,
        },
        ledger: None,
    })
}
pub fn execute(engine: &mut Engine, command: &str, mut body: Value) -> Result<Value> {
    if !body.is_object() {
        return Err(bad("资料请求必须是对象"));
    }
    if command == "processing-result" {
        return crate::processing_result::execute(engine, body);
    }
    if command.starts_with("processing-") {
        return crate::processing_scheduler::execute(engine, command, body);
    }
    if [
        "state",
        "check-state",
        "analysis-state",
        "reference-relocation",
    ]
    .contains(&command)
    {
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
            "reference-relocation" => crate::reference_relocation::verify(engine, &body),
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
        if command.starts_with("health-") {
            body["at"] = json!(crate::video_health::timestamp(&engine.config)?);
        }
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
