//! Read-only evidence bridge for immutable feature caches after committed native moves.
//! Cache/model interpretation stays in the worker; journal interpretation stays here.
use crate::{
    db,
    engine::Engine,
    files,
    model::{self, Error, Plan, Result},
    processing_result, review,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{fs, path::Path};

fn rejected() -> Error {
    Error::conflict("特征来源没有完整且仍有效的原生移动回执")
}
fn text<'a>(body: &'a Value, key: &str) -> Result<&'a str> {
    body[key]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(rejected)
}
fn identity_matches(a: &Value, b: &Value) -> bool {
    ["dev", "ino", "size", "mtime_ns"]
        .iter()
        .all(|k| a[*k].is_string() && a[*k] == b[*k])
}
fn origin_matches(origin: &Value, path: &str, identity: &Value) -> bool {
    origin["path"] == path
        && ["size", "mtime_ns"]
            .iter()
            .all(|k| origin[*k] == identity[*k])
        && ["dev", "ino"]
            .iter()
            .all(|k| origin.get(*k).is_none_or(|v| *v == identity[*k]))
}
pub fn verify(engine: &Engine, body: &Value) -> Result<Value> {
    let config = &engine.config;
    let vid = text(body, "vid")?;
    let source = text(body, "source")?;
    let cache = Path::new(text(body, "cache")?);
    let digest = text(body, "cache_sha256")?;
    let origin = &body["origin"];
    let sources = review::indexed_sources(config)?;
    if review::source(config, vid, &sources).as_deref() != Some(source)
        || !Path::new(source).starts_with(config.media.join("pikpak"))
        || fs::canonicalize(source)? != Path::new(source)
        || fs::canonicalize(cache)? != cache
        || processing_result::artifact_hash(cache)? != digest
    {
        return Err(rejected());
    }
    let current = files::identity(&fs::metadata(source)?);
    if !identity_matches(&current, &body["current_identity"]) {
        return Err(rejected());
    }
    let legacy = config
        .base
        .join("processing/faces")
        .join(format!("{:x}.npz", Sha256::digest(vid.as_bytes())));
    if cache != legacy {
        if !cache.starts_with(config.base.join("processing/attempts"))
            || cache.file_name().and_then(|s| s.to_str()) != Some("features.npz")
        {
            return Err(rejected());
        }
        let conn = db::open(&config.base.join("processing/processing.db"))?;
        let mut found = false;
        for row in db::rows(
            &conn,
            "SELECT result FROM _face_admin_processing_results WHERE json_extract(result,'$.vid')=?",
            [vid],
        )? {
            let result: Value = serde_json::from_str(row["result"].as_str().ok_or_else(rejected)?)?;
            let face = &result["data"]["face"];
            let identity = face
                .get("source_identity")
                .unwrap_or(&result["source_identity"]);
            if result["ok"] == true
                && result["accepted"] == true
                && face["feature_cache"] == cache.to_str().unwrap()
                && face["feature_cache_sha256"] == digest
                && origin_matches(
                    origin,
                    result["task"]["path"].as_str().unwrap_or(""),
                    identity,
                )
                && ["dev", "ino"].iter().all(|k| origin[*k] == identity[*k])
            {
                found = true;
                break;
            }
        }
        if !found {
            return Err(rejected());
        }
    }
    let operations = db::rows(
        &engine.store.db,
        "SELECT rowid AS sequence,id,plan,evidence FROM operations WHERE status='complete' AND json_array_length(plan,'$.files')>0 ORDER BY rowid DESC",
        [],
    )?;
    let mut endpoint = source.to_owned();
    let mut identity = current.clone();
    let mut chain = vec![];
    for row in operations {
        let plan: Plan = serde_json::from_str(row["plan"].as_str().ok_or_else(rejected)?)?;
        let admitted: Value = serde_json::from_str(row["evidence"].as_str().ok_or_else(rejected)?)?;
        for (index, file) in plan.files.iter().enumerate() {
            if file.dst != endpoint || !file.vids.iter().any(|v| v == vid) {
                continue;
            }
            if let Some(scope) = engine.store.latest_step(&plan.id, "commit_scope")?
                && !scope["successful_file_indexes"]
                    .as_array()
                    .is_some_and(|a| a.contains(&json!(index)))
            {
                continue;
            }
            let from = if admitted["files"].is_array() {
                &admitted["files"][index]["source_identity"]
            } else {
                &admitted["source_identity"]
            };
            if file.version
                != model::hash(&json!([
                    file.src,
                    from["dev"],
                    from["ino"],
                    from["size"],
                    from["mtime_ns"]
                ]))
            {
                return Err(rejected());
            }
            let evidence = engine.store.file_evidence(&plan.id, index)?;
            let to = evidence.copy.as_ref().map(|p| &p.identity).unwrap_or(from);
            if !identity_matches(to, &identity) {
                continue;
            }
            if evidence.copy.as_ref().is_some_and(|p| p.digest.is_none()) {
                return Err(rejected());
            }
            if chain.is_empty() {
                files::inspect_reference_destination(file, &evidence)?;
            }
            chain.push(json!({"id":plan.id,"file_index":index,"version":plan.version,"move_evidence":evidence}));
            endpoint = file.src.clone();
            identity = from.clone();
            if origin_matches(origin, &endpoint, &identity) {
                // Recheck the exact current file and cache after reading the chain.
                if files::identity(&fs::metadata(source)?) != current
                    || processing_result::artifact_hash(cache)? != digest
                {
                    return Err(rejected());
                }
                let proof = json!({"vid":vid,"cache":cache,"cache_sha256":digest,"origin":origin,"source":source,"current_identity":current,"chain":chain});
                return Ok(
                    json!({"verified":true,"version":model::hash(&proof),"current_identity":current,"cache_sha256":digest}),
                );
            }
            break;
        }
    }
    Err(rejected())
}
