//! Read-only evidence bridge for immutable feature caches after committed native moves.
//! Cache/model interpretation stays in the worker; journal interpretation stays here.
use crate::{
    config::Config,
    db,
    engine::Engine,
    files,
    model::{self, Error, FileMove, Result},
    processing_result, review, store,
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
    let result = verify_origin(config, &engine.store.db, body)?;
    if processing_result::artifact_hash(cache)? != digest {
        return Err(rejected());
    }
    Ok(
        json!({"verified":true,"version":model::hash(&json!([result["version"],cache,digest])),"current_identity":current,"cache_sha256":digest}),
    )
}

/// Scalar-only release diagnostic. This does not authorize or adopt a feature
/// cache: it reads no NPZ member/media content and is not used by model loading.
pub fn diagnostic(config: &Config, body: &Value) -> Result<Value> {
    let db = rusqlite::Connection::open_with_flags(
        config.state_db(),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    db.busy_timeout(std::time::Duration::from_secs(2))?;
    let schema: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if schema != 9 {
        return Err(rejected());
    }
    let check = || -> Result<()> {
        if crate::control::read(&config.base.join(".review-write.lock"), crate::control::MARKER_LIMIT)?.is_some()
            || db.query_row("SELECT EXISTS(SELECT 1 FROM operations WHERE status IN ('accepted','running','recovery_needed'))", [], |r| r.get::<_,bool>(0))? { return Err(rejected()); }
        Ok(())
    };
    check()?;
    let result = verify_origin(config, &db, body)?;
    check()?;
    Ok(result)
}

fn verify_origin(config: &Config, db: &rusqlite::Connection, body: &Value) -> Result<Value> {
    let vid = text(body, "vid")?;
    let source = text(body, "source")?;
    let origin = &body["origin"];
    if !Path::new(source).starts_with(config.media.join("pikpak"))
        || fs::canonicalize(source)? != Path::new(source)
    {
        return Err(rejected());
    }
    let current = files::identity(&fs::metadata(source)?);
    if !identity_matches(&current, &body["current_identity"]) {
        return Err(rejected());
    }
    // Only this stable video's history is streamed, with the same fixed bounds
    // for strict cache verification and the scalar-only release diagnostic.
    // Scalar projections deliberately avoid fetching complete Plan/DomainSnapshot
    // JSON, which can contain historical embedding BLOB snapshots.
    let mut query = db.prepare(
        "SELECT o.id,json_extract(o.plan,'$.version'),f.key,f.value,CASE WHEN json_type(o.evidence,'$.files')='array' THEN json_extract(o.evidence,'$.files[' || f.key || '].source_identity') ELSE json_extract(o.evidence,'$.source_identity') END FROM operations o JOIN json_each(o.plan,'$.files') f WHERE o.status='complete' AND EXISTS(SELECT 1 FROM json_each(f.value,'$.vids') v WHERE v.value=?1) ORDER BY o.rowid DESC,f.key LIMIT 1025")?;
    let mut operations = query.query([vid])?;
    let mut endpoint = source.to_owned();
    let mut identity = current.clone();
    let mut chain = vec![];
    let mut visited = 0;
    let mut used_operation = String::new();
    while let Some(row) = operations.next()? {
        visited += 1;
        if visited > 1024 || chain.len() >= 128 {
            return Err(rejected());
        }
        let id: String = row.get(0)?;
        if id == used_operation {
            continue;
        }
        let version: String = row.get(1)?;
        let index = usize::try_from(row.get::<_, i64>(2)?).map_err(|_| rejected())?;
        let file: FileMove = serde_json::from_str(&row.get::<_, String>(3)?)?;
        if file.dst != endpoint || !file.vids.iter().any(|v| v == vid) {
            continue;
        }
        if !store::read_committed_file(db, &id, index)? {
            continue;
        }
        let from: Value = serde_json::from_str(&row.get::<_, String>(4)?)?;
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
        let evidence = store::read_file_evidence(db, &id, index)?;
        let to = evidence.copy.as_ref().map(|p| &p.identity).unwrap_or(&from);
        if !identity_matches(to, &identity) {
            continue;
        }
        if evidence.copy.as_ref().is_some_and(|p| p.digest.is_none()) {
            return Err(rejected());
        }
        if chain.is_empty() {
            files::inspect_reference_destination(&file, &evidence)?;
        }
        chain.push(json!({"id":id,"file_index":index,"version":version,"move_evidence":evidence}));
        used_operation = id;
        endpoint = file.src.clone();
        identity = from;
        if origin_matches(origin, &endpoint, &identity) {
            if files::identity(&fs::metadata(source)?) != current {
                return Err(rejected());
            }
            let proof = json!({"vid":vid,"origin":origin,"source":source,"current_identity":current,"chain":chain});
            return Ok(
                json!({"verified":true,"metadata_only":true,"version":model::hash(&proof),"current_identity":current}),
            );
        }
    }
    Err(rejected())
}
