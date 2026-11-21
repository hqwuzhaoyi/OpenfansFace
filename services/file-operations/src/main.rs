mod admission;
pub mod agent;
mod agent_deployment;
mod agent_dispatcher;
mod agent_registry;
mod archive;
mod archive_api;
mod archive_cancel;
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
mod processing_scheduler;
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
    // Read-only compute and review endpoints run off the single executor lane.
    compute_reads: Arc<Semaphore>,
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
    if envelope.mode == "preview" && app.config.fixture.is_none() && app.worker.has_pending() {
        let permit = app
            .status_reads
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| Error::new(503, "unavailable", "读取服务不可用"))?;
        let config = app.config;
        let request = envelope.request;
        return Ok(Json(
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                let mut engine = Engine::open_preview(config)?;
                engine.preview(request)
            })
            .await
            .map_err(|_| Error::new(503, "unavailable", "预览读取失败"))??,
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
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    unsafe {
        // Bound glibc per-thread arena proliferation under tokio spawn_blocking pools.
        libc::mallopt(libc::M_ARENA_MAX, 2);
    }
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
    if args.get(1).map(String::as_str) == Some("--evaluate-rules") {
        let body: Value = serde_json::from_str(args.get(2).ok_or("missing rule input")?)?;
        match processing_result::evaluate_rules(&body) {
            Ok(out) => {
                println!("{}", serde_json::to_string(&out)?);
                return Ok(());
            }
            Err(e) => {
                eprintln!("{}", e.error);
                std::process::exit(1);
            }
        }
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
        compute_reads: Arc::new(Semaphore::new(4)),
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
        .route("/v1/agent/register", axum::routing::post(agent_register))
        .route("/v1/agent/heartbeat", axum::routing::post(agent_heartbeat))
        .route("/v1/agent/node-control", axum::routing::post(agent_node_control))
        .route("/v1/agent/nodes/control", axum::routing::post(agent_node_control))
        .route("/v1/agent/nodes", axum::routing::get(agent_nodes_list))
        .route(
            "/v1/agent/enrollment-token",
            axum::routing::post(agent_enrollment_token),
        )
        .route("/v1/agent/packages", axum::routing::post(agent_packages_register))
        .route("/v1/agent/deploy", axum::routing::post(agent_deploy))
        .route("/v1/agent/deployments/rollback", axum::routing::post(agent_deploy))
        .route("/v1/agent/deploy-receipt", axum::routing::post(agent_deploy_receipt))
        .route("/v1/agent/deployments", axum::routing::get(agent_deployments_list))
        .route(
            "/v1/agent/claim",
            axum::routing::post(agent_claim_task)
                .layer(DefaultBodyLimit::max(2 * 1024 * 1024)),
        )
        .route(
            "/v1/agent/task-result",
            axum::routing::post(agent_commit_task_result)
                .layer(DefaultBodyLimit::max(2 * 1024 * 1024)),
        )
        .route("/v1/agent/tasks", axum::routing::get(agent_tasks_list))
        .route("/v1/agent/reap-leases", axum::routing::post(agent_reap_leases))
        .route("/v1/agent/reap", axum::routing::post(agent_reap_leases))
        .route("/v1/agent/coordinator-generation", axum::routing::get(agent_coordinator_generation))
        .route("/v1/agent/liveness-check", axum::routing::post(agent_liveness_check))
        .route("/v1/agent/tasks/attempts", axum::routing::get(agent_task_attempts))
        .route("/v1/agent/task-attempts", axum::routing::get(agent_task_attempts))
        .layer(DefaultBodyLimit::max(8 * 1024 * 1024))
        .with_state(app);
    let listener = tokio::net::TcpListener::bind(&config.bind).await?;
    eprintln!("face-file-operations listening on {}", config.bind);

    let ticker_config = config.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(5));
        loop {
            interval.tick().await;
            let cfg = ticker_config.clone();
            let _ = tokio::task::spawn_blocking(move || {
                let _ = agent_dispatcher::maintenance_sweep(&cfg);
            })
            .await;
        }
    });

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
    if command == "archive-preview" && app.config.fixture.is_none() {
        let permit = app
            .status_reads
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| Error::new(503, "unavailable", "读取服务不可用"))?;
        let config = app.config;
        return Ok(Json(
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                archive::preview(&config)
            })
            .await
            .map_err(|_| Error::new(503, "unavailable", "归档预览读取失败"))??,
        ));
    }
    if matches!(command.as_str(), "review-page" | "review-status")
        && app.config.fixture.is_none()
    {
        let permit = app
            .compute_reads
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| Error::new(503, "unavailable", "读取服务不可用"))?;
        let config = app.config;
        let body = payload["body"].clone();
        return Ok(Json(
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                let mut engine = Engine::open_preview(config)?;
                review_sessions::execute(&mut engine, &command, body)
            })
            .await
            .map_err(|_| Error::new(503, "unavailable", "核对读取失败"))??,
        ));
    }
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
    if app.config.fixture.is_none()
        && matches!(
            command.as_str(),
            "processing-settings"
                | "processing-inventory"
                | "processing-overview"
                | "processing-dashboard"
                | "processing-home-tasks"
                | "processing-evaluate-rules"
                | "processing-artifact-check"
        )
    {
        let permit = app
            .status_reads
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| Error::new(503, "unavailable", "读取服务不可用"))?;
        let config = app.config;
        let body = payload["body"].clone();
        return Ok(Json(
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                let mut engine = Engine::open_preview(config)?;
                shared_metadata::execute(&mut engine, &command, body)
            })
            .await
            .map_err(|_| Error::new(503, "unavailable", "状态读取失败"))??,
        ));
    }
    if app.config.fixture.is_none()
        && matches!(
            command.as_str(),
            "state"
                | "check-state"
                | "analysis-state"
                | "reference-relocation"
                | "processing-cache-check"
                | "processing-reference-state"
        )
    {
        let permit = app
            .compute_reads
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| Error::new(503, "unavailable", "计算服务不可用"))?;
        let config = app.config;
        let body = payload["body"].clone();
        return Ok(Json(
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                let mut engine = Engine::open_preview(config)?;
                shared_metadata::execute(&mut engine, &command, body)
            })
            .await
            .map_err(|_| Error::new(503, "unavailable", "分析状态读取失败"))??,
        ));
    }
    if app.config.fixture.is_none() && command == "processing-discover" {
        let permit = app
            .media_reads
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| Error::new(503, "unavailable", "发现服务不可用"))?;
        let config = app.config;
        let body = payload["body"].clone();
        return Ok(Json(
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                processing_scheduler::discover(&config, &body)
            })
            .await
            .map_err(|_| Error::new(503, "unavailable", "媒体发现执行失败"))??,
        ));
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
    if body["mode"] == "preview" && app.config.fixture.is_none() && app.worker.has_pending() {
        let permit = app
            .status_reads
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| Error::new(503, "unavailable", "读取服务不可用"))?;
        let config = app.config;
        return Ok(Json(
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                let mut engine = Engine::open_preview(config)?;
                person_batch::execute(&mut engine, body)
            })
            .await
            .map_err(|_| Error::new(503, "unavailable", "批次预览读取失败"))??,
        ));
    }
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

async fn agent_register(
    State(app): State<App>,
    Json(payload): Json<agent_registry::RegisterRequest>,
) -> Result<Json<agent_registry::RegisterResponse>> {
    let config = app.config;
    let resp = tokio::task::spawn_blocking(move || {
        agent_registry::register_node(&config, payload)
    })
    .await
    .map_err(|_| Error::new(503, "internal_error", "注册处理异常"))??;
    Ok(Json(resp))
}

async fn agent_heartbeat(
    State(app): State<App>,
    headers: HeaderMap,
    Json(payload): Json<agent_registry::HeartbeatRequest>,
) -> Result<Json<agent_registry::HeartbeatResponse>> {
    let auth = headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .ok_or_else(|| Error::new(401, "unauthorized", "缺少 Bearer 令牌"))?
        .to_string();

    let config = app.config;
    let resp = tokio::task::spawn_blocking(move || {
        agent_registry::handle_heartbeat(&config, &auth, payload)
    })
    .await
    .map_err(|_| Error::new(503, "internal_error", "心跳处理异常"))??;
    Ok(Json(resp))
}

async fn agent_node_control(
    State(app): State<App>,
    headers: HeaderMap,
    Json(payload): Json<agent_registry::NodeControlRequest>,
) -> Result<Json<agent_registry::NodeControlResponse>> {
    authorize(&app.config, &headers)?;
    let config = app.config;
    let resp = tokio::task::spawn_blocking(move || {
        agent_registry::control_node(&config, payload)
    })
    .await
    .map_err(|_| Error::new(503, "internal_error", "控制处理异常"))??;
    Ok(Json(resp))
}

async fn agent_nodes_list(
    State(app): State<App>,
    headers: HeaderMap,
) -> Result<Json<Value>> {
    authorize(&app.config, &headers)?;
    let config = app.config;
    let resp = tokio::task::spawn_blocking(move || {
        agent_registry::list_nodes(&config)
    })
    .await
    .map_err(|_| Error::new(503, "internal_error", "节点列表读取异常"))??;
    Ok(Json(resp))
}

async fn agent_enrollment_token(
    State(app): State<App>,
    headers: HeaderMap,
    Json(payload): Json<agent_registry::EnrollmentTokenRequest>,
) -> Result<Json<agent_registry::EnrollmentTokenResponse>> {
    authorize(&app.config, &headers)?;
    let config = app.config;
    let resp = tokio::task::spawn_blocking(move || {
        agent_registry::create_enrollment_token(&config, payload)
    })
    .await
    .map_err(|_| Error::new(503, "internal_error", "令牌创建异常"))??;
    Ok(Json(resp))
}

async fn agent_packages_register(
    State(app): State<App>,
    headers: HeaderMap,
    Json(payload): Json<agent_deployment::RegisterPackageRequest>,
) -> Result<Json<agent_deployment::RegisterPackageResponse>> {
    authorize(&app.config, &headers)?;
    let config = app.config;
    let resp = tokio::task::spawn_blocking(move || {
        agent_deployment::register_package(&config, payload)
    })
    .await
    .map_err(|_| Error::new(503, "internal_error", "运行包注册异常"))??;
    Ok(Json(resp))
}

#[derive(serde::Deserialize)]
struct DeployRequestPayload {
    #[serde(default)]
    action: Option<String>,
    node_id: String,
    #[serde(default)]
    package_id: Option<String>,
    #[serde(default)]
    target_version: Option<String>,
}

async fn agent_deploy(
    State(app): State<App>,
    headers: HeaderMap,
    Json(payload): Json<DeployRequestPayload>,
) -> Result<Json<agent_deployment::DeployPackageResponse>> {
    authorize(&app.config, &headers)?;
    let config = app.config;
    let resp = tokio::task::spawn_blocking(move || {
        if payload.action.as_deref() == Some("rollback") {
            agent_deployment::rollback_deployment(
                &config,
                agent_deployment::RollbackRequest {
                    node_id: payload.node_id,
                    target_version: payload.target_version,
                },
            )
        } else {
            let pkg_id = payload.package_id.unwrap_or_default();
            agent_deployment::deploy_package(
                &config,
                agent_deployment::DeployPackageRequest {
                    node_id: payload.node_id,
                    package_id: pkg_id,
                },
            )
        }
    })
    .await
    .map_err(|_| Error::new(503, "internal_error", "部署调度异常"))??;
    Ok(Json(resp))
}

async fn agent_deploy_receipt(
    State(app): State<App>,
    Json(payload): Json<agent_deployment::DeployReceiptRequest>,
) -> Result<Json<agent_deployment::DeployReceiptResponse>> {
    let config = app.config;
    let resp = tokio::task::spawn_blocking(move || {
        agent_deployment::submit_deploy_receipt(&config, payload)
    })
    .await
    .map_err(|_| Error::new(503, "internal_error", "部署回执处理异常"))??;
    Ok(Json(resp))
}

async fn agent_deployments_list(
    State(app): State<App>,
    headers: HeaderMap,
) -> Result<Json<Value>> {
    authorize(&app.config, &headers)?;
    let config = app.config;
    let resp = tokio::task::spawn_blocking(move || {
        agent_deployment::list_deployments(&config)
    })
    .await
    .map_err(|_| Error::new(503, "internal_error", "部署记录读取异常"))??;
    Ok(Json(resp))
}

async fn agent_claim_task(
    State(app): State<App>,
    headers: HeaderMap,
    payload: std::result::Result<Json<agent_dispatcher::ClaimTaskRequest>, axum::extract::rejection::JsonRejection>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let auth = headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("")
        .to_string();
    let Json(payload) = match payload {
        Ok(p) => p,
        Err(e) => {
            let status = e.status();
            let code = if status == axum::http::StatusCode::PAYLOAD_TOO_LARGE {
                "payload_too_large"
            } else {
                "invalid_request"
            };
            return (
                status,
                Json(json!({"error": code, "message": format!("请求解析失败: {}", e)})),
            )
                .into_response();
        }
    };
    let config = app.config;
    let resp = tokio::task::spawn_blocking(move || {
        agent_dispatcher::claim_task(&config, &auth, payload)
    })
    .await;

    match resp {
        Ok(Ok(Some(task))) => (axum::http::StatusCode::OK, Json(json!(task))).into_response(),
        Ok(Ok(None)) => (axum::http::StatusCode::NO_CONTENT, Json(json!({}))).into_response(),
        Ok(Err(err)) => (
            axum::http::StatusCode::from_u16(err.status)
                .unwrap_or(axum::http::StatusCode::INTERNAL_SERVER_ERROR),
            Json(json!({"error": err.code, "message": err.error})),
        )
            .into_response(),
        Err(_) => (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error": "internal_error", "message": "任务认领处理异常"})),
        )
            .into_response(),
    }
}

async fn agent_commit_task_result(
    State(app): State<App>,
    headers: HeaderMap,
    payload: std::result::Result<Json<agent_dispatcher::CommitTaskResultRequest>, axum::extract::rejection::JsonRejection>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let auth = headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("")
        .to_string();
    let Json(payload) = match payload {
        Ok(p) => p,
        Err(e) => {
            let status = e.status();
            let code = if status == axum::http::StatusCode::PAYLOAD_TOO_LARGE {
                "payload_too_large"
            } else {
                "missing_fields"
            };
            return (
                status,
                Json(json!({"error": code, "message": format!("回执解析失败: {}", e)})),
            )
                .into_response();
        }
    };
    let config = app.config;
    let resp = tokio::task::spawn_blocking(move || {
        agent_dispatcher::commit_task_result(&config, &auth, payload)
    })
    .await;

    match resp {
        Ok(Ok(res)) => (axum::http::StatusCode::OK, Json(json!(res))).into_response(),
        Ok(Err(err)) => (
            axum::http::StatusCode::from_u16(err.status)
                .unwrap_or(axum::http::StatusCode::INTERNAL_SERVER_ERROR),
            Json(json!({"error": err.code, "message": err.error})),
        )
            .into_response(),
        Err(_) => (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error": "internal_error", "message": "回执提交处理异常"})),
        )
            .into_response(),
    }
}

async fn agent_tasks_list(
    State(app): State<App>,
    headers: HeaderMap,
    Query(query): Query<agent_dispatcher::ListTasksQuery>,
) -> Result<Json<Value>> {
    authorize(&app.config, &headers)?;
    let config = app.config;
    let resp = tokio::task::spawn_blocking(move || {
        agent_dispatcher::list_agent_tasks(&config, query)
    })
    .await
    .map_err(|_| Error::new(503, "internal_error", "任务列表读取异常"))??;
    Ok(Json(resp))
}

async fn agent_reap_leases(
    State(app): State<App>,
    headers: HeaderMap,
) -> Result<Json<Value>> {
    authorize(&app.config, &headers)?;
    let config = app.config;
    let res = tokio::task::spawn_blocking(move || {
        agent_dispatcher::reap_leases(&config)
    })
    .await
    .map_err(|_| Error::new(503, "internal_error", "租约回收执行失败"))??;

    Ok(Json(json!({
        "ok": true,
        "count": res.count,
        "reaped_vids": res.reaped_vids,
        "coordinator_generation": res.coordinator_generation,
    })))
}

async fn agent_coordinator_generation(
    State(app): State<App>,
    headers: HeaderMap,
) -> Result<Json<Value>> {
    authorize(&app.config, &headers)?;
    let config = app.config;
    let generation = tokio::task::spawn_blocking(move || {
        agent_dispatcher::get_coordinator_generation(&config)
    })
    .await
    .map_err(|_| Error::new(503, "internal_error", "获取协调器代际失败"))??;

    Ok(Json(json!({
        "ok": true,
        "coordinator_generation": generation,
    })))
}

async fn agent_liveness_check(
    State(app): State<App>,
    headers: HeaderMap,
) -> Result<Json<Value>> {
    authorize(&app.config, &headers)?;
    let config = app.config;
    let offline = tokio::task::spawn_blocking(move || {
        agent_registry::liveness_check(&config)
    })
    .await
    .map_err(|_| Error::new(503, "internal_error", "节点存活检测失败"))??;

    Ok(Json(json!({
        "ok": true,
        "offline_nodes": offline,
    })))
}

#[derive(Debug, serde::Deserialize)]
struct TaskAttemptsQuery {
    vid: Option<String>,
}

async fn agent_task_attempts(
    State(app): State<App>,
    headers: HeaderMap,
    Query(query): Query<TaskAttemptsQuery>,
) -> Result<Json<Value>> {
    authorize(&app.config, &headers)?;
    let vid = query.vid.unwrap_or_default();
    if vid.is_empty() {
        return Err(Error::new(400, "missing_fields", "缺少 vid 参数"));
    }
    let config = app.config;
    let res = tokio::task::spawn_blocking(move || {
        agent_dispatcher::get_task_attempts(&config, &vid)
    })
    .await
    .map_err(|_| Error::new(503, "internal_error", "任务尝试记录获取失败"))??;

    Ok(Json(res))
}
