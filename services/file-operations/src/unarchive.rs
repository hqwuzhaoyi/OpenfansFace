//! Native `archive-unarchive`: safely move an archived video back from `creators/<person>/`
//! to its original unorganized location (or a keep-both filename if occupied), restore
//! `faces.db`, `archive-history.json` (marking `undone: true`), `video-path-overrides.json`,
//! Peach `ledger.db` (path + remove creator link), and `processing.db` (`auto_opt_out = 1`,
//! `stage = 'review'`, plus `auto_unarchived` audit event).
use crate::{
    archive_cancel, archive_ledger, auto_watermark,
    config::Config,
    db, files, ledger, metadata,
    model::{
        AffectedStores, Error, ExecutionIntent, FailurePolicy, FileMove, MoveScope, Plan, Request,
        Result, ScopeRoot,
    },
    native::DomainSnapshot,
    permissions, person_registration, processing_scheduler, review,
};
use rusqlite::params;
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
        .ok_or_else(|| Error::new(400, "invalid_request", "缺少撤回归档参数"))
}

pub fn validate(request: &Request) -> Result<()> {
    if !request.files.is_empty() {
        return Err(Error::new(400, "invalid_request", "范围必须由原生计划确定"));
    }
    let body = payload(request)?;
    let vid = body["vid"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| Error::new(400, "invalid_request", "缺少要撤回的视频编号 vid"))?;
    if vid.len() > 8192 || vid.chars().any(char::is_control) {
        return Err(Error::new(400, "invalid_request", "视频编号无效"));
    }
    if let Some(op_id) = body.get("op_id").filter(|v| !v.is_null())
        && op_id.as_str().is_none_or(|s| s.trim().is_empty())
    {
        return Err(Error::new(400, "invalid_request", "op_id 参数无效"));
    }
    Ok(())
}

fn find_history_entry(
    history: &[Value],
    vid: &str,
    op_id: Option<&str>,
) -> Result<(usize, Value)> {
    let found = if let Some(target_op) = op_id {
        let rust_manifest = format!("rust:{target_op}");
        history
            .iter()
            .enumerate()
            .rev()
            .find(|(_, row)| {
                row["vid"].as_str() == Some(vid)
                    && (row["operation_id"].as_str() == Some(target_op)
                        || row["manifest"].as_str() == Some(target_op)
                        || row["manifest"].as_str() == Some(rust_manifest.as_str()))
            })
    } else {
        history
            .iter()
            .enumerate()
            .rev()
            .find(|(_, row)| row["vid"].as_str() == Some(vid))
    };
    let Some((index, row)) = found else {
        return Err(Error::new(
            404,
            "not_found",
            "找不到该视频的归档记录",
        ));
    };
    if row["undone"] == true {
        return Err(Error::conflict("该归档记录已撤回，请勿重复操作"));
    }
    if row["operation"].as_str() == Some("stage-duplicate") {
        return Err(Error::conflict("重复件已移入回收站，请通过回收站恢复"));
    }
    Ok((index, row.clone()))
}

fn validate_unorganized_original(config: &Config, raw_path: &str) -> Result<std::path::PathBuf> {
    let path = Path::new(raw_path);
    let pikpak_root = config.media.join("pikpak");
    if !path.is_absolute()
        || !path.starts_with(&pikpak_root)
        || path == pikpak_root
        || path.starts_with(config.creators())
        || path.starts_with(config.media.join("pikpak/.face-cleanup-staging"))
        || path
            .components()
            .any(|c| !matches!(c, Component::RootDir | Component::Normal(_)))
    {
        return Err(Error::conflict("原未整理路径超出允许范围，无法安全撤回").at(raw_path));
    }
    let parent = path
        .parent()
        .ok_or_else(|| Error::conflict("原未整理父目录无效").at(raw_path))?;
    let parent_meta = parent.symlink_metadata().map_err(|_| {
        Error::conflict("原未整理父目录不存在，无法直接撤回").at(parent.display().to_string())
    })?;
    if !parent_meta.is_dir() || parent_meta.file_type().is_symlink() {
        return Err(
            Error::conflict("原未整理父目录无效或为符号链接").at(parent.display().to_string()),
        );
    }
    let file_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| Error::conflict("原文件名无效").at(raw_path))?;
    files::validate_name(file_name)?;
    Ok(path.to_path_buf())
}

fn resolve_archived_source(
    config: &Config,
    vid: &str,
    entry: &Value,
    creator: &str,
    overrides: &Value,
) -> Result<String> {
    let candidates = [
        overrides[vid].as_str(),
        entry["dst"].as_str(),
        entry["win_dst"].as_str(),
    ];
    let mut resolved = None;
    for raw in candidates.into_iter().flatten() {
        if let Some(disk_path) = person_registration::disk(config, raw) {
            resolved = Some(disk_path);
            break;
        }
    }
    let src = resolved
        .ok_or_else(|| Error::conflict("无法解析已归档视频的当前磁盘路径"))?;
    let creator_dir = config.creators().join(creator);
    let src_path = Path::new(&src);
    if !src_path.starts_with(&creator_dir) || src_path == creator_dir {
        return Err(
            Error::conflict("视频当前不在原归档人物目录中，请先在人物工作台核对位置").at(&src),
        );
    }
    let stat = files::regular_stat(&src)
        .map_err(|_| Error::conflict("已归档视频文件不存在或不是常规文件").at(&src))?;
    if stat.len() == 0 {
        return Err(Error::conflict("已归档视频为 0 字节，拒绝撤回").at(&src));
    }
    Ok(src)
}

fn prepare_unarchive_ledger(
    config: &Config,
    src: &str,
    dst: &str,
    dst_name: &str,
    size: u64,
    creator: &str,
) -> Result<Option<ledger::Delta>> {
    if !config.ledger.exists() {
        return Ok(None);
    }
    let mut conn = db::open_readonly(&config.ledger)?;
    let tx = conn.transaction()?;
    let win_src = metadata::win(config, src);
    let win_dst = metadata::win(config, dst);

    let active = db::rows(
        &tx,
        "SELECT * FROM asset WHERE path=? AND disposal IS NULL ORDER BY id",
        [&win_src],
    )?;
    if active.is_empty() {
        return Ok(None);
    }
    let asset = active[0].clone();
    let aid = db::integer(&asset["id"])?;

    let new_all = db::rows(&tx, "SELECT * FROM asset WHERE path=? ORDER BY id", [&win_dst])?;
    for stub in &new_all {
        if stub["id"] == asset["id"] {
            return Err(Error::new(
                409,
                "ledger_conflict",
                "Peach 源路径与目标恢复路径指向同一记录",
            ));
        }
        let stub_id = db::integer(&stub["id"])?;
        if !db::rows(
            &tx,
            "SELECT 1 FROM asset_entity WHERE asset_id=? LIMIT 1",
            [stub_id],
        )?
        .is_empty()
        {
            return Err(
                Error::new(409, "ledger_conflict", "Peach 目标恢复路径已有带关联的资产记录")
                    .at(dst),
            );
        }
        archive_cancel::is_pure_scanner_stub(
            &tx,
            stub,
            &asset,
            Some(dst_name),
            Some(size as i64),
        )
        .map_err(|_| {
            Error::new(
                409,
                "ledger_conflict",
                "Peach 目标恢复路径已有非扫描占位资产记录",
            )
            .at(dst)
        })?;
    }

    let normalized = creator.to_lowercase();
    let entities = db::rows(
        &tx,
        "SELECT * FROM entity WHERE kind='creator' AND (canonical_name=? OR normalized_name=?) ORDER BY id",
        params![creator, normalized],
    )?;
    if entities.len() > 1 {
        return Err(Error::new(409, "ledger_conflict", "Peach 人物记录不唯一"));
    }
    let creator_eid = entities
        .first()
        .map(|row| db::integer(&row["id"]))
        .transpose()?;

    let links = db::rows(
        &tx,
        "SELECT * FROM asset_entity WHERE asset_id=? ORDER BY entity_id,role,source",
        [aid],
    )?;
    let after_links: Vec<Value> = links
        .iter()
        .filter(|row| {
            !(row["role"] == "creator"
                && creator_eid
                    .is_some_and(|eid| db::integer(&row["entity_id"]).ok() == Some(eid)))
        })
        .cloned()
        .collect();

    let mut after_asset = asset.clone();
    after_asset["path"] = json!(win_dst);
    let src_name = Path::new(src).file_name().and_then(|s| s.to_str());
    if after_asset.get("name").is_some_and(Value::is_string) && src_name != Some(dst_name) {
        after_asset["name"] = json!(dst_name);
    }

    let old_all = db::rows(&tx, "SELECT * FROM asset WHERE path=? ORDER BY id", [&win_src])?;
    let mut old_after = old_all.clone();
    old_after.retain(|row| row["id"] != asset["id"]);

    let mut assets_before = vec![asset.clone()];
    let mut guards = vec![];
    for stub in &new_all {
        assets_before.push(stub.clone());
        guards.push(db::Guard::AssetLinks {
            asset_id: stub["id"].clone(),
            before: vec![],
            after: vec![],
        });
    }
    guards.push(db::Guard::AssetPath {
        path: win_src,
        before: old_all,
        after: old_after,
    });
    guards.push(db::Guard::AssetPath {
        path: win_dst,
        before: new_all,
        after: vec![after_asset.clone()],
    });
    guards.push(db::Guard::AssetLinks {
        asset_id: asset["id"].clone(),
        before: links.clone(),
        after: after_links.clone(),
    });
    guards.push(db::Guard::CreatorName {
        canonical: creator.into(),
        normalized,
        before: entities.clone(),
        after: entities.clone(),
    });

    let entity_keys = archive_ledger::keys(&tx, "entity")?;
    let link_keys = archive_ledger::keys(&tx, "asset_entity")?;
    Ok(Some(ledger::Delta {
        creators: vec![],
        changes: db::Delta {
            guards,
            tables: vec![
                archive_ledger::table(
                    db::Table::Entity,
                    entity_keys,
                    entities.clone(),
                    entities,
                ),
                archive_ledger::table(
                    db::Table::Asset,
                    vec!["id".into()],
                    assets_before,
                    vec![after_asset],
                ),
                archive_ledger::table(db::Table::AssetEntity, link_keys, links, after_links),
            ],
        },
    }))
}

fn load_documents(config: &Config) -> Result<Vec<metadata::Document>> {
    let mut documents = vec![];
    for (name, fallback) in [
        ("integration-queue.json", json!([])),
        ("archive-history.json", json!([])),
        ("video-path-overrides.json", json!({})),
        ("report.json", json!({})),
        ("claims-detail.json", json!([])),
        ("clusters.json", json!([])),
        ("template-overrides.json", json!({})),
        ("watermarks/mappings.json", json!({})),
    ] {
        let (before, _) = review::read(config, name, fallback)?;
        documents.push(metadata::Document {
            name: name.into(),
            after: before.clone(),
            before,
        });
    }
    Ok(documents)
}

pub fn plan(config: &Config, request: Request) -> Result<Plan> {
    validate(&request)?;
    let body = payload(&request)?;
    let vid = body["vid"].as_str().unwrap().trim().to_owned();
    let op_id = body
        .get("op_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let at = body["at"]
        .as_str()
        .ok_or_else(|| Error::new(400, "invalid_request", "缺少持久化操作时间"))?;

    let (_, queue) = review::read(config, "integration-queue.json", json!([]))?;
    if queue
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["vid"].as_str() == Some(vid.as_str()))
    {
        return Err(Error::conflict("该视频已在待归档队列中，无需撤回归档"));
    }

    let (_, history) = review::read(config, "archive-history.json", json!([]))?;
    let history_rows = history.as_array().unwrap();
    let (history_index, entry) = find_history_entry(history_rows, &vid, op_id)?;

    let aliases = review::read(config, "person-renames.json", json!({}))?.1;
    let creator = review::creator_with_aliases(&entry["creator"], &aliases)?;
    files::validate_name(&creator)?;

    let raw_original = entry["original_path"]
        .as_str()
        .or_else(|| entry["src"].as_str())
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| Error::conflict("归档历史缺少原未整理路径 original_path，无法撤回"))?;
    let original_path = validate_unorganized_original(config, raw_original)?;
    let original_parent = original_path.parent().unwrap();
    let original_name = original_path.file_name().unwrap().to_str().unwrap();

    let (_, overrides) = review::read(config, "video-path-overrides.json", json!({}))?;
    let src = resolve_archived_source(config, &vid, &entry, &creator, &overrides)?;
    let stat = files::regular_stat(&src)?;

    let mut reserved = BTreeSet::new();
    let (target_name, renamed_keep_both) =
        auto_watermark::keep_both_target_name(original_parent, original_name, &vid, &mut reserved)?;
    let dst_path = original_parent.join(&target_name);
    let dst = dst_path.to_string_lossy().to_string();
    if dst_path.symlink_metadata().is_ok() {
        return Err(Error::conflict("恢复目标文件已存在，无法覆盖").at(&dst));
    }

    permissions::require_move_permissions(Path::new(&src), &dst_path)?;

    let faces_path = config.base.join("faces.db");
    let mut vids = if faces_path.exists() {
        metadata::registered_ids(config, &src)?
    } else {
        vec![]
    };
    if !vids.contains(&vid) {
        vids.push(vid.clone());
    }

    // Ensure `processing.db` has the `auto_opt_out` column before taking the before-snapshot
    // so `delta::apply` / `inspect` / `restore` see identical columns.
    let jobs_path = config.base.join("processing/processing.db");
    if jobs_path.exists() {
        let conn = db::open(&jobs_path)?;
        processing_scheduler::ensure_columns(&conn)?;
    }

    let win_dst = metadata::win(config, &dst);
    let win_src = metadata::win(config, &src);

    let all_videos = if faces_path.exists() {
        db::rows(&db::open(&faces_path)?, "SELECT * FROM videos ORDER BY vid", [])?
    } else {
        vec![]
    };
    let all_jobs = if jobs_path.exists() {
        db::rows(&db::open(&jobs_path)?, "SELECT * FROM tasks ORDER BY vid", [])?
    } else {
        vec![]
    };
    for row in all_videos.iter().chain(all_jobs.iter()) {
        if (row["path"] == dst || row["path"] == win_dst)
            && !vids.iter().any(|id| row["vid"] == *id)
        {
            return Err(Error::conflict("目标恢复路径已有其他本地视频或任务记录").at(&dst));
        }
    }

    let videos: Vec<Value> = all_videos
        .into_iter()
        .filter(|r| {
            vids.iter().any(|id| r["vid"] == *id)
                || r["path"] == src
                || r["path"] == win_src
        })
        .collect();
    let jobs: Vec<Value> = all_jobs
        .into_iter()
        .filter(|r| {
            vids.iter().any(|id| r["vid"] == *id)
                || r["path"] == src
                || r["path"] == win_src
        })
        .collect();

    let ledger_delta =
        prepare_unarchive_ledger(config, &src, &dst, &target_name, stat.len(), &creator)?;
    let documents = load_documents(config)?;

    let file_move = FileMove {
        vids,
        src: src.clone(),
        dst: dst.clone(),
        size: stat.len(),
        version: files::fingerprint(&src, &stat),
    };
    let scope = MoveScope {
        source: ScopeRoot::Person(creator.clone()),
        destination: ScopeRoot::Unorganized,
    };

    let data = json!({
        "vid": vid,
        "person": creator,
        "history_index": history_index,
        "history_entry": entry,
        "original_path": raw_original,
        "restored_path": dst,
        "target_name": target_name,
        "renamed_keep_both": renamed_keep_both,
        "mtime_ns": files::mtime_ns(&stat),
        "size": stat.len(),
        "at": at,
        "videos": videos,
        "jobs": jobs,
        "documents": documents,
        "ledger": ledger_delta,
    });

    let result_person = creator.clone();
    Ok(Plan {
        id: request.id.clone(),
        request,
        version: String::new(),
        action: "archive-unarchive".into(),
        person: creator,
        target_person: String::new(),
        files: vec![file_move],
        total_bytes: stat.len(),
        directories: vec![],
        ledger_version: String::new(),
        metadata_version: String::new(),
        execution: Some(ExecutionIntent {
            scopes: vec![scope],
            failure_policy: FailurePolicy::Rollback,
            stores: AffectedStores {
                faces: faces_path.exists() && !data["videos"].as_array().unwrap().is_empty(),
                jobs: jobs_path.exists() && !data["jobs"].as_array().unwrap().is_empty(),
                ledger: ledger_delta.is_some(),
            },
            data,
            result: json!({
                "ok": true,
                "vid": vid,
                "person": result_person,
                "restored_path": dst,
                "renamed_keep_both": renamed_keep_both,
                "auto_opt_out": true,
            }),
        }),
    })
}

pub fn revalidate(config: &Config, plan: &Plan) -> Result<()> {
    let intent = plan
        .execution
        .as_ref()
        .ok_or_else(|| Error::conflict("撤回归档计划缺少执行证据"))?;
    let data = &intent.data;
    let file = plan
        .files
        .first()
        .ok_or_else(|| Error::conflict("撤回归档计划缺少文件"))?;
    let stat = files::regular_stat(&file.src)?;
    if files::fingerprint(&file.src, &stat) != file.version {
        return Err(Error::conflict("已归档视频已变化，请刷新后重试").at(&file.src));
    }
    if Path::new(&file.dst).symlink_metadata().is_ok() {
        return Err(Error::conflict("恢复目标文件已存在，无法覆盖").at(&file.dst));
    }
    for doc in serde_json::from_value::<Vec<metadata::Document>>(data["documents"].clone())? {
        if files::read_optional(&config.base.join(&doc.name))? != doc.before {
            return Err(Error::conflict("归档历史或关联记录已变化，请刷新后重试").at(doc.name));
        }
    }
    let vid = data["vid"].as_str().unwrap_or("");
    let (_, history) = review::read(config, "archive-history.json", json!([]))?;
    let idx = data["history_index"].as_u64().unwrap_or(usize::MAX as u64) as usize;
    let current = history
        .as_array()
        .and_then(|rows| rows.get(idx))
        .ok_or_else(|| Error::conflict("归档历史记录位置已变化"))?;
    if current != &data["history_entry"] || current["vid"].as_str() != Some(vid) {
        return Err(Error::conflict("归档历史记录已变化，请刷新后重试"));
    }
    Ok(())
}

pub fn snapshot(config: &Config, plan: &Plan, successful: &[usize]) -> Result<DomainSnapshot> {
    let execution = plan
        .execution
        .as_ref()
        .ok_or_else(|| Error::conflict("撤回归档意图缺失"))?;
    if successful.len() != plan.files.len() {
        return Err(Error::conflict("撤回归档仅支持完整文件恢复"));
    }
    let data = &execution.data;
    let file = &plan.files[0];
    let at = data["at"]
        .as_str()
        .ok_or_else(|| Error::conflict("撤回归档缺少时间戳"))?;
    let now_epoch = review::timestamp(at)?;
    let restored_name = Path::new(&file.dst)
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let restored_parent = Path::new(&file.dst)
        .parent()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let size = data["size"].as_u64().unwrap_or(file.size) as i64;
    let mtime_ns = data["mtime_ns"].as_str().unwrap_or("");

    let videos: Vec<Value> = serde_json::from_value(data["videos"].clone())?;
    let mut after_videos = videos.clone();
    for row in &mut after_videos {
        row["path"] = json!(file.dst);
        if row.get("name").is_some() {
            row["name"] = json!(restored_name);
        }
        if row.get("collection").is_some() {
            row["collection"] = json!(restored_parent);
        }
    }

    let jobs: Vec<Value> = serde_json::from_value(data["jobs"].clone())?;
    let mut after_jobs = jobs.clone();
    for row in &mut after_jobs {
        let old = review::task_data(row)?;
        row["path"] = json!(file.dst);
        row["name"] = json!(restored_name);
        if row.get("size").is_some() {
            row["size"] = review::integer(size);
        }
        if row.get("mtime_ns").is_some() {
            row["mtime_ns"] = json!(mtime_ns);
        }
        row["stage"] = json!(if old["ready"] == true {
            "review"
        } else {
            "queued"
        });
        if row.get("deferred").is_some() {
            row["deferred"] = review::integer(0);
        }
        if row.get("manual").is_some() {
            row["manual"] = review::integer(0);
        }
        if row.get("auto_opt_out").is_some() {
            row["auto_opt_out"] = review::integer(1);
        }
        if row.get("updated_at").is_some() {
            row["updated_at"] = json!(now_epoch);
        }
        row["data"] = json!(serde_json::to_string(&metadata::transform(
            config,
            &old,
            plan
        ))?);
    }

    let history_index = data["history_index"].as_u64().unwrap() as usize;
    let mut documents: Vec<metadata::Document> = serde_json::from_value(data["documents"].clone())?;
    for doc in &mut documents {
        let mut value: Value = doc
            .before
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?
            .unwrap_or_else(|| {
                if [
                    "integration-queue.json",
                    "archive-history.json",
                    "claims-detail.json",
                    "clusters.json",
                ]
                .contains(&doc.name.as_str())
                {
                    json!([])
                } else {
                    json!({})
                }
            });
        let original = value.clone();
        if ![
            "integration-queue.json",
            "archive-history.json",
            "video-path-overrides.json",
        ]
        .contains(&doc.name.as_str())
        {
            value = metadata::transform(config, &value, plan);
        }
        if doc.name == "archive-history.json" {
            let rows = value.as_array_mut().unwrap();
            let entry = rows
                .get_mut(history_index)
                .ok_or_else(|| Error::conflict("归档历史索引越界"))?;
            entry["undone"] = json!(true);
            entry["undone_at"] = json!(at);
            entry["undone_operation_id"] = json!(plan.id);
            entry["restored_path"] = json!(file.dst);
        }
        if doc.name == "video-path-overrides.json" {
            for id in &file.vids {
                value[id] = json!(file.dst);
            }
        }
        doc.after = if value == original {
            doc.before.clone()
        } else {
            Some(serde_json::to_string_pretty(&value)?)
        };
    }

    let paths = vec![
        file.src.clone(),
        file.dst.clone(),
        metadata::win(config, &file.src),
        metadata::win(config, &file.dst),
    ];

    let faces_delta = if execution.stores.faces {
        Some(db::Delta {
            guards: vec![db::Guard::LocalFiles {
                table: db::Table::Videos,
                vids: file.vids.clone(),
                paths: paths.clone(),
                before: videos.clone(),
                after: after_videos.clone(),
            }],
            tables: vec![archive_ledger::table(
                db::Table::Videos,
                vec!["vid".into()],
                videos,
                after_videos,
            )],
        })
    } else {
        None
    };

    let jobs_delta = if execution.stores.jobs {
        Some(db::Delta {
            guards: vec![db::Guard::LocalFiles {
                table: db::Table::Tasks,
                vids: file.vids.clone(),
                paths,
                before: jobs.clone(),
                after: after_jobs.clone(),
            }],
            tables: vec![archive_ledger::table(
                db::Table::Tasks,
                vec!["vid".into()],
                jobs,
                after_jobs,
            )],
        })
    } else {
        None
    };

    let ledger_delta: Option<ledger::Delta> = if execution.stores.ledger {
        serde_json::from_value(data["ledger"].clone())?
    } else {
        None
    };

    Ok(DomainSnapshot {
        metadata: metadata::Snapshot {
            documents,
            videos: vec![],
            jobs: vec![],
            faces_delta,
            jobs_delta,
        },
        ledger: ledger_delta,
    })
}

pub fn result(plan: &Plan, successful: &[usize], failures: &[Value]) -> Value {
    let data = &plan.execution.as_ref().unwrap().data;
    json!({
        "ok": failures.is_empty() && successful.len() == plan.files.len(),
        "id": plan.id,
        "vid": data["vid"],
        "person": data["person"],
        "restored_path": data["restored_path"],
        "renamed_keep_both": data["renamed_keep_both"] == true,
        "auto_opt_out": true,
        "failures": failures,
    })
}
