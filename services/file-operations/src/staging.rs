//! Global staging is a domain of the shared durable mover. No separate executor or ledger writer.
use crate::{
    config::Config,
    db, engine, files, metadata,
    model::{
        AffectedStores, Error, ExecutionIntent, FailurePolicy, FileMove, MoveScope, Plan, Request,
        Result, ScopeRoot,
    },
    native::DomainSnapshot,
    person_registration as registration,
};
use rusqlite::OptionalExtension;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
};
const ORIGINS: &str = "staging-origins.json";
pub fn root(config: &Config) -> PathBuf {
    config.media.join("pikpak/.face-cleanup-staging")
}
fn original_scope(config: &Config, path: &Path) -> Result<ScopeRoot> {
    if path
        .components()
        .any(|c| !matches!(c, Component::RootDir | Component::Normal(_)))
        || !path.is_absolute()
    {
        return Err(Error::conflict("原路径无效"));
    }
    if let Ok(relative) = path.strip_prefix(config.creators()) {
        let person = relative
            .iter()
            .next()
            .and_then(|s| s.to_str())
            .ok_or_else(|| Error::conflict("原人物目录无效"))?;
        files::validate_name(person)?;
        return Ok(ScopeRoot::Person(person.into()));
    }
    if path.starts_with(config.media.join("pikpak")) && !path.starts_with(root(config)) {
        return Ok(ScopeRoot::Unorganized);
    }
    Err(Error::conflict("原路径不在可恢复范围，请指定人物"))
}
fn folder(value: &str) -> Result<()> {
    if value.len() > 2048 || value.starts_with('/') || value.contains('\\') {
        return Err(Error::new(400, "invalid_request", "子目录无效"));
    }
    if !value.is_empty() {
        for part in value.split('/') {
            files::validate_name(part)?;
        }
    }
    Ok(())
}
pub fn validate(request: &Request) -> Result<()> {
    crate::model::validate_id(&request.id)?;
    if request.action == "delete-files" {
        files::validate_name(&request.person)?;
        if request.payload.is_some()
            || request.target_person.is_some()
            || request.target_folder.is_some()
        {
            return Err(Error::new(400, "invalid_request", "删除不接受目标参数"));
        }
    } else if request.action == "restore-files" {
        if request.files.len() != 1 {
            return Err(Error::new(
                400,
                "invalid_request",
                "每次恢复一个文件，请分别核对目标",
            ));
        }
        if !request.person.is_empty() || request.payload.is_some() {
            return Err(Error::new(400, "invalid_request", "恢复来源由服务端核对"));
        }
        if let Some(person) = &request.target_person {
            files::validate_name(person)?;
        }
        folder(request.target_folder.as_deref().unwrap_or(""))?;
        if request.target_person.is_none() && request.target_folder.is_some() {
            return Err(Error::new(400, "invalid_request", "原位置恢复不接受子目录"));
        }
    } else {
        return Err(Error::new(400, "invalid_request", "暂存操作无效"));
    }
    if !request.filename.is_empty() || request.files.is_empty() || request.files.len() > 500 {
        return Err(Error::new(400, "invalid_request", "请选择 1–500 个文件"));
    }
    for f in &request.files {
        if f.path.len() > 4096 || f.vid.len() > 4096 || f.version.len() != 64 {
            return Err(Error::new(400, "invalid_request", "文件路径或版本无效"));
        }
    }
    Ok(())
}
/// Read immutable source receipts for older person operations without rewriting their audit history.
pub(crate) fn origin(config: &Config, path: &str, origins: &Value) -> Result<Value> {
    if let Some(value) = origins.get(path) {
        return Ok(value.clone());
    }
    let relative = Path::new(path)
        .strip_prefix(root(config))
        .map_err(|_| Error::conflict("路径不属于暂存区"))?;
    let parts: Vec<_> = relative.iter().collect();
    if parts.len() >= 3
        && parts[0] == "person-deletions"
        && let Some(id) = parts[1]
            .to_str()
            .filter(|id| crate::model::validate_id(id).is_ok())
    {
        let connection = rusqlite::Connection::open_with_flags(
            config.state_db(),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        let saved: Option<String> = connection
            .query_row(
                "SELECT plan FROM operations WHERE id=? AND status='complete'",
                [id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(raw) = saved {
            let plan: Value = serde_json::from_str(&raw)?;
            if let Some(file) = plan["files"]
                .as_array()
                .and_then(|rows| rows.iter().find(|f| f["dst"] == path))
                && let Some(src) = file["src"]
                    .as_str()
                    .filter(|src| original_scope(config, Path::new(src)).is_ok())
            {
                return Ok(
                    json!({"operation":id,"action":plan["action"],"original_path":src,"person":plan["person"],"vids":file["vids"],"source":"native-receipt"}),
                );
            }
        }
        let (raw, value) = registration::document(
            config,
            &format!("person-operations/{id}/operation.json"),
            false,
        )?;
        if raw.is_some()
            && value["status"] == "complete"
            && let Some(file) = value["plan"]["files"]
                .as_array()
                .and_then(|rows| rows.iter().find(|f| f["dst"] == path))
            && let Some(src) = file["src"]
                .as_str()
                .filter(|src| original_scope(config, Path::new(src)).is_ok())
        {
            return Ok(
                json!({"operation":id,"action":value["plan"]["action"],"original_path":src,"person":value["plan"]["person"],"vids":file["vids"],"source":"legacy-receipt"}),
            );
        }
    }
    // Older archive duplicate manifests record the exact staged destination. Never infer IDs
    // from the canonical archive asset or turn a name match into provenance.
    let manifests = config.base.join("manifests");
    let entries = match fs::read_dir(&manifests) {
        Ok(v) => Some(v),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    let mut found: Option<Value> = None;
    if let Some(entries) = entries {
        for entry in entries {
            let entry = entry?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| Error::conflict("归档来源文件名无效"))?;
            if !name.starts_with("apply-") || !name.ends_with(".json") {
                continue;
            }
            let (_, manifest) =
                registration::document(config, &format!("manifests/{name}"), false)?;
            if !matches!(manifest["status"].as_str(), Some("complete" | "partial")) {
                continue;
            }
            for row in manifest["moves"].as_array().into_iter().flatten() {
                if row["dst"] != path || row["restored"] == true {
                    continue;
                }
                let Some(src) = row["src"]
                    .as_str()
                    .filter(|s| original_scope(config, Path::new(s)).is_ok())
                else {
                    continue;
                };
                let value = json!({"operation":manifest["operation_id"].as_str().unwrap_or(&name),"action":"stage-duplicate","original_path":src,"person":"","vids":[],"source":"legacy-manifest"});
                if found
                    .as_ref()
                    .is_some_and(|previous| previous["original_path"] != value["original_path"])
                {
                    return Ok(Value::Null);
                }
                found = Some(value);
            }
        }
    }
    Ok(found.unwrap_or(Value::Null))
}
pub fn plan(config: &Config, request: Request) -> Result<Plan> {
    validate(&request)?;
    let deleting = request.action == "delete-files";
    let person = if deleting {
        registration::canonical(config, &request.person)?
    } else {
        String::new()
    };
    if deleting && !registration::known_person(config, &person)? {
        return Err(Error::new(404, "missing_person", "找不到当前人物"));
    }
    let (_, origins) = registration::document(config, ORIGINS, false)?;
    let sources = registration::SourceIndex::load(
        config,
        &request
            .files
            .iter()
            .map(|f| f.path.clone())
            .collect::<Vec<_>>(),
    )?;
    let mut moves = vec![];
    let mut scopes = vec![];
    let mut directories = BTreeSet::new();
    let mut accepted_origins = vec![];
    let mut seen = BTreeSet::new();
    let mut target_person = person.clone();
    let mut source_person = person.clone();
    let mut total = 0u64;
    for selected in &request.files {
        let src = Path::new(&selected.path);
        if !seen.insert(selected.path.clone()) {
            return Err(Error::new(400, "invalid_request", "重复选择文件"));
        }
        let source_origin = if deleting {
            Value::Null
        } else {
            origin(config, &selected.path, &origins)?
        };
        let (dst, scope) = if deleting {
            let ext = src
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_lowercase();
            if ![
                "mp4", "m4v", "mov", "mkv", "webm", "avi", "wmv", "ts", "mts", "m2ts",
            ]
            .contains(&ext.as_str())
            {
                return Err(Error::new(400, "invalid_request", "只允许删除人物目录视频"));
            }
            let relative = src
                .strip_prefix(config.creators().join(&person))
                .map_err(|_| Error::conflict("文件不属于当前人物"))?;
            (
                root(config)
                    .join("person-deletions")
                    .join(&request.id)
                    .join(&person)
                    .join(relative),
                MoveScope {
                    source: ScopeRoot::Person(person.clone()),
                    destination: ScopeRoot::Staging,
                },
            )
        } else {
            let historical_person = source_origin["person"].as_str().unwrap_or("");
            // The origin remains immutable audit; active metadata and ledger roles
            // follow the currently known alias of that historical source person.
            source_person = if historical_person.is_empty() {
                String::new()
            } else {
                registration::canonical(config, historical_person)?
            };
            let dst = if let Some(target) = &request.target_person {
                target_person = registration::canonical(config, target)?;
                config
                    .creators()
                    .join(&target_person)
                    .join(request.target_folder.as_deref().unwrap_or(""))
                    .join(
                        src.file_name()
                            .ok_or_else(|| Error::conflict("文件名无效"))?,
                    )
            } else {
                PathBuf::from(source_origin["original_path"].as_str().ok_or_else(|| {
                    Error::new(409, "unknown_origin", "来源未知，请明确指定目标人物")
                })?)
            };
            let destination = original_scope(config, &dst)?;
            target_person = match &destination {
                ScopeRoot::Person(p) => p.clone(),
                _ => String::new(),
            };
            if request.target_person.is_none()
                && !target_person.is_empty()
                && registration::canonical(config, &target_person)? != target_person
            {
                return Err(Error::conflict("原人物已更名，请明确选择当前人物"));
            }
            (
                dst,
                MoveScope {
                    source: ScopeRoot::Staging,
                    destination,
                },
            )
        };
        let mut file = FileMove {
            src: selected.path.clone(),
            dst: dst.to_string_lossy().into_owned(),
            vids: vec![],
            size: 0,
            version: selected.version.clone(),
        };
        let stat = files::check_move(config, &scope, &file)?;
        file.vids = sources.selected_ids(&file.src, &selected.vid, &stat)?;
        file.size = stat.len();
        total = total
            .checked_add(file.size)
            .ok_or_else(|| Error::conflict("文件总大小超出范围"))?;
        let mut parent = dst.parent();
        while let Some(dir) = parent {
            if dir.exists() {
                break;
            }
            directories.insert(dir.to_string_lossy().into_owned());
            parent = dir.parent();
        }
        moves.push(file);
        scopes.push(scope);
        accepted_origins.push(source_origin);
    }
    let faces = db::open(&config.base.join("faces.db"))?;
    let timestamp: String =
        faces.query_row("SELECT strftime('%Y-%m-%dT%H:%M:%fZ','now')", [], |r| {
            r.get(0)
        })?;
    let mut thumbs = BTreeSet::new();
    for file in &moves {
        for vid in &file.vids {
            for row in db::rows(&faces, "SELECT thumb FROM faces WHERE vid=?", [vid])? {
                if let Some(t) = row["thumb"].as_str() {
                    thumbs.insert(t.to_owned());
                }
            }
        }
    }
    let execution = ExecutionIntent {
        scopes,
        failure_policy: FailurePolicy::Rollback,
        stores: AffectedStores {
            faces: true,
            jobs: config.base.join("processing/processing.db").exists(),
            ledger: true,
        },
        data: json!({"timestamp":timestamp,"affected_thumbs":thumbs,"origins":accepted_origins}),
        result: json!({"ok":true,"id":request.id,"person":if deleting{&person}else{&target_person},"staged":deleting}),
    };
    let mut plan = Plan {
        id: request.id.clone(),
        action: request.action.clone(),
        person: source_person,
        target_person,
        request,
        version: String::new(),
        files: moves,
        total_bytes: total,
        directories: directories.into_iter().collect(),
        ledger_version: String::new(),
        metadata_version: String::new(),
        execution: Some(execution),
    };
    engine::seal_plan(config, &mut plan)?;
    Ok(plan)
}
pub fn revalidate(config: &Config, plan: &Plan) -> Result<()> {
    let fresh = self::plan(config, plan.request.clone())?;
    // Preserve admitted timestamps but compare all source, origin, target and stable-ID intent.
    if fresh
        .files
        .iter()
        .map(|f| (&f.src, &f.dst, &f.vids))
        .collect::<Vec<_>>()
        != plan
            .files
            .iter()
            .map(|f| (&f.src, &f.dst, &f.vids))
            .collect::<Vec<_>>()
        || fresh.person != plan.person
        || fresh.target_person != plan.target_person
        || fresh.execution.as_ref().unwrap().data["origins"]
            != plan.execution.as_ref().unwrap().data["origins"]
        || fresh.execution.as_ref().unwrap().data["affected_thumbs"]
            != plan.execution.as_ref().unwrap().data["affected_thumbs"]
    {
        return Err(Error::conflict("暂存来源或目标已变化，请重新预览"));
    }
    Ok(())
}
fn selected(plan: &Plan, row: &Value) -> bool {
    row["vid"]
        .as_str()
        .is_some_and(|id| plan.files.iter().any(|f| f.vids.iter().any(|v| v == id)))
}
pub fn snapshot(config: &Config, plan: &Plan, successful: &[usize]) -> Result<DomainSnapshot> {
    if successful.iter().copied().ne(0..plan.files.len()) {
        return Err(Error::conflict("暂存操作必须提交完整范围"));
    }
    let deleting = plan.action == "delete-files";
    let mut domain_plan = plan.clone();
    if !deleting && !plan.target_person.is_empty() {
        domain_plan.action = "move-files".into();
    }
    let mut metadata = crate::person_metadata::snapshot(config, &domain_plan)?;
    for doc in &mut metadata.documents {
        let Some(raw) = &doc.after else {
            continue;
        };
        let mut value: Value = serde_json::from_str(raw)?;
        let previous = value.clone();
        if deleting {
            match doc.name.as_str() {
                "integration-queue.json" | "claims-detail.json" => {
                    value.as_array_mut().unwrap().retain(|r| !selected(plan, r))
                }
                "clusters.json" => {
                    for row in value.as_array_mut().unwrap() {
                        if let Some(vids) = row["vids"].as_array_mut() {
                            vids.retain(|r| !selected(plan, r));
                        }
                    }
                }
                "report.json" => {
                    if let Some(people) = value["identities"].as_array_mut() {
                        for person in people {
                            let thumb = person["rep_thumb"].as_str().unwrap_or("");
                            let invalid_thumb =
                                plan.execution.as_ref().unwrap().data["affected_thumbs"]
                                    .as_array()
                                    .is_some_and(|rows| {
                                        rows.iter().any(|row| {
                                            row.as_str().is_some_and(|raw| {
                                                let encoded: String = raw
                                                    .bytes()
                                                    .map(|c| {
                                                        if c.is_ascii_alphanumeric()
                                                            || b"-_.!~*'()".contains(&c)
                                                        {
                                                            (c as char).to_string()
                                                        } else {
                                                            format!("%{c:02X}")
                                                        }
                                                    })
                                                    .collect();
                                                thumb == raw
                                                    || thumb == format!("/thumb?p={encoded}")
                                            })
                                        })
                                    });
                            if let Some(vids) = person["vids"].as_array_mut() {
                                let old = vids.len();
                                vids.retain(|r| !selected(plan, r));
                                if (vids.is_empty() && old > 0) || invalid_thumb {
                                    person["rep_thumb"] = Value::Null;
                                    person["tpl_faces"] = json!(0);
                                }
                            }
                        }
                    }
                }
                _ => (),
            }
        } else if doc.name == "people-deleted.json" && !plan.target_person.is_empty() {
            value.as_object_mut().unwrap().remove(&plan.target_person);
        }
        if doc.name == "archive-history.json" {
            for row in value.as_array_mut().unwrap() {
                if selected(plan, row) {
                    row["file_state"] = json!(if deleting { "staged" } else { "restored" });
                }
            }
        }
        if value != previous {
            doc.after = Some(serde_json::to_string_pretty(&value)?);
        }
    }
    // Create even when the deleted-marker document did not previously exist only through the shared metadata snapshot.
    let (before, mut origins) = registration::document(config, ORIGINS, false)?;
    for file in &plan.files {
        if deleting {
            origins[&file.dst] = json!({"operation":plan.id,"action":plan.action,"original_path":file.src,"person":plan.person,"vids":file.vids,"source":"native-receipt"});
        } else {
            let mut record = origin(config, &file.src, &origins)?;
            if record.is_null() {
                record = json!({"original_path":null,"source":"unknown"});
            }
            record["restored_by"] = json!(plan.id);
            record["restored_path"] = json!(file.dst);
            origins[&file.src] = record;
        }
    }
    metadata.documents.push(metadata::Document {
        name: ORIGINS.into(),
        before,
        after: Some(serde_json::to_string_pretty(&origins)?),
    });
    Ok(DomainSnapshot {
        metadata,
        ledger: Some(crate::person_ledger::snapshot(config, &domain_plan)?),
    })
}
pub fn list(config: &Config, offset: usize, query: &str) -> Result<Value> {
    let root = root(config);
    let (_, origins) = registration::document(config, ORIGINS, false)?;
    let mut pending = vec![root.clone()];
    let mut paths = BTreeMap::new();
    let mut bytes = 0u64;
    let mut visited = 0usize;
    while let Some(dir) = pending.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(v) => v,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && dir == root => continue,
            Err(e) => return Err(Error::from(e).at(dir.display().to_string())),
        };
        // Pinned traversal rejects a symlink anywhere above this directory.
        files::parent_identity(&dir.join("probe").to_string_lossy())?;
        for entry in entries {
            let entry = entry?;
            visited += 1;
            if visited > 100_000 {
                return Err(Error::new(
                    413,
                    "listing_limit",
                    "暂存区超过单次安全扫描限制",
                ));
            }
            let path = entry.path();
            let stat = path.symlink_metadata()?;
            if stat.file_type().is_symlink() {
                continue;
            }
            if stat.is_dir() {
                pending.push(path);
            } else if stat.is_file() {
                bytes = bytes.saturating_add(stat.len());
                paths.insert(path.to_string_lossy().into_owned(), stat);
            }
        }
    }
    let total = paths.len();
    let query = query.to_lowercase();
    let filtered: Vec<_> = paths
        .into_iter()
        .filter(|(path, _)| path.to_lowercase().contains(&query))
        .collect();
    let matched = filtered.len();
    let selected: Vec<_> = filtered.into_iter().skip(offset).take(100).collect();
    let sources = registration::SourceIndex::load(
        config,
        &selected.iter().map(|(p, _)| p.clone()).collect::<Vec<_>>(),
    )?;
    let mut rows = vec![];
    for (path, stat) in selected {
        let ids = sources.ids_for_path(&path);
        let source = origin(config, &path, &origins)?;
        let vid = ids.first().cloned().unwrap_or_else(|| path.clone());
        rows.push(json!({"vid":vid,"path":path,"name":Path::new(&path).file_name().unwrap().to_string_lossy(),"size":stat.len(),"version":files::fingerprint(&path,&stat),"origin":source}));
    }
    Ok(
        json!({"files":total,"gb":(bytes as f64/1073741824.0*10.0).round()/10.0,"bytes":bytes,"path":root,"rows":rows,"matched":matched,"offset":offset,"next_offset":if offset.saturating_add(100)<matched{Some(offset+100)}else{None}}),
    )
}
