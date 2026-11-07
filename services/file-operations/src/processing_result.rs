//! Commit-only analysis publication on the existing Engine authority and writer lane.
//! Processing leases stay with the VM. Only an admitted current attempt may publish.
//! A processing DB receipt is definitive even when subsequent legitimate progress changes rows.
use crate::{
    config::Config,
    db,
    engine::Engine,
    files::{self, WriteGate},
    metadata,
    model::{self, AffectedStores, Error, ExecutionIntent, FailurePolicy, Plan, Request, Result},
    native::DomainSnapshot,
    review, shared_metadata,
};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeSet, fs, path::Path};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Task {
    vid: String,
    path: String,
    size: u64,
    mtime_ns: String,
    stage: String,
    run_id: String,
    batch_id: String,
    lease_expires_at: f64,
    coordinator_generation: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Submission {
    operation_id: String,
    scope: Value,
    version: String,
    task: Task,
    data: Value,
    next_stage: String,
    #[serde(default)]
    reference_input_version: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Evidence {
    gate: Value,
    vid: String,
    before: Option<Value>,
    after: Option<Value>,
    result: Value,
    #[serde(default)]
    group: Option<Value>,
}
fn invalid(message: &str) -> Error {
    Error::new(400, "invalid_processing_result", message)
}
fn stale(message: &str) -> Error {
    Error::new(409, "stale_processing_result", message)
}
fn required<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v[key]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| invalid("缺少分析结果字段"))
}
fn submission(plan: &Plan) -> Result<Submission> {
    serde_json::from_value(
        plan.request
            .payload
            .clone()
            .ok_or_else(|| invalid("缺少结果"))?,
    )
    .map_err(|_| invalid("分析结果请求格式无效"))
}
pub fn validate(r: &Request) -> Result<()> {
    model::validate_id(&r.id)?;
    if !r.files.is_empty()
        || !r.person.is_empty()
        || !r.filename.is_empty()
        || r.target_person.is_some()
        || r.target_folder.is_some()
    {
        return Err(invalid("分析发布不能包含媒体操作"));
    }
    let s: Submission =
        serde_json::from_value(r.payload.clone().ok_or_else(|| invalid("缺少结果"))?)
            .map_err(|_| invalid("分析结果请求格式无效"))?;
    model::validate_id(&s.task.run_id)?;
    if s.operation_id != r.id
        || s.task.run_id != r.id
        || !matches!(s.task.stage.as_str(), "text" | "face")
        || !matches!(s.next_stage.as_str(), "review" | "face_queued")
        || s.scope["vids"] != json!([s.task.vid])
        || s.version.is_empty()
        || !s.task.lease_expires_at.is_finite()
        || s.task.coordinator_generation.is_empty()
    {
        return Err(invalid("执行编号、阶段或范围无效"));
    }
    if s.task.stage == "text" && s.scope["all_watermark_rules"] != true {
        return Err(invalid("文字分析必须绑定完整账号规则快照"));
    }
    if s.task.stage == "face"
        && (s.next_stage != "review"
            || s.reference_input_version
                .as_deref()
                .is_none_or(str::is_empty))
    {
        return Err(invalid("人脸分析缺少参考输入版本"));
    }
    let data = s
        .data
        .as_object()
        .ok_or_else(|| invalid("分析数据不是对象"))?;
    const KEYS: &[&str] = &[
        "watermark",
        "folder",
        "text_candidates",
        "text_complete",
        "face_skipped",
        "face",
        "face_complete",
        "ready",
        "kind",
        "candidates",
        "suggested",
    ];
    if data.keys().any(|k| !KEYS.contains(&k.as_str()))
        || s.data["text_complete"] != true
        || s.data["ready"] != json!(s.next_stage == "review")
        || !s.data["face_skipped"].is_boolean()
        || !s.data["candidates"].is_array()
        || !matches!(
            s.data["kind"].as_str(),
            Some("none" | "suggested" | "conflict")
        )
    {
        return Err(invalid("结果包含非分析字段或不一致的就绪状态"));
    }
    if s.task.stage == "face"
        && (s.data["face_complete"] != true
            || !s.data["face"].is_object()
            || s.data["face_skipped"] != false)
    {
        return Err(invalid("人脸阶段结果不完整"));
    }
    Ok(())
}
pub fn plan(_config: &Config, r: Request) -> Result<Plan> {
    validate(&r)?;
    Ok(Plan {
        id: r.id.clone(),
        action: r.action.clone(),
        person: String::new(),
        target_person: String::new(),
        request: r,
        version: String::new(),
        files: vec![],
        total_bytes: 0,
        directories: vec![],
        metadata_version: String::new(),
        ledger_version: String::new(),
        execution: Some(ExecutionIntent {
            scopes: vec![],
            failure_policy: FailurePolicy::StopIrreversible,
            stores: AffectedStores {
                faces: false,
                jobs: true,
                ledger: false,
            },
            data: json!({"publication_schema":1,"commit_only":true}),
            result: json!({}),
        }),
    })
}
pub fn snapshot() -> DomainSnapshot {
    DomainSnapshot {
        metadata: metadata::Snapshot {
            documents: vec![],
            videos: vec![],
            jobs: vec![],
            faces_delta: None,
            jobs_delta: None,
        },
        ledger: None,
    }
}
pub fn execute(engine: &mut Engine, body: Value) -> Result<Value> {
    let id = required(&body, "operation_id")?.to_owned();
    model::validate_id(&id)?;
    let request = Request {
        id,
        action: "processing-result".into(),
        person: String::new(),
        files: vec![],
        filename: String::new(),
        target_person: None,
        target_folder: None,
        payload: Some(body),
    };
    let plan: Plan = serde_json::from_value(engine.preview(request)?)?;
    if engine
        .store
        .get(&plan.id)?
        .is_some_and(|r| matches!(r.status.as_str(), "recovery_needed" | "rolling_back"))
    {
        engine.reconcile_request(plan.request.clone(), Some(plan.version.clone()))?;
    }
    engine.apply(plan.request, plan.version)
}
fn open(config: &Config) -> Result<rusqlite::Connection> {
    db::open(&config.base.join("processing/processing.db"))
}
fn receipt(
    conn: &rusqlite::Connection,
    id: &str,
    signature: Option<&str>,
) -> Result<Option<Value>> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='_face_admin_processing_results')",
        [],
        |r| r.get(0),
    )?;
    if !exists {
        return Ok(None);
    }
    let row: Option<(String, String)> = conn
        .query_row(
            "SELECT signature,result FROM _face_admin_processing_results WHERE id=?",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    row.map(|(sig, raw)| {
        if signature.is_some_and(|s| s != sig) {
            return Err(stale("发布回执与原请求不一致"));
        }
        Ok(serde_json::from_str(&raw)?)
    })
    .transpose()
}
fn row(conn: &rusqlite::Connection, sql: &str, id: &str) -> Result<Value> {
    db::rows(conn, sql, [id])?
        .into_iter()
        .next()
        .ok_or_else(|| stale("任务或执行记录已不存在"))
}
fn current(conn: &rusqlite::Connection, config: &Config, s: &Submission) -> Result<Value> {
    let t = row(conn, "SELECT * FROM tasks WHERE vid=?", &s.task.vid)?;
    for (key, expected) in [
        ("vid", &s.task.vid),
        ("path", &s.task.path),
        ("mtime_ns", &s.task.mtime_ns),
        ("stage", &s.task.stage),
        ("run_id", &s.task.run_id),
        ("batch_id", &s.task.batch_id),
        ("coordinator_generation", &s.task.coordinator_generation),
    ] {
        if t[key] != *expected {
            return Err(stale("任务租约、路径或执行阶段已变化"));
        }
    }
    let now: f64 = conn.query_row("SELECT unixepoch('now','subsec')", [], |r| r.get(0))?;
    if review::number(&t["size"]) != Some(s.task.size as i64)
        || review::number(&t["manual"]) != Some(0)
        || t["lease_expires_at"].as_f64() != Some(s.task.lease_expires_at)
        || s.task.lease_expires_at <= now
    {
        return Err(stale("任务已由人工接管或租约已失效"));
    }
    let generation: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key='coordinator_generation'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    if generation
        .map(|g| serde_json::from_str::<Value>(&g))
        .transpose()?
        .as_ref()
        != Some(&json!(s.task.coordinator_generation))
    {
        return Err(stale("协调器已更换执行代次"));
    }
    let run = row(
        conn,
        "SELECT * FROM processing_runs WHERE id=?",
        &s.task.run_id,
    )?;
    if run["vid"] != s.task.vid
        || run["batch_id"] != s.task.batch_id
        || run["stage"] != s.task.stage
        || run["status"] != "running"
    {
        return Err(stale("执行回执已结束或不属于此任务"));
    }
    let members = db::rows(
        conn,
        "SELECT * FROM batch_items WHERE batch_id=? AND vid=?",
        params![s.task.batch_id, s.task.vid],
    )?;
    if members.len() != 1
        || members[0]["path"] != s.task.path
        || review::number(&members[0]["size"]) != Some(s.task.size as i64)
        || members[0]["mtime_ns"] != s.task.mtime_ns
        || review::number(&members[0]["excluded"]) != Some(0)
    {
        return Err(stale("批次成员或文件版本已变化"));
    }
    for name in ["integration-queue.json", "archive-history.json"] {
        if review::read(config, name, json!([]))?
            .1
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["vid"] == s.task.vid)
        {
            return Err(stale("视频已由人工处理"));
        }
    }
    let sources = review::sources(config)?;
    if sources.contains_key(&s.task.vid)
        && review::source(config, &s.task.vid, &sources).as_deref() != Some(s.task.path.as_str())
    {
        return Err(stale("稳定视频编号的当前路径已变化"));
    }
    let stat = review::safe_source(config, &s.task.path)?;
    if stat.len() != s.task.size || files::mtime_ns(&stat) != s.task.mtime_ns {
        return Err(stale("源文件指纹已变化"));
    }
    shared_metadata::check_state(config, &s.scope, &s.version)?;
    if let Some(version) = &s.reference_input_version
        && shared_metadata::analysis_state(config)?["version"] != *version
    {
        return Err(stale("参考模型输入已变化"));
    }
    Ok(t)
}
pub fn artifact_hash(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    if fs::canonicalize(path)? != path || !fs::symlink_metadata(path)?.is_file() {
        return Err(invalid("特征缓存路径无效"));
    }
    let mut file = fs::File::open(path)?;
    let before = file.metadata()?;
    if before.len() > 256 * 1024 * 1024 {
        return Err(invalid("特征缓存超过 256 MiB"));
    }
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        digest.update(&buffer[..n]);
    }
    let after = file.metadata()?;
    if files::identity(&before) != files::identity(&after)
        || files::identity(&after) != files::identity(&fs::metadata(path)?)
    {
        return Err(stale("读取期间特征缓存已改变"));
    }
    Ok(format!("{:x}", digest.finalize()))
}
/// Reference-building token includes receipt-backed and legacy cache bytes for assigned videos.
/// Legacy identity/NPZ compatibility validation remains with the model loader.
pub fn assigned_reference_inputs(config: &Config) -> Result<Value> {
    let mut assigned = BTreeSet::new();
    for name in ["integration-queue.json", "archive-history.json"] {
        for row in review::read(config, name, json!([]))?.1.as_array().unwrap() {
            if let Some(vid) = row["vid"].as_str() {
                assigned.insert(vid.to_owned());
            }
        }
    }
    if assigned.is_empty() {
        return Ok(json!([]));
    }
    use sha2::{Digest, Sha256};
    let mut out = vec![];
    for vid in &assigned {
        let name = format!("{:x}.npz", Sha256::digest(vid.as_bytes()));
        let path = config.base.join("processing/faces").join(name);
        let digest = match artifact_hash(&path) {
            Ok(value) => json!(value),
            Err(e) if e.code == "source_changed" => json!({"missing":true}),
            Err(e) => return Err(e),
        };
        out.push(json!({"vid":vid,"legacy_cache":path,"digest":digest}));
    }
    if !config.base.join("processing/processing.db").exists() {
        return Ok(json!(out));
    }
    let conn = open(config)?;
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='_face_admin_processing_results')",
        [],
        |r| r.get(0),
    )?;
    if !exists {
        return Ok(json!(out));
    }
    let mut query=conn.prepare("SELECT id,result FROM _face_admin_processing_results WHERE json_extract(result,'$.vid')=? AND json_type(result,'$.data.face.feature_cache')='text' ORDER BY id")?;
    for vid in &assigned {
        let mut rows = query.query([vid])?;
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            let raw: String = row.get(1)?;
            let result: Value = serde_json::from_str(&raw)?;
            if let Some(path) = result["data"]["face"]["feature_cache"].as_str() {
                if !Path::new(path).starts_with(config.base.join("processing/attempts")) {
                    continue;
                }
                let digest = match artifact_hash(Path::new(path)) {
                    Ok(v) => json!(v),
                    Err(e) if e.code == "source_changed" => json!({"missing":true}),
                    Err(e) => return Err(e),
                };
                out.push(json!({"id":id,"task":result["task"],"source_identity":result["source_identity"],"face_version":result["data"]["face"]["version"],"path":path,"expected":result["data"]["face"]["feature_cache_sha256"],"current":digest}));
            }
        }
    }
    Ok(json!(out))
}
fn artifact(
    config: &Config,
    conn: &rusqlite::Connection,
    s: &Submission,
    old: &Value,
    path: &str,
    field: &str,
) -> Result<()> {
    let p = Path::new(path);
    let root = config.base.join("processing/attempts");
    let relative = match p.strip_prefix(&root) {
        Ok(r) => r,
        Err(_) => {
            // Old caches are accepted only as exact previously visible task evidence.
            if field == "feature_cache" && old["face"][field] == path
                || field == "thumbs"
                    && old["face"]["thumbs"]
                        .as_array()
                        .is_some_and(|a| a.contains(&json!(path)))
            {
                return Ok(());
            }
            return Err(invalid("分析附件超出执行产物目录"));
        }
    };
    let parts: Vec<_> = relative.components().collect();
    if parts.len() < 2
        || parts
            .iter()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
        || fs::canonicalize(p)? != p
        || !fs::metadata(p)?.is_file()
    {
        return Err(invalid("分析附件路径无效"));
    }
    let attempt = parts[0]
        .as_os_str()
        .to_str()
        .ok_or_else(|| invalid("附件执行编号无效"))?;
    model::validate_id(attempt)?;
    if attempt != s.task.run_id {
        let saved =
            receipt(conn, attempt, None)?.ok_or_else(|| stale("缓存附件尚未通过原生发布"))?;
        for key in ["vid", "path", "size", "mtime_ns"] {
            if saved["task"][key] != serde_json::to_value(&s.task)?[key] {
                return Err(stale("缓存附件属于其他文件版本"));
            }
        }
        if saved["data"]["face"]["version"] != s.data["face"]["version"] {
            return Err(stale("缓存模型版本已变化"));
        }
        if field == "feature_cache" && saved["data"]["face"][field] != path
            || field == "thumbs"
                && !saved["data"]["face"]["thumbs"]
                    .as_array()
                    .is_some_and(|a| a.contains(&json!(path)))
        {
            return Err(stale("缓存附件不在原发布范围"));
        }
    }
    Ok(())
}
fn prepare_data(
    config: &Config,
    conn: &rusqlite::Connection,
    s: &Submission,
    task: &Value,
) -> Result<(Value, Option<Value>)> {
    let old = review::task_data(task)?;
    let mut data = s.data.clone();
    if s.task.stage == "text" {
        data.as_object_mut().unwrap().remove("face");
        data.as_object_mut().unwrap().remove("face_complete");
        if data["face_skipped"] != data["ready"] {
            return Err(invalid("文字阶段完成标记不一致"));
        }
    } else {
        for field in [
            "watermark",
            "folder",
            "text_candidates",
            "text_complete",
            "face_skipped",
        ] {
            if data[field] != old[field] {
                return Err(stale("人脸结果不能覆盖文字阶段证据"));
            }
        }
        required(&data["face"], "version")?;
        required(&data["face"], "reference_version")?;
        if let Some(path) = data["face"]["feature_cache"].as_str() {
            artifact(config, conn, s, &old, path, "feature_cache")?;
            if data["face"]["feature_cache_sha256"] != artifact_hash(Path::new(path))? {
                return Err(stale("特征缓存内容校验失败"));
            }
        }
        for value in data["face"]["thumbs"]
            .as_array()
            .ok_or_else(|| invalid("缩略图列表无效"))?
        {
            artifact(
                config,
                conn,
                s,
                &old,
                value.as_str().ok_or_else(|| invalid("缩略图路径无效"))?,
                "thumbs",
            )?
        }
        for candidate in data["face"]["matches"]
            .as_array()
            .ok_or_else(|| invalid("人脸候选列表无效"))?
        {
            if let Some(path) = candidate["thumb"].as_str() {
                artifact(config, conn, s, &old, path, "thumbs")?
            }
        }
    }
    let watermark = &data["watermark"];
    if !watermark.is_object()
        || watermark["vid"] != s.task.vid
        || watermark["path"] != s.task.path
        || watermark["size"].as_u64() != Some(s.task.size)
        || watermark["mtime_ns"] != s.task.mtime_ns
        || watermark["status"] != "done"
        || !watermark["candidates"].is_array()
    {
        return Err(invalid("文字证据与源文件不匹配"));
    }
    required(watermark, "pipeline")?;
    if let Some(attempt) = watermark["artifact_attempt"].as_str() {
        model::validate_id(attempt)?;
        if attempt != s.task.run_id {
            let previous =
                receipt(conn, attempt, None)?.ok_or_else(|| stale("文字缓存尚未发布"))?;
            if previous["data"]["watermark"] != *watermark {
                return Err(stale("文字缓存与回执不一致"));
            }
        }
    } else if old["watermark"] != *watermark
        && document(config)?.get(&s.task.vid) != Some(watermark)
    {
        return Err(invalid("文字产物缺少执行编号"));
    }
    for candidate in watermark["candidates"].as_array().unwrap() {
        let key = required(candidate, "key")?;
        if key.len() != 64 || !key.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(invalid("文字候选编号无效"));
        }
        if let Some(evidence) = candidate["evidence"].as_str() {
            if !evidence.ends_with(".jpg")
                || !evidence
                    .trim_end_matches(".jpg")
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() || b == b'-')
            {
                return Err(invalid("文字证据路径无效"));
            }
            let path = if let Some(attempt) = watermark["artifact_attempt"].as_str() {
                config
                    .base
                    .join("processing/attempts")
                    .join(attempt)
                    .join("watermarks/evidence")
                    .join(evidence)
            } else {
                config.base.join("watermarks/evidence").join(evidence)
            };
            if fs::canonicalize(&path)? != path || !fs::metadata(&path)?.is_file() {
                return Err(invalid("文字证据附件无效"));
            }
        }
    }
    let input = data["face"]["unknown_group_input"].clone();
    if let Some(face) = data.get_mut("face").and_then(Value::as_object_mut) {
        face.remove("unknown_group_input");
    }
    let group = if !input.is_null() {
        if s.task.stage != "face"
            || input["version"] != data["face"]["version"]
            || !data["face"]["matches"].as_array().unwrap().is_empty()
        {
            return Err(invalid("未知人脸分组输入不匹配"));
        }
        let vector = input["embedding"]
            .as_array()
            .filter(|v| v.len() == 512)
            .ok_or_else(|| invalid("人脸向量必须为 512 维"))?;
        let mut vector: Vec<f32> = vector
            .iter()
            .map(|v| {
                v.as_f64()
                    .filter(|f| f.is_finite())
                    .map(|f| f as f32)
                    .filter(|f| f.is_finite())
                    .ok_or_else(|| invalid("无效人脸向量"))
            })
            .collect::<Result<_>>()?;
        let norm = vector
            .iter()
            .map(|x| (*x as f64).powi(2))
            .sum::<f64>()
            .sqrt();
        if norm < 1e-9 {
            return Err(invalid("人脸向量为零"));
        }
        for x in &mut vector {
            *x = (*x as f64 / norm) as f32;
        }
        let mut handled = BTreeSet::new();
        for name in ["integration-queue.json", "archive-history.json"] {
            for r in review::read(config, name, json!([]))?.1.as_array().unwrap() {
                if let Some(id) = r["vid"].as_str() {
                    handled.insert(id.to_owned());
                }
            }
        }
        let mut active = BTreeSet::new();
        for row in db::rows(
            conn,
            "SELECT vid,data FROM tasks WHERE stage IN ('review','face') AND manual=0 AND json_extract(data,'$.kind')='none'",
            [],
        )? {
            if !row["vid"].as_str().is_some_and(|id| handled.contains(id))
                && let Some(id) = review::task_data(&row)?["face"]["unknown_group"].as_str()
            {
                active.insert(id.to_owned());
            }
        }
        let mut best = 0.60f64;
        let mut chosen = None;
        for row in db::rows(
            conn,
            "SELECT * FROM face_groups WHERE version=? ORDER BY id",
            [required(&input, "version")?],
        )? {
            if !row["id"].as_str().is_some_and(|id| active.contains(id)) {
                continue;
            }
            let bytes: Vec<u8> = serde_json::from_value(row["embedding"]["$blob"].clone())?;
            if bytes.len() != 2048 {
                return Err(Error::new(
                    503,
                    "analysis_contract",
                    "已保存的人脸分组向量无效",
                ));
            }
            let other: Vec<f32> = bytes
                .chunks_exact(4)
                .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
                .collect();
            if other.iter().any(|x| !x.is_finite()) {
                return Err(Error::new(503, "analysis_contract", "人脸分组向量损坏"));
            }
            let norm = other
                .iter()
                .map(|x| (*x as f64).powi(2))
                .sum::<f64>()
                .sqrt();
            if norm < 1e-9 {
                continue;
            }
            let score = vector
                .iter()
                .zip(&other)
                .map(|(a, b)| *a as f64 * *b as f64)
                .sum::<f64>()
                / norm;
            if score >= best {
                best = score;
                chosen = row["id"].as_str().map(str::to_owned)
            }
        }
        if let Some(id) = chosen {
            data["face"]["unknown_group"] = json!(id);
            None
        } else {
            let id = model::hash(&json!([
                s.task.vid,
                s.task.mtime_ns,
                input["version"],
                s.task.run_id
            ]))[..16]
                .to_owned();
            data["face"]["unknown_group"] = json!(id);
            Some(json!({"id":id,"version":input["version"],"embedding":vector}))
        }
    } else {
        if data["face"].is_object() {
            data["face"]["unknown_group"] = Value::Null;
        }
        None
    };
    if data["face"].is_object() {
        data["face"]["source_identity"] =
            files::identity(&review::safe_source(config, &s.task.path)?);
    }
    Ok((data, group))
}
fn document(config: &Config) -> Result<Value> {
    Ok(review::read(config, "watermarks/results.json", json!({}))?.1)
}
fn patch_document(config: &Config, e: &Evidence, rollback: bool) -> Result<()> {
    let expected = if rollback { &e.after } else { &e.before };
    let target = if rollback { &e.before } else { &e.after };
    if expected == target {
        return Ok(());
    }
    let mut current = document(config)?;
    let actual = current.get(&e.vid).cloned();
    if actual == *target {
        return Ok(());
    }
    if actual != *expected {
        return Err(stale("当前视频文字结果已被外部改变，保留现场"));
    }
    if let Some(value) = target {
        current[&e.vid] = value.clone();
    } else {
        current.as_object_mut().unwrap().remove(&e.vid);
    }
    let path = config.base.join("watermarks/results.json");
    fs::create_dir_all(path.parent().unwrap())?;
    files::atomic_write(&path, serde_json::to_string_pretty(&current)?.as_bytes())
}
fn finish(engine: &mut Engine, plan: &Plan, e: &Evidence) -> Result<Value> {
    let result = receipt(&open(&engine.config)?, &plan.id, Some(&plan.version))?
        .ok_or_else(|| Error::new(503, "outcome_unknown", "分析事务回执尚未确认"))?;
    // No task before/after comparison: a later stage, coordinator restart or human action is valid.
    if e.after.is_some() && document(&engine.config)?.get(&e.vid).cloned() != e.after {
        return Err(Error::new(
            503,
            "recovery_required",
            "已提交分析的文字发布结果被外部改变",
        ));
    }
    crate::fault::checkpoint(&engine.config, "processing_final_receipt_intent")?;
    engine.store.complete(plan, &result)?;
    Ok(result)
}
fn resolve(
    engine: &mut Engine,
    plan: &Plan,
    e: &Evidence,
    gate: WriteGate,
    cause: Option<Error>,
) -> Result<Value> {
    let result = (|| {
        if receipt(&open(&engine.config)?, &plan.id, Some(&plan.version))?.is_some() {
            return finish(engine, plan, e);
        }
        patch_document(&engine.config, e, true)?;
        let failure = Error::new(
            409,
            "publication_aborted",
            format!(
                "分析结果未提交，原文字结果已保留：{}",
                cause
                    .as_ref()
                    .map(|e| e.error.as_str())
                    .unwrap_or("发布进程中断")
            ),
        );
        engine.store.rolled_back(&plan.id, &failure)?;
        Err(failure)
    })();
    match result {
        Ok(value) => {
            gate.release()?;
            Ok(value)
        }
        Err(e)
            if e.status == 409
                && engine
                    .store
                    .get(&plan.id)?
                    .is_some_and(|r| r.status == "rolled_back") =>
        {
            gate.release()?;
            Err(e)
        }
        Err(e) => {
            let failure = Error::new(
                503,
                "recovery_required",
                format!("分析发布需要核对，保留写锁：{}", e.error),
            );
            engine.store.fail(&plan.id, &failure, true)?;
            Err(failure)
        }
    }
}
pub fn recover(engine: &mut Engine, plan: &Plan, gate: WriteGate) -> Result<()> {
    let e: Evidence = serde_json::from_value(
        engine
            .store
            .evidence(&plan.id)?
            .ok_or_else(|| stale("缺少分析发布证据"))?,
    )?;
    match resolve(engine, plan, &e, gate, None) {
        Ok(_) => Ok(()),
        Err(e) if e.code == "publication_aborted" => Ok(()),
        Err(e) => Err(e),
    }
}
pub fn apply(engine: &mut Engine, plan: &Plan, gate: WriteGate) -> Result<Value> {
    let s = submission(plan)?;
    let mut conn = match open(&engine.config) {
        Ok(c) => c,
        Err(e) => {
            gate.release()?;
            return Err(e);
        }
    };
    let tx = match conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate) {
        Ok(tx) => tx,
        Err(e) => {
            gate.release()?;
            return Err(e.into());
        }
    };
    let prepared = (|| -> Result<Evidence> {
        let task = current(&tx, &engine.config, &s)?;
        let (data, group) = prepare_data(&engine.config, &tx, &s, &task)?;
        let before = document(&engine.config)?.get(&s.task.vid).cloned();
        let after = if s.task.stage == "text" {
            Some(data["watermark"].clone())
        } else {
            before.clone()
        };
        Ok(Evidence {
            gate: gate.evidence()?,
            vid: s.task.vid.clone(),
            before,
            after,
            group,
            result: json!({"ok":true,"accepted":true,"id":plan.id,"vid":s.task.vid,"run_id":s.task.run_id,"stage":s.task.stage,"next_stage":s.next_stage,"task":s.task,"source_identity":files::identity(&review::safe_source(&engine.config,&s.task.path)?),"data":data}),
        })
    })();
    let e = match prepared {
        Ok(e) => e,
        Err(error) => {
            drop(tx);
            gate.release()?;
            return Err(error);
        }
    };
    if let Err(error) = engine.store.admit(plan, serde_json::to_value(&e)?) {
        drop(tx);
        let failure = Error::new(
            503,
            "recovery_required",
            format!("分析意图持久化结果未知：{error}"),
        );
        let _ = engine.store.fail(&plan.id, &failure, true);
        return Err(failure);
    }
    let attempted = (|| -> Result<()> {
        crate::fault::checkpoint(&engine.config, "processing_admitted")?;
        engine.store.step(
            &plan.id,
            "running",
            "processing_json_intent",
            json!({"vid":s.task.vid}),
        )?;
        patch_document(&engine.config, &e, false)?;
        crate::fault::checkpoint(&engine.config, "processing_json_published")?;
        // Check actual file again at the database commit boundary, with the lease transaction held.
        current(&tx, &engine.config, &s)?;
        let data = &e.result["data"];
        if let Some(path) = data["face"]["feature_cache"].as_str()
            && data["face"]["feature_cache_sha256"] != artifact_hash(Path::new(path))?
        {
            return Err(stale("发布期间特征缓存已变化"));
        }
        let now: f64 = tx.query_row("SELECT unixepoch('now','subsec')", [], |r| r.get(0))?;
        let execution = if s.task.stage == "face" {
            data["face"]["execution"].clone()
        } else {
            data["watermark"]["execution"].clone()
        };
        let execution = if execution.is_object() {
            execution
        } else {
            json!({"backend":"unknown","cache_reused":true})
        };
        tx.execute(
            "UPDATE tasks SET data=?,stage=?,error=NULL,next_retry=0,updated_at=? WHERE vid=?",
            params![serde_json::to_string(data)?, s.next_stage, now, s.task.vid],
        )?;
        if tx.execute("UPDATE processing_runs SET status='done',error=NULL,finished_at=?,execution=? WHERE id=? AND status='running'",params![now,serde_json::to_string(&execution)?,s.task.run_id])?!=1{return Err(stale("执行回执已结束"))}
        let text = data["text_complete"] == true;
        let skipped = data["face_skipped"] == true;
        let face = data["face_complete"] == true;
        let ready = data["ready"] == true;
        tx.execute("UPDATE batch_items SET text_done=?,face_required=?,face_done=?,face_skipped=?,completed=?,completed_at=CASE WHEN ? THEN ? ELSE NULL END WHERE batch_id=? AND vid=? AND completed=0",params![text,if text{Some(!skipped)}else{None},face,skipped,ready,ready,now,s.task.batch_id,s.task.vid])?;
        tx.execute(
            "INSERT INTO events(at,vid,kind,message) VALUES(?,?,?,?)",
            params![
                now,
                s.task.vid,
                if ready { "ready" } else { "stage_complete" },
                if ready {
                    "分析完成，等待人工确认"
                } else {
                    "文字线索完成，等待人脸分析"
                }
            ],
        )?;
        if let Some(group) = &e.group {
            let vector: Vec<f32> = serde_json::from_value(group["embedding"].clone())?;
            let bytes: Vec<u8> = vector.iter().flat_map(|x| x.to_le_bytes()).collect();
            tx.execute(
                "INSERT INTO face_groups(id,version,embedding) VALUES(?,?,?)",
                params![required(group, "id")?, required(group, "version")?, bytes],
            )?;
        }
        tx.execute_batch("CREATE TABLE IF NOT EXISTS _face_admin_processing_results(id TEXT PRIMARY KEY,signature TEXT NOT NULL,result TEXT NOT NULL)")?;
        tx.execute(
            "INSERT INTO _face_admin_processing_results(id,signature,result) VALUES(?,?,?)",
            params![plan.id, plan.version, serde_json::to_string(&e.result)?],
        )?;
        crate::fault::checkpoint(&engine.config, "processing_db_commit_intent")?;
        tx.commit()?;
        crate::fault::checkpoint(&engine.config, "processing_db_committed")?;
        Ok(())
    })();
    resolve(engine, plan, &e, gate, attempted.err())
}
