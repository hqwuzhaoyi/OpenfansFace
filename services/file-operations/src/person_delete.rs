//! Whole-person soft deletion: full tree retained in global staging, history retained.
use crate::{
    config::Config,
    db, engine, files,
    model::{
        AffectedStores, Error, ExecutionIntent, FailurePolicy, FileMove, MoveScope, Plan, Request,
        Result, ScopeRoot,
    },
    native::DomainSnapshot,
    person_directory, person_registration as registration,
};
use serde_json::{Value, json};
use std::{fs, path::Path};
pub fn validate(request: &Request) -> Result<()> {
    crate::model::validate_id(&request.id)?;
    files::validate_name(&request.person)?;
    if request.action != "delete-person"
        || !request.files.is_empty()
        || !request.filename.is_empty()
        || request.target_person.is_some()
        || request.target_folder.is_some()
        || request.payload.is_some()
    {
        return Err(Error::new(
            400,
            "invalid_request",
            "人物删除不接受附加文件或目标参数",
        ));
    }
    Ok(())
}
fn check_person(config: &Config, person: &str) -> Result<()> {
    let (_, deleted) = registration::document(config, "people-deleted.json", false)?;
    if !deleted[person].is_null() {
        return Err(Error::new(
            404,
            "missing_person",
            "找不到当前人物或人物已删除",
        ));
    }
    if registration::known_person(config, person)? {
        return Ok(());
    }
    // Catalog-only people remain deletable even when their directory and active rows are absent.
    let (_, aliases) = registration::document(config, "person-renames.json", false)?;
    for (file, array, field) in [
        ("people-manual.json", true, "name"),
        ("integration-queue.json", true, "creator"),
        ("report.json", false, "name"),
        ("archive-history.json", true, "creator"),
    ] {
        let (_, value) = registration::document(config, file, array)?;
        let rows = if array {
            value.as_array()
        } else {
            value["identities"].as_array()
        };
        for row in rows.into_iter().flatten() {
            if let Some(name) = row[field].as_str()
                && registration::canonical_in(name, &aliases)? == person
            {
                return Ok(());
            }
        }
    }
    Err(Error::new(
        404,
        "missing_person",
        "找不到当前人物或人物已删除",
    ))
}
pub fn plan(config: &Config, request: Request) -> Result<Plan> {
    validate(&request)?;
    let person = registration::selected(config, &request.person)?;
    check_person(config, &person)?;
    let target = person.clone();
    let source_root = config.creators().join(&person);
    let target_root = crate::staging::root(config)
        .join("person-deletions")
        .join(&request.id)
        .join(&person);
    match target_root.symlink_metadata() {
        Ok(_) => return Err(Error::conflict("暂存目标已存在")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => return Err(e.into()),
    }
    crate::permissions::writable_directory(&target_root, "destination_directory")?;
    let inventory = person_directory::inventory(&source_root, &target_root)?;
    let mut directories = std::collections::BTreeSet::new();
    for entry in &inventory.directories {
        let mut parent = Some(Path::new(&entry.dst));
        while let Some(dir) = parent {
            match dir.symlink_metadata() {
                Ok(_) => break,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(e) => return Err(e.into()),
            }
            directories.insert(dir.display().to_string());
            parent = dir.parent();
        }
    }
    let sources = registration::SourceIndex::load(config, &inventory.files)?;
    let mut moves = vec![];
    let mut scopes = vec![];
    let mut total = 0u64;
    for src in &inventory.files {
        let dst = target_root
            .join(Path::new(src).strip_prefix(&source_root).unwrap())
            .display()
            .to_string();
        let stat = fs::symlink_metadata(src)?;
        let vids = sources.tracked_ids(src, &stat)?;
        let file = FileMove {
            src: src.clone(),
            dst,
            vids,
            size: stat.len(),
            version: files::fingerprint(src, &stat),
        };
        let scope = MoveScope {
            source: ScopeRoot::Person(person.clone()),
            destination: ScopeRoot::Staging,
        };
        files::check_move(config, &scope, &file)?;
        total = total
            .checked_add(file.size)
            .ok_or_else(|| Error::conflict("文件总大小超出范围"))?;
        moves.push(file);
        scopes.push(scope);
    }
    let connection = db::open(&config.base.join("faces.db"))?;
    let timestamp: String =
        connection.query_row("SELECT strftime('%Y-%m-%dT%H:%M:%fZ','now')", [], |r| {
            r.get(0)
        })?;
    let thumbs = affected_thumbs(config, &moves)?;
    let names = crate::person_rename::aliases(config, &person)?;
    let execution = ExecutionIntent {
        scopes,
        failure_policy: FailurePolicy::Rollback,
        stores: AffectedStores {
            faces: true,
            jobs: config.base.join("processing/processing.db").exists(),
            ledger: true,
        },
        data: json!({"timestamp":timestamp,"identity_names":names,"directory_inventory":inventory,"affected_thumbs":thumbs}),
        result: json!({"ok":true,"id":request.id,"person":target,"staged":true}),
    };
    let mut plan = Plan {
        id: request.id.clone(),
        action: request.action.clone(),
        request,
        person,
        target_person: target,
        files: moves,
        total_bytes: total,
        directories: directories.into_iter().collect(),
        version: String::new(),
        metadata_version: String::new(),
        ledger_version: String::new(),
        execution: Some(execution),
    };
    engine::seal_plan(config, &mut plan)?;
    Ok(plan)
}
pub fn revalidate(config: &Config, plan: &Plan) -> Result<()> {
    check_person(config, &plan.person)?;
    if json!(affected_thumbs(config, &plan.files)?)
        != plan.execution.as_ref().unwrap().data["affected_thumbs"]
    {
        return Err(Error::conflict("人物缩略图引用已变化"));
    }
    if registration::selected(config, &plan.request.person)? != plan.person
        || json!(crate::person_rename::aliases(config, &plan.person)?)
            != plan.execution.as_ref().unwrap().data["identity_names"]
    {
        return Err(Error::conflict("人物别名已变化"));
    }
    let (source, target) = person_directory::roots(config, plan);
    match target.symlink_metadata() {
        Ok(_) => return Err(Error::conflict("暂存目标已存在")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => return Err(e.into()),
    }
    if Some(person_directory::inventory(&source, &target)?) != person_directory::intent(plan)? {
        return Err(Error::conflict("人物目录结构或文件范围已变化"));
    }
    let index = registration::SourceIndex::load(
        config,
        &plan.files.iter().map(|f| f.src.clone()).collect::<Vec<_>>(),
    )?;
    for file in &plan.files {
        let stat = fs::symlink_metadata(&file.src)?;
        if index.tracked_ids(&file.src, &stat)? != file.vids {
            return Err(Error::conflict("稳定视频标识已变化"));
        }
    }
    Ok(())
}
pub fn snapshot(config: &Config, plan: &Plan, successful: &[usize]) -> Result<DomainSnapshot> {
    if successful.iter().copied().ne(0..plan.files.len()) {
        return Err(Error::conflict("人物删除必须提交完整范围"));
    }
    crate::staging::snapshot(config, plan, successful)
}
/// Remove only active identity decisions. Stable IDs, free text, OCR and history stay intact.
pub fn transform(value: &Value, plan: &Plan) -> Value {
    let names = crate::person_rename::names(plan);
    fn visit(value: &Value, key: &str, names: &[String]) -> Value {
        match value {
            Value::String(s) if key == "suggested" && names.contains(s) => Value::Null,
            Value::Array(rows) => Value::Array(
                rows.iter()
                    .filter(|r| {
                        !(["candidates", "matches"].contains(&key)
                            && r["name"]
                                .as_str()
                                .is_some_and(|s| names.iter().any(|n| n == s)))
                    })
                    .map(|r| visit(r, "", names))
                    .collect(),
            ),
            Value::Object(map) => {
                let mut next: Value = Value::Object(
                    map.iter()
                        .map(|(k, v)| (k.clone(), visit(v, k, names)))
                        .collect(),
                );
                if let (Some(before), Some(after)) = (
                    value["candidates"].as_array(),
                    next["candidates"].as_array(),
                ) && before.len() != after.len()
                {
                    let multiple = after.len() > 1;
                    next["ready"] = json!(false);
                    next["suggested"] = Value::Null;
                    next["kind"] = json!(if multiple { "conflict" } else { "none" });
                }
                next
            }
            _ => value.clone(),
        }
    }
    visit(value, "", &names)
}
pub fn updated(config: &Config, name: &str, value: &Value, plan: &Plan) -> Value {
    let names = crate::person_rename::names(plan);
    let related = |v: &Value| v.as_str().is_some_and(|s| names.iter().any(|n| n == s));
    // Public profile snapshots and historical labels are immutable; only the active link is removed.
    if name.starts_with("external-profiles/") {
        let mut next = value.clone();
        if let Some(links) = next["links"].as_array_mut() {
            links.retain(|r| !related(&r["creator"]) && !related(&r["person"]));
        }
        return next;
    }
    let mapped = crate::metadata::transform(config, value, plan);
    let mut next = if name == "archive-history.json" {
        mapped
    } else {
        transform(&mapped, plan)
    };
    match name {
        "people-deleted.json" => {
            for name in names {
                next[&name] = json!({"operation":plan.id});
            }
        }
        "report.json" => {
            if let Some(rows) = next["identities"].as_array_mut() {
                rows.retain(|r| !related(&r["name"]));
            }
        }
        "people-manual.json" | "claims-detail.json" | "integration-queue.json" => {
            let field = if name == "integration-queue.json" {
                "creator"
            } else {
                "name"
            };
            next.as_array_mut().unwrap().retain(|r| !related(&r[field]));
        }
        "template-overrides.json" => {
            next.as_object_mut()
                .unwrap()
                .retain(|k, _| !related(&json!(k)));
        }
        "person-rejections.json" | "watermarks/mappings.json" => {
            next.as_object_mut()
                .unwrap()
                .retain(|_, r| !related(&r["name"]) && !related(&r["creator"]));
        }
        "video-path-overrides.json" => {
            next = Value::Object(
                value
                    .as_object()
                    .unwrap()
                    .iter()
                    .map(|(id, path)| {
                        (
                            id.clone(),
                            path.as_str()
                                .map(|s| json!(crate::metadata::mapped(config, s, plan)))
                                .unwrap_or_else(|| path.clone()),
                        )
                    })
                    .collect(),
            );
            for file in &plan.files {
                for id in &file.vids {
                    next[id] = json!(file.dst);
                }
            }
        }
        "archive-history.json" => {
            for row in next.as_array_mut().unwrap() {
                if row["vid"]
                    .as_str()
                    .is_some_and(|id| plan.files.iter().any(|f| f.vids.iter().any(|v| v == id)))
                {
                    row["last_file_operation"] = json!(plan.id);
                }
            }
        }
        _ => (),
    }
    next
}

fn affected_thumbs(config: &Config, moves: &[FileMove]) -> Result<Vec<String>> {
    let connection = db::open(&config.base.join("faces.db"))?;
    let mut thumbs = std::collections::BTreeSet::new();
    for file in moves {
        for vid in &file.vids {
            for row in db::rows(&connection, "SELECT thumb FROM faces WHERE vid=?", [vid])? {
                if let Some(thumb) = row["thumb"].as_str() {
                    thumbs.insert(thumb.to_owned());
                }
            }
        }
    }
    Ok(thumbs.into_iter().collect())
}
