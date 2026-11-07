//! Selected-file metadata deltas. Identifiers and raw OCR/audit strings are never rewritten.
use crate::{
    config::Config,
    db, files,
    metadata::{self, Document, Snapshot},
    model::{Plan, Result},
    person_registration::document,
};
use serde_json::{Value, json};
use std::{collections::BTreeSet, fs, path::Path};
const DOCUMENTS: &[&str] = &[
    "people-deleted.json",
    "report.json",
    "claims-detail.json",
    "clusters.json",
    "integration-queue.json",
    "archive-history.json",
    "people-manual.json",
    "template-overrides.json",
    "watermarks/mappings.json",
    "person-rejections.json",
    "video-path-overrides.json",
    "person-renames.json",
    "external-profiles/manual-links.json",
    "external-profiles/reviewed-links.json",
];
fn selected(plan: &Plan, id: &str) -> bool {
    plan.files.iter().any(|f| f.vids.iter().any(|v| v == id))
}
fn owner_transform(value: &Value, plan: &Plan) -> Value {
    match value {
        Value::Array(rows) => Value::Array(rows.iter().map(|v| owner_transform(v, plan)).collect()),
        Value::Object(fields) => {
            let is_selected = value["vid"].as_str().is_some_and(|id| selected(plan, id));
            Value::Object(
                fields
                    .iter()
                    .map(|(key, v)| {
                        let next = if is_selected
                            && ["name", "creator"].contains(&key.as_str())
                            && v == &plan.person
                            && plan.action == "move-files"
                        {
                            json!(plan.target_person)
                        } else {
                            owner_transform(v, plan)
                        };
                        (key.clone(), next)
                    })
                    .collect(),
            )
        }
        _ => value.clone(),
    }
}
fn updated(config: &Config, name: &str, value: &Value, plan: &Plan) -> Value {
    if name.starts_with("external-profiles/") {
        return value.clone();
    }
    let mut next = owner_transform(&metadata::transform(config, value, plan), plan);
    if name == "video-path-overrides.json" {
        next = Value::Object(
            value
                .as_object()
                .unwrap()
                .iter()
                .map(|(id, v)| {
                    (
                        id.clone(),
                        v.as_str()
                            .map(|s| json!(metadata::mapped(config, s, plan)))
                            .unwrap_or(v.clone()),
                    )
                })
                .collect(),
        );
        for file in &plan.files {
            for id in &file.vids {
                next[id] = json!(file.dst);
            }
        }
    }
    if name == "archive-history.json" {
        for row in next.as_array_mut().unwrap() {
            if row["vid"].as_str().is_some_and(|id| selected(plan, id)) {
                row["last_file_operation"] = json!(plan.id);
            }
        }
    }
    if name == "person-rejections.json" {
        next = Value::Object(
            next.as_object()
                .unwrap()
                .values()
                .map(|row| {
                    (
                        serde_json::to_string(&json!([row["name"], row["vid"]])).unwrap(),
                        row.clone(),
                    )
                })
                .collect(),
        );
    }
    if name == "report.json" && plan.action == "move-files" {
        let mut identities = next["identities"].as_array().cloned().unwrap_or_default();
        let mut moved = vec![];
        let thumbs = plan.execution.as_ref().unwrap().data["affected_thumbs"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        for person in &mut identities {
            let keep = person["name"] == plan.target_person;
            let vids = person["vids"].as_array().cloned().unwrap_or_default();
            let (removed, retained): (Vec<_>, Vec<_>) = vids
                .into_iter()
                .partition(|v| !keep && v["vid"].as_str().is_some_and(|id| selected(plan, id)));
            let removed_any = !removed.is_empty();
            moved.extend(removed);
            let thumb = person["rep_thumb"].as_str().unwrap_or("");
            let invalid = thumbs.iter().any(|t| {
                t.as_str()
                    .is_some_and(|t| thumb == t || thumb == format!("/thumb?p={}", encode_query(t)))
            });
            if (retained.is_empty() && removed_any) || invalid {
                person["rep_thumb"] = Value::Null;
                person["tpl_faces"] = json!(0);
            }
            person["vids"] = json!(retained);
        }
        if !moved.is_empty() {
            let target_index = identities
                .iter()
                .position(|p| p["name"] == plan.target_person)
                .unwrap_or_else(|| {
                    identities.push(json!({"name":plan.target_person,"vids":[],"tpl_faces":0}));
                    identities.len() - 1
                });
            let mut vids = identities[target_index]["vids"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            for row in moved {
                if let Some(index) = vids.iter().position(|v| v["vid"] == row["vid"]) {
                    vids[index] = row;
                } else {
                    vids.push(row);
                }
            }
            identities[target_index]["vids"] = json!(vids);
        }
        next["identities"] = json!(identities);
    }
    if name == "people-manual.json"
        && plan.action == "move-files"
        && !next
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["name"] == plan.target_person)
    {
        let timestamp = &plan.execution.as_ref().unwrap().data["timestamp"];
        next.as_array_mut().unwrap().push(json!({"name":plan.target_person,"created_at":timestamp,"last_used_at":timestamp,"rep_thumb":null}));
    }
    next
}
fn encode_query(s: &str) -> String {
    s.bytes()
        .map(|c| {
            if c.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&c) {
                (c as char).to_string()
            } else {
                format!("%{c:02X}")
            }
        })
        .collect()
}
fn selected_rows(
    config: &Config,
    connection: &rusqlite::Connection,
    table: &str,
    plan: &Plan,
) -> Result<Vec<Value>> {
    let mut ids = BTreeSet::new();
    let mut paths = BTreeSet::new();
    for file in &plan.files {
        ids.extend(file.vids.iter().cloned());
        paths.insert(file.src.clone());
        paths.insert(metadata::win(config, &file.src));
    }
    let placeholders = |n| std::iter::repeat_n("?", n).collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT * FROM {table} WHERE vid IN ({}) OR path IN ({}) ORDER BY vid",
        placeholders(ids.len()),
        placeholders(paths.len())
    );
    db::rows(
        connection,
        &sql,
        rusqlite::params_from_iter(ids.iter().chain(&paths)),
    )
}
fn delta(table: db::Table, before: Vec<Value>, after: Vec<Value>, plan: &Plan) -> db::Delta {
    let scope = before
        .iter()
        .chain(&after)
        .map(|r| r["vid"].as_str().unwrap().to_owned())
        .chain(plan.files.iter().flat_map(|f| f.vids.iter().cloned()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|id| json!({"vid":id}))
        .collect();
    db::Delta {
        tables: vec![db::TableDelta {
            table,
            key_columns: vec!["vid".into()],
            scope,
            before,
            after,
        }],
        guards: vec![],
    }
}
pub fn snapshot(config: &Config, plan: &Plan) -> Result<Snapshot> {
    let mut documents = vec![];
    let mut document_bytes = 0usize;
    for name in DOCUMENTS {
        let array = [
            "claims-detail.json",
            "clusters.json",
            "integration-queue.json",
            "archive-history.json",
            "people-manual.json",
        ]
        .contains(name);
        let (before, value) = document(config, name, array)?;
        let next = updated(config, name, &value, plan);
        let after = if value == next {
            before.clone()
        } else {
            Some(serde_json::to_string_pretty(&next)?)
        };
        document_bytes = document_bytes
            .saturating_add(before.as_ref().map_or(0, String::len))
            .saturating_add(after.as_ref().map_or(0, String::len));
        if document_bytes > 64 * 1024 * 1024 {
            return Err(crate::model::Error::new(
                413,
                "snapshot_limit",
                "操作元数据快照超过 64 MiB 安全限制",
            ));
        }
        documents.push(Document {
            name: (*name).into(),
            before,
            after,
        });
    }
    let connection = db::open(&config.base.join("faces.db"))?;
    let before = selected_rows(config, &connection, "videos", plan)?;
    let after = before
        .iter()
        .map(|row| {
            let mut next = row.clone();
            if let Some(file) = plan.files.iter().find(|f| {
                f.vids.iter().any(|v| row["vid"] == v)
                    || row["path"] == f.src
                    || row["path"] == metadata::win(config, &f.src)
            }) {
                next["path"] = json!(file.dst);
                if row.get("name").is_some() {
                    next["name"] =
                        json!(Path::new(&file.dst).file_name().unwrap().to_string_lossy());
                }
                if row.get("collection").is_some() {
                    next["collection"] =
                        json!(Path::new(&file.dst).parent().unwrap().to_string_lossy());
                }
            }
            next
        })
        .collect();
    let faces_delta = Some(delta(db::Table::Videos, before, after, plan));
    let jobs_path = config.base.join("processing/processing.db");
    let jobs_delta = if jobs_path.exists() {
        let before = selected_rows(config, &db::open(&jobs_path)?, "tasks", plan)?;
        let mut after = vec![];
        let timestamp = plan.execution.as_ref().unwrap().data["timestamp"]
            .as_str()
            .unwrap();
        let updated_at: f64 =
            connection.query_row("SELECT unixepoch(?,'subsec')", [timestamp], |r| r.get(0))?;
        for row in &before {
            let mut next = row.clone();
            if let Some(file) = plan.files.iter().find(|f| {
                f.vids.iter().any(|v| row["vid"] == v)
                    || row["path"] == f.src
                    || row["path"] == metadata::win(config, &f.src)
            }) {
                let stat = fs::metadata(&file.src)?;
                let data: Value = serde_json::from_str(
                    row["data"]
                        .as_str()
                        .ok_or_else(|| crate::model::Error::conflict("任务数据无效"))?,
                )?;
                next["path"] = json!(file.dst);
                next["name"] = json!(Path::new(&file.dst).file_name().unwrap().to_string_lossy());
                next["size"] = json!({"$integer":file.size.to_string()});
                next["mtime_ns"] = json!(files::mtime_ns(&stat));
                next["stage"] = json!("excluded");
                next["updated_at"] = json!(updated_at);
                next["data"] = json!(serde_json::to_string(&owner_transform(
                    &metadata::transform(config, &data, plan),
                    plan
                ))?);
            }
            after.push(next);
        }
        Some(delta(db::Table::Tasks, before, after, plan))
    } else {
        None
    };
    Ok(Snapshot {
        documents,
        videos: vec![],
        jobs: vec![],
        faces_delta,
        jobs_delta,
    })
}
