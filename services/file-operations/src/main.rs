mod admission;
mod archive;
mod archive_api;
mod archive_ledger;
mod archive_resolution;
mod config;
mod control;
mod creator_delta;
mod db;
mod delta;
mod engine;
mod fault;
mod files;
mod ledger;
mod metadata;
mod model;
mod native;
mod permissions;
mod person_batch;
mod person_delete;
mod person_directory;
mod person_ledger;
mod person_library;
mod person_merge;
mod person_merge_ledger;
mod person_merge_metadata;
mod person_metadata;
mod person_move;
mod person_registration;
mod person_rename;
mod processing_result;
mod reference_relocation;
mod review;
mod review_sessions;
mod shared_metadata;
mod staging;
mod staging_delete;
mod store;
mod video_health;

use crate::{
    config::Config,
    engine::Engine,
    model::{Envelope, Error, Result},
};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Query, State},
    http::HeaderMap,
    routing::get,
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Arc, mpsc},
    thread,
};
use tokio::sync::{Semaphore, oneshot};

enum Command {
    Files(Box<Envelope>),
    Batch(Value),
    Metadata { command: String, body: Value },
    Archive { command: String, body: Value },
}
struct Work {
    command: Command,
    reply: oneshot::Sender<Result<Value>>,
}
#[derive(Clone)]
struct App {
    config: Config,
    worker: admission::Worker,
    // Local journal reads must remain available while media/NFS reads are stalled.
    status_reads: Arc<Semaphore>,
    media_reads: Arc<Semaphore>,
}
fn authorize(config: &Config, headers: &HeaderMap) -> Result<()> {
    if let Some(token) = &config.token
        && headers.get("authorization").and_then(|h| h.to_str().ok())
            != Some(format!("Bearer {token}").as_str())
    {
        return Err(Error::new(401, "unauthorized", "访问令牌无效"));
    }
    Ok(())
}
async fn post(
    State(app): State<App>,
    headers: HeaderMap,
    payload: std::result::Result<Json<Envelope>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<Value>> {
    authorize(&app.config, &headers)?;
    let Json(envelope) = payload.map_err(|e| {
        Error::new(
            400,
            "invalid_request",
            format!("操作请求无效：{}", e.body_text()),
        )
    })?;
    if envelope.request.action == "metadata-write"
        && envelope
            .request
            .payload
            .as_ref()
            .and_then(|v| v["command"].as_str())
            .is_some_and(|c| c.starts_with("health-"))
    {
        return Err(Error::new(
            400,
            "invalid_request",
            "体检发布只允许后台调度器执行",
        ));
    }
    let (reply, wait) = oneshot::channel();
    app.worker.try_send(Work {
        command: Command::Files(Box::new(envelope)),
        reply,
    })?;
    // The dedicated worker owns execution. Dropping this receiver never aborts admitted work.
    Ok(Json(wait.await.map_err(|_| {
        Error::new(503, "outcome_unknown", "执行器连接中断，请按操作编号查询")
    })??))
}
async fn status(
    State(app): State<App>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Json<Value>> {
    authorize(&app.config, &headers)?;
    let id = query
        .get("id")
        .cloned()
        .ok_or_else(|| Error::new(400, "invalid_request", "缺少操作编号"))?;
    model::validate_id(&id)?;
    let permit = app
        .status_reads
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| Error::new(503, "unavailable", "读取服务不可用"))?;
    let path = app.config.state_db();
    let value = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        store::status(&path, &id)
    })
    .await
    .map_err(|_| Error::new(503, "unavailable", "状态读取失败"))??;
    Ok(Json(value))
}
async fn staging_list(
    State(app): State<App>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Json<Value>> {
    authorize(&app.config, &headers)?;
    let offset = query
        .get("offset")
        .map(|s| s.parse::<usize>())
        .transpose()
        .map_err(|_| Error::new(400, "invalid_request", "分页位置无效"))?
        .unwrap_or(0);
    let permit = if query.contains_key("id") {
        app.status_reads
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| Error::new(503, "unavailable", "读取服务不可用"))?
    } else {
        app.media_reads
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::new(503, "reader_busy", "媒体目录读取繁忙，请稍后重试"))?
    };
    Ok(Json(
        tokio::task::spawn_blocking(move || {
            // A disconnected HTTP waiter cannot cancel a blocking syscall. Keep the
            // permit here until the real read returns; never admit replacement work early.
            let _permit = permit;
            if let Some(id) = query.get("id") {
                return staging_delete::historical_status(&app.config, id);
            }
            media_listing_checkpoint(&app.config, offset)?;
            staging::list(
                &app.config,
                offset,
                query.get("q").map(String::as_str).unwrap_or(""),
            )
        })
        .await
        .map_err(|_| Error::new(503, "unavailable", "暂存列表读取失败"))??,
    ))
}
fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--health-observe") {
        let result = video_health::helper(std::path::Path::new(
            args.get(2).ok_or("missing observation input")?,
        ));
        if let Err(e) = result {
            eprintln!("{}", e.error);
            std::process::exit(1);
        }
        return Ok(());
    }
    serve()
}

/// Slow-media test hook, only reachable with Config's full marked-fixture guard.
/// The per-request marker proves admission happened inside the bounded blocking lane.
fn media_listing_checkpoint(config: &Config, offset: usize) -> Result<()> {
    let Some(root) = &config.fixture else {
        return Ok(());
    };
    let Some(raw) = control::read(&root.join("rust-fault.json"), control::MARKER_LIMIT)? else {
        return Ok(());
    };
    let fault: Value = serde_json::from_str(&raw)?;
    if fault["phase"] != "media_listing" {
        return Ok(());
    }
    if fault["mode"] == "pause" {
        files::atomic_write(
            &root.join(format!("media-read-{offset}.started")),
            b"admitted",
        )?;
    }
    fault::checkpoint(config, "media_listing")
}
#[tokio::main]
async fn serve() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let config = Config::from_env()?;
    let mut engine = Engine::new(config.clone())?;
    if config.parity_root.is_none() {
        match video_health::needs_recovery(&config) {
            Ok(true) => {
                if let Err(error) =
                    shared_metadata::execute(&mut engine, "health-interrupted", json!({}))
                {
                    eprintln!("health recovery: {}", error.error);
                }
            }
            Err(error) => eprintln!("health state unavailable: {}", error.error),
            Ok(false) => (),
        }
    }
    let (sender, queue) = mpsc::sync_channel::<Work>(32);
    let worker = admission::Worker::new(config.clone(), sender)?;
    let completion = worker.completion();
    let executor = thread::Builder::new()
        .name("file-operation-executor".into())
        .spawn(move || {
            for Work { command, reply } in queue {
                let result = match command {
                    Command::Batch(body) => person_batch::execute(&mut engine, body),
                    Command::Archive { command, body } => {
                        archive_api::execute(&mut engine, &command, body)
                    }
                    Command::Metadata { command, body } => {
                        shared_metadata::execute(&mut engine, &command, body)
                    }
                    Command::Files(envelope) => match envelope.mode.as_str() {
                        "preview" => engine.preview(envelope.request),
                        "reconcile" => engine.reconcile_request(envelope.request, envelope.version),
                        "apply"
                            if staging_delete::action(&envelope.request.action)
                                && envelope.confirmation.as_deref()
                                    != Some(staging_delete::confirmation(
                                        &envelope.request.action,
                                    )) =>
                        {
                            Err(Error::new(
                                400,
                                "confirmation_required",
                                "请输入完整确认文字：永久删除或清空回收站",
                            ))
                        }
                        "apply" => match envelope.version {
                            Some(version) => engine.apply(envelope.request, version),
                            None => Err(Error::new(400, "invalid_request", "缺少预览版本")),
                        },
                        _ => Err(Error::new(400, "invalid_request", "操作模式无效")),
                    },
                };
                completion.complete();
                let _ = reply.send(result);
            }
        })?;
    let health_stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let health = if config.parity_root.is_none()
        && std::env::var("VIDEO_HEALTH_ENABLED").as_deref() == Ok("1")
    {
        Some(video_health::spawn(
            config.clone(),
            worker.clone(),
            health_stop.clone(),
        ))
    } else {
        None
    };
    let app = App {
        config: config.clone(),
        worker,
        status_reads: Arc::new(Semaphore::new(4)),
        media_reads: Arc::new(Semaphore::new(4)),
    };
    let router = Router::new()
        .route(
            "/health",
            get(|| async { Json(json!({"ok":true,"service":"face-file-operations"})) }),
        )
        .route("/v1/readiness", get(readiness))
        .route("/v1/admission", axum::routing::post(admission_control))
        .route("/v1/person-files", get(status).post(post))
        .route("/v1/person-batch", get(batch_status).post(batch_post))
        .route("/v1/archive", get(archive_status).post(archive_post))
        .route("/v1/staging", get(staging_list))
        .route("/v1/person-library", get(library_list))
        .route(
            "/v1/person-library/observations",
            axum::routing::post(library_observations),
        )
        .route("/v1/video-health", get(health_summary).post(health_report))
        .route("/v1/metadata", axum::routing::post(metadata_post))
        .route(
            "/v1/reference-origin",
            axum::routing::post(reference_origin),
        )
        .layer(DefaultBodyLimit::max(8 * 1024 * 1024))
        .with_state(app);
    let listener = tokio::net::TcpListener::bind(&config.bind).await?;
    eprintln!("face-file-operations listening on {}", config.bind);
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    health_stop.store(true, std::sync::atomic::Ordering::Relaxed);
    if let Some(health) = health {
        health.join().map_err(|_| "health worker panicked")?;
    }
    executor.join().map_err(|_| "execution worker panicked")?;
    Ok(())
}

async fn shutdown_signal() {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("install SIGTERM handler");
    tokio::select! { _=tokio::signal::ctrl_c()=>{}, _=terminate.recv()=>{} }
}

async fn archive_post(
    State(app): State<App>,
    headers: HeaderMap,
    payload: std::result::Result<Json<Value>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<Value>> {
    authorize(&app.config, &headers)?;
    let Json(payload) = payload.map_err(|e| {
        Error::new(
            400,
            "invalid_request",
            format!("操作请求无效：{}", e.body_text()),
        )
    })?;
    let command = payload["command"]
        .as_str()
        .ok_or_else(|| Error::new(400, "invalid_request", "缺少命令"))?
        .to_owned();
    let (reply, wait) = oneshot::channel();
    app.worker.try_send(Work {
        command: Command::Archive {
            command,
            body: payload["body"].clone(),
        },
        reply,
    })?;
    Ok(Json(wait.await.map_err(|_| {
        Error::new(503, "outcome_unknown", "执行连接中断")
    })??))
}
async fn archive_status(
    State(app): State<App>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Json<Value>> {
    authorize(&app.config, &headers)?;
    let permit = app
        .status_reads
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| Error::new(503, "unavailable", "读取服务不可用"))?;
    let config = app.config;
    let id = query.get("id").cloned();
    Ok(Json(
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            archive_api::progress(&config, id.as_deref())
        })
        .await
        .map_err(|_| Error::new(503, "unavailable", "状态读取失败"))??,
    ))
}

async fn metadata_post(
    State(app): State<App>,
    headers: HeaderMap,
    payload: std::result::Result<Json<Value>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<Value>> {
    authorize(&app.config, &headers)?;
    let Json(payload) = payload.map_err(|e| {
        Error::new(
            400,
            "invalid_request",
            format!("操作请求无效：{}", e.body_text()),
        )
    })?;
    let command = payload["command"]
        .as_str()
        .ok_or_else(|| Error::new(400, "invalid_request", "缺少命令"))?
        .to_owned();
    if command.starts_with("health-") {
        return Err(Error::new(400, "invalid_request", "请使用视频体检报告入口"));
    }
    let (reply, wait) = oneshot::channel();
    app.worker.try_send(Work {
        command: Command::Metadata {
            command,
            body: payload["body"].clone(),
        },
        reply,
    })?;
    Ok(Json(wait.await.map_err(|_| {
        Error::new(503, "outcome_unknown", "执行连接中断")
    })??))
}

async fn batch_post(
    State(app): State<App>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    authorize(&app.config, &headers)?;
    let (reply, wait) = oneshot::channel();
    app.worker.try_send(Work {
        command: Command::Batch(body),
        reply,
    })?;
    Ok(Json(wait.await.map_err(|_| {
        Error::new(503, "outcome_unknown", "执行器连接中断，请查询原批次")
    })??))
}
async fn batch_status(
    State(app): State<App>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Json<Value>> {
    authorize(&app.config, &headers)?;
    let id = query
        .get("id")
        .cloned()
        .ok_or_else(|| Error::new(400, "invalid_request", "缺少批次编号"))?;
    let permit = app
        .status_reads
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| Error::new(503, "unavailable", "读取服务不可用"))?;
    Ok(Json(
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            person_batch::progress(&app.config, &id)
        })
        .await
        .map_err(|_| Error::new(503, "unavailable", "批次状态读取失败"))??,
    ))
}

async fn health_summary(
    State(app): State<App>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Json<Value>> {
    authorize(&app.config, &headers)?;
    let permit = app
        .media_reads
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| Error::new(503, "unavailable", "读取服务不可用"))?;
    Ok(Json(
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            if let Some(vid) = query.get("vid") {
                video_health::playback_guard(
                    &app.config,
                    vid,
                    query.get("path").map(String::as_str),
                )
            } else {
                video_health::summary(&app.config)
            }
        })
        .await
        .map_err(|_| Error::new(503, "unavailable", "体检记录读取失败"))??,
    ))
}
async fn health_report(
    State(app): State<App>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    authorize(&app.config, &headers)?;
    let (reply, wait) = oneshot::channel();
    app.worker.try_send(Work {
        command: Command::Metadata {
            command: "health-report".into(),
            body,
        },
        reply,
    })?;
    Ok(Json(wait.await.map_err(|_| {
        Error::new(503, "outcome_unknown", "报告回执未知，请查询原操作编号")
    })??))
}

async fn readiness(State(app): State<App>, headers: HeaderMap) -> Result<Json<Value>> {
    authorize(&app.config, &headers)?;
    Ok(Json(
        tokio::task::spawn_blocking(move || app.worker.readiness())
            .await
            .map_err(|_| Error::new(503, "unavailable", "准入状态读取失败"))??,
    ))
}
async fn admission_control(
    State(app): State<App>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    authorize(&app.config, &headers)?;
    let accepting = body["accepting"]
        .as_bool()
        .ok_or_else(|| Error::new(400, "invalid_request", "缺少 accepting 布尔值"))?;
    Ok(Json(
        tokio::task::spawn_blocking(move || app.worker.set_accepting(accepting))
            .await
            .map_err(|_| Error::new(503, "unavailable", "准入控制失败"))??,
    ))
}

async fn library_list(
    State(app): State<App>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Json<Value>> {
    authorize(&app.config, &headers)?;
    if let Some(name) = query.get("name") {
        files::validate_name(name)?;
    }
    let permit = app
        .media_reads
        .clone()
        .try_acquire_owned()
        .map_err(|_| Error::new(503, "reader_busy", "媒体目录读取繁忙，请稍后重试"))?;
    Ok(Json(
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            media_listing_checkpoint(
                &app.config,
                query
                    .get("offset")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0),
            )?;
            person_library::list(
                &app.config,
                query.get("name").map(String::as_str),
                query.get("stats").is_some_and(|s| s == "1"),
            )
        })
        .await
        .map_err(|_| Error::new(503, "unavailable", "人物目录读取失败"))??,
    ))
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct LibraryObservationQuery {
    vids: Vec<String>,
}
async fn library_observations(
    State(app): State<App>,
    headers: HeaderMap,
    payload: std::result::Result<
        Json<LibraryObservationQuery>,
        axum::extract::rejection::JsonRejection,
    >,
) -> Result<Json<Value>> {
    authorize(&app.config, &headers)?;
    let Json(query) = payload.map_err(|_| Error::new(400, "invalid_request", "视频清单无效"))?;
    if query.vids.len() > 512
        || query
            .vids
            .iter()
            .any(|id| id.is_empty() || id.len() > 8192 || id.chars().any(char::is_control))
    {
        return Err(Error::new(
            400,
            "invalid_request",
            "单次最多读取 512 个有效视频编号",
        ));
    }
    let permit = app
        .media_reads
        .clone()
        .try_acquire_owned()
        .map_err(|_| Error::new(503, "reader_busy", "媒体目录读取繁忙，请稍后重试"))?;
    Ok(Json(
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            crate::video_health::library_observations(&app.config, &query.vids)
        })
        .await
        .map_err(|_| Error::new(503, "unavailable", "视频依据读取失败"))??,
    ))
}

// Read-only diagnostics remain available under closed write admission. Bounded
// media permits stay held by the blocking reader even if the caller disconnects.
async fn reference_origin(
    State(app): State<App>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    authorize(&app.config, &headers)?;
    if !body.is_object() || serde_json::to_vec(&body)?.len() > 16384 {
        return Err(Error::new(400, "invalid_request", "标量来源请求格式无效"));
    }
    let permit = app
        .media_reads
        .clone()
        .try_acquire_owned()
        .map_err(|_| Error::new(503, "reader_busy", "媒体读取繁忙"))?;
    Ok(Json(
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            reference_relocation::diagnostic(&app.config, &body)
        })
        .await
        .map_err(|_| Error::new(503, "unavailable", "来源诊断读取失败"))??,
    ))
}
