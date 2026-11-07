//! Person file planning; execution and compensation belong to the shared durable runner.
use crate::native::DomainSnapshot;
use crate::{
    config::Config,
    db, engine, files,
    model::{
        AffectedStores, Error, ExecutionIntent, FailurePolicy, FileMove, MoveScope, Plan, Request,
        Result, ScopeRoot,
    },
    person_registration as registration,
};
use serde_json::json;
use std::{
    collections::BTreeSet,
    path::{Component, Path},
};

pub fn validate(request: &Request) -> Result<()> {
    crate::model::validate_id(&request.id)?;
    files::validate_name(&request.person)?;
    if !["rename-file", "move-files"].contains(&request.action.as_str()) {
        return Err(Error::new(400, "invalid_request", "请选择文件操作"));
    }
    if request.files.is_empty() || request.files.len() > 500 {
        return Err(Error::new(
            400,
            "invalid_request",
            "请选择 1–500 个已有视频",
        ));
    }
    if request.action == "rename-file" {
        if request.files.len() != 1 {
            return Err(Error::new(
                400,
                "invalid_request",
                "重命名每次只处理一个文件",
            ));
        }
        files::validate_name(&request.filename)?;
    } else {
        files::validate_name(
            request
                .target_person
                .as_deref()
                .ok_or_else(|| Error::new(400, "invalid_request", "请选择目标人物"))?,
        )?;
        subfolder(request.target_folder.as_deref().unwrap_or(""))?;
    }
    if request.payload.is_some() {
        return Err(Error::new(
            400,
            "invalid_request",
            "人物文件操作不接受附加执行参数",
        ));
    }
    for file in &request.files {
        if file.path.len() > 4096 || file.vid.len() > 4096 || file.version.len() != 64 {
            return Err(Error::new(400, "invalid_request", "文件路径或版本无效"));
        }
    }
    Ok(())
}
fn subfolder(value: &str) -> Result<()> {
    if value.is_empty() {
        return Ok(());
    }
    if value.starts_with('/') || value.contains('\\') {
        return Err(Error::new(
            400,
            "invalid_request",
            "请输入相对人物目录的子文件夹",
        ));
    }
    for segment in value.split('/') {
        files::validate_name(segment)?;
    }
    Ok(())
}
pub fn plan(config: &Config, request: Request) -> Result<Plan> {
    validate(&request)?;
    let person = registration::canonical(config, &request.person)?;
    let target = if request.action == "move-files" {
        registration::canonical(config, request.target_person.as_deref().unwrap())?
    } else {
        person.clone()
    };
    if !registration::known_person(config, &person)? {
        return Err(Error::new(404, "missing_person", "找不到当前人物，请刷新"));
    }
    if !registration::known_person(config, &target)? {
        return Err(Error::new(404, "missing_person", "请选择已有的目标人物"));
    }
    let target_root = config
        .creators()
        .join(&target)
        .join(request.target_folder.as_deref().unwrap_or(""));
    let sources = registration::SourceIndex::load(
        config,
        &request
            .files
            .iter()
            .map(|f| f.path.clone())
            .collect::<Vec<_>>(),
    )?;
    let mut seen = BTreeSet::new();
    let mut targets = BTreeSet::new();
    let mut moves = vec![];
    let mut scopes = vec![];
    let mut total = 0u64;
    let mut directories = BTreeSet::new();
    for selected in &request.files {
        let src = Path::new(&selected.path);
        if !src.is_absolute()
            || src
                .components()
                .any(|c| !matches!(c, Component::RootDir | Component::Normal(_)))
        {
            return Err(
                Error::new(400, "invalid_request", "文件路径必须是绝对路径且不能含 ..")
                    .at(&selected.path),
            );
        }
        if !seen.insert(selected.path.clone()) {
            return Err(Error::new(400, "invalid_request", "重复选择了同一个文件"));
        }
        let extension = src
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();
        if ![
            "mp4", "m4v", "mov", "mkv", "webm", "avi", "wmv", "ts", "mts", "m2ts",
        ]
        .contains(&extension.as_str())
        {
            return Err(
                Error::new(400, "invalid_request", "只允许操作人物目录中的视频").at(&selected.path),
            );
        }
        let dst = if request.action == "rename-file" {
            if Path::new(&request.filename)
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_lowercase()
                != extension
            {
                return Err(Error::new(400, "invalid_request", "重命名需保留原扩展名"));
            }
            src.parent()
                .ok_or_else(|| Error::new(400, "invalid_request", "文件路径无效"))?
                .join(&request.filename)
        } else {
            target_root.join(
                src.file_name()
                    .ok_or_else(|| Error::new(400, "invalid_request", "文件路径无效"))?,
            )
        };
        let dst = dst.to_string_lossy().into_owned();
        if dst == selected.path {
            return Err(Error::new(400, "invalid_request", "源文件与目标路径相同"));
        }
        if !targets.insert(dst.to_lowercase()) {
            return Err(
                Error::new(409, "target_exists", "选中文件在目标目录中同名，请先改名").at(&dst),
            );
        }
        let scope = MoveScope {
            source: ScopeRoot::Person(person.clone()),
            destination: ScopeRoot::Person(target.clone()),
        };
        let mut file = FileMove {
            src: selected.path.clone(),
            dst,
            vids: vec![],
            size: 0,
            version: selected.version.clone(),
        };
        let stat = files::check_move(config, &scope, &file)?;
        file.vids = sources.selected_ids(&file.src, &selected.vid, &stat)?;
        file.size = stat.len();
        total = total
            .checked_add(file.size)
            .ok_or_else(|| Error::new(400, "invalid_request", "文件总大小超出范围"))?;
        let mut parent = Path::new(&file.dst).parent();
        while let Some(dir) = parent {
            if dir.exists() {
                break;
            }
            directories.insert(dir.to_string_lossy().into_owned());
            parent = dir.parent();
        }
        moves.push(file);
        scopes.push(scope);
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
                if let Some(thumb) = row["thumb"].as_str() {
                    thumbs.insert(thumb.to_owned());
                }
            }
        }
    }
    let next_entity_id: i64 = db::open(&config.ledger)?.query_row(
        "SELECT COALESCE(MAX(id),0)+1 FROM entity",
        [],
        |r| r.get(0),
    )?;
    let execution = ExecutionIntent {
        scopes,
        failure_policy: FailurePolicy::Rollback,
        stores: AffectedStores {
            faces: true,
            jobs: config.base.join("processing/processing.db").exists(),
            ledger: true,
        },
        data: json!({"timestamp":timestamp,"affected_thumbs":thumbs,"next_entity_id":next_entity_id.to_string()}),
        result: json!({"ok":true,"id":request.id,"person":person,"staged":false}),
    };
    let mut plan = Plan {
        id: request.id.clone(),
        action: request.action.clone(),
        person,
        target_person: target,
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
pub fn snapshot(config: &Config, plan: &Plan, successful: &[usize]) -> Result<DomainSnapshot> {
    if successful.len() != plan.files.len() || successful.iter().copied().ne(0..plan.files.len()) {
        return Err(Error::conflict("人物移动必须提交完整的已确认范围"));
    }
    Ok(DomainSnapshot {
        metadata: crate::person_metadata::snapshot(config, plan)?,
        ledger: Some(crate::person_ledger::snapshot(config, plan)?),
    })
}
pub fn revalidate(config: &Config, plan: &Plan) -> Result<()> {
    if registration::canonical(config, &plan.request.person)? != plan.person
        || !registration::known_person(config, &plan.person)?
        || !registration::known_person(config, &plan.target_person)?
    {
        return Err(Error::conflict("人物目录或别名已变化"));
    }
    if plan.action == "move-files"
        && registration::canonical(config, plan.request.target_person.as_deref().unwrap())?
            != plan.target_person
    {
        return Err(Error::conflict("目标人物别名已变化"));
    }
    let connection = db::open(&config.base.join("faces.db"))?;
    let mut thumbs = BTreeSet::new();
    for file in &plan.files {
        for vid in &file.vids {
            for row in db::rows(&connection, "SELECT thumb FROM faces WHERE vid=?", [vid])? {
                if let Some(thumb) = row["thumb"].as_str() {
                    thumbs.insert(thumb.to_owned());
                }
            }
        }
    }
    if json!(thumbs) != plan.execution.as_ref().unwrap().data["affected_thumbs"] {
        return Err(Error::conflict("相关缩略图记录已变化，请重新预览"));
    }
    let sources = registration::SourceIndex::load(
        config,
        &plan.files.iter().map(|f| f.src.clone()).collect::<Vec<_>>(),
    )?;
    for (index, file) in plan.files.iter().enumerate() {
        let stat = files::check_move(
            config,
            &plan.execution.as_ref().unwrap().scopes[index],
            file,
        )?;
        if sources.selected_ids(&file.src, &plan.request.files[index].vid, &stat)? != file.vids {
            return Err(Error::conflict("视频稳定编号或路径记录已变化"));
        }
    }
    Ok(())
}
