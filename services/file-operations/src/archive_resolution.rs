//! Resolution persists intent only, using the same local metadata transaction as review.
use crate::{
    archive,
    config::Config,
    files,
    model::{Error, Result},
    review::{self, Prepared},
};
use serde_json::{Value, json};
use std::path::Path;

pub fn alternate_name(name: &str, vid: &str) -> String {
    use sha2::{Digest, Sha256};
    let p = Path::new(name);
    let digest = format!("{:x}", Sha256::digest(vid.as_bytes()));
    let suffix = format!(
        " (保留两份-{}).{}",
        &digest[..8],
        p.extension().unwrap_or_default().to_string_lossy()
    );
    let mut stem = p
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    while stem.len() + suffix.len() > 240 {
        stem.pop();
    }
    format!("{stem}{suffix}")
}
pub fn validate_name(name: &str, original: &str) -> Result<()> {
    files::validate_name(name)?;
    if Path::new(name).extension().map(|s| s.to_ascii_lowercase())
        != Path::new(original)
            .extension()
            .map(|s| s.to_ascii_lowercase())
    {
        return Err(Error::new(400, "invalid_request", "请保留视频原来的扩展名"));
    }
    Ok(())
}
pub fn prepare(config: &Config, body: &Value) -> Result<Prepared> {
    let ids = review::vids(&body["vids"])?;
    let kind = body["kind"]
        .as_str()
        .filter(|s| ["rename", "stage-duplicate", "clear"].contains(s))
        .ok_or_else(|| Error::new(400, "invalid_request", "请选择处理方式"))?;
    let current = archive::preview(config)?;
    if current["version"] != body["version"] {
        return Err(Error::conflict("文件状态已变化，请刷新后重新选择"));
    }
    let selected = current["rows"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| ids.iter().any(|id| r["vid"] == *id))
        .collect::<Vec<_>>();
    if selected.len() != ids.len() {
        return Err(Error::conflict("部分任务已不在待归档中，请刷新"));
    }
    if kind != "clear"
        && selected.iter().any(|r| {
            !["duplicate", "conflict", "dupe-target"]
                .iter()
                .any(|s| r["status"] == *s)
                && r["resolution"].is_null()
        })
    {
        return Err(Error::conflict("只可为同名文件选择处理方式"));
    }
    if kind == "stage-duplicate"
        && selected.iter().any(|r| {
            r["original_size"] != r["size"]
                || r["size"].as_u64().unwrap_or(0) == 0
                || r["src"].is_null()
        })
    {
        return Err(Error::conflict(
            "体积不同，无法作为重复件暂存；请选择保留两份",
        ));
    }
    let (_, mut queue) = review::read(config, "integration-queue.json", json!([]))?;
    for item in queue.as_array_mut().unwrap() {
        let Some(row) = selected.iter().find(|r| r["vid"] == item["vid"]) else {
            continue;
        };
        item.as_object_mut().unwrap().remove("resolution");
        if kind == "rename" {
            let filename = if ids.len() == 1 && body.get("name").is_some() {
                body["name"]
                    .as_str()
                    .ok_or_else(|| Error::new(400, "invalid_request", "文件名无效"))?
                    .to_owned()
            } else {
                alternate_name(row["name"].as_str().unwrap(), row["vid"].as_str().unwrap())
            };
            validate_name(&filename, row["name"].as_str().unwrap())?;
            item["resolution"] = json!({"kind":"rename","name":filename});
        } else if kind == "stage-duplicate" {
            item["resolution"] = json!({"kind":"stage-duplicate"});
        }
    }
    let next = archive::preview_queue(config, &queue)?;
    if kind != "clear"
        && next["rows"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| ids.iter().any(|id| r["vid"] == *id) && r["status"] != "ok")
    {
        return Err(Error::conflict(
            "所选路径仍存在冲突，请换一个文件名或刷新检查",
        ));
    }
    Ok(Prepared {
        documents: vec![review::document(
            config,
            "integration-queue.json",
            json!([]),
            queue,
        )?],
        jobs: vec![],
        job_updates: vec![],
        settings_before: vec![],
        settings_after: vec![],
        guards: json!({"version":current["version"]}),
        result: json!({"ok":true,"prepared":ids.len(),"version":next["version"]}),
        optional_rule: None,
        optional_restore: None,
    })
}
