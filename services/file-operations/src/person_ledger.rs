//! Native Peach selected-file path/creator changes, preserving all unrelated roles and fields.
use crate::{
    config::Config,
    db, ledger,
    metadata::win,
    model::{Error, Plan, Result},
};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::collections::BTreeMap;
fn integer(n: i64) -> Value {
    json!({"$integer":n.to_string()})
}
fn id(row: &Value) -> Result<i64> {
    db::integer(&row["id"])
}
fn key(row: &Value, names: &[&str]) -> Value {
    Value::Object(
        names
            .iter()
            .map(|k| ((*k).into(), row[*k].clone()))
            .collect(),
    )
}
/// Fill actual schema defaults without mutating even a temporary production row during preview.
fn new_row(connection: &Connection, table: &str, fields: Value) -> Result<Value> {
    let mut row = serde_json::Map::new();
    for column in db::rows(connection, &format!("PRAGMA table_info({table})"), [])? {
        let name = column["name"].as_str().unwrap();
        let value = if let Some(value) = fields.get(name) {
            value.clone()
        } else if let Some(default) = column["dflt_value"].as_str() {
            let default = db::rows(connection, &format!("SELECT {default} AS value"), [])?;
            default[0]["value"].clone()
        } else if db::integer(&column["notnull"])? != 0 {
            return Err(
                Error::new(503, "ledger_schema", "Peach 新记录有未支持的必填字段")
                    .at(format!("{table}.{name}")),
            );
        } else {
            Value::Null
        };
        row.insert(name.into(), value);
    }
    Ok(Value::Object(row))
}
pub fn snapshot(config: &Config, plan: &Plan) -> Result<ledger::Delta> {
    if plan.action == "rename-person" {
        return rename_snapshot(config, plan);
    }
    let mut connection = db::open(&config.ledger)?;
    let tx = connection.transaction()?;
    let mut assets = vec![];
    let mut next_assets = vec![];
    let mut asset_scope = vec![];
    let mut guards = vec![];
    let mut links = vec![];
    let mut next_links = vec![];
    let mut link_scope = vec![];
    let source = db::rows(
        &tx,
        "SELECT * FROM entity WHERE kind='creator' AND (canonical_name=? OR normalized_name=?) ORDER BY id",
        params![plan.person, plan.person.to_lowercase()],
    )?;
    let target = db::rows(
        &tx,
        "SELECT * FROM entity WHERE kind='creator' AND (canonical_name=? OR normalized_name=?) ORDER BY id",
        params![plan.target_person, plan.target_person.to_lowercase()],
    )?;
    if source.len() > 1 || target.len() > 1 {
        return Err(Error::new(409, "ledger_conflict", "Peach 人物记录不唯一"));
    }
    let mut entities: BTreeMap<i64, Value> = BTreeMap::new();
    for row in source.iter().chain(&target) {
        entities.insert(id(row)?, row.clone());
    }
    let entity_scope: Vec<Value> = entities.values().map(|r| key(r, &["id"])).collect();
    for file in &plan.files {
        let old = win(config, &file.src);
        let new = win(config, &file.dst);
        let rows = db::rows(
            &tx,
            "SELECT * FROM asset WHERE path=? AND disposal IS NULL ORDER BY id",
            [&old],
        )?;
        if rows.len() > 1 {
            return Err(Error::new(
                409,
                "ledger_conflict",
                "Peach 原路径存在多个有效记录，请先核对",
            )
            .at(old));
        }
        if !db::rows(&tx, "SELECT id FROM asset WHERE path=?", [&new])?.is_empty() {
            return Err(Error::new(409, "ledger_conflict", "Peach 目标路径已有记录").at(new));
        }
        // The selectors guard both late target occupation and source registrations.
        let old_all = db::rows(&tx, "SELECT * FROM asset WHERE path=? ORDER BY id", [&old])?;
        let old_after = old_all
            .iter()
            .filter(|r| !rows.iter().any(|active| active["id"] == r["id"]))
            .cloned()
            .collect();
        guards.push(db::Guard::AssetPath {
            path: old.clone(),
            before: old_all,
            after: old_after,
        });
        let new_after = rows
            .iter()
            .map(|r| {
                let mut row = r.clone();
                row["path"] = json!(new);
                row
            })
            .collect();
        guards.push(db::Guard::AssetPath {
            path: new.clone(),
            before: vec![],
            after: new_after,
        });
        for row in rows {
            let asset_id = id(&row)?;
            let mut next = row.clone();
            next["path"] = json!(new);
            let related = db::rows(
                &tx,
                "SELECT * FROM asset_entity WHERE asset_id=? ORDER BY entity_id,role,source",
                [asset_id],
            )?;
            asset_scope.push(key(&row, &["id"]));
            links.extend(related);
            assets.push(row);
            next_assets.push(next);
        }
    }
    let next_entities = entities.clone();
    let changed_owner =
        plan.action == "move-files" && plan.person != plan.target_person && !assets.is_empty();
    let target_id = if changed_owner {
        target.first().map(id).transpose()?
    } else {
        None
    };
    let mut creators = vec![];
    if changed_owner && target_id.is_none() {
        let timestamp = &plan.execution.as_ref().unwrap().data["timestamp"];
        let mut pending_links = vec![];
        for asset in &assets {
            let mut row = new_row(
                &tx,
                "asset_entity",
                json!({"asset_id":asset["id"],"entity_id":null,"role":"creator","source":"face-persons","confidence":1.0,"metadata_json":"{}","first_seen_at":timestamp,"last_seen_at":timestamp}),
            )?;
            row.as_object_mut().unwrap().remove("entity_id");
            pending_links.push(row);
        }
        creators.push(ledger::CreatorIntent {
            canonical_name: plan.target_person.clone(),
            normalized_name: plan.target_person.to_lowercase(),
            links: pending_links,
        });
    }
    let source_id = source.first().map(id).transpose()?;
    for link in &links {
        if changed_owner
            && source_id != target_id
            && source_id.is_some_and(|old| link["entity_id"] == integer(old))
            && link["role"] == "creator"
        {
            continue;
        }
        next_links.push(link.clone());
    }
    if let Some(target_id) = target_id {
        let timestamp = &plan.execution.as_ref().unwrap().data["timestamp"];
        for asset in &assets {
            let asset_id = id(asset)?;
            if !next_links.iter().any(|link| {
                link["asset_id"] == integer(asset_id)
                    && link["entity_id"] == integer(target_id)
                    && link["role"] == "creator"
            }) {
                next_links.push(new_row(&tx,"asset_entity",json!({"asset_id":integer(asset_id),"entity_id":integer(target_id),"role":"creator","source":"face-persons","confidence":1.0,"metadata_json":"{}","first_seen_at":timestamp,"last_seen_at":timestamp}))?);
            }
        }
    }
    // The shared native store compares sets by complete key, independent of SQL return order.
    assets.sort_by_key(|r| id(r).unwrap());
    next_assets.sort_by_key(|r| id(r).unwrap());
    let link_keys = ["asset_id", "entity_id", "role", "source"];
    links.sort_by_key(|r| key(r, &link_keys).to_string());
    next_links.sort_by_key(|r| key(r, &link_keys).to_string());
    for asset in &assets {
        let asset_id = asset["id"].clone();
        guards.push(db::Guard::AssetLinks {
            asset_id: asset_id.clone(),
            before: links
                .iter()
                .filter(|r| r["asset_id"] == asset_id)
                .cloned()
                .collect(),
            after: next_links
                .iter()
                .filter(|r| r["asset_id"] == asset_id)
                .cloned()
                .collect(),
        });
    }
    for link in links.iter().chain(&next_links) {
        let selector = key(link, &link_keys);
        if !link_scope.contains(&selector) {
            link_scope.push(selector);
        }
    }
    guards.push(db::Guard::CreatorName {
        canonical: plan.person.clone(),
        normalized: plan.person.to_lowercase(),
        before: source.clone(),
        after: next_entities
            .values()
            .filter(|r| {
                r["canonical_name"] == plan.person
                    || r["normalized_name"] == plan.person.to_lowercase()
            })
            .cloned()
            .collect(),
    });
    guards.push(db::Guard::CreatorName {
        canonical: plan.target_person.clone(),
        normalized: plan.target_person.to_lowercase(),
        before: target,
        after: next_entities
            .values()
            .filter(|r| {
                r["canonical_name"] == plan.target_person
                    || r["normalized_name"] == plan.target_person.to_lowercase()
            })
            .cloned()
            .collect(),
    });
    Ok(ledger::Delta {
        changes: db::Delta {
            tables: vec![
                db::TableDelta {
                    table: db::Table::Entity,
                    key_columns: vec!["id".into()],
                    scope: entity_scope,
                    before: entities.into_values().collect(),
                    after: next_entities.into_values().collect(),
                },
                db::TableDelta {
                    table: db::Table::Asset,
                    key_columns: vec!["id".into()],
                    scope: asset_scope,
                    before: assets,
                    after: next_assets,
                },
                db::TableDelta {
                    table: db::Table::AssetEntity,
                    key_columns: link_keys.iter().map(|s| (*s).into()).collect(),
                    scope: link_scope,
                    before: links,
                    after: next_links,
                },
            ],
            guards,
        },
        creators,
    })
}

fn rename_snapshot(config: &Config, plan: &Plan) -> Result<ledger::Delta> {
    let mut connection = db::open(&config.ledger)?;
    let tx = connection.transaction()?;
    let source = db::rows(
        &tx,
        "SELECT * FROM entity WHERE kind='creator' AND (canonical_name=? OR normalized_name=?) ORDER BY id",
        params![plan.person, plan.person.to_lowercase()],
    )?;
    let target = db::rows(
        &tx,
        "SELECT * FROM entity WHERE kind='creator' AND (canonical_name=? OR normalized_name=?) ORDER BY id",
        params![plan.target_person, plan.target_person.to_lowercase()],
    )?;
    if source.len() > 1 {
        return Err(Error::conflict("Peach 来源人物记录不唯一"));
    }
    if !target.is_empty() {
        return Err(Error::conflict("Peach 已有同名人物，不能直接覆盖或合并"));
    }
    let after_entities = source
        .iter()
        .map(|r| {
            let mut next = r.clone();
            next["canonical_name"] = json!(plan.target_person);
            next["normalized_name"] = json!(plan.target_person.to_lowercase());
            next
        })
        .collect::<Vec<_>>();
    let old = format!(
        "{}\\",
        win(
            config,
            &config.creators().join(&plan.person).display().to_string()
        )
    );
    let new = format!(
        "{}\\",
        win(
            config,
            &config
                .creators()
                .join(&plan.target_person)
                .display()
                .to_string()
        )
    );
    let assets = db::rows(
        &tx,
        "SELECT * FROM asset WHERE substr(path,1,length(?))=? ORDER BY id",
        [&old, &old],
    )?;
    let occupied = db::rows(
        &tx,
        "SELECT * FROM asset WHERE substr(path,1,length(?))=? ORDER BY id",
        [&new, &new],
    )?;
    if !occupied.is_empty() {
        return Err(Error::conflict("Peach 目标人物目录已有资产记录"));
    }
    let mut unique_paths = std::collections::BTreeSet::new();
    if assets
        .iter()
        .any(|row| !unique_paths.insert(row["path"].as_str().unwrap()))
    {
        return Err(Error::conflict("Peach 来源目录有重复资产路径，请先核对"));
    }
    let after_assets = assets
        .iter()
        .map(|r| {
            let mut next = r.clone();
            next["path"] = json!(format!(
                "{}{}",
                new,
                &r["path"].as_str().unwrap()[old.len()..]
            ));
            next
        })
        .collect::<Vec<_>>();
    let mut guards = vec![
        db::Guard::CreatorName {
            canonical: plan.person.clone(),
            normalized: plan.person.to_lowercase(),
            before: source.clone(),
            after: vec![],
        },
        db::Guard::CreatorName {
            canonical: plan.target_person.clone(),
            normalized: plan.target_person.to_lowercase(),
            before: vec![],
            after: after_entities.clone(),
        },
        db::Guard::AssetPrefix {
            prefix: old,
            before: assets.clone(),
            after: vec![],
        },
        db::Guard::AssetPrefix {
            prefix: new,
            before: vec![],
            after: after_assets.clone(),
        },
    ];
    let mut links = vec![];
    for asset in &assets {
        let found = db::rows(
            &tx,
            "SELECT * FROM asset_entity WHERE asset_id=? ORDER BY entity_id,role,source",
            [db::integer(&asset["id"])?],
        )?;
        guards.push(db::Guard::AssetLinks {
            asset_id: asset["id"].clone(),
            before: found.clone(),
            after: found.clone(),
        });
        links.extend(found);
    }
    Ok(ledger::Delta {
        changes: db::Delta {
            tables: vec![
                db::TableDelta {
                    table: db::Table::Entity,
                    key_columns: vec!["id".into()],
                    scope: source.iter().map(|r| key(r, &["id"])).collect(),
                    before: source,
                    after: after_entities,
                },
                db::TableDelta {
                    table: db::Table::Asset,
                    key_columns: vec!["id".into()],
                    scope: assets.iter().map(|r| key(r, &["id"])).collect(),
                    before: assets,
                    after: after_assets,
                },
            ],
            guards,
        },
        creators: vec![],
    })
}
