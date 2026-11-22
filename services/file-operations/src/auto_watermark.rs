//! Deterministic Rule 2a + permissive-watermark-trust ("宽松信水印") classifier
//! for direct auto-archive of unambiguous watermark matches into existing person directories.
use crate::{
    config::Config,
    db, files,
    model::{Error, Result, hash},
    person_registration, processing_result, review,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};
use unicode_normalization::UnicodeNormalization;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoMatchTier {
    ConfirmedRule,
    DirectHandle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoSkipCategory {
    NoExistingPerson,
    FaceContradicts,
    MultiWatermark,
    FolderConflict,
    SpaceTruncated,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutoMatchEligible {
    pub vid: String,
    pub name: String,
    pub path: String,
    pub size: u64,
    pub mtime_ns: String,
    pub person: String,
    pub match_tier: AutoMatchTier,
    pub watermark_text: String,
    pub watermark_platform: String,
    pub watermark_key: String,
    pub target_name: String,
    pub renamed_keep_both: bool,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AutoMatchDecision {
    Eligible(AutoMatchEligible),
    Skipped {
        reason: String,
        category: AutoSkipCategory,
    },
}

pub struct AutoMatchContext {
    current_pipeline: Option<String>,
    aliases: Value,
    deleted: BTreeSet<String>,
    mappings: Value,
    rejections: Value,
    queued_vids: BTreeSet<String>,
    queued_paths: BTreeSet<String>,
    archived_vids: BTreeSet<String>,
    legacy_by_vid: BTreeMap<String, BTreeSet<String>>,
    health_hidden_candidates: BTreeSet<String>,
    creator_dirs_exact: BTreeSet<String>,
    creator_dirs_folded: BTreeMap<String, Vec<String>>,
}

fn folded(value: &str) -> String {
    value.nfc().collect::<String>().to_lowercase()
}

fn is_video_filename(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|s| s.to_str())
        .is_some_and(|ext| {
            [
                "mp4", "m4v", "mov", "mkv", "webm", "avi", "wmv", "ts", "mts", "m2ts",
            ]
            .contains(&ext.to_ascii_lowercase().as_str())
        })
}

fn count_existing_videos_in_dir(dir: &Path) -> usize {
    fn walk(path: &Path, depth: usize, budget: &mut usize, count: &mut usize) {
        if depth > 32 || *budget >= 50_000 {
            return;
        }
        let Ok(entries) = fs::read_dir(path) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name_str) = name.to_str() else {
                continue;
            };
            if name_str.starts_with('.') {
                continue;
            }
            *budget += 1;
            if *budget >= 50_000 {
                return;
            }
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                walk(&entry.path(), depth + 1, budget, count);
            } else if ft.is_file() && is_video_filename(name_str) {
                *count += 1;
            }
        }
    }
    let mut budget = 0;
    let mut count = 0;
    walk(dir, 0, &mut budget, &mut count);
    count
}

fn read_current_pipeline(config: &Config) -> Option<String> {
    let hb = review::read(config, "processing/heartbeat.json", json!({}))
        .ok()
        .map(|(_, v)| v)
        .unwrap_or_else(|| json!({}));
    if let Some(p) = hb["text_pipeline"].as_str().filter(|s| !s.is_empty()) {
        return Some(p.to_owned());
    }
    let worker = review::read(config, "watermarks/worker.json", json!({}))
        .ok()
        .map(|(_, v)| v)
        .unwrap_or_else(|| json!({}));
    if let Some(p) = worker["pipeline"].as_str().filter(|s| !s.is_empty()) {
        return Some(p.to_owned());
    }
    None
}

impl AutoMatchContext {
    pub fn load(config: &Config) -> Result<Self> {
        let current_pipeline = read_current_pipeline(config);
        let aliases = review::read(config, "person-renames.json", json!({}))?.1;
        let deleted_doc = review::read(config, "people-deleted.json", json!({}))?.1;
        let mut deleted = BTreeSet::new();
        for (k, v) in deleted_doc.as_object().into_iter().flatten() {
            if !v.is_null() {
                deleted.insert(k.clone());
                deleted.insert(processing_result::canonical_person(k, &aliases));
            }
        }
        let mappings = review::read(config, "watermarks/mappings.json", json!({}))?.1;
        let rejections = review::read(config, "person-rejections.json", json!({}))?.1;
        let queue = review::read(config, "integration-queue.json", json!([]))?.1;
        let history = review::read(config, "archive-history.json", json!([]))?.1;

        let queued_vids: BTreeSet<String> = queue
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|r| r["vid"].as_str().map(str::to_owned))
            .collect();
        let queued_sources = if queued_vids.is_empty() {
            BTreeMap::new()
        } else {
            review::sources_for_vids(config, &queued_vids)?
        };
        let queued_paths: BTreeSet<String> = queued_vids
            .iter()
            .filter_map(|v| review::source(config, v, &queued_sources))
            .collect();
        let archived_vids: BTreeSet<String> = history
            .as_array()
            .into_iter()
            .flatten()
            .filter(|r| r["undone"] != true)
            .filter_map(|r| r["vid"].as_str().map(str::to_owned))
            .collect();

        let report = review::read(config, "report.json", json!({}))?.1;
        let mut legacy_by_vid: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for person in report["identities"].as_array().into_iter().flatten() {
            let Some(raw_name) = person["name"].as_str().filter(|s| !s.is_empty()) else {
                continue;
            };
            let canonical = processing_result::canonical_person(raw_name, &aliases);
            if deleted.contains(&canonical) || deleted.contains(raw_name) {
                continue;
            }
            for row in person["vids"].as_array().into_iter().flatten() {
                if let Some(vid) = row["vid"].as_str().filter(|s| !s.is_empty()) {
                    legacy_by_vid
                        .entry(vid.to_owned())
                        .or_default()
                        .insert(canonical.clone());
                }
            }
        }

        let health = review::read(config, "video-health.json", json!({}))?.1;
        let health_hidden_candidates: BTreeSet<String> = health["records"]
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(_, row)| row["hidden"] == true)
            .map(|(vid, _)| vid.clone())
            .collect();

        let mut creator_dirs_exact = BTreeSet::new();
        let mut creator_dirs_folded: BTreeMap<String, Vec<String>> = BTreeMap::new();
        match fs::read_dir(config.creators()) {
            Ok(entries) => {
                for entry in entries {
                    let entry = entry?;
                    let ft = entry.file_type()?;
                    if !ft.is_dir() || ft.is_symlink() {
                        continue;
                    }
                    let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                        continue;
                    };
                    if name.starts_with('.') {
                        continue;
                    }
                    let key = folded(&name);
                    creator_dirs_exact.insert(name.clone());
                    creator_dirs_folded.entry(key).or_default().push(name);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        for names in creator_dirs_folded.values_mut() {
            names.sort();
            names.dedup();
        }

        Ok(Self {
            current_pipeline,
            aliases,
            deleted,
            mappings,
            rejections,
            queued_vids,
            queued_paths,
            archived_vids,
            legacy_by_vid,
            health_hidden_candidates,
            creator_dirs_exact,
            creator_dirs_folded,
        })
    }

    fn is_deleted(&self, name: &str) -> bool {
        if self.deleted.contains(name) {
            return true;
        }
        let canonical = processing_result::canonical_person(name, &self.aliases);
        self.deleted.contains(&canonical)
    }

    /// Resolve a candidate name uniquely to a live, non-deleted on-disk directory in `pikpak/creators/`.
    fn resolve_live_person_dir(&self, raw_name: &str) -> Option<String> {
        let canonical = processing_result::canonical_person(raw_name, &self.aliases);
        if self.is_deleted(raw_name) || self.is_deleted(&canonical) {
            return None;
        }
        let key = folded(&canonical);
        let matches = self.creator_dirs_folded.get(&key)?;
        if matches.len() != 1 {
            // Multiple case/Unicode variants on disk => ambiguous.
            return None;
        }
        let dir_name = &matches[0];
        if !self.creator_dirs_exact.contains(dir_name) || self.is_deleted(dir_name) {
            return None;
        }
        // If `dir_name` itself is aliased away to a different live person, don't auto-merge into stale dir.
        let dir_canon = processing_result::canonical_person(dir_name, &self.aliases);
        if !person_registration::same_spelling(&dir_canon, dir_name) {
            return None;
        }
        Some(dir_name.clone())
    }

    fn is_rejected(&self, person: &str, vid: &str, path: &str, size: u64, mtime_ns: &str) -> bool {
        let Ok(key) = serde_json::to_string(&json!([person, vid])) else {
            return false;
        };
        let row = &self.rejections[&key];
        row.is_object()
            && row["path"].as_str() == Some(path)
            && review::number(&row["size"]) == i64::try_from(size).ok()
            && row["mtime_ns"].as_str() == Some(mtime_ns)
    }

    fn is_health_hidden(
        &self,
        config: &Config,
        vid: &str,
        path: &str,
        stat: &fs::Metadata,
    ) -> Result<bool> {
        if !self.health_hidden_candidates.contains(vid) {
            return Ok(false);
        }
        review::health_hidden(config, vid, path, stat)
    }
}

fn make_keep_both_name(original_name: &str, vid: &str, index: usize) -> String {
    let p = Path::new(original_name);
    let digest = format!("{:x}", Sha256::digest(vid.as_bytes()));
    let tag = if index <= 1 {
        digest[..8].to_owned()
    } else {
        format!("{}-{index}", &digest[..8])
    };
    let ext = p
        .extension()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty());
    let suffix = match ext {
        Some(ext) => format!(" (保留两份-{tag}).{ext}"),
        None => format!(" (保留两份-{tag})"),
    };
    let mut stem = p
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    while !stem.is_empty() && stem.len() + suffix.len() > 240 {
        stem.pop();
    }
    format!("{stem}{suffix}")
}

/// Choose the destination filename inside `dir`, automatically switching to a deterministic
/// keep-both filename (`<stem> (保留两份-<sha256(vid)[:8]>).<ext>`, with `-2`/`-3` on further
/// collisions) whenever an NFC+lowercase collision exists on disk or in `reserved_targets`.
pub fn keep_both_target_name(
    dir: &Path,
    original_name: &str,
    vid: &str,
    reserved_targets: &mut BTreeSet<String>,
) -> Result<(String, bool)> {
    files::validate_name(original_name)?;
    let mut existing_folded = BTreeSet::new();
    match fs::read_dir(dir) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry?;
                if let Some(name) = entry.file_name().to_str() {
                    existing_folded.insert(folded(name));
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let dir_prefix = dir.to_string_lossy();
    let reserve_key = |name: &str| format!("{dir_prefix}\0{}", folded(name));

    let orig_folded = folded(original_name);
    let orig_key = reserve_key(original_name);
    if !existing_folded.contains(&orig_folded) && !reserved_targets.contains(&orig_key) {
        reserved_targets.insert(orig_key);
        return Ok((original_name.to_owned(), false));
    }

    for index in 1..=1000usize {
        let candidate = make_keep_both_name(original_name, vid, index);
        let cand_folded = folded(&candidate);
        let cand_key = reserve_key(&candidate);
        if !existing_folded.contains(&cand_folded) && !reserved_targets.contains(&cand_key) {
            files::validate_name(&candidate)?;
            reserved_targets.insert(cand_key);
            return Ok((candidate, true));
        }
    }
    Err(Error::conflict("目标目录同名文件过多，无法生成保留两份文件名"))
}

/// Detect OCR space-truncation or slash-truncation where `watermark_worker.py`'s regex stopped
/// capturing `token` before trailing handle text (e.g. `onlyfans.com/YunaTamago zZ` -> `YunaTamago`,
/// `fansly.com/meriol/chan` -> `meriol`).
///
/// Exact rule: NFKC + trim `text` and `token`, locate `token` via case-insensitive `rfind`,
/// compute `tail = text[idx + matched_len..].trim()`, and reject unless `tail.is_empty() || tail == "/"`.
pub fn is_space_truncated_ocr(text: &str, token: &str) -> bool {
    let norm_text: String = text.nfkc().collect::<String>().trim().to_owned();
    let norm_token: String = token.nfkc().collect::<String>().trim().to_owned();
    if norm_text.is_empty() || norm_token.is_empty() {
        return false;
    }
    let text_lower = norm_text.to_lowercase();
    let token_lower = norm_token.to_lowercase();
    let stripped_lower = token_lower.trim_start_matches('@');
    let (idx, matched_len) = if let Some(i) = text_lower.rfind(&token_lower) {
        (i, token_lower.len())
    } else if !stripped_lower.is_empty()
        && let Some(i) = text_lower.rfind(stripped_lower)
    {
        (i, stripped_lower.len())
    } else {
        return false;
    };
    let tail = text_lower[idx + matched_len..].trim();
    !(tail.is_empty() || tail == "/")
}

fn valid_direct_handle(handle: &str) -> bool {
    let char_count = handle.chars().count();
    char_count >= 4
        && char_count <= 120
        && handle != "."
        && handle != ".."
        && !handle
            .chars()
            .any(|c| c.is_whitespace() || c == '/' || c == '\\' || ('\x00'..='\x1f').contains(&c))
}

enum MarkResolution {
    Matched {
        person: String,
        tier: AutoMatchTier,
    },
    SpaceTruncated(String),
    NoExistingPerson(String),
    InvalidHandle(String),
}

fn parsed_task_data(task: &Value) -> Result<Value> {
    if task["data"].is_object() {
        Ok(task["data"].clone())
    } else if let Some(raw) = task["data"].as_str() {
        Ok(serde_json::from_str(raw)?)
    } else {
        Ok(json!({}))
    }
}

fn skip(category: AutoSkipCategory, reason: impl Into<String>) -> AutoMatchDecision {
    AutoMatchDecision::Skipped {
        reason: reason.into(),
        category,
    }
}

pub fn evaluate_auto_watermark_match(
    config: &Config,
    ctx: &AutoMatchContext,
    task: &Value,
    reserved_targets: &mut BTreeSet<String>,
) -> Result<AutoMatchDecision> {
    let vid = match task["vid"].as_str().filter(|s| !s.is_empty()) {
        Some(v) => v,
        None => return Ok(skip(AutoSkipCategory::Other, "任务缺少视频编号")),
    };
    let path = match task["path"].as_str().filter(|s| !s.is_empty()) {
        Some(p) => p,
        None => return Ok(skip(AutoSkipCategory::Other, "任务缺少文件路径")),
    };
    let name = task["name"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            Path::new(path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| vid.to_owned());
    let Some(task_size) = review::number(&task["size"]).and_then(|n| u64::try_from(n).ok()) else {
        return Ok(skip(AutoSkipCategory::Other, "任务缺少文件体积"));
    };
    let Some(task_mtime_ns) = task["mtime_ns"].as_str().filter(|s| !s.is_empty()) else {
        return Ok(skip(AutoSkipCategory::Other, "任务缺少文件修改时间"));
    };

    // 1. Task eligibility flags & stage
    if review::number(&task["manual"]).unwrap_or(0) != 0 {
        return Ok(skip(AutoSkipCategory::Other, "人工接管任务不自动合入"));
    }
    if review::number(&task["deferred"]).unwrap_or(0) != 0 {
        return Ok(skip(AutoSkipCategory::Other, "暂缓任务不自动合入"));
    }
    if review::number(&task["auto_opt_out"]).unwrap_or(0) != 0 {
        return Ok(skip(
            AutoSkipCategory::Other,
            "已人工撤回的任务不再自动合入",
        ));
    }
    let stage = task["stage"].as_str().unwrap_or("");
    let retry_stage = task["retry_stage"].as_str().unwrap_or("");
    let stage_eligible = matches!(stage, "review" | "face_queued")
        || (stage == "retry" && retry_stage == "face");
    if !stage_eligible {
        return Ok(skip(
            AutoSkipCategory::Other,
            format!("任务当前阶段为 {stage}，不参与自动合入"),
        ));
    }

    let data = parsed_task_data(task)?;
    if data["ready"] != true || data["text_complete"] != true {
        return Ok(skip(AutoSkipCategory::Other, "分析结果尚未就绪"));
    }
    let watermark = &data["watermark"];
    if watermark["status"] != "done" {
        return Ok(skip(AutoSkipCategory::Other, "文字水印提取未完成"));
    }
    let pipeline = watermark["pipeline"].as_str().unwrap_or("");
    if pipeline.is_empty()
        || ctx
            .current_pipeline
            .as_deref()
            .is_some_and(|expected| expected != pipeline)
    {
        return Ok(skip(AutoSkipCategory::Other, "文字水印管线版本已过期"));
    }

    // 2. Queue / archive-history / disk fingerprint / health checks
    if ctx.queued_vids.contains(vid)
        || ctx.archived_vids.contains(vid)
        || ctx.queued_paths.contains(path)
    {
        return Ok(skip(AutoSkipCategory::Other, "视频已在待归档或已归档"));
    }
    let stat = match review::safe_source(config, path) {
        Ok(s) => s,
        Err(_) => {
            return Ok(skip(
                AutoSkipCategory::Other,
                "源文件不存在或不在允许的未整理目录",
            ));
        }
    };
    if stat.len() == 0
        || stat.len() != task_size
        || files::mtime_ns(&stat) != task_mtime_ns
    {
        return Ok(skip(AutoSkipCategory::Other, "源文件指纹已变化"));
    }
    if ctx.is_health_hidden(config, vid, path, &stat)? {
        return Ok(skip(
            AutoSkipCategory::Other,
            "视频体检异常已隐藏，不自动合入",
        ));
    }

    // 3. Raw unmutated data.watermark.candidates extraction
    let raw_marks: Vec<&Value> = watermark["candidates"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|mark| {
            let frames = review::number(&mark["frames"]).unwrap_or(0);
            if frames < 2 {
                return false;
            }
            let Some(key) = mark["key"].as_str().filter(|s| !s.is_empty()) else {
                return false;
            };
            let mapping = &ctx.mappings[key];
            let map_kind = mapping["kind"].as_str();
            if map_kind == Some("source") {
                return false;
            }
            if map_kind == Some("person") {
                return mapping["creator"]
                    .as_str()
                    .is_some_and(|s| !s.is_empty());
            }
            mark["kind"].as_str() == Some("account")
                && mark["platform"].as_str() != Some("Telegram")
        })
        .collect();

    if raw_marks.is_empty() {
        return Ok(skip(AutoSkipCategory::Other, "无作者账号水印线索"));
    }

    // 4. Resolve every remaining raw watermark mark individually (no prefix folding!)
    struct ResolvedMark<'a> {
        mark: &'a Value,
        resolution: MarkResolution,
        frames: i64,
        score: f64,
    }
    let mut resolved_marks = Vec::with_capacity(raw_marks.len());
    for mark in raw_marks {
        let key = mark["key"].as_str().unwrap_or("");
        let text = mark["text"].as_str().unwrap_or("");
        let token = mark["token"].as_str().unwrap_or("");
        let frames = review::number(&mark["frames"]).unwrap_or(0);
        let score = mark["score"].as_f64().unwrap_or(0.0);
        let mapping = &ctx.mappings[key];

        let resolution = if mapping["kind"].as_str() == Some("person")
            && let Some(creator) = mapping["creator"].as_str().filter(|s| !s.is_empty())
        {
            match ctx.resolve_live_person_dir(creator) {
                Some(person) => MarkResolution::Matched {
                    person,
                    tier: AutoMatchTier::ConfirmedRule,
                },
                None => MarkResolution::NoExistingPerson(format!(
                    "已确认规则目标人物「{creator}」尚无在库目录或已删除"
                )),
            }
        } else if is_space_truncated_ocr(text, token) {
            MarkResolution::SpaceTruncated(format!(
                "水印文字疑似空格或斜杠截断，保留人工核对：{text}"
            ))
        } else {
            let handle = token.trim_start_matches('@').trim();
            if !valid_direct_handle(handle) {
                MarkResolution::InvalidHandle(format!("水印账号长度或字符不满足直匹要求：{token}"))
            } else {
                match ctx.resolve_live_person_dir(handle) {
                    Some(person) => MarkResolution::Matched {
                        person,
                        tier: AutoMatchTier::DirectHandle,
                    },
                    None => MarkResolution::NoExistingPerson(format!(
                        "账号「{handle}」未唯一匹配到已有在库人物目录"
                    )),
                }
            }
        };
        resolved_marks.push(ResolvedMark {
            mark,
            resolution,
            frames,
            score,
        });
    }

    // 5. Consensus across ALL remaining raw account marks
    let mut matched_people = BTreeSet::new();
    let mut unmatched_count = 0usize;
    let mut first_space_truncated: Option<String> = None;
    let mut first_no_existing: Option<String> = None;
    let mut first_invalid: Option<String> = None;
    let mut distinct_handles = BTreeSet::new();

    for r in &resolved_marks {
        let token = r.mark["token"]
            .as_str()
            .unwrap_or("")
            .trim_start_matches('@')
            .trim();
        distinct_handles.insert(folded(token));
        match &r.resolution {
            MarkResolution::Matched { person, .. } => {
                matched_people.insert(person.clone());
            }
            MarkResolution::SpaceTruncated(msg) => {
                unmatched_count += 1;
                if first_space_truncated.is_none() {
                    first_space_truncated = Some(msg.clone());
                }
            }
            MarkResolution::NoExistingPerson(msg) => {
                unmatched_count += 1;
                if first_no_existing.is_none() {
                    first_no_existing = Some(msg.clone());
                }
            }
            MarkResolution::InvalidHandle(msg) => {
                unmatched_count += 1;
                if first_invalid.is_none() {
                    first_invalid = Some(msg.clone());
                }
            }
        }
    }

    if matched_people.len() > 1 {
        return Ok(skip(
            AutoSkipCategory::MultiWatermark,
            format!(
                "视频包含多个不同人物的账号水印：{}",
                matched_people.iter().cloned().collect::<Vec<_>>().join(" / ")
            ),
        ));
    }
    if let Some(msg) = first_space_truncated {
        return Ok(skip(AutoSkipCategory::SpaceTruncated, msg));
    }
    if unmatched_count > 0 {
        if !matched_people.is_empty() || distinct_handles.len() > 1 {
            return Ok(skip(
                AutoSkipCategory::MultiWatermark,
                "视频包含多个不同账号水印或未确认变体，保留人工核对",
            ));
        }
        if let Some(msg) = first_no_existing {
            return Ok(skip(AutoSkipCategory::NoExistingPerson, msg));
        }
        return Ok(skip(
            AutoSkipCategory::Other,
            first_invalid.unwrap_or_else(|| "水印账号不满足直匹条件".to_owned()),
        ));
    }

    let person = matched_people.into_iter().next().unwrap();

    // Pick effective match tier (ConfirmedRule if any mark used a confirmed mapping) and best display mark.
    let match_tier = if resolved_marks.iter().any(|r| {
        matches!(
            r.resolution,
            MarkResolution::Matched {
                tier: AutoMatchTier::ConfirmedRule,
                ..
            }
        )
    }) {
        AutoMatchTier::ConfirmedRule
    } else {
        AutoMatchTier::DirectHandle
    };
    let primary = resolved_marks
        .iter()
        .max_by(|a, b| {
            let a_conf = matches!(
                a.resolution,
                MarkResolution::Matched {
                    tier: AutoMatchTier::ConfirmedRule,
                    ..
                }
            );
            let b_conf = matches!(
                b.resolution,
                MarkResolution::Matched {
                    tier: AutoMatchTier::ConfirmedRule,
                    ..
                }
            );
            a_conf
                .cmp(&b_conf)
                .then_with(|| a.frames.cmp(&b.frames))
                .then_with(|| a.score.total_cmp(&b.score))
        })
        .unwrap();

    // 6. Verify live on-disk directory & rejection guard
    if !person_registration::has_own_directory(config, &person)? || ctx.is_deleted(&person) {
        return Ok(skip(
            AutoSkipCategory::NoExistingPerson,
            format!("人物「{person}」目录不存在或已删除"),
        ));
    }
    if ctx.is_rejected(&person, vid, path, task_size, task_mtime_ns) {
        return Ok(skip(
            AutoSkipCategory::Other,
            format!("该视频已被标记为不是「{person}」"),
        ));
    }

    // 7. Contradiction guards
    // 7a. Folder/filename hints must contain no other live person
    for hint in data["folder"].as_array().into_iter().flatten() {
        let Some(raw) = hint["name"].as_str().filter(|s| !s.is_empty()) else {
            continue;
        };
        if ctx.is_deleted(raw) {
            continue;
        }
        let canon = processing_result::canonical_person(raw, &ctx.aliases);
        if ctx.is_deleted(&canon) {
            continue;
        }
        if !person_registration::same_spelling(&canon, &person) {
            return Ok(skip(
                AutoSkipCategory::FolderConflict,
                format!("文件名或目录线索指向其他人物「{canon}」"),
            ));
        }
    }

    // 7b. Legacy report identities guard: apply ONLY when modern face analysis did NOT run
    // (e.g. face_skipped == true on ConfirmedRule). When face_complete == true, modern
    // data.face.matches supersedes stale report.json entries.
    let face_complete = data["face_complete"] == true;
    if !face_complete
        && let Some(legacy_names) = ctx.legacy_by_vid.get(vid)
    {
        for legacy in legacy_names {
            if !person_registration::same_spelling(legacy, &person) {
                return Ok(skip(
                    AutoSkipCategory::Other,
                    format!("历史识别报告包含其他人物候选「{legacy}」"),
                ));
            }
        }
    }

    // 7c. Face guard (宽松信水印):
    // - For DirectHandle, require face_complete == true.
    // - Whenever face_complete == true and face.matches (canonicalized, non-deleted) is non-empty,
    //   `person` MUST appear in face.matches.
    if match_tier == AutoMatchTier::DirectHandle && !face_complete {
        return Ok(skip(
            AutoSkipCategory::Other,
            "未确认水印尚未完成人脸分析，等待人脸复核",
        ));
    }
    if face_complete {
        let face_people: Vec<String> = data["face"]["matches"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|m| m["name"].as_str().filter(|s| !s.is_empty()))
            .filter(|raw| !ctx.is_deleted(raw))
            .map(|raw| processing_result::canonical_person(raw, &ctx.aliases))
            .filter(|canon| !ctx.is_deleted(canon))
            .collect();
        if !face_people.is_empty()
            && !face_people
                .iter()
                .any(|f| person_registration::same_spelling(f, &person))
        {
            return Ok(skip(
                AutoSkipCategory::FaceContradicts,
                format!(
                    "人脸相似度仅匹配到其他人物（{}），与水印「{person}」矛盾",
                    face_people.join(" / ")
                ),
            ));
        }
    }

    // 8. Compute deterministic target filename (keep-both on collision)
    let target_dir = config.creators().join(&person);
    let (target_name, renamed_keep_both) =
        keep_both_target_name(&target_dir, &name, vid, reserved_targets)?;

    Ok(AutoMatchDecision::Eligible(AutoMatchEligible {
        vid: vid.to_owned(),
        name,
        path: path.to_owned(),
        size: task_size,
        mtime_ns: task_mtime_ns.to_owned(),
        person,
        match_tier,
        watermark_text: primary.mark["text"].as_str().unwrap_or("").to_owned(),
        watermark_platform: primary.mark["platform"]
            .as_str()
            .unwrap_or("unknown")
            .to_owned(),
        watermark_key: primary.mark["key"].as_str().unwrap_or("").to_owned(),
        target_name,
        renamed_keep_both,
        note: task["note"].as_str().unwrap_or("").to_owned(),
    }))
}

fn load_candidate_tasks(
    config: &Config,
    only_vids: Option<&BTreeSet<String>>,
) -> Result<Vec<Value>> {
    let path = config.base.join("processing/processing.db");
    if !path.exists() {
        return Ok(vec![]);
    }
    let conn = db::open_readonly(&path)?;
    if let Some(filter) = only_vids {
        if filter.is_empty() {
            return Ok(vec![]);
        }
        let placeholders = std::iter::repeat_n("?", filter.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT * FROM tasks WHERE stage!='excluded' AND vid IN ({placeholders}) ORDER BY priority DESC,is_new DESC,created_at DESC,vid"
        );
        db::rows(&conn, &sql, rusqlite::params_from_iter(filter.iter()))
    } else {
        db::rows(
            &conn,
            "SELECT * FROM tasks WHERE stage IN ('review','face_queued','retry') AND manual=0 AND deferred=0 ORDER BY priority DESC,is_new DESC,created_at DESC,vid",
            [],
        )
    }
}

pub fn evaluate_for_vids(
    config: &Config,
    only_vids: Option<&BTreeSet<String>>,
) -> Result<Vec<AutoMatchEligible>> {
    let ctx = AutoMatchContext::load(config)?;
    let tasks = load_candidate_tasks(config, only_vids)?;
    let mut reserved = BTreeSet::new();
    let mut eligible = Vec::new();
    for task in &tasks {
        if let AutoMatchDecision::Eligible(item) =
            evaluate_auto_watermark_match(config, &ctx, task, &mut reserved)?
        {
            eligible.push(item);
        }
    }
    Ok(eligible)
}

pub fn preview_auto_watermark_matches(config: &Config) -> Result<Value> {
    let ctx = AutoMatchContext::load(config)?;
    let tasks = load_candidate_tasks(config, None)?;
    let mut reserved = BTreeSet::new();

    let mut no_existing_person = 0usize;
    let mut face_contradicts = 0usize;
    let mut multi_watermark = 0usize;
    let mut folder_conflict = 0usize;
    let mut space_truncated = 0usize;
    let mut other = 0usize;

    let mut by_person: BTreeMap<String, Vec<AutoMatchEligible>> = BTreeMap::new();
    for task in &tasks {
        match evaluate_auto_watermark_match(config, &ctx, task, &mut reserved)? {
            AutoMatchDecision::Eligible(item) => {
                by_person.entry(item.person.clone()).or_default().push(item);
            }
            AutoMatchDecision::Skipped { category, .. } => match category {
                AutoSkipCategory::NoExistingPerson => no_existing_person += 1,
                AutoSkipCategory::FaceContradicts => face_contradicts += 1,
                AutoSkipCategory::MultiWatermark => multi_watermark += 1,
                AutoSkipCategory::FolderConflict => folder_conflict += 1,
                AutoSkipCategory::SpaceTruncated => space_truncated += 1,
                AutoSkipCategory::Other => other += 1,
            },
        }
    }

    let mut groups: Vec<Value> = Vec::with_capacity(by_person.len());
    let mut eligible_count = 0usize;
    for (person, mut items) in by_person {
        items.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.vid.cmp(&b.vid)));
        eligible_count += items.len();
        let dir = config.creators().join(&person);
        let existing_files = count_existing_videos_in_dir(&dir);
        let item_values: Vec<Value> = items
            .iter()
            .map(|item| {
                json!({
                    "vid": item.vid,
                    "name": item.name,
                    "path": item.path,
                    "size": item.size,
                    "mtime_ns": item.mtime_ns,
                    "target_name": item.target_name,
                    "renamed_keep_both": item.renamed_keep_both,
                    "match_tier": item.match_tier,
                    "watermark_text": item.watermark_text,
                    "watermark_platform": item.watermark_platform,
                    "watermark_key": item.watermark_key,
                })
            })
            .collect();
        groups.push(json!({
            "person": person,
            "directory": dir.to_string_lossy(),
            "existing_files": existing_files,
            "count": items.len(),
            "items": item_values,
        }));
    }
    groups.sort_by(|a, b| {
        b["count"]
            .as_u64()
            .cmp(&a["count"].as_u64())
            .then_with(|| a["person"].as_str().cmp(&b["person"].as_str()))
    });

    let version = hash(&json!(groups));
    Ok(json!({
        "ok": true,
        "version": version,
        "eligible_count": eligible_count,
        "group_count": groups.len(),
        "groups": groups,
        "skipped_summary": {
            "no_existing_person": no_existing_person,
            "face_contradicts": face_contradicts,
            "multi_watermark": multi_watermark,
            "folder_conflict": folder_conflict,
            "space_truncated": space_truncated,
            "other": other,
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    const PIPELINE: &str = "paddleocr-3.3.2-ppocrv5-mobile-watermark-v2";

    struct Fixture {
        root: PathBuf,
        config: Config,
    }

    impl Fixture {
        fn new() -> Self {
            let id = COUNTER.fetch_add(1, Ordering::Relaxed);
            let raw = std::env::temp_dir().join(format!(
                "face-admin-auto-watermark-{}-{id}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&raw);
            fs::create_dir_all(&raw).unwrap();
            let root = fs::canonicalize(&raw).unwrap();
            let base = root.join("base");
            let media = root.join("media");
            fs::create_dir_all(base.join("processing")).unwrap();
            fs::create_dir_all(base.join("watermarks")).unwrap();
            fs::create_dir_all(media.join("pikpak/My Pack")).unwrap();
            fs::create_dir_all(media.join("pikpak/creators")).unwrap();
            fs::write(
                base.join("processing/heartbeat.json"),
                json!({"at": 1_700_000_000.0, "text_pipeline": PIPELINE}).to_string(),
            )
            .unwrap();
            let faces = rusqlite::Connection::open(base.join("faces.db")).unwrap();
            faces
                .execute_batch("CREATE TABLE videos (vid TEXT PRIMARY KEY, path TEXT NOT NULL);")
                .unwrap();
            drop(faces);
            let config = Config {
                bind: "127.0.0.1:0".into(),
                base,
                media,
                ledger: root.join("ledger.db"),
                token: None,
                fixture: None,
                parity_root: None,
            };
            drop(crate::processing_scheduler::open_or_init(&config).unwrap());
            Self { root, config }
        }

        fn add_creator(&self, name: &str, existing_videos: &[&str]) {
            let dir = self.config.creators().join(name);
            fs::create_dir_all(&dir).unwrap();
            for v in existing_videos {
                fs::write(dir.join(v), b"existing-video-bytes").unwrap();
            }
        }

        fn add_source_video(&self, rel: &str, content: &[u8]) -> (String, u64, String) {
            let p = self.config.media.join("pikpak/My Pack").join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, content).unwrap();
            let stat = fs::metadata(&p).unwrap();
            (
                p.to_string_lossy().into_owned(),
                stat.len(),
                files::mtime_ns(&stat),
            )
        }

        fn insert_task(
            &self,
            vid: &str,
            path: &str,
            size: u64,
            mtime_ns: &str,
            stage: &str,
            retry_stage: &str,
            manual: i64,
            deferred: i64,
            auto_opt_out: i64,
            data: Value,
        ) {
            let conn = crate::processing_scheduler::open_or_init(&self.config).unwrap();
            let name = Path::new(path)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            conn.execute(
                "INSERT INTO tasks(vid,path,name,size,mtime_ns,stage,retry_stage,batch_id,created_at,updated_at,manual,deferred,auto_opt_out,data)
                 VALUES(?,?,?,?,?,?,?,'b1',1.0,1.0,?,?,?,?)",
                params![
                    vid,
                    path,
                    name,
                    size as i64,
                    mtime_ns,
                    stage,
                    retry_stage,
                    manual,
                    deferred,
                    auto_opt_out,
                    data.to_string()
                ],
            )
            .unwrap();
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn wm_key(platform: &str, token: &str) -> String {
        format!("{:x}", Sha256::digest(format!("{platform}\0{token}").as_bytes()))
    }

    fn wm_cand(platform: &str, token: &str, text: &str, kind: &str, frames: i64) -> Value {
        json!({
            "key": wm_key(platform, token),
            "kind": kind,
            "platform": platform,
            "token": token,
            "text": text,
            "frames": frames,
            "score": 0.995,
        })
    }

    #[test]
    fn dreamymoo_face_queued_with_higher_rival_is_eligible() {
        let fx = Fixture::new();
        fx.add_creator("dreamymoo", &["old1.mp4", "old2.mp4"]);
        fx.add_creator("soficoquita", &["s1.mp4"]);
        let tg_key = wm_key("Telegram", "ysjof");
        fs::write(
            fx.config.base.join("watermarks/mappings.json"),
            json!({
                tg_key: {"kind": "source", "platform": "Telegram", "token": "ysjof", "text": "TG@ysjof"}
            })
            .to_string(),
        )
        .unwrap();

        let (path, size, mtime_ns) = fx.add_source_video("8_2024_07_18_224902.mp4", b"video-dreamy");
        fx.insert_task(
            "v_dreamy",
            &path,
            size,
            &mtime_ns,
            "face_queued",
            "face",
            0,
            0,
            0,
            json!({
                "ready": true,
                "text_complete": true,
                "face_complete": true,
                "face_skipped": false,
                "folder": [],
                "watermark": {
                    "status": "done",
                    "pipeline": PIPELINE,
                    "candidates": [
                        wm_cand("Telegram", "ysjof", "https:/t.me/ysjof/", "account", 8),
                        wm_cand("OnlyFans", "dreamymoo", "OnlyFans.com/dreamymoo", "account", 3),
                    ]
                },
                "face": {
                    "matches": [
                        {"name": "soficoquita", "source": "face", "score": 0.710, "support": 7},
                        {"name": "dreamymoo", "source": "face", "score": 0.689, "support": 7},
                    ]
                }
            }),
        );

        let preview = preview_auto_watermark_matches(&fx.config).unwrap();
        assert_eq!(preview["eligible_count"], 1);
        assert_eq!(preview["group_count"], 1);
        assert_eq!(preview["groups"][0]["person"], "dreamymoo");
        assert_eq!(preview["groups"][0]["existing_files"], 2);
        let item = &preview["groups"][0]["items"][0];
        assert_eq!(item["vid"], "v_dreamy");
        assert_eq!(item["match_tier"], "direct_handle");
        assert_eq!(item["target_name"], "8_2024_07_18_224902.mp4");
        assert_eq!(item["renamed_keep_both"], false);
    }

    #[test]
    fn rurigraph_confirmed_clean_passes_and_extra_typo_blocks() {
        let fx = Fixture::new();
        fx.add_creator("rurigraph", &["r1.mp4"]);
        let key = wm_key("unknown", "@Ruri_LapisL");
        fs::write(
            fx.config.base.join("watermarks/mappings.json"),
            json!({
                key: {"kind": "person", "creator": "rurigraph", "platform": "unknown", "token": "@Ruri_LapisL", "text": "@Ruri_LapisL"}
            })
            .to_string(),
        )
        .unwrap();

        let (p1, s1, m1) = fx.add_source_video("ruri_clean.mp4", b"clean");
        fx.insert_task(
            "v_ruri_clean",
            &p1,
            s1,
            &m1,
            "review",
            "text",
            0,
            0,
            0,
            json!({
                "ready": true,
                "text_complete": true,
                "face_skipped": true,
                "folder": [],
                "watermark": {
                    "status": "done",
                    "pipeline": PIPELINE,
                    "candidates": [
                        wm_cand("unknown", "@Ruri_LapisL", "@Ruri_LapisL", "account", 8)
                    ]
                }
            }),
        );

        let (p2, s2, m2) = fx.add_source_video("ruri_typo.mp4", b"typo");
        fx.insert_task(
            "v_ruri_typo",
            &p2,
            s2,
            &m2,
            "face_queued",
            "face",
            0,
            0,
            0,
            json!({
                "ready": true,
                "text_complete": true,
                "face_complete": true,
                "face_skipped": false,
                "folder": [{"name": "rurigraph", "source": "folder"}],
                "watermark": {
                    "status": "done",
                    "pipeline": PIPELINE,
                    "candidates": [
                        wm_cand("unknown", "@Ruri_LapisL", "@Ruri_LapisL", "account", 6),
                        wm_cand("unknown", "@RuriLapisL", "@RuriLapisL", "account", 2)
                    ]
                },
                "face": {"matches": []}
            }),
        );

        let preview = preview_auto_watermark_matches(&fx.config).unwrap();
        assert_eq!(preview["eligible_count"], 1);
        assert_eq!(preview["groups"][0]["items"][0]["vid"], "v_ruri_clean");
        assert_eq!(preview["groups"][0]["items"][0]["match_tier"], "confirmed_rule");
        assert_eq!(preview["skipped_summary"]["multi_watermark"], 1);
    }

    #[test]
    fn live_traps_blocked_yuna_zyfls_dreamymoo28_retsu() {
        let fx = Fixture::new();
        fx.add_creator("YunaTamago", &["y1.mp4"]);
        fx.add_creator("YunaTamago_zZ", &["yz1.mp4"]);
        fx.add_creator("ZYFLS44", &["z1.mp4"]);
        fx.add_creator("XMYao1998", &["x1.mp4"]);
        fx.add_creator("soficoquita", &["s1.mp4"]);
        fx.add_creator("dreamymoo", &["d1.mp4"]);
        fx.add_creator("retsu_dao5", &["r5.mp4"]);
        fx.add_creator("retsu_dao", &["r0.mp4"]);

        // 1. Space-truncated OCR: onlyfans.com/YunaTamago zZ
        let (p_yuna, s_yuna, m_yuna) = fx.add_source_video("yuna.mp4", b"yuna");
        fx.insert_task(
            "v_yuna",
            &p_yuna,
            s_yuna,
            &m_yuna,
            "review",
            "face",
            0,
            0,
            0,
            json!({
                "ready": true,
                "text_complete": true,
                "face_complete": true,
                "folder": [],
                "watermark": {
                    "status": "done",
                    "pipeline": PIPELINE,
                    "candidates": [
                        wm_cand("OnlyFans", "YunaTamago", "onlyfans.com/YunaTamago zZ", "account", 5)
                    ]
                },
                "face": {
                    "matches": [
                        {"name": "YunaTamago_zZ", "source": "face", "score": 0.791, "support": 6},
                        {"name": "YunaTamago", "source": "face", "score": 0.650, "support": 5}
                    ]
                }
            }),
        );

        // 2. ZYFLS44 pirate folder vs face XMYao1998 only
        let (p_zy, s_zy, m_zy) = fx.add_source_video("zyfls.mp4", b"zyfls");
        fx.insert_task(
            "v_zyfls",
            &p_zy,
            s_zy,
            &m_zy,
            "review",
            "face",
            0,
            0,
            0,
            json!({
                "ready": true,
                "text_complete": true,
                "face_complete": true,
                "folder": [],
                "watermark": {
                    "status": "done",
                    "pipeline": PIPELINE,
                    "candidates": [
                        wm_cand("unknown", "@ZYFLS44", "@ZYFLS44", "account", 8)
                    ]
                },
                "face": {
                    "matches": [
                        {"name": "XMYao1998", "source": "face", "score": 0.8123, "support": 7}
                    ]
                }
            }),
        );

        // 3. Dreamymoo-28.mp4: WM soficoquita + conflicting folder dreamymoo
        let (p_d28, s_d28, m_d28) = fx.add_source_video("Dreamymoo-28.mp4", b"d28");
        fx.insert_task(
            "v_d28",
            &p_d28,
            s_d28,
            &m_d28,
            "review",
            "face",
            0,
            0,
            0,
            json!({
                "ready": true,
                "text_complete": true,
                "face_complete": true,
                "folder": [{"name": "dreamymoo", "source": "folder"}],
                "watermark": {
                    "status": "done",
                    "pipeline": PIPELINE,
                    "candidates": [
                        wm_cand("OnlyFans", "soficoquita", "OnlyFans.com/soficoquita", "account", 6)
                    ]
                },
                "face": {
                    "matches": [
                        {"name": "soficoquita", "source": "face", "score": 0.72, "support": 5},
                        {"name": "dreamymoo", "source": "face", "score": 0.65, "support": 5}
                    ]
                }
            }),
        );

        // 4. retsu_dao5 + fansly.com/retsu_dao (multi-watermark)
        let (p_re, s_re, m_re) = fx.add_source_video("retsu.mp4", b"retsu");
        fx.insert_task(
            "v_retsu",
            &p_re,
            s_re,
            &m_re,
            "review",
            "face",
            0,
            0,
            0,
            json!({
                "ready": true,
                "text_complete": true,
                "face_complete": true,
                "folder": [],
                "watermark": {
                    "status": "done",
                    "pipeline": PIPELINE,
                    "candidates": [
                        wm_cand("unknown", "@retsu_dao5", "@retsu_dao5", "account", 4),
                        wm_cand("Fansly", "retsu_dao", "fansly.com/retsu_dao", "account", 4)
                    ]
                },
                "face": {"matches": []}
            }),
        );

        let preview = preview_auto_watermark_matches(&fx.config).unwrap();
        assert_eq!(preview["eligible_count"], 0);
        assert_eq!(preview["skipped_summary"]["space_truncated"], 1);
        assert_eq!(preview["skipped_summary"]["face_contradicts"], 1);
        assert_eq!(preview["skipped_summary"]["folder_conflict"], 1);
        assert_eq!(preview["skipped_summary"]["multi_watermark"], 1);
    }

    #[test]
    fn non_existing_deleted_rejected_and_opt_out_blocked() {
        let fx = Fixture::new();
        fx.add_creator("deleted_girl", &[]);
        fx.add_creator("rejected_girl", &[]);
        fx.add_creator("opted_out_girl", &[]);
        fx.add_creator("unarchived_girl", &[]);
        fs::write(
            fx.config.base.join("people-deleted.json"),
            json!({"deleted_girl": {"operation": "op-del"}}).to_string(),
        )
        .unwrap();

        let (p_no, s_no, m_no) = fx.add_source_video("no_exist.mp4", b"1");
        let (p_del, s_del, m_del) = fx.add_source_video("del.mp4", b"2");
        let (p_rej, s_rej, m_rej) = fx.add_source_video("rej.mp4", b"3");
        let (p_opt, s_opt, m_opt) = fx.add_source_video("opt.mp4", b"4");
        let (p_un, s_un, m_un) = fx.add_source_video("unarchived.mp4", b"5");

        let rej_key = serde_json::to_string(&json!(["rejected_girl", "v_rej"])).unwrap();
        fs::write(
            fx.config.base.join("person-rejections.json"),
            json!({
                rej_key: {"id": "r1", "name": "rejected_girl", "vid": "v_rej", "path": p_rej, "size": s_rej, "mtime_ns": m_rej}
            })
            .to_string(),
        )
        .unwrap();
        // Undone history entry must NOT exclude v_un; active history entry MUST exclude.
        fs::write(
            fx.config.base.join("archive-history.json"),
            json!([
                {"vid": "v_un", "creator": "unarchived_girl", "undone": true}
            ])
            .to_string(),
        )
        .unwrap();

        let make_data = |handle: &str| {
            json!({
                "ready": true,
                "text_complete": true,
                "face_complete": true,
                "folder": [],
                "watermark": {
                    "status": "done",
                    "pipeline": PIPELINE,
                    "candidates": [wm_cand("OnlyFans", handle, &format!("onlyfans.com/{handle}"), "account", 4)]
                },
                "face": {"matches": []}
            })
        };

        fx.insert_task("v_no", &p_no, s_no, &m_no, "review", "face", 0, 0, 0, make_data("ghost_creator"));
        fx.insert_task("v_del", &p_del, s_del, &m_del, "review", "face", 0, 0, 0, make_data("deleted_girl"));
        fx.insert_task("v_rej", &p_rej, s_rej, &m_rej, "review", "face", 0, 0, 0, make_data("rejected_girl"));
        fx.insert_task("v_opt", &p_opt, s_opt, &m_opt, "review", "face", 0, 0, 1, make_data("opted_out_girl"));
        fx.insert_task("v_un", &p_un, s_un, &m_un, "review", "face", 0, 0, 0, make_data("unarchived_girl"));

        let preview = preview_auto_watermark_matches(&fx.config).unwrap();
        assert_eq!(preview["eligible_count"], 1);
        assert_eq!(preview["groups"][0]["person"], "unarchived_girl");
        assert_eq!(preview["skipped_summary"]["no_existing_person"], 2);
        assert_eq!(preview["skipped_summary"]["other"], 2);
    }

    #[test]
    fn keep_both_naming_handles_disk_and_batch_collisions() {
        let fx = Fixture::new();
        fx.add_creator("kovicki", &["clip.mp4"]);
        let dir = fx.config.creators().join("kovicki");
        let mut reserved = BTreeSet::new();

        let (t1, r1) = keep_both_target_name(&dir, "clip.mp4", "vid-a", &mut reserved).unwrap();
        assert!(r1);
        assert!(t1.starts_with("clip (保留两份-"));
        assert!(t1.ends_with(").mp4"));

        // Second identical vid collides with reserved -> gets -2 suffix
        let (t2, r2) = keep_both_target_name(&dir, "clip.mp4", "vid-a", &mut reserved).unwrap();
        assert!(r2);
        assert!(t2.ends_with("-2).mp4"));

        // Fresh filename passes without rename, then second video with same fresh filename gets keep-both
        let (t3, r3) = keep_both_target_name(&dir, "fresh.mp4", "vid-b", &mut reserved).unwrap();
        assert!(!r3);
        assert_eq!(t3, "fresh.mp4");
        let (t4, r4) = keep_both_target_name(&dir, "fresh.mp4", "vid-c", &mut reserved).unwrap();
        assert!(r4);
        assert!(t4.starts_with("fresh (保留两份-"));
    }

    #[test]
    fn slash_truncation_dual_platform_consensus_and_legacy_supersede() {
        let fx = Fixture::new();
        fx.add_creator("meriol", &[]);
        fx.add_creator("yuumeilyn", &[]);
        fx.add_creator("Lostwindlo-", &[]);
        fx.add_creator("hamayoko333", &[]);
        fx.add_creator("rurigraph", &[]);
        fx.add_creator("other_girl", &[]);

        let ruri_key = wm_key("unknown", "@Ruri_LapisL");
        fs::write(
            fx.config.base.join("watermarks/mappings.json"),
            json!({
                ruri_key: {"kind": "person", "creator": "rurigraph", "platform": "unknown", "token": "@Ruri_LapisL", "text": "@Ruri_LapisL"}
            })
            .to_string(),
        )
        .unwrap();
        // Stale report.json assigns both v_hama (face_complete=true) and v_ruri_legacy (face_skipped=true) to other_girl.
        fs::write(
            fx.config.base.join("report.json"),
            json!({
                "identities": [{
                    "name": "other_girl",
                    "vids": [{"vid": "v_hama"}, {"vid": "v_ruri_legacy"}]
                }]
            })
            .to_string(),
        )
        .unwrap();
        // Active archive history blocks v_active_hist, while undone:true does not.
        fs::write(
            fx.config.base.join("archive-history.json"),
            json!([
                {"vid": "v_active_hist", "creator": "yuumeilyn"},
                {"vid": "v_yuumei", "creator": "yuumeilyn", "undone": true}
            ])
            .to_string(),
        )
        .unwrap();

        // 1. Slash truncation: fansly.com/meriol/chan -> token "meriol" -> SpaceTruncated
        let (p1, s1, m1) = fx.add_source_video("meriol.mp4", b"m");
        fx.insert_task(
            "v_meriol",
            &p1,
            s1,
            &m1,
            "review",
            "face",
            0,
            0,
            0,
            json!({
                "ready": true,
                "text_complete": true,
                "face_complete": true,
                "folder": [],
                "watermark": {
                    "status": "done",
                    "pipeline": PIPELINE,
                    "candidates": [wm_cand("Fansly", "meriol", "fansly.com/meriol/chan", "account", 4)]
                },
                "face": {"matches": []}
            }),
        );

        // 2. Dual-platform agreement (X.com/Yuumeilyn + Onlyfans.com/Yuumeilyn) + undone history -> Eligible
        let (p2, s2, m2) = fx.add_source_video("yuumei.mp4", b"y");
        fx.insert_task(
            "v_yuumei",
            &p2,
            s2,
            &m2,
            "review",
            "face",
            0,
            0,
            0,
            json!({
                "ready": true,
                "text_complete": true,
                "face_complete": true,
                "folder": [{"name": "yuumeilyn", "source": "folder"}],
                "watermark": {
                    "status": "done",
                    "pipeline": PIPELINE,
                    "candidates": [
                        wm_cand("X", "Yuumeilyn", "X.com/Yuumeilyn", "account", 6),
                        wm_cand("OnlyFans", "Yuumeilyn", "Onlyfans.com/Yuumeilyn/", "account", 8)
                    ]
                },
                "face": {"matches": [{"name": "yuumeilyn", "source": "face", "score": 0.62, "support": 6}]}
            }),
        );

        // 3. Active archive-history excludes v_active_hist
        let (p2b, s2b, m2b) = fx.add_source_video("yuumei_archived.mp4", b"ya");
        fx.insert_task(
            "v_active_hist",
            &p2b,
            s2b,
            &m2b,
            "review",
            "face",
            0,
            0,
            0,
            json!({
                "ready": true,
                "text_complete": true,
                "face_complete": true,
                "folder": [],
                "watermark": {
                    "status": "done",
                    "pipeline": PIPELINE,
                    "candidates": [wm_cand("OnlyFans", "Yuumeilyn", "Onlyfans.com/Yuumeilyn", "account", 6)]
                },
                "face": {"matches": []}
            }),
        );

        // 4. NFC+lowercase rescue: @lostwindlo- -> existing dir Lostwindlo-
        let (p3, s3, m3) = fx.add_source_video("lost.mp4", b"l");
        fx.insert_task(
            "v_lost",
            &p3,
            s3,
            &m3,
            "review",
            "face",
            0,
            0,
            0,
            json!({
                "ready": true,
                "text_complete": true,
                "face_complete": true,
                "folder": [],
                "watermark": {
                    "status": "done",
                    "pipeline": PIPELINE,
                    "candidates": [wm_cand("unknown", "@lostwindlo-", "@lostwindlo-", "account", 2)]
                },
                "face": {"matches": []}
            }),
        );

        // 5. Modern face_complete=true supersedes stale report.json (hamayoko333 passes!)
        let (p4, s4, m4) = fx.add_source_video("hama.mp4", b"h");
        fx.insert_task(
            "v_hama",
            &p4,
            s4,
            &m4,
            "review",
            "face",
            0,
            0,
            0,
            json!({
                "ready": true,
                "text_complete": true,
                "face_complete": true,
                "folder": [],
                "watermark": {
                    "status": "done",
                    "pipeline": PIPELINE,
                    "candidates": [wm_cand("unknown", "@hamayoko333", "@hamayoko333", "account", 6)]
                },
                "face": {"matches": []}
            }),
        );

        // 6. When face_complete != true (face_skipped=true), legacy report.json conflict STILL blocks!
        let (p5, s5, m5) = fx.add_source_video("ruri_legacy.mp4", b"rl");
        fx.insert_task(
            "v_ruri_legacy",
            &p5,
            s5,
            &m5,
            "review",
            "text",
            0,
            0,
            0,
            json!({
                "ready": true,
                "text_complete": true,
                "face_skipped": true,
                "folder": [],
                "watermark": {
                    "status": "done",
                    "pipeline": PIPELINE,
                    "candidates": [wm_cand("unknown", "@Ruri_LapisL", "@Ruri_LapisL", "account", 8)]
                }
            }),
        );

        let preview = preview_auto_watermark_matches(&fx.config).unwrap();
        assert_eq!(preview["eligible_count"], 3);
        let people: BTreeSet<&str> = preview["groups"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|g| g["person"].as_str())
            .collect();
        assert_eq!(people, BTreeSet::from(["Lostwindlo-", "hamayoko333", "yuumeilyn"]));
        assert_eq!(preview["skipped_summary"]["space_truncated"], 1);
        assert_eq!(preview["skipped_summary"]["other"], 2);
    }

    #[test]
    fn space_truncation_helper_matches_live_cases() {
        assert!(is_space_truncated_ocr("onlyfans.com/YunaTamago zZ", "YunaTamago"));
        assert!(is_space_truncated_ocr("onlyfans.com/YunaTamago zz", "YunaTamago"));
        assert!(is_space_truncated_ocr("OnlyFans.com/free kovicki", "free"));
        assert!(is_space_truncated_ocr("OnlyFans.com/room 103", "room"));
        assert!(is_space_truncated_ocr("fansly.com/retsu dao", "retsu"));
        assert!(is_space_truncated_ocr("fansly.com/meriol/chan", "meriol"));
        assert!(!is_space_truncated_ocr("OnlyFans.com/dreamymoo", "dreamymoo"));
        assert!(!is_space_truncated_ocr("OnlyFans.com/dreamymoo/", "dreamymoo"));
        assert!(!is_space_truncated_ocr("X@riorio0701", "riorio0701"));
        assert!(!is_space_truncated_ocr("@Lostwindlo-", "@Lostwindlo-"));
        assert!(!is_space_truncated_ocr("onlyfans.com/YunaTamago_zZ", "YunaTamago_zZ"));
    }

    #[test]
    fn unicode_case_expansion_and_long_extension_are_safe() {
        // Multi-byte Unicode case expansion (Turkish İ -> i + combining dot), fullwidth, CJK:
        assert!(!is_space_truncated_ocr("OnlyFans.com/İstanbül", "İstanbül"));
        assert!(is_space_truncated_ocr("OnlyFans.com/İstanbül extra", "İstanbül"));
        assert!(is_space_truncated_ocr("onlyfans.com/İ_prefix/alice tail", "alice"));
        assert!(!is_space_truncated_ocr("onlyfans.com/İ_prefix/alice/", "alice"));
        assert!(is_space_truncated_ocr("ＯｎｌｙＦａｎｓ．ｃｏｍ／ＹｕｎａＴａｍａｇｏ　ｚＺ", "YunaTamago"));
        assert!(!is_space_truncated_ocr("ＯｎｌｙＦａｎｓ．ｃｏｍ／ｄｒｅａｍｙｍｏｏ", "dreamymoo"));
        assert!(!is_space_truncated_ocr("抖音号：猫猫酱", "猫猫酱"));
        assert!(is_space_truncated_ocr("抖音号：猫猫酱 二号", "猫猫酱"));

        // Long extension (>240 bytes total suffix) must terminate cleanly without infinite loop.
        let long_ext = "a".repeat(235);
        let long_name = format!("clip.{long_ext}");
        let result = make_keep_both_name(&long_name, "vid-long-ext", 1);
        assert!(result.starts_with(" (保留两份-"));
        assert!(result.ends_with(&format!(".{long_ext}")));
    }
}
