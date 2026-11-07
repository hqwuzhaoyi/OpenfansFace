//! Pending-queue archive, including confirmed keep-both and verified duplicate staging.
use crate::native::DomainSnapshot;
use crate::{
    archive_ledger,
    config::Config,
    db, files, metadata,
    model::{
        AffectedStores, Error, ExecutionIntent, FailurePolicy, FileMove, MoveScope, Plan, Request,
        Result, ScopeRoot, hash,
    },
    permissions, review,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

pub fn preview(config: &Config) -> Result<Value> {
    let (_, queue) = review::read(config, "integration-queue.json", json!([]))?;
    preview_queue(config, &queue)
}
pub fn preview_queue(config: &Config, queue: &Value) -> Result<Value> {
    let sources = review::sources(config)?;
    let videos = db::rows(
        &db::open(&config.base.join("faces.db"))?,
        "SELECT * FROM videos ORDER BY vid",
        [],
    )?;
    let mut videos_by_path: BTreeMap<String, Vec<&Value>> = BTreeMap::new();
    for row in &videos {
        if let Some(path) = row["vid"]
            .as_str()
            .and_then(|id| review::source(config, id, &sources))
        {
            videos_by_path.entry(path).or_default().push(row);
        }
    }
    let jobs_path = config.base.join("processing/processing.db");
    let jobs = if jobs_path.exists() {
        db::rows(
            &db::open(&jobs_path)?,
            "SELECT * FROM tasks ORDER BY vid",
            [],
        )?
    } else {
        vec![]
    };
    let ledger = db::open(&config.ledger)?;
    let mut related_documents = vec![];
    for (name, fallback) in [
        ("report.json", json!({})),
        ("claims-detail.json", json!([])),
        ("clusters.json", json!([])),
        ("template-overrides.json", json!({})),
        ("watermarks/mappings.json", json!({})),
    ] {
        related_documents.push((name, review::read(config, name, fallback)?.1));
    }
    let mut video_info = BTreeMap::new();
    for (name, value) in &related_documents {
        let groups = if *name == "report.json" {
            value["identities"].as_array()
        } else if *name == "clusters.json" {
            value.as_array()
        } else {
            None
        };
        if let Some(groups) = groups {
            for group in groups {
                if let Some(videos) = group["vids"].as_array() {
                    for video in videos {
                        if let Some(vid) = video["vid"].as_str() {
                            video_info.insert(vid.to_owned(), video.clone());
                        }
                    }
                }
            }
        }
    }
    let mut rows = vec![];
    let mut targets = BTreeSet::new();
    for item in queue.as_array().unwrap() {
        let vid = item["vid"]
            .as_str()
            .ok_or_else(|| Error::conflict("待归档记录缺少视频编号"))?;
        let src = review::source(config, vid, &sources);
        let name = src
            .as_deref()
            .and_then(|s| Path::new(s).file_name())
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| {
                vid.replace('\\', "/")
                    .rsplit('/')
                    .next()
                    .unwrap_or(vid)
                    .to_owned()
            });
        let info = video_info.get(vid);
        let thumb = info
            .and_then(|v| v["thumb"].as_str())
            .filter(|v| !v.is_empty())
            .map(|value| {
                if value.starts_with("/thumb?") {
                    value.to_owned()
                } else {
                    format!("/thumb?p={}", review::percent(value))
                }
            });
        let name = if src.is_none() {
            info.and_then(|v| v["name"].as_str())
                .map(str::to_owned)
                .unwrap_or(name)
        } else {
            name
        };
        let mut row = item.clone();
        for(k,v)in json!({"name":name,"thumb":thumb,"src":src,"dst":null,"size":0,"size_gb":0,"mtime":0,"status":"ok","reason":"可以归档","operation":"archive"}).as_object().unwrap(){row[k]=v.clone()}
        let checked = (|| -> Result<()> {
            let creator = review::creator(config, &item["creator"])?;
            row["creator"] = json!(creator);
            let src = src
                .as_deref()
                .ok_or_else(|| Error::conflict("记录中的原路径找不到文件，请核对实际位置"))?;
            if Path::new(src).starts_with(config.creators()) {
                row["status"] = json!("organized");
            }
            let stat = review::safe_source(config, src)?;
            if stat.len() == 0 {
                row["status"] = json!("empty");
                return Err(Error::conflict("源文件为 0 字节，无法归档"));
            }
            if review::health_hidden(config, vid, src, &stat)? {
                row["status"] = json!("corrupt");
                return Err(Error::conflict("视频体检连续确认异常，请查看回收站记录"));
            }
            let original = config.creators().join(&creator).join(&name);
            let mut dst = original.clone();
            row["original_dst"] = json!(original.to_string_lossy());
            row["suggested_name"] = json!(crate::archive_resolution::alternate_name(&name, vid));
            row["size"] = json!(stat.len());
            row["size_gb"] = json!((stat.len() as f64 / 1073741824.0 * 100.0).round() / 100.0);
            row["mtime"] = json!(files::mtime_ns(&stat).parse::<i128>().unwrap_or(0) / 1_000_000);
            row["file_version"] = json!(files::fingerprint(src, &stat));
            if let Ok(target) = files::regular_stat(&original.to_string_lossy()) {
                row["original_size"] = json!(target.len());
            }
            match item["resolution"]["kind"].as_str() {
                Some("rename") => {
                    let filename = item["resolution"]["name"]
                        .as_str()
                        .ok_or_else(|| Error::conflict("保留文件名无效"))?;
                    crate::archive_resolution::validate_name(filename, &name)?;
                    dst = config.creators().join(&creator).join(filename);
                    row["reason"] = json!("已选择保留两份，将按新文件名归档");
                }
                Some("stage-duplicate") | None => (),
                _ => return Err(Error::conflict("冲突处理选择无效")),
            }
            row["dst"] = json!(dst.to_string_lossy());
            if dst.symlink_metadata().is_ok() {
                let target = std::fs::symlink_metadata(&dst)?;
                row["dst_size"] = json!(target.len());
                row["dst_mtime"] =
                    json!(files::mtime_ns(&target).parse::<i128>().unwrap_or(0) / 1_000_000);
                row["status"] = json!(if target.is_file()
                    && !target.file_type().is_symlink()
                    && target.len() == stat.len()
                {
                    "duplicate"
                } else {
                    "conflict"
                });
                if row["status"] == "duplicate" && item["resolution"]["kind"] == "stage-duplicate" {
                    let target = files::regular_stat(&dst.to_string_lossy())?;
                    row["canonical_version"] =
                        json!(files::fingerprint(&dst.to_string_lossy(), &target));
                    row["operation"] = json!("stage-duplicate");
                    row["status"] = json!("ok");
                    row["reason"] = json!("执行时完整校验内容，完全一致才把源文件移入暂存");
                } else {
                    return Err(Error::conflict("目标存在同名文件，需先处理冲突"));
                }
            } else if item["resolution"]["kind"] == "stage-duplicate" {
                return Err(Error::conflict("目标文件已变化，无法暂存重复件"));
            }
            if row["operation"] != "stage-duplicate" && !targets.insert(dst.clone()) {
                row["status"] = json!("dupe-target");
                return Err(Error::conflict("队列里另一个视频也将使用此目标文件名"));
            }
            let scoped_videos = videos_by_path.get(src).cloned().unwrap_or_default();
            row["vid_aliases"] = json!(
                scoped_videos
                    .iter()
                    .filter_map(|row| row["vid"].as_str())
                    .collect::<Vec<_>>()
            );
            let destination_videos = videos_by_path
                .get(&dst.to_string_lossy().to_string())
                .cloned()
                .unwrap_or_default();
            let scoped_jobs = jobs
                .iter()
                .filter(|r| {
                    r["vid"] == vid
                        || r["path"] == src
                        || r["path"] == dst.to_string_lossy().as_ref()
                        || r["path"] == metadata::win(config, src)
                        || scoped_videos.iter().any(|v| v["vid"] == r["vid"])
                })
                .collect::<Vec<_>>();
            let src_assets = db::rows(
                &ledger,
                "SELECT * FROM asset WHERE path=? ORDER BY id",
                [metadata::win(config, src)],
            )?;
            let dst_assets = db::rows(
                &ledger,
                "SELECT * FROM asset WHERE path=? ORDER BY id",
                [metadata::win(config, &dst.to_string_lossy())],
            )?;
            let creators = db::rows(
                &ledger,
                "SELECT * FROM entity WHERE kind='creator' AND (canonical_name=? OR normalized_name=?) ORDER BY id",
                rusqlite::params![creator, creator.to_lowercase()],
            )?;
            let mut links = vec![];
            for asset in src_assets.iter().chain(&dst_assets) {
                links.extend(db::rows(
                    &ledger,
                    "SELECT * FROM asset_entity WHERE asset_id=? ORDER BY entity_id,role,source",
                    [db::integer(&asset["id"])?],
                )?);
            }
            let documents = related_documents
                .iter()
                .map(|(name, value)| {
                    let mut matches = vec![];
                    related(value, vid, src, &metadata::win(config, src), &mut matches);
                    json!([name, matches])
                })
                .collect::<Vec<_>>();
            row["record_version"] = json!(hash(
                &json!({"videos":scoped_videos,"destination_videos":destination_videos,"jobs":scoped_jobs,"source_assets":src_assets,"destination_assets":dst_assets,"entities":creators,"links":links,"documents":documents})
            ));
            let actual = if row["operation"] == "stage-duplicate" {
                config
                    .media
                    .join("pikpak/.face-cleanup-staging/archive-duplicates/pending")
            } else {
                dst
            };
            permissions::require_move_permissions(Path::new(src), &actual)?;
            Ok(())
        })();
        if let Err(error) = checked {
            if row["status"] == "ok" {
                row["status"] = json!(if error.status == 403 {
                    "permission"
                } else if src.is_none() || error.code == "source_changed" {
                    "missing"
                } else {
                    "invalid"
                })
            }
            row["reason"] = json!(error.error)
        }
        rows.push(row);
    }
    let n_ok = rows.iter().filter(|r| r["status"] == "ok").count();
    let bytes: u64 = rows
        .iter()
        .filter(|r| r["status"] == "ok")
        .map(|r| r["size"].as_u64().unwrap_or(0))
        .sum();
    let history = review::read(config, "archive-history.json", json!([]))?.1;
    Ok(
        json!({"version":hash(&rows),"n":rows.len(),"n_ok":n_ok,"total_gb":(bytes as f64/1073741824.0*10.0).round()/10.0,"rows":rows,"history":history.as_array().unwrap().iter().rev().take(200).collect::<Vec<_>>()}),
    )
}
fn payload(request: &Request) -> Result<&Value> {
    request
        .payload
        .as_ref()
        .ok_or_else(|| Error::new(400, "invalid_request", "缺少归档范围"))
}
pub fn plan(config: &Config, request: Request) -> Result<Plan> {
    let body = payload(&request)?;
    let ids = review::vids(&body["vids"])?;
    let current = preview(config)?;
    if current["version"] != body["version"] {
        return Err(Error::conflict("待归档内容或文件状态已变化，请刷新预览"));
    }
    let rows = current["rows"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| ids.iter().any(|id| r["vid"] == *id))
        .cloned()
        .collect::<Vec<_>>();
    if rows.len() != ids.len() || rows.iter().any(|r| r["status"] != "ok") {
        return Err(Error::conflict("只能归档检查通过的视频，请处理异常项"));
    }
    let at = body["at"]
        .as_str()
        .ok_or_else(|| Error::new(400, "invalid_request", "缺少持久化操作时间"))?;
    let mut moves = vec![];
    let mut scopes = vec![];
    let mut sources = BTreeSet::new();
    for row in &rows {
        let src = row["src"].as_str().unwrap();
        if !sources.insert(src) {
            return Err(Error::conflict("多个待归档编号指向同一个文件"));
        }
        let mut vids: Vec<String> = serde_json::from_value(row["vid_aliases"].clone())?;
        let vid = row["vid"].as_str().unwrap().to_owned();
        if !vids.contains(&vid) {
            vids.push(vid)
        }
        moves.push(FileMove {
            vids: vids.clone(),
            src: src.into(),
            dst: if row["operation"] == "stage-duplicate" {
                config
                    .media
                    .join("pikpak/.face-cleanup-staging/archive-duplicates")
                    .join(&request.id)
                    .join(&hash(&json!(vids))[..16])
                    .join(Path::new(src).file_name().unwrap())
                    .to_string_lossy()
                    .to_string()
            } else {
                row["dst"].as_str().unwrap().into()
            },
            size: row["size"].as_u64().unwrap(),
            version: row["file_version"].as_str().unwrap().into(),
        });
        scopes.push(MoveScope {
            source: ScopeRoot::Unorganized,
            destination: if row["operation"] == "stage-duplicate" {
                ScopeRoot::Staging
            } else {
                ScopeRoot::Person(row["creator"].as_str().unwrap().into())
            },
        });
    }
    let ledger = archive_ledger::prepare(config, &ledger_rows(&rows, &moves), at, &request.id)?;
    for (database, table) in [
        (config.base.join("faces.db"), "videos"),
        (config.base.join("processing/processing.db"), "tasks"),
    ] {
        if !database.exists() {
            continue;
        }
        for row in db::rows(
            &db::open(&database)?,
            &format!("SELECT * FROM {table} ORDER BY vid"),
            [],
        )? {
            if moves.iter().any(|file| {
                (row["path"] == file.dst || row["path"] == metadata::win(config, &file.dst))
                    && !file.vids.iter().any(|id| row["vid"] == *id)
            }) {
                return Err(Error::conflict("目标路径已有其他本地视频或任务记录"));
            }
        }
    }
    let videos = db::rows(
        &db::open(&config.base.join("faces.db"))?,
        "SELECT * FROM videos ORDER BY vid",
        [],
    )?
    .into_iter()
    .filter(|r| {
        moves.iter().any(|f| {
            f.vids.iter().any(|id| r["vid"] == *id)
                || r["path"] == f.src
                || r["path"] == metadata::win(config, &f.src)
        })
    })
    .collect::<Vec<_>>();
    let jobs_path = config.base.join("processing/processing.db");
    let jobs = if jobs_path.exists() {
        db::rows(
            &db::open(&jobs_path)?,
            "SELECT * FROM tasks ORDER BY vid",
            [],
        )?
        .into_iter()
        .filter(|r| {
            moves.iter().any(|f| {
                f.vids.iter().any(|id| r["vid"] == *id)
                    || r["path"] == f.src
                    || r["path"] == metadata::win(config, &f.src)
            })
        })
        .collect::<Vec<_>>()
    } else {
        vec![]
    };
    let mut documents = vec![];
    for (name, fallback) in [
        ("integration-queue.json", json!([])),
        ("archive-history.json", json!([])),
        ("video-path-overrides.json", json!({})),
        ("staging-origins.json", json!({})),
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
        })
    }
    let data = json!({"rows":rows,"ledger":ledger,"videos":videos,"jobs":jobs,"documents":documents,"queue_count":current["n"],"at":at});
    let total_bytes = moves.iter().map(|f| f.size).sum();
    let mut directories = BTreeSet::new();
    for file in &moves {
        let mut parent = Path::new(&file.dst).parent();
        while let Some(path) = parent {
            match path.symlink_metadata() {
                Ok(meta) => {
                    if !meta.is_dir() || meta.file_type().is_symlink() {
                        return Err(
                            Error::conflict("目标父目录无效").at(path.display().to_string())
                        );
                    }
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    directories.insert(path.to_string_lossy().to_string());
                    parent = path.parent()
                }
                Err(error) => return Err(error.into()),
            }
        }
    }
    let mut directories = directories.into_iter().collect::<Vec<_>>();
    directories.sort_by_key(|p| Path::new(p).components().count());
    let plan = Plan {
        id: request.id.clone(),
        request,
        version: String::new(),
        action: "archive".into(),
        person: String::new(),
        target_person: String::new(),
        files: moves,
        total_bytes,
        directories,
        ledger_version: String::new(),
        metadata_version: String::new(),
        execution: Some(ExecutionIntent {
            scopes,
            failure_policy: FailurePolicy::ContinueUnchanged,
            stores: AffectedStores {
                faces: true,
                jobs: jobs_path.exists(),
                ledger: true,
            },
            data,
            result: json!({"ok":true,"moved":0,"staged":0,"ledger":0,"remaining":current["n"],"errors":[]}),
        }),
    };
    Ok(plan)
}

pub fn revalidate(config: &Config, plan: &Plan) -> Result<()> {
    let intent = plan
        .execution
        .as_ref()
        .ok_or_else(|| Error::conflict("归档计划缺少执行证据"))?;
    let actual = preview(config)?;
    for (index, row) in intent.data["rows"].as_array().unwrap().iter().enumerate() {
        let current = actual["rows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["vid"] == row["vid"])
            .ok_or_else(|| Error::conflict("人工归名已撤销"))?;
        if current != row {
            return Err(Error::conflict("相关归名或文件状态已变化"));
        }
        let file = &plan.files[index];
        let stat = review::safe_source(config, &file.src)?;
        if files::fingerprint(&file.src, &stat) != file.version {
            return Err(Error::conflict("源视频已变化"));
        }
    }
    for doc in serde_json::from_value::<Vec<metadata::Document>>(intent.data["documents"].clone())?
    {
        if files::read_optional(&config.base.join(&doc.name))? != doc.before {
            return Err(Error::conflict("相关归档记录已变化").at(doc.name));
        }
    }
    let ledger = archive_ledger::prepare(
        config,
        &ledger_rows(intent.data["rows"].as_array().unwrap(), &plan.files),
        intent.data["at"].as_str().unwrap(),
        &plan.id,
    )?;
    if ledger != intent.data["ledger"] {
        return Err(Error::conflict("Peach 相关记录已变化"));
    }
    Ok(())
}

pub fn snapshot(config: &Config, plan: &Plan, successful: &[usize]) -> Result<DomainSnapshot> {
    let data = &plan
        .execution
        .as_ref()
        .ok_or_else(|| Error::conflict("归档意图缺失"))?
        .data;
    let mut accepted = BTreeSet::new();
    for &i in successful {
        if i >= plan.files.len() || !accepted.insert(i) {
            return Err(Error::conflict("归档提交范围无效"));
        }
    }
    let mut selected_plan = plan.clone();
    selected_plan.files = successful.iter().map(|&i| plan.files[i].clone()).collect();
    let selected = |r: &Value| {
        successful.iter().find_map(|&i| {
            (plan.files[i].vids.iter().any(|id| r["vid"] == *id)
                || r["path"] == plan.files[i].src
                || r["path"] == metadata::win(config, &plan.files[i].src))
            .then_some(&plan.files[i])
        })
    };
    let videos = data["videos"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| selected(r).is_some())
        .cloned()
        .collect::<Vec<_>>();
    let mut after_videos = videos.clone();
    for row in &mut after_videos {
        let f = selected(row).unwrap();
        row["path"] = json!(f.dst);
        if row.get("name").is_some() {
            row["name"] = json!(Path::new(&f.dst).file_name().unwrap().to_string_lossy())
        }
        if row.get("collection").is_some() {
            row["collection"] = json!(Path::new(&f.dst).parent().unwrap().to_string_lossy())
        }
    }
    let jobs = data["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| selected(r).is_some())
        .cloned()
        .collect::<Vec<_>>();
    let mut after_jobs = jobs.clone();
    for row in &mut after_jobs {
        let f = selected(row).unwrap();
        row["path"] = json!(f.dst);
        row["name"] = json!(Path::new(&f.dst).file_name().unwrap().to_string_lossy());
        row["stage"] = json!("excluded");
        let old = review::task_data(row)?;
        row["data"] = json!(serde_json::to_string(&metadata::transform(
            config,
            &old,
            &selected_plan
        ))?);
    }
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
            "staging-origins.json",
        ]
        .contains(&doc.name.as_str())
        {
            value = metadata::transform(config, &value, &selected_plan);
        }
        if doc.name == "integration-queue.json" {
            value
                .as_array_mut()
                .unwrap()
                .retain(|r| selected(r).is_none())
        }
        if doc.name == "archive-history.json" {
            for &i in successful {
                let mut row = data["rows"][i].clone();
                row["manifest"] = json!(format!("rust:{}", plan.id));
                row["win_dst"] = json!(metadata::win(config, row["dst"].as_str().unwrap()));
                if row["operation"] == "stage-duplicate" {
                    row["staged_path"] = json!(plan.files[i].dst);
                }
                row["operation_id"] = json!(plan.id);
                value
                    .as_array_mut()
                    .unwrap()
                    .retain(|r| r["vid"] != row["vid"]);
                value.as_array_mut().unwrap().push(row)
            }
        }
        if doc.name == "staging-origins.json" {
            for &i in successful {
                if data["rows"][i]["operation"] == "stage-duplicate" {
                    let f = &plan.files[i];
                    value[&f.dst] = json!({"operation":plan.id,"action":"stage-duplicate","original_path":f.src,"person":"","vids":f.vids,"source":"native-receipt"});
                }
            }
        }
        if doc.name == "video-path-overrides.json" {
            for &i in successful {
                for id in &plan.files[i].vids {
                    value[id] = json!(plan.files[i].dst)
                }
            }
        }
        doc.after = if successful.is_empty() || value == original {
            doc.before.clone()
        } else {
            Some(serde_json::to_string_pretty(&value)?)
        };
    }
    let vids = successful
        .iter()
        .flat_map(|&i| plan.files[i].vids.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let paths = successful
        .iter()
        .flat_map(|&i| {
            [
                plan.files[i].src.clone(),
                plan.files[i].dst.clone(),
                metadata::win(config, &plan.files[i].src),
                metadata::win(config, &plan.files[i].dst),
            ]
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let faces_guard = db::Guard::LocalFiles {
        table: db::Table::Videos,
        vids: vids.clone(),
        paths: paths.clone(),
        before: videos.clone(),
        after: after_videos.clone(),
    };
    let jobs_guard = db::Guard::LocalFiles {
        table: db::Table::Tasks,
        vids,
        paths,
        before: jobs.clone(),
        after: after_jobs.clone(),
    };
    Ok(DomainSnapshot {
        metadata: metadata::Snapshot {
            documents,
            videos: vec![],
            jobs: vec![],
            faces_delta: if plan.execution.as_ref().unwrap().stores.faces {
                Some(db::Delta {
                    guards: vec![faces_guard],
                    tables: vec![archive_ledger::table(
                        db::Table::Videos,
                        vec!["vid".into()],
                        videos,
                        after_videos,
                    )],
                })
            } else {
                None
            },
            jobs_delta: if plan.execution.as_ref().unwrap().stores.jobs {
                Some(db::Delta {
                    guards: vec![jobs_guard],
                    tables: vec![archive_ledger::table(
                        db::Table::Tasks,
                        vec!["vid".into()],
                        jobs,
                        after_jobs,
                    )],
                })
            } else {
                None
            },
        },
        ledger: Some(archive_ledger::delta(&data["ledger"], successful)?),
    })
}

pub fn result(plan: &Plan, successful: &[usize], failures: &[Value]) -> Value {
    let mut result = plan.execution.as_ref().unwrap().result.clone();
    result["ok"] = json!(failures.is_empty());
    result["id"] = json!(plan.id);
    result["moved"] = json!(successful.len());
    result["ledger"] = json!(successful.len());
    result["staged"] = json!(
        successful
            .iter()
            .filter(
                |&&i| plan.execution.as_ref().unwrap().data["rows"][i]["operation"]
                    == "stage-duplicate"
            )
            .count()
    );
    result["remaining"] = json!(
        plan.execution.as_ref().unwrap().data["queue_count"]
            .as_u64()
            .unwrap_or(0)
            .saturating_sub(successful.len() as u64)
    );
    let failures=failures.iter().map(|failure|{
        let index=failure["file_index"].as_u64().unwrap_or(0) as usize;
        let row=&plan.execution.as_ref().unwrap().data["rows"][index];
        let message=failure["error"]["error"].as_str().or(failure["error"].as_str()).or(failure["message"].as_str()).unwrap_or("文件未完成");
        json!({"vid":row["vid"],"name":row["name"],"message":message,"path":failure["src"],"code":failure["error"]["code"],"step":failure["error"]["step"]})
    }).collect::<Vec<_>>();
    result["errors"] = json!(
        failures
            .iter()
            .map(|failure| format!(
                "{}：{}",
                failure["name"].as_str().unwrap_or(""),
                failure["message"].as_str().unwrap_or("")
            ))
            .collect::<Vec<_>>()
    );
    result["failures"] = json!(failures);
    result["manifest"] = json!(format!("rust:{}", plan.id));
    result["locations"] = json!(
        successful
            .iter()
            .map(|&i| json!({"vids":plan.files[i].vids,"path":plan.files[i].dst}))
            .collect::<Vec<_>>()
    );
    result
}

fn related(value: &Value, vid: &str, src: &str, win: &str, out: &mut Vec<Value>) {
    match value {
        Value::Array(rows) => {
            for row in rows {
                related(row, vid, src, win, out)
            }
        }
        Value::Object(map) => {
            if value["vid"] == vid
                || [
                    "path",
                    "src",
                    "dst",
                    "win_dst",
                    "win_src",
                    "original_dst",
                    "canonical_dst",
                    "collection",
                    "root",
                ]
                .iter()
                .any(|key| value[*key].as_str().is_some_and(|v| v == src || v == win))
            {
                out.push(value.clone())
            } else {
                for value in map.values() {
                    related(value, vid, src, win, out)
                }
            }
        }
        _ => {}
    }
}

fn ledger_rows(rows: &[Value], files: &[FileMove]) -> Vec<Value> {
    rows.iter()
        .zip(files)
        .map(|(row, file)| {
            let mut value = row.clone();
            if row["operation"] == "stage-duplicate" {
                value["canonical"] = row["dst"].clone();
            }
            value["dst"] = json!(file.dst);
            value
        })
        .collect()
}

pub fn verify_duplicate(config: &Config, plan: &Plan, index: usize, compare: bool) -> Result<()> {
    if plan.action != "archive" {
        return Ok(());
    }
    let row = &plan.execution.as_ref().unwrap().data["rows"][index];
    if row["operation"] != "stage-duplicate" {
        return Ok(());
    }
    let target = row["dst"].as_str().unwrap();
    let version = row["canonical_version"].as_str().unwrap();
    if compare {
        crate::fault::checkpoint(config, "duplicate_verification_started")?;
        files::identical_files(config, &plan.files[index], target, version)?;
        crate::fault::checkpoint(config, "duplicate_verified")?;
        crate::fault::checkpoint(config, &format!("duplicate_verified:{index}"))?;
    } else if files::fingerprint(target, &files::regular_stat(target)?) != version {
        return Err(Error::conflict("重复件目标文件已变化，请重新核对").at(target));
    }
    Ok(())
}
