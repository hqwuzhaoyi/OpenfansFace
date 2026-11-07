mod config;
mod db;
mod engine;
mod fault;
mod files;
mod ledger;
mod metadata;
mod model;
mod store;

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

struct Work {
    envelope: Envelope,
    reply: oneshot::Sender<Result<Value>>,
}
#[derive(Clone)]
struct App {
    config: Config,
    worker: mpsc::SyncSender<Work>,
    reads: Arc<Semaphore>,
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
    let (reply, wait) = oneshot::channel();
    app.worker.try_send(Work { envelope, reply }).map_err(|_| {
        Error::new(
            503,
            "writer_busy",
            "执行队列已满或执行器不可用，请稍后查询原编号",
        )
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
        .reads
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
#[tokio::main]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let config = Config::from_env()?;
    let mut engine = Engine::new(config.clone())?;
    let (worker, queue) = mpsc::sync_channel::<Work>(32);
    let executor = thread::Builder::new()
        .name("file-operation-executor".into())
        .spawn(move || {
            for Work { envelope, reply } in queue {
                let result = match envelope.mode.as_str() {
                    "preview" => engine.preview(envelope.request),
                    "reconcile" => engine.reconcile_request(envelope.request, envelope.version),
                    "apply" => match envelope.version {
                        Some(version) => engine.apply(envelope.request, version),
                        None => Err(Error::new(400, "invalid_request", "缺少预览版本")),
                    },
                    _ => Err(Error::new(400, "invalid_request", "操作模式无效")),
                };
                let _ = reply.send(result);
            }
        })?;
    let app = App {
        config: config.clone(),
        worker,
        reads: Arc::new(Semaphore::new(4)),
    };
    let router = Router::new()
        .route(
            "/health",
            get(|| async { Json(json!({"ok":true,"service":"face-file-operations"})) }),
        )
        .route("/v1/person-files", get(status).post(post))
        .layer(DefaultBodyLimit::max(64 * 1024))
        .with_state(app);
    let listener = tokio::net::TcpListener::bind(&config.bind).await?;
    eprintln!("face-file-operations listening on {}", config.bind);
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    executor.join().map_err(|_| "execution worker panicked")?;
    Ok(())
}

async fn shutdown_signal() {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("install SIGTERM handler");
    tokio::select! { _=tokio::signal::ctrl_c()=>{}, _=terminate.recv()=>{} }
}
