//! Two whole-person merge plans; the shared runner owns every side effect and receipt.
use crate::{
    config::Config,
    db, engine, files, metadata,
    model::{
        AffectedStores, Error, ExecutionIntent, FailurePolicy, FileMove, MoveScope, Plan, Request,
        Result, ScopeRoot,
    },
    native::DomainSnapshot,
    person_directory::{self, Inventory},
    person_registration as registration,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};
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
            .ok_or_else(|| Error::new(400, "invalid_request", "请选择保留人物"))?,
    )?;
    if request.action != "merge-person"
        || !request.files.is_empty()
        || !request.filename.is_empty()
        || request.target_folder.is_some()
        || request.payload.is_some()
    {
        return Err(Error::new(
            400,
            "invalid_request",
            "整个人物合并不接受附加文件或执行参数",
        ));
    }
    Ok(())
}
fn people(config: &Config, request: &Request) -> Result<(String, String)> {
    let source = registration::canonical(config, &request.person)?;
    let target = registration::canonical(config, request.target_person.as_ref().unwrap())?;
    if normalized(&source) == normalized(&target) {
        return Err(Error::conflict("请选择两个不同人物并指定保留人物"));
    }
    for person in [&source, &target] {
        if !registration::known_person(config, person)? {
            return Err(Error::new(404, "missing_person", "找不到所选人物").at(person));
        }
    }
    Ok((source, target))
}
fn existing(path: &Path) -> Result<Option<(PathBuf, bool)>> {
    let parent = path.parent().unwrap();
    let entries = match fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let name = normalized(
        path.file_name()
            .unwrap()
            .to_str()
            .ok_or_else(|| Error::conflict("目录名称不是有效 UTF-8"))?,
    );
    let mut found = None;
    for entry in entries {
        let entry = entry?;
        if normalized(
            entry
                .file_name()
                .to_str()
                .ok_or_else(|| Error::conflict("目录名称不是有效 UTF-8"))?,
        ) == name
        {
            if found.is_some() {
                return Err(Error::conflict("目标目录存在大小写或 Unicode 歧义")
                    .at(parent.display().to_string()));
            }
            let kind = entry.file_type()?;
            found = Some((entry.path(), kind.is_dir() && !kind.is_symlink()));
        }
    }
    Ok(found)
}
fn unique(desired: &Path, source: &str, reserved: &mut BTreeSet<String>) -> Result<PathBuf> {
    let mut result = desired.to_path_buf();
    let mut index = 0;
    while existing(&result)?.is_some()
        || reserved.contains(&normalized(&result.display().to_string()))
    {
        result = desired.with_file_name(crate::archive_resolution::alternate_name(
            desired
                .file_name()
                .unwrap()
                .to_str()
                .ok_or_else(|| Error::conflict("文件名不是有效 UTF-8"))?,
            &format!("{source}:{index}"),
        ));
        index += 1;
    }
    reserved.insert(normalized(&result.display().to_string()));
    Ok(result)
}
fn mapped_inventory(source: &Path, target: &Path) -> Result<Inventory> {
    let mut inventory = person_directory::inventory(source, target)?;
    let mut reserved = BTreeSet::new();
    let mut directories = BTreeMap::<String, String>::new();
    for directory in &mut inventory.directories {
        let src = Path::new(&directory.src);
        let dst = if src == source {
            target.to_path_buf()
        } else {
            let parent = directories
                .get(&src.parent().unwrap().display().to_string())
                .unwrap();
            let desired = Path::new(parent).join(src.file_name().unwrap());
            match existing(&desired)? {
                Some((path, true)) => path,
                _ => unique(&desired, &directory.src, &mut reserved)?,
            }
        };
        directory.dst = dst.display().to_string();
        directories.insert(directory.src.clone(), directory.dst.clone());
    }
    Ok(inventory)
}
fn target_preview(config: &Config, root: &Path) -> Result<Value> {
    let inventory = person_directory::inventory(root, root)?;
    let index = registration::SourceIndex::load(config, &inventory.files)?;
    let files=inventory.files.iter().map(|p|{let stat=fs::symlink_metadata(p)?;Ok(json!({"path":p,"size":stat.len(),"version":files::fingerprint(p,&stat),"vids":index.tracked_ids(p,&stat)?}))}).collect::<Result<Vec<_>>>()?;
    Ok(json!({"inventory":inventory,"files":files}))
}
pub fn plan(config: &Config, request: Request) -> Result<Plan> {
    validate(&request)?;
    let (person, target) = people(config, &request)?;
    let source_root = config.creators().join(&person);
    let target_root = config.creators().join(&target);
    let inventory = mapped_inventory(&source_root, &target_root)?;
    let target_preview = target_preview(config, &target_root)?;
    let index = registration::SourceIndex::load(config, &inventory.files)?;
    let mut reserved = inventory
        .directories
        .iter()
        .map(|d| normalized(&d.dst))
        .collect::<BTreeSet<_>>();
    let mut moves = vec![];
    let mut scopes = vec![];
    let mut total = 0u64;
    for src in &inventory.files {
        let path = Path::new(src);
        let parent = inventory
            .directories
            .iter()
            .find(|d| Path::new(&d.src) == path.parent().unwrap())
            .unwrap();
        let dst = unique(
            &Path::new(&parent.dst).join(path.file_name().unwrap()),
            src,
            &mut reserved,
        )?
        .display()
        .to_string();
        let stat = fs::symlink_metadata(src)?;
        let file = FileMove {
            src: src.clone(),
            dst,
            vids: index.tracked_ids(src, &stat)?,
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
    let conn = db::open(&config.base.join("faces.db"))?;
    let timestamp: String =
        conn.query_row("SELECT strftime('%Y-%m-%dT%H:%M:%fZ','now')", [], |r| {
            r.get(0)
        })?;
    let source_names = crate::person_rename::aliases(config, &person)?;
    let target_names = crate::person_rename::aliases(config, &target)?;
    let names = source_names
        .iter()
        .chain(&target_names)
        .cloned()
        .collect::<Vec<_>>();
    let execution = ExecutionIntent {
        scopes,
        failure_policy: FailurePolicy::Rollback,
        stores: AffectedStores {
            faces: true,
            jobs: config.base.join("processing/processing.db").exists(),
            ledger: true,
        },
        data: json!({"timestamp":timestamp,"identity_names":names,"source_names":source_names,"target_names":target_names,"directory_inventory":inventory,"target_preview":target_preview}),
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
            .filter(|d| {
                Path::new(&d.dst)
                    .symlink_metadata()
                    .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
            })
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
    if people(config, &plan.request)? != (plan.person.clone(), plan.target_person.clone()) {
        return Err(Error::conflict("人物名称已变化"));
    }
    let data = &plan.execution.as_ref().unwrap().data;
    if json!(crate::person_rename::aliases(config, &plan.person)?) != data["source_names"]
        || json!(crate::person_rename::aliases(config, &plan.target_person)?)
            != data["target_names"]
    {
        return Err(Error::conflict("人物别名已变化"));
    }
    let (source, target) = person_directory::roots(config, plan);
    if Some(mapped_inventory(&source, &target)?) != person_directory::intent(plan)?
        || target_preview(config, &target)? != data["target_preview"]
    {
        return Err(Error::conflict("人物目录或预览内容已变化"));
    }
    let index = registration::SourceIndex::load(
        config,
        &plan.files.iter().map(|f| f.src.clone()).collect::<Vec<_>>(),
    )?;
    for file in &plan.files {
        if index.tracked_ids(&file.src, &fs::symlink_metadata(&file.src)?)? != file.vids {
            return Err(Error::conflict("稳定视频标识已变化"));
        }
    }
    Ok(())
}
pub fn snapshot(config: &Config, plan: &Plan, successful: &[usize]) -> Result<DomainSnapshot> {
    if successful.iter().copied().ne(0..plan.files.len()) {
        return Err(Error::conflict("人物合并必须提交完整范围"));
    }
    Ok(DomainSnapshot {
        metadata: crate::person_metadata::snapshot(config, plan)?,
        ledger: Some(crate::person_merge_ledger::snapshot(config, plan)?),
    })
}
pub fn mapped(config: &Config, value: &str, plan: &Plan) -> String {
    let Ok(Some(mut inventory)) = person_directory::intent(plan) else {
        return value.into();
    };
    inventory
        .directories
        .sort_by_key(|d| std::cmp::Reverse(d.src.len()));
    for directory in inventory.directories {
        for (src, dst, sep) in [
            (directory.src.clone(), directory.dst.clone(), '/'),
            (
                metadata::win(config, &directory.src),
                metadata::win(config, &directory.dst),
                '\\',
            ),
        ] {
            if value == src {
                return dst;
            }
            if value.starts_with(&format!("{src}{sep}")) {
                return format!("{dst}{}", &value[src.len()..]);
            }
        }
    }
    crate::person_rename::mapped(config, value, plan)
}
