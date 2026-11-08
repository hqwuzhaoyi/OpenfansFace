//! Native identity rename: no implicit merge, full directory and semantically scoped metadata.
use crate::{
    config::Config,
    db, engine, files, metadata,
    model::{
        AffectedStores, Error, ExecutionIntent, FailurePolicy, FileMove, MoveScope, Plan, Request,
        Result, ScopeRoot,
    },
    native::DomainSnapshot,
    permissions, person_directory, person_registration as registration,
};
use serde_json::{Value, json};
use std::{collections::BTreeSet, fs, path::Path};
use unicode_normalization::UnicodeNormalization;
fn normalized(s: &str) -> String {
    s.nfc().collect::<String>().to_lowercase()
}
pub fn validate(request: &Request) -> Result<()> {
    crate::model::validate_id(&request.id)?;
    files::validate_name(&request.person)?;
    files::validate_name(
        request
            .target_person
            .as_deref()
            .ok_or_else(|| Error::new(400, "invalid_request", "请输入新人物名"))?,
    )?;
    if request.action != "rename-person"
        || !request.files.is_empty()
        || !request.filename.is_empty()
        || request.target_folder.is_some()
        || request.payload.is_some()
    {
        return Err(Error::new(
            400,
            "invalid_request",
            "人物改名不接受附加文件或执行参数",
        ));
    }
    Ok(())
}
pub fn aliases(config: &Config, person: &str) -> Result<Vec<String>> {
    let (_, values) = registration::document(config, "person-renames.json", false)?;
    let mut names = BTreeSet::from([person.to_owned()]);
    for alias in values.as_object().unwrap().keys() {
        if registration::canonical_in(alias, &values)? == person {
            names.insert(alias.clone());
        }
    }
    Ok(names.into_iter().collect())
}
fn check_target(config: &Config, person: &str, target: &str) -> Result<()> {
    if person == target {
        return Err(Error::new(400, "invalid_request", "新人物名与当前名字相同"));
    }
    if !registration::known_person(config, person)? {
        return Err(Error::new(404, "missing_person", "找不到当前人物"));
    }
    let canonical = registration::canonical(config, target)?;
    if canonical != target && canonical != person {
        return Err(Error::conflict("该名称属于其他人物，不能自动合并"));
    }
    let (_, alias_map) = registration::document(config, "person-renames.json", false)?;
    let mut names = BTreeSet::new();
    for entry in fs::read_dir(config.creators())? {
        let entry = entry?;
        names.insert(entry.file_name().to_string_lossy().into_owned());
    }
    for (file, array, key) in [
        ("people-manual.json", true, "name"),
        ("integration-queue.json", true, "creator"),
        ("report.json", false, "name"),
    ] {
        let (_, value) = registration::document(config, file, array)?;
        let rows = if array {
            value.as_array()
        } else {
            value["identities"].as_array()
        };
        for row in rows.into_iter().flatten() {
            if let Some(name) = row[key].as_str() {
                names.insert(registration::canonical_in(name, &alias_map)?);
            }
        }
    }
    if names.iter().any(|n| normalized(n) == normalized(target)) {
        return Err(Error::conflict("目标人物已存在，不能覆盖或自动合并"));
    }
    if config.creators().join(target).symlink_metadata().is_ok() {
        return Err(Error::conflict("目标人物目录已存在"));
    }
    let (_, templates) = registration::document(config, "template-overrides.json", false)?;
    if templates.get(target).is_some() {
        return Err(Error::conflict("目标人物已有模板设置，不能覆盖"));
    }
    permissions::writable_directory(&config.creators().join(target), "destination_directory")?;
    Ok(())
}
pub fn plan(config: &Config, request: Request) -> Result<Plan> {
    validate(&request)?;
    let person = registration::canonical(config, &request.person)?;
    let target = request.target_person.as_ref().unwrap().clone();
    check_target(config, &person, &target)?;
    let source_root = config.creators().join(&person);
    let target_root = config.creators().join(&target);
    let inventory = person_directory::inventory(&source_root, &target_root)?;
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
            destination: ScopeRoot::Person(target.clone()),
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
    let names = aliases(config, &person)?;
    let execution = ExecutionIntent {
        scopes,
        failure_policy: FailurePolicy::Rollback,
        stores: AffectedStores {
            faces: true,
            jobs: config.base.join("processing/processing.db").exists(),
            ledger: true,
        },
        data: json!({"timestamp":timestamp,"identity_names":names,"directory_inventory":inventory}),
        result: json!({"ok":true,"id":request.id,"person":target,"staged":false}),
    };
    let mut plan = Plan {
        id: request.id.clone(),
        action: request.action.clone(),
        request,
        person,
        target_person: target,
        files: moves,
        total_bytes: total,
        directories: inventory
            .directories
            .iter()
            .map(|d| d.dst.clone())
            .collect(),
        version: String::new(),
        metadata_version: String::new(),
        ledger_version: String::new(),
        execution: Some(execution),
    };
    engine::seal_plan(config, &mut plan)?;
    Ok(plan)
}
pub fn revalidate(config: &Config, plan: &Plan) -> Result<()> {
    check_target(config, &plan.person, &plan.target_person)?;
    if registration::canonical(config, &plan.request.person)? != plan.person
        || json!(aliases(config, &plan.person)?)
            != plan.execution.as_ref().unwrap().data["identity_names"]
    {
        return Err(Error::conflict("人物别名已变化"));
    }
    let (source, target) = person_directory::roots(config, plan);
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
        return Err(Error::conflict("人物改名必须提交完整范围"));
    }
    Ok(DomainSnapshot {
        metadata: crate::person_metadata::snapshot(config, plan)?,
        ledger: Some(crate::person_ledger::snapshot(config, plan)?),
    })
}
pub fn names(plan: &Plan) -> Vec<String> {
    plan.execution
        .as_ref()
        .and_then(|e| e.data["identity_names"].as_array())
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect()
}
/// Rename only semantic identity fields. Raw OCR, stable identifiers and profile snapshots remain evidence.
pub fn transform(value: &Value, plan: &Plan) -> Value {
    let names = names(plan);
    fn visit(v: &Value, key: &str, parent: &Value, names: &[String], target: &str) -> Value {
        match v {
            Value::String(s)
                if (["name", "creator", "suggested"].contains(&key)
                    || key == "source_id" && parent["source"] == "person")
                    && names.contains(s) =>
            {
                json!(target)
            }
            Value::String(s)
                if key == "return_href"
                    && names
                        .iter()
                        .any(|n| s == &format!("/persons/{}", encode(n))) =>
            {
                json!(format!("/persons/{}", encode(target)))
            }
            Value::Array(rows) => Value::Array(
                rows.iter()
                    .map(|v| visit(v, "", &Value::Null, names, target))
                    .collect(),
            ),
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(k, child)| (k.clone(), visit(child, k, v, names, target)))
                    .collect(),
            ),
            _ => v.clone(),
        }
    }
    visit(value, "", &Value::Null, &names, &plan.target_person)
}
pub fn encode(s: &str) -> String {
    s.bytes()
        .map(|c| {
            if c.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&c) {
                (c as char).to_string()
            } else {
                format!("%{c:02X}")
            }
        })
        .collect()
}
pub fn mapped(config: &Config, value: &str, plan: &Plan) -> String {
    let (src, dst) = person_directory::roots(config, plan);
    for (src, dst, separator) in [
        (src.display().to_string(), dst.display().to_string(), '/'),
        (
            metadata::win(config, &src.display().to_string()),
            metadata::win(config, &dst.display().to_string()),
            '\\',
        ),
    ] {
        if value == src {
            return dst;
        }
        if value.starts_with(&format!("{src}{separator}")) {
            return format!("{}{}", dst, &value[src.len()..]);
        }
    }
    value.into()
}
