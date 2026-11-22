//! Direct watermark auto-archive execution (Option B) with strict Peach ledger safety,
//! keep-both collision naming, safe fallback-to-queue on pre-flight blockers, and a
//! debounced bounded background publisher woken by post-commit processing results.
use crate::{
    archive_cancel, archive_ledger, auto_watermark,
    config::Config,
    db, files, metadata,
    model::{
        AffectedStores, Error, ExecutionIntent, FailurePolicy, FileMove, MoveScope, Plan, Request,
        Result, ScopeRoot,
    },
    native::DomainSnapshot,
    permissions, processing_scheduler, review,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

const DEFAULT_BG_MAX_BATCH: usize = 12;
const MAX_BG_BATCH_LIMIT: usize = 50;
const MAX_EXPLICIT_BATCH_LIMIT: usize = 500;
const DEFAULT_DEBOUNCE_MS: u64 = 3_000;
const DEFAULT_SWEEP_SECS: u64 = 60;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RequestedItem {
    vid: String,
    person: String,
    path: String,
    size: u64,
    mtime_ns: String,
}

#[derive(Default)]
struct WakeState {
    vids: BTreeSet<String>,
    notified: bool,
    stopped: bool,
    locks: Option<Arc<crate::locks::ActiveLockRegistry>>,
}

/// Non-blocking wakeup signal connecting post-commit `processing-result` completions
/// to the debounced background auto-archive publisher.
pub struct AutoArchiveWake {
    state: Mutex<WakeState>,
    cv: Condvar,
}

impl AutoArchiveWake {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(WakeState::default()),
            cv: Condvar::new(),
        })
    }

    pub fn set_locks(&self, locks: Arc<crate::locks::ActiveLockRegistry>) {
        let mut guard = self.state.lock().unwrap();
        guard.locks = Some(locks);
    }

    pub fn locks(&self) -> Option<Arc<crate::locks::ActiveLockRegistry>> {
        let guard = self.state.lock().unwrap();
        guard.locks.clone()
    }

    pub fn notify_vid(&self, vid: &str) {
        if vid.is_empty() {
            return;
        }
        let mut guard = self.state.lock().unwrap();
        // Bound in-memory pending set; overflow still triggers a bounded sweep when drained.
        if guard.vids.len() < 4096 {
            guard.vids.insert(vid.to_owned());
        }
        guard.notified = true;
        self.cv.notify_all();
    }

    pub fn notify_stop(&self) {
        let mut guard = self.state.lock().unwrap();
        guard.stopped = true;
        self.cv.notify_all();
    }

    fn requeue_vids(&self, vids: &[String]) {
        let mut guard = self.state.lock().unwrap();
        for vid in vids {
            if guard.vids.len() < 4096 {
                guard.vids.insert(vid.clone());
            }
        }
        if !guard.vids.is_empty() {
            guard.notified = true;
            self.cv.notify_all();
        }
    }

    fn wait_until_notified(&self, timeout: Duration, stop: &AtomicBool) -> bool {
        let mut guard = self.state.lock().unwrap();
        let deadline = Instant::now() + timeout;
        while !guard.notified && !guard.stopped && !stop.load(Ordering::Relaxed) {
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            let wait = (deadline - now).min(Duration::from_millis(250));
            let (next, _) = self.cv.wait_timeout(guard, wait).unwrap();
            guard = next;
        }
        if guard.stopped || stop.load(Ordering::Relaxed) {
            return false;
        }
        guard.notified = false;
        true
    }

    fn re_notify_if_pending(&self) {
        let mut guard = self.state.lock().unwrap();
        if !guard.vids.is_empty() {
            guard.notified = true;
            self.cv.notify_all();
        }
    }

    fn drain_batch(&self, max_items: usize) -> (Vec<String>, bool) {
        let mut guard = self.state.lock().unwrap();
        let mut batch = Vec::with_capacity(max_items.min(guard.vids.len()));
        while batch.len() < max_items {
            let Some(first) = guard.vids.pop_first() else {
                break;
            };
            batch.push(first);
        }
        let has_more = !guard.vids.is_empty();
        if has_more {
            guard.notified = true;
        }
        (batch, has_more)
    }
}

fn configured_u64(key: &str, default: u64, min: u64, max: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(default)
        .clamp(min, max)
}

fn dispatch_auto_archive(
    publisher: &crate::admission::AcceptedPublisher,
    body: Value,
    lock: Option<crate::locks::LockGuard>,
) -> Result<Value> {
    let (reply, wait) = tokio::sync::oneshot::channel();
    publisher
        .send(crate::Work {
            command: crate::Command::Archive {
                command: "auto-watermark-apply".into(),
                body,
            },
            reply,
            lock,
        })
        .map_err(|_| Error::new(503, "writer_busy", "执行队列已满或执行器不可用"))?;
    wait.blocking_recv()
        .map_err(|_| Error::new(503, "outcome_unknown", "自动归档回执未知"))?
}

fn run_publisher_loop(
    config: Config,
    worker: crate::admission::Worker,
    wake: Arc<AutoArchiveWake>,
    stop: Arc<AtomicBool>,
) {
    let debounce_ms = configured_u64("AUTO_ARCHIVE_DEBOUNCE_MS", DEFAULT_DEBOUNCE_MS, 0, 60_000);
    let sweep_secs = configured_u64("AUTO_ARCHIVE_SWEEP_SECS", DEFAULT_SWEEP_SECS, 1, 3_600);
    let max_batch = configured_u64(
        "AUTO_ARCHIVE_MAX_BATCH",
        DEFAULT_BG_MAX_BATCH as u64,
        1,
        MAX_BG_BATCH_LIMIT as u64,
    ) as usize;
    let mut consecutive_busy = 0u32;

    while !stop.load(Ordering::Relaxed) {
        let woke = wake.wait_until_notified(Duration::from_secs(sweep_secs), &stop);
        if stop.load(Ordering::Relaxed) {
            break;
        }
        if !woke {
            continue;
        }
        // Debounce burst completions so rapid `processing-result` commits coalesce into one batch.
        if debounce_ms > 0 {
            let until = Instant::now() + Duration::from_millis(debounce_ms);
            while !stop.load(Ordering::Relaxed) && Instant::now() < until {
                let remaining = until.saturating_duration_since(Instant::now());
                std::thread::sleep(remaining.min(Duration::from_millis(50)));
            }
            if stop.load(Ordering::Relaxed) {
                break;
            }
        }

        let (batch_vids, has_more) = wake.drain_batch(max_batch);
        if batch_vids.is_empty() {
            continue;
        }

        // Fast read-only gating before requesting an admission slot or executor turn.
        match processing_scheduler::is_auto_watermark_enabled_for_config(&config) {
            Ok(true) => {}
            Ok(false) => {
                consecutive_busy = 0;
                continue;
            }
            Err(err) => {
                eprintln!("auto-archive gating skipped: {}", err.error);
                continue;
            }
        }
        if let Ok(preview_store) = crate::store::Store::open_preview(&config)
            && preview_store.blocked().unwrap_or(true)
        {
            continue;
        }

        // Pre-filter read-only so ineligible completions never allocate an operation journal row.
        let vid_set: BTreeSet<String> = batch_vids.iter().cloned().collect();
        let eligible = match auto_watermark::evaluate_for_vids(&config, Some(&vid_set)) {
            Ok(eligible) if eligible.is_empty() => {
                consecutive_busy = 0;
                if has_more {
                    wake.re_notify_if_pending();
                }
                continue;
            }
            Ok(eligible) => eligible,
            Err(err) => {
                eprintln!("auto-archive pre-evaluate failed: {}", err.error);
                continue;
            }
        };

        let publisher = match worker.begin_publisher() {
            Ok(p) => p,
            Err(_) => {
                // Admission closed or read-only parity; stop retrying until next event.
                continue;
            }
        };

        let op_id = uuid::Uuid::new_v4().to_string();
        let lock_guard = wake.locks().map(|locks| {
            let lock_vids: Vec<String> = eligible
                .iter()
                .take(max_batch)
                .map(|e| e.vid.clone())
                .collect();
            let lock_people: Vec<String> = eligible
                .iter()
                .take(max_batch)
                .map(|e| e.person.clone())
                .collect();
            locks.register_queued(&op_id, "auto-archive", lock_vids, lock_people)
        });

        let body = json!({
            "operation_id": op_id,
            "background": true,
            "vids": batch_vids,
            "max_items": max_batch,
        });
        match dispatch_auto_archive(&publisher, body, lock_guard) {
            Ok(_) => {
                consecutive_busy = 0;
            }
            Err(err) if err.code == "writer_busy" || err.code == "busy" => {
                consecutive_busy = consecutive_busy.saturating_add(1);
                if consecutive_busy <= 3 {
                    wake.requeue_vids(&batch_vids);
                    std::thread::sleep(Duration::from_millis(500));
                } else {
                    eprintln!(
                        "auto-archive dropped batch after retries ({}): {}",
                        batch_vids.len(),
                        err.error
                    );
                    consecutive_busy = 0;
                }
            }
            Err(err) => {
                consecutive_busy = 0;
                eprintln!("auto-archive background run failed: {}", err.error);
            }
        }
        if has_more {
            let mut guard = wake.state.lock().unwrap();
            if !guard.vids.is_empty() {
                guard.notified = true;
            }
        }
    }
}

pub fn spawn_auto_archive_publisher(
    config: Config,
    worker: crate::admission::Worker,
    wake: Arc<AutoArchiveWake>,
    stop: Arc<AtomicBool>,
) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("auto-archive-publisher".into())
        .spawn(move || run_publisher_loop(config, worker, wake, stop))
        .expect("start auto-archive publisher")
}

fn payload(request: &Request) -> Result<&Value> {
    request
        .payload
        .as_ref()
        .filter(|v| v.is_object())
        .ok_or_else(|| Error::new(400, "invalid_request", "缺少自动归档请求参数"))
}

fn parse_requested_items(value: &Value) -> Result<Vec<RequestedItem>> {
    let arr = value
        .as_array()
        .ok_or_else(|| Error::new(400, "invalid_request", "items 必须为数组"))?;
    if arr.is_empty() || arr.len() > MAX_EXPLICIT_BATCH_LIMIT {
        return Err(Error::new(
            400,
            "invalid_request",
            format!("单次自动归档需选择 1 至 {MAX_EXPLICIT_BATCH_LIMIT} 个视频"),
        ));
    }
    let mut out = Vec::with_capacity(arr.len());
    let mut seen = BTreeSet::new();
    for raw in arr {
        let item: RequestedItem = serde_json::from_value(raw.clone())
            .map_err(|_| Error::new(400, "invalid_request", "待归档视频项格式无效"))?;
        if item.vid.trim().is_empty()
            || item.person.trim().is_empty()
            || item.path.trim().is_empty()
            || item.mtime_ns.trim().is_empty()
            || item.size == 0
        {
            return Err(Error::new(
                400,
                "invalid_request",
                "待归档视频项缺少必需指纹字段",
            ));
        }
        files::validate_name(&item.person)?;
        if !seen.insert(item.vid.clone()) {
            return Err(Error::new(400, "invalid_request", "待归档列表中包含重复视频编号"));
        }
        out.push(item);
    }
    Ok(out)
}

pub fn validate(request: &Request) -> Result<()> {
    if !request.files.is_empty() {
        return Err(Error::new(400, "invalid_request", "范围必须由原生计划确定"));
    }
    let body = payload(request)?;
    if body["background"] == true {
        if let Some(vids) = body.get("vids").filter(|v| !v.is_null()) {
            let arr = vids
                .as_array()
                .ok_or_else(|| Error::new(400, "invalid_request", "vids 必须为数组"))?;
            if arr.len() > MAX_BG_BATCH_LIMIT
                || arr
                    .iter()
                    .any(|v| v.as_str().is_none_or(|s| s.trim().is_empty()))
            {
                return Err(Error::new(400, "invalid_request", "后台批次 vids 参数无效"));
            }
        }
        if let Some(max) = body.get("max_items").and_then(Value::as_u64)
            && (max == 0 || max as usize > MAX_BG_BATCH_LIMIT)
        {
            return Err(Error::new(400, "invalid_request", "max_items 超出范围"));
        }
        return Ok(());
    }
    if let Some(version) = body.get("version")
        && !version.is_null()
        && !version.is_string()
    {
        return Err(Error::new(400, "invalid_request", "version 必须为字符串"));
    }
    parse_requested_items(&body["items"])?;
    Ok(())
}

fn has_existing_person_dir(config: &Config, person: &str) -> Result<std::path::PathBuf> {
    files::validate_name(person)?;
    let dir = config.creators().join(person);
    let meta = dir.symlink_metadata().map_err(|_| {
        Error::conflict("目标人物目录不存在，仅允许自动合并到已创建的人物目录")
            .at(dir.display().to_string())
    })?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err(
            Error::conflict("目标人物目录无效或为符号链接").at(dir.display().to_string()),
        );
    }
    Ok(dir)
}

fn match_tier_str(tier: auto_watermark::AutoMatchTier) -> &'static str {
    match tier {
        auto_watermark::AutoMatchTier::ConfirmedRule => "confirmed_rule",
        auto_watermark::AutoMatchTier::DirectHandle => "direct_handle",
    }
}

fn check_peach_preflight(
    config: &Config,
    src: &str,
    dst: &str,
    dst_name: &str,
    size: u64,
    creator: &str,
    at: &str,
    op_id: &str,
) -> Result<()> {
    if !config.ledger.exists() {
        return Ok(());
    }
    let row = json!({
        "src": src,
        "dst": dst,
        "creator": creator,
    });
    let prepared = archive_ledger::prepare(config, &[row], at, op_id)?;
    let entry = &prepared["files"][0];
    if entry["skipped"] == true {
        return Ok(());
    }
    if entry["entity_matches"]
        .as_array()
        .is_some_and(|rows| rows.len() > 1)
    {
        return Err(Error::new(
            409,
            "ledger_conflict",
            "Peach 人物记录不唯一",
        ));
    }
    let stubs = entry["new_path_assets"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if !stubs.is_empty() {
        let conn = db::open_readonly(&config.ledger)?;
        for stub in &stubs {
            archive_cancel::is_pure_scanner_stub(
                &conn,
                stub,
                &entry["asset"],
                Some(dst_name),
                Some(size as i64),
            )
            .map_err(|_| {
                Error::new(
                    409,
                    "ledger_conflict",
                    "Peach 目标路径已有非扫描占位资产记录",
                )
                .at(dst)
            })?;
        }
    }
    Ok(())
}

fn make_fallback_row(
    item: &auto_watermark::AutoMatchEligible,
    reason: &str,
    at: &str,
    op_id: &str,
) -> Value {
    let tier = match_tier_str(item.match_tier);
    let mut row = json!({
        "vid": item.vid,
        "creator": item.person,
        "at": at,
        "batch_id": op_id,
        "source": "auto-watermark",
        "source_id": item.watermark_key,
        "return_href": "/review",
        "auto_fallback_reason": reason,
        "auto_reason": tier,
        "match_tier": tier,
        "watermark_text": item.watermark_text,
        "watermark_platform": item.watermark_platform,
        "watermark_key": item.watermark_key,
    });
    if !item.note.trim().is_empty() {
        row["note"] = json!(item.note);
    }
    row
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
    let at = body["at"]
        .as_str()
        .ok_or_else(|| Error::new(400, "invalid_request", "缺少持久化操作时间"))?
        .to_owned();
    let background = body["background"] == true;

    let mut skipped_items: Vec<Value> = vec![];
    let mut candidates: Vec<auto_watermark::AutoMatchEligible> = vec![];

    if background {
        if !processing_scheduler::is_auto_watermark_enabled_for_config(config)? {
            return build_plan(
                config,
                request,
                &at,
                vec![],
                vec![],
                vec![],
                vec![],
                vec![],
                true,
            );
        }
        let max_items = body
            .get("max_items")
            .and_then(Value::as_u64)
            .map(|n| (n as usize).clamp(1, MAX_BG_BATCH_LIMIT))
            .unwrap_or(DEFAULT_BG_MAX_BATCH);
        let filter_vids: Option<BTreeSet<String>> = body.get("vids").and_then(Value::as_array).map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        });
        let mut eligible = auto_watermark::evaluate_for_vids(config, filter_vids.as_ref())?;
        eligible.truncate(max_items);
        candidates = eligible;
    } else {
        let requested = parse_requested_items(&body["items"])?;
        let requested_vids: BTreeSet<String> = requested.iter().map(|i| i.vid.clone()).collect();
        let fresh = auto_watermark::evaluate_for_vids(config, Some(&requested_vids))?;
        let fresh_by_vid: BTreeMap<String, auto_watermark::AutoMatchEligible> = fresh
            .into_iter()
            .map(|item| (item.vid.clone(), item))
            .collect();
        for req in requested {
            match fresh_by_vid.get(&req.vid) {
                Some(matched)
                    if matched.person == req.person
                        && matched.path == req.path
                        && matched.size == req.size
                        && matched.mtime_ns == req.mtime_ns =>
                {
                    candidates.push(matched.clone());
                }
                Some(matched) => {
                    let reason = if matched.person != req.person {
                        format!(
                            "匹配人物已由 {} 变更为 {}，已跳过",
                            req.person, matched.person
                        )
                    } else {
                        "源文件位置或指纹已变化，已跳过".into()
                    };
                    skipped_items.push(json!({
                        "vid": req.vid,
                        "person": req.person,
                        "status": "skipped",
                        "reason": reason,
                    }));
                }
                None => {
                    skipped_items.push(json!({
                        "vid": req.vid,
                        "person": req.person,
                        "status": "skipped",
                        "reason": "已不再满足自动水印归档条件或已被处理",
                    }));
                }
            }
        }
    }

    // Load local state once for pre-flight checks.
    let sources = review::sources(config)?;
    let (_, existing_queue) = review::read(config, "integration-queue.json", json!([]))?;
    let queued_vids: BTreeSet<String> = existing_queue
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r["vid"].as_str().map(str::to_owned))
        .collect();
    let queued_paths: BTreeSet<String> = queued_vids
        .iter()
        .filter_map(|v| review::source(config, v, &sources))
        .collect();

    let faces_path = config.base.join("faces.db");
    let all_videos = if faces_path.exists() {
        db::rows(&db::open(&faces_path)?, "SELECT * FROM videos ORDER BY vid", [])?
    } else {
        vec![]
    };
    let mut videos_by_path: BTreeMap<String, Vec<&Value>> = BTreeMap::new();
    for row in &all_videos {
        if let Some(p) = row["vid"]
            .as_str()
            .and_then(|id| review::source(config, id, &sources))
        {
            videos_by_path.entry(p).or_default().push(row);
        }
    }
    let jobs_path = config.base.join("processing/processing.db");
    let all_jobs = if jobs_path.exists() {
        db::rows(&db::open(&jobs_path)?, "SELECT * FROM tasks ORDER BY vid", [])?
    } else {
        vec![]
    };

    let mut moves: Vec<FileMove> = vec![];
    let mut scopes: Vec<MoveScope> = vec![];
    let mut planned_rows: Vec<Value> = vec![];
    let mut fallback_items: Vec<Value> = vec![];
    let mut reserved_targets: BTreeSet<String> = BTreeSet::new();
    let mut seen_sources: BTreeSet<String> = BTreeSet::new();
    let mut permission_cache = BTreeMap::new();

    for item in candidates {
        if queued_vids.contains(&item.vid) || queued_paths.contains(&item.path) {
            skipped_items.push(json!({
                "vid": item.vid,
                "person": item.person,
                "status": "skipped",
                "reason": "视频已在待归档队列中",
            }));
            continue;
        }
        // Source file validation & freshness check.
        let stat = match review::safe_source(config, &item.path) {
            Ok(s) => s,
            Err(err) => {
                skipped_items.push(json!({
                    "vid": item.vid,
                    "person": item.person,
                    "status": "skipped",
                    "reason": format!("源文件不可用：{}", err.error),
                }));
                continue;
            }
        };
        if stat.len() == 0
            || stat.len() != item.size
            || files::mtime_ns(&stat) != item.mtime_ns
        {
            skipped_items.push(json!({
                "vid": item.vid,
                "person": item.person,
                "status": "skipped",
                "reason": "源文件大小或修改时间已变化",
            }));
            continue;
        }
        if review::health_hidden(config, &item.vid, &item.path, &stat)? {
            skipped_items.push(json!({
                "vid": item.vid,
                "person": item.person,
                "status": "skipped",
                "reason": "视频体检异常，已跳过",
            }));
            continue;
        }
        if !seen_sources.insert(item.path.clone()) {
            fallback_items.push(make_fallback_row(
                &item,
                "同批次内存在指向相同物理源文件的其他任务",
                &at,
                &request.id,
            ));
            continue;
        }

        // Pre-flight 1: target person directory must already exist (never auto-create!).
        let person_dir = match has_existing_person_dir(config, &item.person) {
            Ok(dir) => dir,
            Err(err) => {
                fallback_items.push(make_fallback_row(&item, &err.error, &at, &request.id));
                continue;
            }
        };

        // Pre-flight 2: keep-both collision naming (never overwrite!).
        let (target_name, renamed_keep_both) = match auto_watermark::keep_both_target_name(
            &person_dir,
            &item.name,
            &item.vid,
            &mut reserved_targets,
        ) {
            Ok(v) => v,
            Err(err) => {
                fallback_items.push(make_fallback_row(
                    &item,
                    &format!("计算保留两份文件名失败：{}", err.error),
                    &at,
                    &request.id,
                ));
                continue;
            }
        };
        let dst_path = person_dir.join(&target_name);
        let dst_str = dst_path.to_string_lossy().to_string();
        if dst_path.symlink_metadata().is_ok() {
            fallback_items.push(make_fallback_row(
                &item,
                "目标文件名已被占用，转入待归档人工核对",
                &at,
                &request.id,
            ));
            continue;
        }

        // Pre-flight 3: move permissions.
        if let Err(err) = permissions::require_move_permissions_cached(
            Path::new(&item.path),
            &dst_path,
            &mut permission_cache,
        ) {
            fallback_items.push(make_fallback_row(
                &item,
                &format!("目录权限预检未通过：{}", err.error),
                &at,
                &request.id,
            ));
            continue;
        }

        // Collect all vid aliases pointing to this physical source.
        let scoped_videos = videos_by_path.get(&item.path).cloned().unwrap_or_default();
        let mut vids: Vec<String> = scoped_videos
            .iter()
            .filter_map(|r| r["vid"].as_str().map(str::to_owned))
            .collect();
        if !vids.contains(&item.vid) {
            vids.push(item.vid.clone());
        }

        // Pre-flight 4: local DB target-path collision check.
        let win_dst = metadata::win(config, &dst_str);
        let local_conflict = all_videos
            .iter()
            .chain(all_jobs.iter())
            .any(|row| {
                (row["path"] == dst_str || row["path"] == win_dst)
                    && !vids.iter().any(|id| row["vid"] == *id)
            });
        if local_conflict {
            fallback_items.push(make_fallback_row(
                &item,
                "目标路径已有其他本地视频或任务记录",
                &at,
                &request.id,
            ));
            continue;
        }

        // Pre-flight 5: strict Peach ledger conflict / non-stub check.
        if let Err(err) = check_peach_preflight(
            config,
            &item.path,
            &dst_str,
            &target_name,
            item.size,
            &item.person,
            &at,
            &request.id,
        ) {
            fallback_items.push(make_fallback_row(
                &item,
                &format!("Peach 账本预检冲突：{}", err.error),
                &at,
                &request.id,
            ));
            continue;
        }

        let tier = match_tier_str(item.match_tier);
        let fingerprint = files::fingerprint(&item.path, &stat);
        moves.push(FileMove {
            vids,
            src: item.path.clone(),
            dst: dst_str.clone(),
            size: item.size,
            version: fingerprint,
        });
        scopes.push(MoveScope {
            source: ScopeRoot::Unorganized,
            destination: ScopeRoot::Person(item.person.clone()),
        });
        let mut row = json!({
            "vid": item.vid,
            "name": item.name,
            "target_name": target_name,
            "creator": item.person,
            "src": item.path,
            "original_path": item.path,
            "dst": dst_str,
            "size": item.size,
            "size_gb": (item.size as f64 / 1073741824.0 * 100.0).round() / 100.0,
            "mtime_ns": item.mtime_ns,
            "at": at,
            "batch_id": request.id,
            "operation": "archive",
            "source": "auto-watermark",
            "auto_reason": tier,
            "match_tier": tier,
            "watermark_text": item.watermark_text,
            "watermark_platform": item.watermark_platform,
            "watermark_key": item.watermark_key,
            "renamed_keep_both": renamed_keep_both,
        });
        if !item.note.trim().is_empty() {
            row["note"] = json!(item.note);
        }
        planned_rows.push(row);
    }

    build_plan(
        config,
        request,
        &at,
        moves,
        scopes,
        planned_rows,
        fallback_items,
        skipped_items,
        background,
    )
}

#[allow(clippy::too_many_arguments)]
fn build_plan(
    config: &Config,
    request: Request,
    at: &str,
    moves: Vec<FileMove>,
    scopes: Vec<MoveScope>,
    planned_rows: Vec<Value>,
    fallback_items: Vec<Value>,
    skipped_items: Vec<Value>,
    background: bool,
) -> Result<Plan> {
    let faces_path = config.base.join("faces.db");
    let jobs_path = config.base.join("processing/processing.db");
    let videos = if !moves.is_empty() && faces_path.exists() {
        db::rows(&db::open(&faces_path)?, "SELECT * FROM videos ORDER BY vid", [])?
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
    let jobs = if !moves.is_empty() && jobs_path.exists() {
        db::rows(&db::open(&jobs_path)?, "SELECT * FROM tasks ORDER BY vid", [])?
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
    let has_ledger = !moves.is_empty() && config.ledger.exists();
    let ledger = if has_ledger {
        archive_ledger::prepare(config, &planned_rows, at, &request.id)?
    } else {
        Value::Null
    };
    let documents = load_documents(config)?;
    let total_bytes = moves.iter().map(|f| f.size).sum();

    let data = json!({
        "rows": planned_rows,
        "fallback_items": fallback_items,
        "skipped_items": skipped_items,
        "ledger": ledger,
        "videos": videos,
        "jobs": jobs,
        "documents": documents,
        "at": at,
        "background": background,
    });
    let stores = AffectedStores {
        faces: !moves.is_empty() && faces_path.exists(),
        jobs: !moves.is_empty() && jobs_path.exists(),
        ledger: has_ledger,
    };
    Ok(Plan {
        id: request.id.clone(),
        request,
        version: String::new(),
        action: "auto-archive".into(),
        person: String::new(),
        target_person: String::new(),
        files: moves,
        total_bytes,
        directories: vec![], // Auto-archive only merges into existing creator directories.
        ledger_version: String::new(),
        metadata_version: String::new(),
        execution: Some(ExecutionIntent {
            scopes,
            failure_policy: FailurePolicy::ContinueUnchanged,
            stores,
            data,
            result: json!({
                "ok": true,
                "archived": 0,
                "moved": 0,
                "fallback_queued": 0,
                "skipped": 0,
                "results": [],
                "errors": [],
            }),
        }),
    })
}

/// Re-evaluate all planned items under `WriteGate` right before admission.
pub fn revalidate(config: &Config, plan: &Plan) -> Result<()> {
    let intent = plan
        .execution
        .as_ref()
        .ok_or_else(|| Error::conflict("自动归档计划缺少执行证据"))?;
    let data = &intent.data;
    if data["background"] == true
        && !processing_scheduler::is_auto_watermark_enabled_for_config(config)?
    {
        return Err(Error::conflict("自动处理已暂停或已关闭自动水印归档"));
    }
    let rows = data["rows"]
        .as_array()
        .ok_or_else(|| Error::conflict("自动归档条目缺失"))?;
    if !rows.is_empty() {
        let planned_vids: BTreeSet<String> = rows
            .iter()
            .filter_map(|r| r["vid"].as_str().map(str::to_owned))
            .collect();
        let fresh = auto_watermark::evaluate_for_vids(config, Some(&planned_vids))?;
        let fresh_by_vid: BTreeMap<String, auto_watermark::AutoMatchEligible> = fresh
            .into_iter()
            .map(|item| (item.vid.clone(), item))
            .collect();
        for (index, row) in rows.iter().enumerate() {
            let vid = row["vid"]
                .as_str()
                .ok_or_else(|| Error::conflict("自动归档记录缺少视频编号"))?;
            let matched = fresh_by_vid
                .get(vid)
                .ok_or_else(|| Error::conflict("视频已不再满足自动水印归档条件"))?;
            if matched.person != row["creator"].as_str().unwrap_or("")
                || matched.path != row["src"].as_str().unwrap_or("")
                || matched.size != row["size"].as_u64().unwrap_or(0)
                || matched.mtime_ns != row["mtime_ns"].as_str().unwrap_or("")
            {
                return Err(Error::conflict("视频匹配结果或文件状态已变化，请刷新预览"));
            }
            let file = &plan.files[index];
            let stat = review::safe_source(config, &file.src)?;
            if files::fingerprint(&file.src, &stat) != file.version {
                return Err(Error::conflict("源视频指纹已变化"));
            }
            has_existing_person_dir(config, &matched.person)?;
            if Path::new(&file.dst).symlink_metadata().is_ok() {
                return Err(Error::conflict("目标路径在提交前已被占用").at(&file.dst));
            }
        }
    }
    for doc in serde_json::from_value::<Vec<metadata::Document>>(data["documents"].clone())? {
        if files::read_optional(&config.base.join(&doc.name))? != doc.before {
            return Err(Error::conflict("相关归档记录已变化").at(doc.name));
        }
    }
    Ok(())
}

pub fn snapshot(config: &Config, plan: &Plan, successful: &[usize]) -> Result<DomainSnapshot> {
    let execution = plan
        .execution
        .as_ref()
        .ok_or_else(|| Error::conflict("自动归档意图缺失"))?;
    let data = &execution.data;
    let mut accepted = BTreeSet::new();
    for &i in successful {
        if i >= plan.files.len() || !accepted.insert(i) {
            return Err(Error::conflict("自动归档提交范围无效"));
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
            row["name"] = json!(Path::new(&f.dst).file_name().unwrap().to_string_lossy());
        }
        if row.get("collection").is_some() {
            row["collection"] = json!(Path::new(&f.dst).parent().unwrap().to_string_lossy());
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

    let fallback_items = data["fallback_items"]
        .as_array()
        .cloned()
        .unwrap_or_default();
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
            if !successful.is_empty() {
                value = metadata::transform(config, &value, &selected_plan);
            }
        }
        if doc.name == "integration-queue.json" {
            let queue = value.as_array_mut().unwrap();
            if !successful.is_empty() {
                queue.retain(|r| selected(r).is_none());
            }
            for item in &fallback_items {
                if !queue.iter().any(|r| r["vid"] == item["vid"]) {
                    queue.push(item.clone());
                }
            }
        }
        if doc.name == "archive-history.json" {
            let history = value.as_array_mut().unwrap();
            for &i in successful {
                let mut row = data["rows"][i].clone();
                row["original_path"] = json!(plan.files[i].src);
                row["dst"] = json!(plan.files[i].dst);
                row["win_dst"] = json!(metadata::win(config, &plan.files[i].dst));
                row["manifest"] = json!(format!("rust:{}", plan.id));
                row["operation_id"] = json!(plan.id);
                // Keep historical undone entries intact; replace any active entry for the same vid.
                history.retain(|r| r["vid"] != row["vid"] || r["undone"] == true);
                history.push(row);
            }
        }
        if doc.name == "video-path-overrides.json" {
            for &i in successful {
                for id in &plan.files[i].vids {
                    value[id] = json!(plan.files[i].dst);
                }
            }
        }
        doc.after = if value == original {
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

    let faces_delta = if execution.stores.faces {
        Some(db::Delta {
            guards: vec![db::Guard::LocalFiles {
                table: db::Table::Videos,
                vids: vids.clone(),
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
                vids,
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

    let ledger = if execution.stores.ledger {
        if data["ledger"].is_null() {
            return Err(Error::conflict("自动归档缺少 Peach 账本意图"));
        }
        Some(archive_ledger::delta(&data["ledger"], successful)?)
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
        ledger,
    })
}

pub fn result(plan: &Plan, successful: &[usize], failures: &[Value]) -> Value {
    let data = &plan.execution.as_ref().unwrap().data;
    let rows = data["rows"].as_array().cloned().unwrap_or_default();
    let fallback_items = data["fallback_items"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let skipped_items = data["skipped_items"]
        .as_array()
        .cloned()
        .unwrap_or_default();

    let mut results = Vec::new();
    let mut errors = Vec::new();
    let mut failure_rows = Vec::new();

    for &i in successful {
        if let Some(row) = rows.get(i) {
            results.push(json!({
                "vid": row["vid"],
                "person": row["creator"],
                "status": "archived",
                "dst": plan.files[i].dst,
                "renamed_keep_both": row["renamed_keep_both"] == true,
                "match_tier": row["match_tier"],
            }));
        }
    }
    for failure in failures {
        let index = failure["file_index"].as_u64().unwrap_or(0) as usize;
        let row = rows.get(index);
        let vid = row.and_then(|r| r["vid"].as_str()).unwrap_or("");
        let person = row.and_then(|r| r["creator"].as_str()).unwrap_or("");
        let name = row.and_then(|r| r["name"].as_str()).unwrap_or("");
        let message = failure["error"]["error"]
            .as_str()
            .or(failure["error"].as_str())
            .or(failure["message"].as_str())
            .unwrap_or("自动归档未完成");
        results.push(json!({
            "vid": vid,
            "person": person,
            "status": "failed",
            "reason": message,
        }));
        failure_rows.push(json!({
            "vid": vid,
            "name": name,
            "message": message,
            "path": failure["src"],
            "code": failure["error"]["code"],
            "step": failure["error"]["step"],
        }));
        errors.push(format!("{name}：{message}"));
    }
    for item in &fallback_items {
        results.push(json!({
            "vid": item["vid"],
            "person": item["creator"],
            "status": "fallback_queued",
            "reason": item["auto_fallback_reason"],
        }));
    }
    for item in &skipped_items {
        results.push(item.clone());
    }

    json!({
        "ok": failures.is_empty(),
        "id": plan.id,
        "archived": successful.len(),
        "moved": successful.len(),
        "fallback_queued": fallback_items.len(),
        "skipped": skipped_items.len(),
        "results": results,
        "failures": failure_rows,
        "errors": errors,
        "manifest": format!("rust:{}", plan.id),
    })
}

pub fn failure_result(
    plan: &Plan,
    status: &str,
    error: &str,
    skipped: &[Value],
    retained: &[Value],
) -> Value {
    let mut res = result(plan, &[], skipped);
    let mut failures = skipped.to_vec();
    for (index, file) in plan.files.iter().enumerate() {
        if !skipped
            .iter()
            .any(|failure| failure["file_index"].as_u64() == Some(index as u64))
        {
            let message = if status == "rolled_back" {
                format!("本轮已执行的变更已回退：{error}")
            } else if status == "recovery_needed" {
                format!("结果待核对，暂不能重试：{error}")
            } else {
                error.to_owned()
            };
            failures.push(json!({
                "file_index": index,
                "src": file.src,
                "dst": file.dst,
                "error": {
                    "error": message,
                    "code": if status == "rolled_back" { "archive_rolled_back" } else { "archive_incomplete" },
                    "step": status,
                }
            }));
        }
    }
    let full = result(plan, &[], &failures);
    let mut results = full["results"].clone();
    let rollback_reason = if status == "rolled_back" {
        format!("本轮已执行的变更已回退，未转入待归档：{error}")
    } else if status == "recovery_needed" {
        format!("结果待核对，未确认转入待归档：{error}")
    } else {
        format!("自动归档失败，未转入待归档：{error}")
    };
    if let Some(items) = results.as_array_mut() {
        for item in items {
            if item["status"] == "fallback_queued" {
                item["status"] = json!("failed");
                item["reason"] = json!(rollback_reason);
            }
        }
    }
    res["results"] = results;
    res["failures"] = full["failures"].clone();
    let mut seen = BTreeSet::new();
    if let Some(rows) = res["failures"].as_array_mut() {
        rows.retain(|row| seen.insert(row["vid"].as_str().unwrap_or("").to_owned()));
        for row in rows {
            row["retryable"] = json!(status != "recovery_needed" && retained.is_empty());
        }
    }
    let errors = res["errors"].as_array_mut().unwrap();
    if !errors.iter().any(|message| message == error) {
        errors.push(json!(error));
    }
    // On aggregate rollback/failure, fallback items were NOT committed either.
    res["fallback_queued"] = json!(0);
    res["ok"] = json!(false);
    res["rolled_back"] = json!(status == "rolled_back");
    res["recovery_needed"] = json!(status == "recovery_needed");
    if !retained.is_empty() {
        res["external_index_conflicts"] = json!(
            retained
                .iter()
                .map(|row| json!({
                    "asset_id": row["id"]["$integer"],
                    "path": row["path"],
                    "disposition": "retained_unchanged"
                }))
                .collect::<Vec<_>>()
        );
        res["own_changes_rolled_back"] = json!(status == "rolled_back");
    }
    res
}
