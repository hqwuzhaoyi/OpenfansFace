//! Read-only, single-lane inspection. Only the shared metadata journal publishes results.
use crate::{
    config::Config,
    files,
    metadata::Document,
    model::{Error, Result, hash},
    review,
};
use serde_json::{Value, json};
#[cfg(not(test))]
use std::io::Write;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    os::unix::process::CommandExt,
    path::Path,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const HEALTH: &str = "video-health.json";
const REPORTS: &str = "video-health-reports.json";
const DAY: i64 = 86400;
static UNREAPED: AtomicUsize = AtomicUsize::new(0);
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
pub fn clock(config: &Config) -> i64 {
    if config.fixture.is_some()
        && let Ok(value) = std::env::var("VIDEO_HEALTH_FIXTURE_NOW")
        && let Ok(t) = value.parse()
    {
        return t;
    }
    now()
}
pub fn timestamp(config: &Config) -> Result<String> {
    stamp(clock(config))
}
fn stamp(time: i64) -> Result<String> {
    Ok(rusqlite::Connection::open_in_memory()?.query_row(
        "SELECT strftime('%Y-%m-%dT%H:%M:%SZ',?,'unixepoch')",
        [time],
        |r| r.get(0),
    )?)
}
fn epoch(v: &Value) -> i64 {
    v.as_str()
        .and_then(|s| review::timestamp(s).ok())
        .unwrap_or(0.0) as i64
}
pub fn next_run(time: i64) -> i64 {
    (time + 8 * 3600 - 12600).div_euclid(DAY) * DAY + 12600 - 8 * 3600 + DAY
}
fn read(config: &Config, name: &str, default: Value) -> Result<Value> {
    review::read(config, name, default)
        .map(|(_, value)| value)
        .map_err(|error| {
            Error::new(
                503,
                "health_read_failed",
                format!("无法读取 {name}：{}", error.error),
            )
        })
}

fn report(config: &Config) -> Result<Value> {
    let mut v = read(config, HEALTH, json!({"records":{},"status":"never_run"}))?;
    if v.get("records").is_none() {
        v["records"] = json!({})
    }
    if !v["records"].is_object() {
        return Err(Error::new(503, "health_contract", "体检记录损坏，无法读取"));
    }
    Ok(v)
}
fn definitive(v: &Value) -> bool {
    matches!(
        v["status"].as_str(),
        Some("missing" | "not_file" | "empty" | "corrupt")
    )
}
fn same(a: &Value, b: &Value) -> bool {
    ["path", "fingerprint", "material"]
        .iter()
        .all(|k| a[k] == b[k])
        && (!a["fingerprint"].is_null() || a["status"] == b["status"])
}
fn mapped(config: &Config, raw: &str) -> Option<String> {
    let raw = raw.replace('\\', "/");
    if raw.split('/').any(|p| p == "..") {
        return None;
    }
    if raw.starts_with(&format!("{}/", config.media.display())) {
        Some(raw)
    } else {
        raw.split_once("/media/pikpak/")
            .map(|(_, p)| format!("{}/pikpak/{p}", config.media.display()))
    }
}
/// Metadata only: no media stat in the inventory. An explicit current path never falls back to an old ID.
fn inventory(config: &Config) -> Result<BTreeMap<String, Value>> {
    let mut sources = review::sources(config)?;
    let overrides = read(config, "video-path-overrides.json", json!({}))?;
    let history = read(config, "archive-history.json", json!([]))?;
    let origins = read(config, "staging-origins.json", json!({}))?;
    let mut active = BTreeSet::new();
    for name in [
        "report.json",
        "clusters.json",
        "claims-detail.json",
        "integration-queue.json",
    ] {
        let v = read(
            config,
            name,
            if name == "report.json" {
                json!({})
            } else {
                json!([])
            },
        )?;
        fn ids(v: &Value, out: &mut BTreeSet<String>) {
            match v {
                Value::Object(o) => {
                    if let Some(id) = o.get("vid").and_then(Value::as_str) {
                        out.insert(id.into());
                    }
                    for (k, v) in o {
                        if k == "vids" || k == "identities" {
                            ids(v, out)
                        }
                    }
                }
                Value::Array(a) => {
                    for v in a {
                        ids(v, out)
                    }
                }
                _ => (),
            }
        }
        ids(&v, &mut active);
    }
    for id in &active {
        sources.entry(id.clone()).or_insert_with(|| id.clone());
    }
    let mut archive = BTreeMap::new();
    for row in history
        .as_array()
        .ok_or_else(|| Error::conflict("归档历史格式无效"))?
    {
        if let Some(id) = row["vid"].as_str() {
            archive.insert(id.to_string(), row.clone());
            if !sources.contains_key(id)
                && let Some(p) = row["win_dst"].as_str().or(row["dst"].as_str())
            {
                sources.insert(id.into(), p.into());
            }
        }
    }
    let mut out = BTreeMap::new();
    for (id, raw) in sources {
        let path = mapped(config, &raw);
        let h = archive.get(&id).cloned().unwrap_or(Value::Null);
        // Staged duplicate history does not suppress a surviving canonical copy.
        let origin = path
            .as_ref()
            .and_then(|p| origins.get(p))
            .cloned()
            .unwrap_or(Value::Null);
        let archived_path = h["win_dst"]
            .as_str()
            .or(h["dst"].as_str())
            .and_then(|p| mapped(config, p));
        if path
            .as_ref()
            .is_some_and(|p| Path::new(p).starts_with(crate::staging::root(config)))
            || (h["file_state"] == "deleted" && archived_path == path)
        {
            continue;
        }
        if path
            .as_ref()
            .is_some_and(|p| !Path::new(p).starts_with(config.media.join("pikpak")))
        {
            continue;
        }
        let restorations: Vec<_> = origins
            .as_object()
            .ok_or_else(|| Error::conflict("暂存来源格式无效"))?
            .iter()
            .filter(|(_, r)| {
                r["restored_path"].as_str() == path.as_deref() && !r["restored_by"].is_null()
            })
            .map(|(p, r)| json!([p, r["restored_by"]]))
            .collect();
        let state_operation = if h["file_state"].is_null() {
            Value::Null
        } else {
            h["last_staging_operation"].clone()
        };
        let material = hash(
            &json!({"vid":id,"stored":raw,"override":overrides[&id],"path":path,"file_state":h["file_state"],"archive_path":archived_path,"operation":state_operation,"origin":origin,"restorations":restorations}),
        );
        out.insert(
            id.clone(),
            json!({"vid":id,"path":path,"material":material,"restorations":restorations,"in_review":active.contains(&id)}),
        );
    }
    Ok(out)
}
fn observe(mut item: Value) -> Value {
    item["size"] = Value::Null;
    item["mtime_ns"] = Value::Null;
    item["fingerprint"] = Value::Null;
    let Some(path) = item["path"].as_str().map(str::to_owned) else {
        item["status"] = json!("unmapped");
        return item;
    };
    match fs::symlink_metadata(&path) {
        Ok(s) => {
            item["fingerprint"] = json!(files::fingerprint(&path, &s));
            if s.file_type().is_symlink()
                || fs::canonicalize(&path).ok().as_deref() != Some(Path::new(&path))
            {
                item["status"] = json!("unavailable");
                item["detail"] = json!("路径包含符号链接或暂时不可读取");
            } else if !s.is_file() {
                item["status"] = json!("not_file")
            } else {
                item["status"] = json!(if s.len() == 0 { "empty" } else { "present" });
                item["size"] = json!(s.len());
                item["mtime_ns"] = json!(files::mtime_ns(&s));
            }
        }
        Err(e) => {
            item["status"] = json!(if matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) {
                "missing"
            } else {
                "unavailable"
            });
            item["detail"] = json!(e.to_string());
        }
    };
    item
}
fn transition(old: &Value, mut item: Value, time: i64) -> Result<Value> {
    item["last_checked_at"] = json!(stamp(time)?);
    item["hidden"] = json!(false);
    item["consecutive"] = json!(0);
    if !definitive(&item) {
        return Ok(item);
    }
    let unchanged = same(old, &item) && old["status"] == item["status"];
    let first = if unchanged {
        epoch(&old["first_failure_at"])
    } else {
        time
    };
    let mut counted = if unchanged {
        epoch(&old["last_counted_at"])
    } else {
        time
    };
    let mut count = if unchanged {
        old["consecutive"].as_u64().unwrap_or(0)
    } else {
        1
    };
    if unchanged && time - counted >= 20 * 3600 {
        count += 1;
        counted = time
    }
    item["consecutive"] = json!(count);
    item["first_failure_at"] = json!(stamp(first)?);
    item["last_counted_at"] = json!(stamp(counted)?);
    item["hidden"] = json!(count >= 3 && time - first >= 40 * 3600);
    Ok(item)
}

/// Redirect bounded output to local files: no pipe deadlock or detached blocking reader threads.
fn subprocess(
    config: &Config,
    program: &str,
    args: &[&str],
    timeout: Duration,
    stop: &AtomicBool,
) -> Result<(bool, String, String)> {
    if UNREAPED.load(Ordering::SeqCst) > 0 {
        return Err(Error::new(
            503,
            "probe_io_pending",
            "上次探测底层 I/O 尚未退出，暂停新探测",
        ));
    }
    let output_limit = if args.first() == Some(&"--health-observe") {
        64 * 1024 * 1024
    } else {
        128 * 1024
    };
    let prefix = config
        .state_dir()
        .join(format!("probe-{}", uuid::Uuid::new_v4()));
    let stdout = prefix.with_extension("out");
    let stderr = prefix.with_extension("err");
    let result = (|| {
        let output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&stdout)?;
        let error = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&stderr)?;
        let mut command = Command::new(program);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(output)
            .stderr(error)
            .process_group(0);
        // A killed Linux service must not leave its probe reading the NAS after restart.
        #[cfg(target_os = "linux")]
        unsafe {
            command.pre_exec(|| {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::getppid() == 1 {
                    return Err(std::io::Error::other("health parent exited"));
                }
                libc::nice(10);
                Ok(())
            });
        }
        let mut child = command.spawn()?;
        let deadline = Instant::now() + timeout;
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            let oversized = fs::metadata(&stdout)?.len() > output_limit
                || fs::metadata(&stderr)?.len() > output_limit;
            if Instant::now() >= deadline || stop.load(Ordering::Relaxed) || oversized {
                if let Some(pid) = rustix::process::Pid::from_raw(child.id() as i32) {
                    let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
                }
                let _ = child.kill();
                // Keep ownership until wait proves exit. An uninterruptible kernel I/O retains this one lane.
                let reap_deadline = Instant::now() + Duration::from_millis(500);
                loop {
                    if child.try_wait()?.is_some() {
                        break;
                    }
                    if Instant::now() >= reap_deadline {
                        UNREAPED.fetch_add(1, Ordering::SeqCst);
                        std::thread::spawn(move || {
                            let _ = child.wait();
                            UNREAPED.fetch_sub(1, Ordering::SeqCst);
                        });
                        return Err(Error::new(
                            503,
                            "probe_io_pending",
                            "已请求终止，但底层 I/O 尚未退出；暂停新探测，保留此前结果",
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
                return Err(Error::new(
                    503,
                    "probe_unavailable",
                    if oversized {
                        "探测输出超过上限"
                    } else {
                        "探测超时或已停止；子进程已回收"
                    },
                ));
            }
            std::thread::sleep(Duration::from_millis(25));
        };
        let bounded = |path: &Path| -> Result<String> {
            let mut buf = Vec::new();
            fs::File::open(path)?
                .take(output_limit)
                .read_to_end(&mut buf)?;
            Ok(String::from_utf8_lossy(&buf).into_owned())
        };
        Ok((status.success(), bounded(&stdout)?, bounded(&stderr)?))
    })();
    let _ = fs::remove_file(stdout);
    let _ = fs::remove_file(stderr);
    result
}
fn classify(ok: bool, stdout: &str, stderr: &str) -> Value {
    let error = stderr.to_lowercase();
    // I/O diagnostics take precedence even when ffprobe also prints a structural error.
    let io = [
        "input/output",
        "permission denied",
        "operation not permitted",
        "timed out",
        "network",
        "stale file",
        "resource temporarily",
        "connection",
        "no such file",
        "interrupted",
    ]
    .iter()
    .any(|s| error.contains(s));
    if !ok || io {
        let corrupt = !io
            && [
                "moov atom not found",
                "invalid data found when processing input",
                "end of file",
            ]
            .iter()
            .any(|s| error.contains(s));
        return json!({"status":if corrupt{"corrupt"}else{"unavailable"},"detail":stderr.chars().take(300).collect::<String>()});
    }
    let Ok(data) = serde_json::from_str::<Value>(stdout) else {
        return json!({"status":"unavailable","detail":"探测器未返回有效 JSON"});
    };
    let Some(streams) = data["streams"].as_array() else {
        return json!({"status":"unavailable","detail":"探测器输出缺少轨道信息"});
    };
    let Some(video) = streams.iter().find(|s| s["codec_type"] == "video") else {
        return json!({"status":"corrupt","detail":"文件中没有可识别的视频轨道"});
    };
    let codec = video["codec_name"].as_str().unwrap_or("unknown");
    let pixel = video["pix_fmt"].as_str().unwrap_or("");
    let compatible = ["h264", "hevc", "vp8", "vp9", "av1"].contains(&codec)
        && (codec != "h264" || ["yuv420p", "yuvj420p", "nv12"].contains(&pixel));
    json!({"status":if compatible{"healthy"}else{"compatibility"},"codec":codec,"pixel_format":pixel,"detail":if compatible{"视频结构检查通过"}else{"视频结构正常，编码兼容性需核对"}})
}
/// Helper entry point runs before HTTP/runtime startup; its parent owns timeout and reaping.
pub fn helper(input: &Path) -> Result<()> {
    let value: Value = serde_json::from_str(
        &crate::control::read(input, 64 * 1024 * 1024)?
            .ok_or_else(|| Error::conflict("缺少观察输入"))?,
    )?;
    if value["mount"] == true {
        let root = Path::new(
            value["root"]
                .as_str()
                .ok_or_else(|| Error::conflict("缺少媒体目录"))?,
        );
        if value["fixture"] != true {
            let expected = value["source"].as_str().unwrap_or("");
            if expected.is_empty() {
                return Err(Error::new(
                    503,
                    "mount_unavailable",
                    "未配置 MEDIA_NFS_SOURCE，本轮不收起记录",
                ));
            }
            let output = Command::new("findmnt")
                .args(["-n", "-T", root.to_str().unwrap(), "-o", "FSTYPE,SOURCE"])
                .output()?;
            let text = String::from_utf8_lossy(&output.stdout);
            let fields: Vec<_> = text.split_whitespace().collect();
            if !output.status.success()
                || fields.len() != 2
                || !["nfs", "nfs4"].contains(&fields[0])
                || fields[1] != expected
            {
                return Err(Error::new(
                    503,
                    "mount_unavailable",
                    "网络盘挂载与预期不符，本轮不收起记录",
                ));
            }
        }
        for dir in [
            root.join("pikpak/creators"),
            root.join("pikpak/My Telegram"),
        ] {
            if !dir.is_dir() || fs::read_dir(dir)?.next().transpose()?.is_none() {
                return Err(Error::new(
                    503,
                    "mount_unavailable",
                    "媒体目录不可用或为空，本轮不收起记录",
                ));
            }
        }
    }
    let input: BTreeMap<String, Value> = serde_json::from_value(value["items"].clone())?;
    let output: BTreeMap<_, _> = input
        .into_iter()
        .map(|(id, item)| (id, observe(item)))
        .collect();
    println!("{}", serde_json::to_string(&output)?);
    Ok(())
}
fn observations(
    config: &Config,
    items: BTreeMap<String, Value>,
    mount: bool,
    timeout: Duration,
    stop: &AtomicBool,
) -> Result<BTreeMap<String, Value>> {
    if items.is_empty() && !mount {
        return Ok(items);
    }
    #[cfg(test)]
    {
        let _ = (config, mount, timeout, stop);
        Ok(items.into_iter().map(|(id, v)| (id, observe(v))).collect())
    }
    #[cfg(not(test))]
    {
        let input = config
            .state_dir()
            .join(format!("observe-{}.json", uuid::Uuid::new_v4()));
        let value = json!({"items":items,"mount":mount,"fixture":config.fixture.is_some(),"root":config.media,"source":std::env::var("MEDIA_NFS_SOURCE").unwrap_or_default()});
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&input)?
            .write_all(serde_json::to_string(&value)?.as_bytes())?;
        let result = (|| {
            let executable = std::env::current_exe()?;
            let (ok, out, err) = subprocess(
                config,
                executable.to_str().unwrap(),
                &["--health-observe", input.to_str().unwrap()],
                timeout,
                stop,
            )?;
            if !ok {
                return Err(Error::new(
                    503,
                    "observation_unavailable",
                    err.chars().take(500).collect::<String>(),
                ));
            }
            Ok(serde_json::from_str(&out)?)
        })();
        let _ = fs::remove_file(input);
        result
    }
}
fn observe_one(config: &Config, item: Value) -> Result<Value> {
    let id = item["vid"].as_str().unwrap_or("").to_string();
    observations(
        config,
        BTreeMap::from([(id.clone(), item)]),
        false,
        Duration::from_secs(5),
        &AtomicBool::new(false),
    )?
    .remove(&id)
    .ok_or_else(|| Error::conflict("观察输出不完整"))
}

fn anomaly(records: &serde_json::Map<String, Value>) -> bool {
    records.values().filter(|r| definitive(r)).count() as f64
        > 50.0_f64.max(records.len() as f64 * 0.2)
}
fn report_matches(r: &Value, item: &Value) -> bool {
    r["fingerprint"] == item["fingerprint"]
        && !r["fingerprint"].is_null()
        && r["path"] == item["path"]
        && r["material"] == item["material"]
}
fn scan(
    config: &Config,
    time: i64,
    budget: usize,
    seconds: u64,
    stop: &AtomicBool,
) -> Result<Value> {
    let deadline = Instant::now() + Duration::from_secs(seconds);

    let previous = report(config)?;
    let reports = read(config, REPORTS, json!({}))?;
    let mut records = serde_json::Map::new();
    let mut candidates = vec![];
    for (id, item) in observations(
        config,
        inventory(config)?,
        true,
        Duration::from_secs(10),
        stop,
    )? {
        if stop.load(Ordering::Relaxed) || Instant::now() >= deadline {
            return Err(Error::new(
                503,
                "scan_interrupted",
                "检查预算耗尽，保留上次完整结果",
            ));
        }
        let old = &previous["records"][&id];
        if item["status"] == "present" {
            let fresh = report_matches(&reports[&id], &item)
                && (epoch(&reports[&id]["last_reported_at"]) > epoch(&old["last_probed_at"])
                    || reports[&id]["count"].as_u64().unwrap_or(0)
                        > old["last_report_count"].as_u64().unwrap_or(0));
            let due = !same(old, &item)
                || definitive(old)
                || old["status"] == "unavailable"
                || time - epoch(&old["last_probed_at"]) >= 30 * DAY
                || fresh;
            if due {
                candidates.push((
                    if fresh {
                        0
                    } else if definitive(old) {
                        1
                    } else if item["in_review"] == true {
                        2
                    } else {
                        3
                    },
                    epoch(&old["last_probed_at"]),
                    id.clone(),
                ));
            }
            let value = if same(old, &item) && old["last_probed_at"].is_string() {
                old.clone()
            } else {
                let mut v = item;
                v["status"] = json!("unchecked");
                v["hidden"] = json!(false);
                v
            };
            records.insert(id, value);
        } else {
            records.insert(id, transition(old, item, time)?);
        }
    }
    if anomaly(&records) {
        return Err(Error::new(
            503,
            "mass_anomaly",
            "确定异常数量超过 max(50, 20%)，保留上次完整结果",
        ));
    }
    candidates.sort();
    let waiting = candidates.len();
    let mut probed = 0;
    for (_, _, id) in candidates.into_iter().take(budget) {
        if stop.load(Ordering::Relaxed) {
            return Err(Error::new(
                503,
                "scan_interrupted",
                "检查已停止，保留上次完整结果",
            ));
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        let mut item = records[&id].clone();
        let path = item["path"].as_str().unwrap();
        let program = if config.fixture.is_some() {
            std::env::var("VIDEO_HEALTH_FIXTURE_PROBE").unwrap_or("ffprobe".into())
        } else {
            "ffprobe".into()
        };
        let outcome = match subprocess(
            config,
            &program,
            &[
                "-v",
                "error",
                "-threads",
                "1",
                "-max_alloc",
                "67108864",
                "-probesize",
                "5000000",
                "-analyzeduration",
                "5000000",
                "-show_entries",
                "stream=codec_type,codec_name,pix_fmt:format=format_name,duration",
                "-of",
                "json",
                path,
            ],
            remaining.min(Duration::from_secs(20)),
            stop,
        ) {
            Ok((ok, out, err)) => classify(ok, &out, &err),
            Err(e) => json!({"status":"unavailable","detail":e.error}),
        };
        item.as_object_mut()
            .unwrap()
            .extend(outcome.as_object().unwrap().clone());
        item["last_probed_at"] = json!(stamp(time)?);
        item["last_report_count"] = json!(if report_matches(&reports[&id], &item) {
            reports[&id]["count"].as_u64().unwrap_or(0)
        } else {
            0
        });
        if item["status"] == "healthy"
            && report_matches(&reports[&id], &item)
            && time - epoch(&reports[&id]["last_reported_at"]) < 30 * DAY
        {
            item["status"] = json!("playback_warning");
            item["detail"] = json!("曾报告播放失败，本次结构检查通过；需核对网络或编码兼容性")
        }
        records.insert(
            id.clone(),
            transition(&previous["records"][&id], item, time)?,
        );
        probed += 1;
    }
    if anomaly(&records) {
        return Err(Error::new(
            503,
            "mass_anomaly",
            "探测后确定异常数量超过 max(50, 20%)，保留上次完整结果",
        ));
    }
    if stop.load(Ordering::Relaxed) {
        return Err(Error::new(
            503,
            "scan_interrupted",
            "体检已停止，保留此前完整结果",
        ));
    }
    Ok(json!({"records":records,"probed":probed,"waiting":waiting-probed,"time":time}))
}

/// Closed health mutations admitted and revalidated by shared_metadata's engine/gate/journal.
pub fn prepare(
    config: &Config,
    command: &str,
    body: &Value,
    at: &str,
) -> Result<(Vec<Document>, Value, Value)> {
    let mut value = report(config)?;
    let mut guards = json!({});
    if command == "health-report" {
        let id = body["vid"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| Error::new(400, "invalid_request", "缺少视频编号"))?;
        let raw = inventory(config)?
            .remove(id)
            .ok_or_else(|| Error::new(404, "missing_video", "视频不存在或已移至回收站"))?;
        let current = observe_one(config, raw)?;
        if current["status"] != "present" {
            return Err(Error::conflict("当前文件无法核对，请刷新预览"));
        }
        if let Some(path) = body["path"].as_str()
            && mapped(config, path).as_deref() != current["path"].as_str()
        {
            return Err(Error::conflict("预览路径与当前视频不一致"));
        }
        let fp = body["fingerprint"].as_str().ok_or_else(|| {
            Error::new(
                400,
                "invalid_request",
                "缺少播放时的文件指纹，请重新打开预览",
            )
        })?;
        if current["fingerprint"] != fp || body["material"] != current["material"] {
            return Err(Error::conflict("视频已替换，请刷新预览"));
        }
        let mut reports = read(config, REPORTS, json!({}))?;
        let previous = &reports[id];
        let count = if report_matches(previous, &current) {
            previous["count"].as_u64().unwrap_or(0) + 1
        } else {
            1
        };
        let code = body["code"].as_u64().filter(|v| (1..=4).contains(v));
        reports[id] = json!({"vid":id,"path":current["path"],"fingerprint":current["fingerprint"],"material":current["material"],"last_reported_at":at,"count":count,"media_error_code":code});
        if reports.as_object().unwrap().len() > 1000 {
            let mut ordered: Vec<_> = reports
                .as_object()
                .unwrap()
                .iter()
                .map(|(id, row)| (epoch(&row["last_reported_at"]), id.clone()))
                .collect();
            ordered.sort();
            let count = ordered.len() - 1000;
            for (_, id) in ordered.into_iter().take(count) {
                reports.as_object_mut().unwrap().remove(&id);
            }
        }
        guards = current;
        return Ok((
            vec![review::document(config, REPORTS, json!({}), reports)?],
            guards,
            json!({"ok":true}),
        ));
    }
    match command {
        "health-start" => {
            value["status"] = json!("running");
            value["run_id"] = body["run_id"].clone();
            value["started_at"] = json!(at);
            value["completed_at"] = Value::Null;
            value.as_object_mut().unwrap().remove("error");
        }
        "health-interrupted" => {
            if value["status"] != "running" {
                return Ok((vec![], guards, json!({"ok":true})));
            }
            value["status"] = json!("interrupted");
            value["error"] = json!("服务重启，上轮检查未发布完整结果；保留此前结果");
            value["completed_at"] = json!(at);
        }
        "health-finish" => {
            if value["run_id"] != body["run_id"] || value["status"] != "running" {
                return Err(Error::conflict("体检运行代次已变化"));
            }
            if let Some(error) = body["error"].as_str() {
                value["status"] = json!("skipped");
                value["error"] = json!(error.chars().take(500).collect::<String>());
            } else {
                let inventory = observations(
                    config,
                    inventory(config)?,
                    true,
                    Duration::from_secs(10),
                    &AtomicBool::new(false),
                )?;
                let reports = read(config, REPORTS, json!({}))?;
                let incoming = body["result"]["records"]
                    .as_object()
                    .ok_or_else(|| Error::conflict("体检输出无效"))?;
                let mut accepted = serde_json::Map::new();
                let mut invalidated = 0;
                for (id, item) in incoming {
                    let Some(raw) = inventory.get(id) else {
                        invalidated += 1;
                        continue;
                    };
                    let current = raw.clone();
                    if !same(item, &current) {
                        invalidated += 1;
                        continue;
                    }
                    let mut row = item.clone();
                    if row["status"] == "healthy"
                        && report_matches(&reports[id], &current)
                        && epoch(&json!(at)) - epoch(&reports[id]["last_reported_at"]) < 30 * DAY
                    {
                        row["status"] = json!("playback_warning");
                        row["hidden"] = json!(false);
                        row["detail"] =
                            json!("曾报告播放失败，本次结构检查通过；需核对网络或编码兼容性")
                    }
                    guards[id] = current;
                    accepted.insert(id.clone(), row);
                }
                // Preserve concurrent new-generation results, but never restore the scanner's old snapshot.
                for (id, old) in value["records"].as_object().unwrap() {
                    if !accepted.contains_key(id) && inventory.contains_key(id) {
                        let current = inventory[id].clone();
                        if same(old, &current) {
                            accepted.insert(id.clone(), old.clone());
                        }
                    }
                }
                if anomaly(&accepted) {
                    return Err(Error::conflict("发布时异常数量超过保护阈值"));
                }
                let mut counts = BTreeMap::new();
                for row in accepted.values() {
                    *counts
                        .entry(row["status"].as_str().unwrap_or("unknown").to_string())
                        .or_insert(0usize) += 1;
                }
                value["hidden"] = json!(accepted.values().filter(|r| r["hidden"] == true).count());
                value["total"] = json!(accepted.len());
                value["counts"] = json!(counts);
                value["records"] = json!(accepted);
                value["probed"] = body["result"]["probed"].clone();
                value["waiting"] =
                    json!(body["result"]["waiting"].as_u64().unwrap_or(0) + invalidated);
                value["status"] = json!("complete");
                value["last_success_at"] = json!(at);
                value.as_object_mut().unwrap().remove("error");
            }
            value["completed_at"] = json!(at);
        }
        _ => return Err(Error::new(400, "invalid_request", "未知体检命令")),
    }
    value["schema"] = json!(2);
    value["schedule"] = json!("每天 03:30（北京时间）");
    value["next_run_at"] = json!(stamp(next_run(epoch(&json!(at))))?);
    value["policy"] =
        json!({"checks":3,"minimum_hours":40,"minimum_gap_hours":20,"delete_files":false});
    Ok((
        vec![review::document(config, HEALTH, json!({}), value)?],
        guards,
        json!({"ok":true}),
    ))
}

pub fn effective_hidden(
    config: &Config,
    vid: &str,
    path: &str,
    stat: &fs::Metadata,
) -> Result<bool> {
    let report = report(config)?;
    let row = &report["records"][vid];
    let fresh = clock(config) - epoch(&report["last_success_at"]);
    if !(0..7 * DAY).contains(&fresh)
        || row["hidden"] != true
        || row["path"] != path
        || row["fingerprint"] != files::fingerprint(path, stat)
    {
        return Ok(false);
    }
    Ok(inventory(config)?
        .get(vid)
        .is_some_and(|current| current["material"] == row["material"]))
}
pub fn summary(config: &Config) -> Result<Value> {
    let mut value = report(config)?;
    let mut current = inventory(config)?;
    current.retain(|id, _| {
        value["records"].get(id).is_some_and(|r| {
            !["healthy", "unchecked", "present"].contains(&r["status"].as_str().unwrap_or(""))
        })
    });
    let current = observations(
        config,
        current,
        false,
        Duration::from_secs(5),
        &AtomicBool::new(false),
    )?;
    let age = clock(config) - epoch(&value["last_success_at"]);
    let fresh = (0..7 * DAY).contains(&age);
    let mut rows = vec![];
    for (id, row) in value["records"].as_object().unwrap() {
        if ["healthy", "unchecked", "present"].contains(&row["status"].as_str().unwrap_or("")) {
            continue;
        }
        let Some(raw) = current.get(id) else { continue };
        let observed = raw.clone();
        let mut row = row.clone();
        if !same(&row, &observed) {
            row = observed;
            row["status"] = json!("unchecked");
            row["detail"] = json!("路径或文件已变化，等待下次复查");
            row["hidden"] = json!(false)
        } else if !fresh {
            row["hidden"] = json!(false)
        }
        rows.push(row);
    }
    rows.sort_by_key(|r| {
        (
            !r["hidden"].as_bool().unwrap_or(false),
            r["status"].as_str().unwrap_or("").to_string(),
            r["path"].as_str().unwrap_or("").to_string(),
        )
    });
    value.as_object_mut().unwrap().remove("records");
    value["hidden"] = json!(rows.iter().filter(|r| r["hidden"] == true).count());
    value["rows"] = json!(rows);
    if !fresh && value["last_success_at"].is_string() {
        let stale = "检查结果超过 7 天，暂停自动收起，等待下一次成功检查。";
        value["error"] = json!(match value["error"].as_str().filter(|s| !s.is_empty()) {
            Some(error) => format!("{error}；{stale}"),
            None => stale.into(),
        });
    }
    if value["next_run_at"].is_null() {
        value["next_run_at"] = json!(stamp(next_run(clock(config)))?)
    }
    Ok(value)
}

fn dispatch(
    worker: &crate::admission::AcceptedPublisher,
    command: &str,
    body: Value,
) -> Result<Value> {
    let (reply, wait) = tokio::sync::oneshot::channel();
    worker
        .send(crate::Work {
            command: crate::Command::Metadata {
                command: command.into(),
                body,
            },
            reply,
        })
        .map_err(|_| Error::new(503, "unavailable", "执行器不可用"))?;
    wait.blocking_recv()
        .map_err(|_| Error::new(503, "outcome_unknown", "体检发布回执未知"))?
}
fn configured(key: &str, default: u64, max: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
        .min(max)
}
fn scheduled(config: Config, worker: crate::admission::Worker, stop: Arc<AtomicBool>) {
    let budget = configured("VIDEO_HEALTH_BUDGET", 300, 1000) as usize;
    let seconds = configured("VIDEO_HEALTH_SECONDS", 1200, 1200);
    let mut next = report(&config)
        .ok()
        .map(|r| epoch(&r["next_run_at"]))
        .filter(|t| *t > 0)
        .unwrap_or_else(|| next_run(clock(&config)));
    if config.fixture.is_some()
        && std::env::var("VIDEO_HEALTH_FIXTURE_RUN_ON_START").as_deref() == Ok("1")
    {
        next = clock(&config);
    }
    while !stop.load(Ordering::Relaxed) {
        if clock(&config) < next {
            std::thread::sleep(Duration::from_millis(200));
            continue;
        }
        let cycle = match worker.begin_publisher() {
            Ok(cycle) => cycle,
            Err(_) => {
                next = clock(&config) + 60;
                continue;
            }
        };
        let run_id = uuid::Uuid::new_v4().to_string();
        if let Err(error) = dispatch(&cycle, "health-start", json!({"run_id":run_id})) {
            eprintln!("health start: {}", error.error);
            next = clock(&config) + 60;
            continue;
        }
        let body = match scan(&config, clock(&config), budget, seconds, &stop) {
            Ok(result) => json!({"run_id":run_id,"result":result}),
            Err(error) => json!({"run_id":run_id,"error":error.error}),
        };
        if let Err(error) = dispatch(&cycle, "health-finish", body) {
            eprintln!("health publish: {}", error.error);
            let _ = dispatch(
                &cycle,
                "health-finish",
                json!({"run_id":run_id,"error":format!("发布前资料变化或发布失败：{}",error.error)}),
            );
        }
        next = next_run(clock(&config));
    }
}
pub fn spawn(
    config: Config,
    worker: crate::admission::Worker,
    stop: Arc<AtomicBool>,
) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("video-health-lane".into())
        .spawn(move || scheduled(config, worker, stop))
        .expect("start health lane")
}

pub fn playback_guard(config: &Config, vid: &str, expected_path: Option<&str>) -> Result<Value> {
    let item = inventory(config)?
        .remove(vid)
        .ok_or_else(|| Error::new(404, "missing_video", "视频不存在或已移至回收站"))?;
    let item = observe_one(config, item)?;
    if expected_path.is_some_and(|p| mapped(config, p).as_deref() != item["path"].as_str()) {
        return Err(Error::conflict("预览路径已变化，请重新打开视频"));
    }
    if item["status"] != "present" {
        return Err(Error::conflict("文件暂时无法读取"));
    }
    Ok(
        json!({"vid":vid,"path":item["path"],"fingerprint":item["fingerprint"],"material":item["material"]}),
    )
}

pub fn needs_recovery(config: &Config) -> Result<bool> {
    Ok(report(config)?["status"] == "running")
}

/// Person-workbench evidence reads only the requested registered/active IDs. Reuse the
/// health inventory's authoritative current path and restoration generation, with one
/// metadata snapshot for the entire bounded batch. This never probes or publishes health.
pub fn library_observations(config: &Config, ids: &[String]) -> Result<Value> {
    let current = inventory(config)?;
    let mut assigned_paths = BTreeSet::new();
    for document in ["integration-queue.json", "archive-history.json"] {
        let rows = read(config, document, json!([]))?;
        for row in rows
            .as_array()
            .ok_or_else(|| Error::conflict("归名记录格式无效"))?
        {
            if let Some(path) = row["vid"]
                .as_str()
                .and_then(|id| current.get(id))
                .and_then(|item| item["path"].as_str())
            {
                assigned_paths.insert(path.to_owned());
            }
        }
    }
    let report = report(config)?;
    let age = clock(config) - epoch(&report["last_success_at"]);
    let fresh = (0..7 * DAY).contains(&age);
    let mut rows = serde_json::Map::new();
    for id in ids {
        let Some(item) = current.get(id) else {
            rows.insert(
                id.clone(),
                json!({"status":"missing","eligible":false,"path":null}),
            );
            continue;
        };
        let Some(path) = item["path"].as_str() else {
            rows.insert(
                id.clone(),
                json!({"status":"unmapped","eligible":false,"path":null}),
            );
            continue;
        };
        let stat = match files::regular_stat(path) {
            Ok(stat) => stat,
            Err(error) if error.code == "source_changed" => {
                rows.insert(
                    id.clone(),
                    json!({"status":"missing","eligible":false,"path":path}),
                );
                continue;
            }
            Err(error) => return Err(error.at(path).during("library_observation")),
        };
        let fingerprint = files::fingerprint(path, &stat);
        let saved = &report["records"][id];
        let hidden = fresh
            && saved["hidden"] == true
            && saved["path"] == path
            && saved["fingerprint"] == fingerprint
            && saved["material"] == item["material"];
        let eligible = !Path::new(path).starts_with(config.creators())
            && !Path::new(path).starts_with(crate::staging::root(config))
            && !hidden;
        use std::os::unix::fs::MetadataExt;
        rows.insert(id.clone(), json!({"status":"present","eligible":eligible,"hidden":hidden,"assigned":assigned_paths.contains(path),"path":path,"size":stat.len(),"mtime_ns":files::mtime_ns(&stat),"modified_ms":stat.mtime() as f64*1000.0+stat.mtime_nsec() as f64/1000000.0,"version":fingerprint}));
    }
    Ok(json!({"observations":rows}))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Config {
        let root = std::env::temp_dir().join(format!(
            "face-admin-person-fileops-health-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(root.join("base/file-operations-rust")).unwrap();
        fs::write(root.join(".fileops-fixture"), "person file operations").unwrap();
        fs::create_dir_all(root.join("media/pikpak/creators/a")).unwrap();
        fs::create_dir_all(root.join("media/pikpak/My Telegram")).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let config = Config {
            base: root.join("base"),
            media: root.join("media"),
            ledger: root.join("ledger.db"),
            bind: "127.0.0.1:0".into(),
            token: None,
            parity_root: None,
            fixture: Some(root),
        };
        let db = rusqlite::Connection::open(config.base.join("faces.db")).unwrap();
        db.execute_batch("CREATE TABLE videos(vid TEXT,path TEXT)")
            .unwrap();
        for id in ["a", "b"] {
            let p = config.creators().join("a").join(format!("{id}.mp4"));
            fs::write(&p, b"video").unwrap();
            db.execute("INSERT INTO videos VALUES(?,?)", [id, p.to_str().unwrap()])
                .unwrap();
        }
        config
    }
    fn write(config: &Config, name: &str, value: Value) {
        fs::write(
            config.base.join(name),
            serde_json::to_string(&value).unwrap(),
        )
        .unwrap()
    }
    fn publish(config: &Config, command: &str, body: Value, time: i64) -> Value {
        let (docs, _, result) = prepare(config, command, &body, &stamp(time).unwrap()).unwrap();
        for doc in docs {
            fs::write(config.base.join(doc.name), doc.after.unwrap()).unwrap();
        }
        result
    }
    fn observed(config: &Config, id: &str, status: &str) -> Value {
        let mut v = observe(inventory(config).unwrap()[id].clone());
        v["status"] = json!(status);
        v
    }
    #[test]
    fn confirmation_spacing_span_and_generation() {
        let observation =
            json!({"path":"/media/a","fingerprint":"one","material":"same","status":"corrupt"});
        let t = 1_800_000_000;
        let first = transition(&json!({}), observation.clone(), t).unwrap();
        let early = transition(&first, observation.clone(), t + 20 * 3600 - 1).unwrap();
        assert_eq!(early["consecutive"], 1);
        let second = transition(&early, observation.clone(), t + 20 * 3600).unwrap();
        assert_eq!(second["consecutive"], 2);
        assert_eq!(second["hidden"], false);
        let third = transition(&second, observation.clone(), t + 40 * 3600).unwrap();
        assert_eq!(third["hidden"], true);
        let mut changed = observation.clone();
        changed["fingerprint"] = json!("new");
        assert_eq!(
            transition(&third, changed, t + 60 * 3600).unwrap()["consecutive"],
            1
        );
        for status in [
            "unavailable",
            "healthy",
            "compatibility",
            "playback_warning",
        ] {
            let mut v = observation.clone();
            v["status"] = json!(status);
            assert_eq!(
                transition(&third, v, t + 60 * 3600).unwrap()["hidden"],
                false
            )
        }
        let mut missing = observation;
        missing["fingerprint"] = Value::Null;
        missing["status"] = json!("missing");
        let mut io = missing.clone();
        io["status"] = json!("unavailable");
        assert!(!same(&missing, &io));
    }
    #[test]
    fn probe_classification_preserves_uncertainty() {
        assert_eq!(
            classify(false, "{}", "moov atom not found")["status"],
            "corrupt"
        );
        for error in [
            "Input/output error: moov atom not found",
            "Permission denied: Invalid data found when processing input",
            "Network is unreachable",
            "Connection timed out",
            "Stale file handle",
        ] {
            assert_eq!(classify(false, "{}", error)["status"], "unavailable")
        }
        assert_eq!(classify(true, "not JSON", "")["status"], "unavailable");
        assert_eq!(classify(true, r#"{"streams":[]}"#, "")["status"], "corrupt");
        assert_eq!(
            classify(
                true,
                r#"{"streams":[{"codec_type":"video","codec_name":"wmv3","pix_fmt":"yuv420p"}]}"#,
                ""
            )["status"],
            "compatibility"
        );
        assert_eq!(
            classify(
                true,
                r#"{"streams":[{"codec_type":"video","codec_name":"h264","pix_fmt":"yuv420p"}]}"#,
                ""
            )["status"],
            "healthy"
        );
    }
    #[test]
    fn schedule_and_mass_guard_exact_boundaries() {
        let t = review::timestamp("2026-11-08T19:29:59Z").unwrap() as i64;
        assert_eq!(stamp(next_run(t)).unwrap(), "2026-11-08T19:30:00Z");
        assert_eq!(stamp(next_run(t + 1)).unwrap(), "2026-11-09T19:30:00Z");
        for (total, bad, expected) in [
            (100, 50, false),
            (100, 51, true),
            (500, 100, false),
            (500, 101, true),
        ] {
            let records = (0..total)
                .map(|n| {
                    (
                        n.to_string(),
                        json!({"status":if n<bad{"empty"}else{"healthy"}}),
                    )
                })
                .collect();
            assert_eq!(anomaly(&records), expected)
        }
    }
    #[test]
    fn publication_drops_changed_moved_and_staged_never_resurrects_snapshot() {
        let c = fixture();
        let time = now();
        let a = observed(&c, "a", "corrupt");
        let b = observed(&c, "b", "healthy");
        publish(&c, "health-start", json!({"run_id":"one"}), time);
        fs::write(a["path"].as_str().unwrap(), b"replacement generation").unwrap();
        let stage = crate::staging::root(&c).join("b.mp4");
        fs::create_dir_all(stage.parent().unwrap()).unwrap();
        fs::rename(b["path"].as_str().unwrap(), &stage).unwrap();
        write(&c, "video-path-overrides.json", json!({"b":stage}));
        publish(
            &c,
            "health-finish",
            json!({"run_id":"one","result":{"records":{"a":a,"b":b},"probed":2,"waiting":0}}),
            time,
        );
        let result = report(&c).unwrap();
        assert_eq!(result["records"], json!({}));
        assert_eq!(result["waiting"], 2);
        fs::remove_dir_all(c.fixture.unwrap()).unwrap();
    }
    #[test]
    fn report_merge_idempotence_is_journal_owned_and_generation_bound() {
        let c = fixture();
        let time = now();
        let a = observed(&c, "a", "healthy");
        let body = json!({"vid":"a","path":a["path"],"fingerprint":a["fingerprint"],"material":a["material"],"code":3});
        publish(&c, "health-report", body.clone(), time);
        publish(&c, "health-report", body.clone(), time + 1);
        assert_eq!(read(&c, REPORTS, json!({})).unwrap()["a"]["count"], 2);
        publish(&c, "health-start", json!({"run_id":"one"}), time);
        publish(
            &c,
            "health-finish",
            json!({"run_id":"one","result":{"records":{"a":a},"probed":1,"waiting":0}}),
            time + 2,
        );
        assert_eq!(
            report(&c).unwrap()["records"]["a"]["status"],
            "playback_warning"
        );
        fs::write(a["path"].as_str().unwrap(), b"new generation").unwrap();
        assert_eq!(
            prepare(&c, "health-report", &body, &stamp(time + 3).unwrap())
                .unwrap_err()
                .status,
            409
        );
        fs::remove_dir_all(c.fixture.unwrap()).unwrap();
    }
    #[test]
    fn stale_hidden_and_restart_failure_preserve_completed_evidence() {
        let c = fixture();
        let t = now();
        let mut a = observed(&c, "a", "corrupt");
        a["hidden"] = json!(true);
        write(
            &c,
            HEALTH,
            json!({"records":{"a":a},"last_success_at":stamp(t-7*DAY).unwrap(),"status":"running","run_id":"old"}),
        );
        assert_eq!(summary(&c).unwrap()["rows"][0]["hidden"], false);
        let old = report(&c).unwrap()["records"].clone();
        publish(&c, "health-interrupted", json!({}), t);
        assert_eq!(report(&c).unwrap()["status"], "interrupted");
        assert_eq!(report(&c).unwrap()["records"], old);
        publish(&c, "health-start", json!({"run_id":"new"}), t);
        publish(
            &c,
            "health-finish",
            json!({"run_id":"new","error":"NFS unavailable"}),
            t,
        );
        assert_eq!(report(&c).unwrap()["records"], old);
        assert_eq!(
            report(&c).unwrap()["last_success_at"],
            stamp(t - 7 * DAY).unwrap()
        );
        fs::remove_dir_all(c.fixture.unwrap()).unwrap();
    }
    #[test]
    fn restored_generation_invalidates_but_deleted_duplicate_preserves_canonical() {
        let c = fixture();
        let time = now();
        let original = observed(&c, "a", "healthy");
        let staged = crate::staging::root(&c).join("duplicate.mp4");
        write(
            &c,
            "archive-history.json",
            json!([{"vid":"a","dst":original["path"],"staged_path":staged}]),
        );
        let checked = observed(&c, "a", "corrupt");
        write(
            &c,
            "archive-history.json",
            json!([{"vid":"a","dst":original["path"],"staged_path":staged,"staged_state":"deleted","last_staging_operation":"delete-duplicate"}]),
        );
        assert_eq!(inventory(&c).unwrap()["a"]["material"], checked["material"]);
        publish(&c, "health-start", json!({"run_id":"restore"}), time);
        write(
            &c,
            "staging-origins.json",
            json!({staged.to_str().unwrap():{"restored_path":original["path"],"restored_by":"new-restore"}}),
        );
        assert_ne!(inventory(&c).unwrap()["a"]["material"], checked["material"]);
        publish(
            &c,
            "health-finish",
            json!({"run_id":"restore","result":{"records":{"a":checked},"probed":1,"waiting":0}}),
            time,
        );
        assert!(report(&c).unwrap()["records"]["a"].is_null());
        fs::remove_dir_all(c.fixture.unwrap()).unwrap();
    }
    #[test]
    fn subprocess_timeout_reaps_process_group_and_caps_output() {
        let c = fixture();
        let start = Instant::now();
        let result = subprocess(
            &c,
            "/bin/sh",
            &["-c", "sleep 30 & wait"],
            Duration::from_millis(100),
            &AtomicBool::new(false),
        );
        assert_eq!(result.unwrap_err().code, "probe_unavailable");
        assert!(start.elapsed() < Duration::from_secs(3));
        let result = subprocess(
            &c,
            "/bin/sh",
            &["-c", "yes x"],
            Duration::from_secs(2),
            &AtomicBool::new(false),
        );
        assert_eq!(result.unwrap_err().code, "probe_unavailable");
        assert_eq!(fs::read_dir(c.state_dir()).unwrap().count(), 0);
        fs::remove_dir_all(c.fixture.unwrap()).unwrap();
    }
}
