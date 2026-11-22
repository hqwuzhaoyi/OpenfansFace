//! Native `review-delete`: soft-delete unorganized videos from `/review` to global staging
//! under the shared executor, write lock, fingerprint validation, and Peach synchronization.
use crate::{
    config::Config,
    db, engine, files,
    model::{
        AffectedStores, Error, ExecutionIntent, FailurePolicy, FileMove, MoveScope, Plan, Request,
        Result, ScopeRoot,
    },
    native::DomainSnapshot,
    person_registration as registration, review,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    path::{Component, Path},
};

fn payload(request: &Request) -> Result<&Value> {
    request
        .payload
        .as_ref()
        .filter(|v| v.is_object())
        .ok_or_else(|| Error::new(400, "invalid_request", "缺少人工确认删除参数"))
}

pub fn validate(request: &Request) -> Result<()> {
    crate::model::validate_id(&request.id)?;
    if request.action != "review-delete"
        || !request.person.is_empty()
        || !request.filename.is_empty()
        || !request.files.is_empty()
        || request.target_person.is_some()
        || request.target_folder.is_some()
    {
        return Err(Error::new(
            400,
            "invalid_request",
            "人工确认删除不接受人物或目标参数",
        ));
    }
    let body = payload(request)?;
    let ids = review::vids(&body["vids"])?;
    if ids.len() > 500 {
        return Err(Error::new(
            400,
            "invalid_request",
            "一次最多移入回收站 500 个视频",
        ));
    }
    if body["review_session"]
        .as_str()
        .is_none_or(|s| s.trim().is_empty())
        || !body["review_tokens"].is_object()
    {
        return Err(Error::new(
            400,
            "invalid_request",
            "缺少本次核对会话或证据凭据",
        ));
    }
    Ok(())
}

pub fn plan(config: &Config, request: Request) -> Result<Plan> {
    validate(&request)?;
    let body = payload(&request)?;
    let ids = review::vids(&body["vids"])?;

    let (_, queue) = review::read(config, "integration-queue.json", json!([]))?;
    if ids.iter().any(|vid| {
        queue
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["vid"] == *vid)
    }) {
        return Err(Error::conflict("部分视频已在待归档队列中，请刷新后核对"));
    }
    let (_, history) = review::read(config, "archive-history.json", json!([]))?;
    if ids.iter().any(|vid| {
        history
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["vid"] == *vid && row["undone"] != true)
    }) {
        return Err(Error::conflict("部分视频已归档，请刷新后检查完成记录"));
    }

    let tasks = crate::review_sessions::validate(config, body)?;
    let id_set: BTreeSet<String> = ids.iter().cloned().collect();
    let sources = review::sources_for_vids(config, &id_set)?;

    let mut selected_paths = BTreeSet::new();
    let mut verified = Vec::with_capacity(ids.len());
    for vid in &ids {
        let task = tasks
            .iter()
            .find(|t| t["vid"] == *vid)
            .ok_or_else(|| Error::conflict("视频尚未就绪或状态已变化"))?;
        let path = review::source(config, vid, &sources)
            .ok_or_else(|| Error::conflict("源视频无法读取，请核对路径"))?;
        let stat = review::safe_source(config, &path)?;
        if review::health_hidden(config, vid, &path, &stat)? {
            return Err(Error::conflict("视频体检连续确认异常，请到回收站查看记录"));
        }
        if !selected_paths.insert(path.clone()) {
            return Err(Error::conflict("同一个源文件已在本次选择中"));
        }
        let fingerprint_guard = body.get("fingerprints").and_then(|fp| fp.get(vid));
        let mut expected_list = vec![task];
        if let Some(fp) = fingerprint_guard.filter(|v| v.is_object()) {
            expected_list.push(fp);
        }
        for expected in expected_list {
            if expected["path"] != path
                || review::number(&expected["size"]) != Some(stat.len() as i64)
                || expected["mtime_ns"].as_str() != Some(files::mtime_ns(&stat).as_str())
            {
                return Err(Error::conflict("视频文件已变化，请刷新后重新核对"));
            }
        }
        verified.push((vid.clone(), path, stat));
    }

    let all_paths: Vec<String> = verified.iter().map(|(_, p, _)| p.clone()).collect();
    let source_index = registration::SourceIndex::load(config, &all_paths)?;
    let pikpak_root = config.media.join("pikpak");
    let target_root = crate::staging::root(config)
        .join("review-deletions")
        .join(&request.id);

    let mut moves = Vec::with_capacity(verified.len());
    let mut scopes = Vec::with_capacity(verified.len());
    let mut directories = BTreeSet::new();
    let mut total = 0u64;

    for (vid, path, stat) in verified {
        let src_path = Path::new(&path);
        if src_path
            .components()
            .any(|c| !matches!(c, Component::RootDir | Component::Normal(_)))
        {
            return Err(Error::conflict("源路径包含非法组件").at(&path));
        }
        let relative = src_path
            .strip_prefix(&pikpak_root)
            .map_err(|_| Error::conflict("视频不在允许删除的未整理目录").at(&path))?;
        let dst = target_root.join(relative);

        let mut file_vids = source_index.ids_for_path(&path);
        if !file_vids.iter().any(|existing| existing == &vid) {
            file_vids.push(vid);
        }
        file_vids.sort();
        file_vids.dedup();

        let scope = MoveScope {
            source: ScopeRoot::Unorganized,
            destination: ScopeRoot::Staging,
        };
        let version = files::fingerprint(&path, &stat);
        let file = FileMove {
            src: path,
            dst: dst.to_string_lossy().into_owned(),
            vids: file_vids,
            size: stat.len(),
            version,
        };
        files::check_move(config, &scope, &file)?;
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
        data: json!({
            "timestamp": timestamp,
            "affected_thumbs": thumbs,
            "target_directory": target_root.to_string_lossy(),
        }),
        result: json!({
            "ok": true,
            "id": request.id,
            "person": "",
            "staged": true,
            "vids": ids,
        }),
    };
    let mut plan = Plan {
        id: request.id.clone(),
        action: request.action.clone(),
        person: String::new(),
        target_person: String::new(),
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
    if fresh
        .files
        .iter()
        .map(|f| (&f.src, &f.dst, &f.vids, &f.version))
        .collect::<Vec<_>>()
        != plan
            .files
            .iter()
            .map(|f| (&f.src, &f.dst, &f.vids, &f.version))
            .collect::<Vec<_>>()
        || fresh.execution.as_ref().unwrap().data["affected_thumbs"]
            != plan.execution.as_ref().unwrap().data["affected_thumbs"]
    {
        return Err(Error::conflict("文件或核对状态已变化，请重新预览"));
    }
    Ok(())
}

pub fn snapshot(config: &Config, plan: &Plan, successful: &[usize]) -> Result<DomainSnapshot> {
    crate::staging::snapshot(config, plan, successful)
}
