//! Archive-specific Peach deltas. The common ledger module owns transaction receipts.
use crate::{
    config::Config,
    db, metadata,
    model::{Error, Result},
    review,
};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub fn keys(db: &Connection, table: &str) -> Result<Vec<String>> {
    let mut rows = db::rows(db, &format!("PRAGMA table_info({table})"), [])?;
    rows.retain(|r| review::number(&r["pk"]).unwrap_or(0) > 0);
    rows.sort_by_key(|r| review::number(&r["pk"]).unwrap_or(0));
    if rows.is_empty() {
        return Err(Error::new(
            503,
            "database_contract",
            format!("{table} 缺少唯一主键，拒绝不确定写入"),
        ));
    }
    Ok(rows
        .iter()
        .map(|r| r["name"].as_str().unwrap().to_owned())
        .collect())
}
pub fn table(
    table: db::Table,
    key_columns: Vec<String>,
    before: Vec<Value>,
    after: Vec<Value>,
) -> db::TableDelta {
    let mut scope = vec![];
    for row in before.iter().chain(&after) {
        let key: Value = Value::Object(
            key_columns
                .iter()
                .map(|k| (k.clone(), row[k].clone()))
                .collect(),
        );
        if !scope.contains(&key) {
            scope.push(key)
        }
    }
    db::TableDelta {
        table,
        key_columns,
        scope,
        before,
        after,
    }
}
/// Fill every actual column using its declared default. Never substitute a fixture table.
fn new_row(db: &Connection, table: &str, values: Value) -> Result<Value> {
    let mut row = serde_json::Map::new();
    for col in db::rows(db, &format!("PRAGMA table_info({table})"), [])? {
        let name = col["name"].as_str().unwrap();
        let value = if let Some(value) = values.get(name) {
            value.clone()
        } else if let Some(default) = col["dflt_value"].as_str() {
            db::rows(db, &format!("SELECT {default} AS value"), [])?[0]["value"].clone()
        } else {
            Value::Null
        };
        if value.is_null() && review::number(&col["notnull"]).unwrap_or(0) != 0 {
            return Err(Error::new(
                503,
                "database_contract",
                format!("{table}.{name} 缺少必需字段，停止归档"),
            ));
        }
        row.insert(name.into(), value);
    }
    Ok(Value::Object(row))
}
pub fn prepare(config: &Config, rows: &[Value], at: &str, operation_id: &str) -> Result<Value> {
    let mut db = db::open(&config.ledger)?;
    let tx = db.transaction()?;
    let mut planned = vec![];
    let mut entities = BTreeMap::<String, Value>::new();
    let mut entity_before = vec![];
    let entity_keys = keys(&tx, "entity")?;
    let link_keys = keys(&tx, "asset_entity")?;
    let mut next_link = i64::from_str_radix(
        &crate::model::hash(&json!([operation_id, "link"]))[..15],
        16,
    )
    .unwrap()
    .max(1);
    for row in rows {
        let src = row["src"].as_str().unwrap();
        let dst = row["dst"].as_str().unwrap();
        let name = row["creator"].as_str().unwrap();
        let normalized = name.to_lowercase();
        let assets = db::rows(
            &tx,
            "SELECT * FROM asset WHERE path=? AND disposal IS NULL ORDER BY id",
            [metadata::win(config, src)],
        )?;
        if assets.len() != 1 {
            return Err(Error::new(409, "ledger_conflict", "源视频账本记录缺失或不唯一").at(src));
        }
        if !db::rows(
            &tx,
            "SELECT id FROM asset WHERE path=?",
            [metadata::win(config, dst)],
        )?
        .is_empty()
        {
            return Err(Error::new(409, "ledger_conflict", "目标路径已有账本记录").at(dst));
        }
        let matches = db::rows(
            &tx,
            "SELECT * FROM entity WHERE kind='creator' AND (canonical_name=? OR normalized_name=?) ORDER BY id",
            params![name, normalized],
        )?;
        if matches.len() > 1 {
            return Err(Error::new(
                409,
                "ledger_conflict",
                format!("人物名称存在歧义：{name}"),
            ));
        }
        let entity = if let Some(existing) = matches.first() {
            if !entity_before.contains(existing) {
                entity_before.push(existing.clone());
            }
            existing.clone()
        } else if let Some(existing) = entities.get(&normalized) {
            existing.clone()
        } else {
            json!({"kind":"creator","canonical_name":name,"normalized_name":normalized})
        };
        entities.insert(normalized, entity.clone());
        let asset = &assets[0];
        let aid = db::integer(&asset["id"])?;
        let eid = if matches.is_empty() {
            0
        } else {
            db::integer(&entity["id"])?
        };
        let links = db::rows(
            &tx,
            "SELECT * FROM asset_entity WHERE asset_id=? ORDER BY entity_id,role,source",
            [aid],
        )?;
        let mut after_links = links.clone();
        if !links
            .iter()
            .any(|r| r["entity_id"] == entity["id"] && r["role"] == "creator")
        {
            let mut fields = json!({"asset_id":review::integer(aid),"entity_id":review::integer(eid),"role":"creator","source":"face-curation-8919","confidence":0.95,"metadata_json":"{}","first_seen_at":at,"last_seen_at":at});
            if link_keys == ["id"] {
                fields["id"] = review::integer(next_link);
                next_link += 1;
            }
            after_links.push(new_row(&tx, "asset_entity", fields)?);
        }
        let mut after_asset = asset.clone();
        after_asset["path"] = json!(metadata::win(config, dst));
        planned.push(json!({"asset":asset,"after_asset":after_asset,"links":links,"after_links":after_links,"entity":entity,"requested_name":name,"entity_matches":matches,"old_path_assets":db::rows(&tx,"SELECT * FROM asset WHERE path=? ORDER BY id",[metadata::win(config,src)])?,"new_path_assets":db::rows(&tx,"SELECT * FROM asset WHERE path=? ORDER BY id",[metadata::win(config,dst)])?}));
    }
    Ok(
        json!({"files":planned,"entities_before":entity_before,"entities_after":entities.values().collect::<Vec<_>>(),"entity_keys":entity_keys,"link_keys":link_keys}),
    )
}
pub fn delta(prepared: &Value, successful: &[usize]) -> Result<crate::ledger::Delta> {
    let mut creators = BTreeMap::<String, crate::ledger::CreatorIntent>::new();
    let mut guards = vec![];
    let mut assets = vec![];
    let mut after_assets = vec![];
    let mut links = vec![];
    let mut after_links = vec![];
    let mut entity_ids = BTreeSet::new();
    for &i in successful {
        let row = prepared["files"]
            .get(i)
            .ok_or_else(|| Error::conflict("归档提交范围越界"))?;
        assets.push(row["asset"].clone());
        after_assets.push(row["after_asset"].clone());
        links.extend(row["links"].as_array().unwrap().clone());
        let pending = row["entity_matches"].as_array().unwrap().is_empty();
        if pending {
            after_links.extend(row["links"].as_array().unwrap().clone());
            let normalized = row["entity"]["normalized_name"].as_str().unwrap();
            let intent =
                creators
                    .entry(normalized.into())
                    .or_insert_with(|| crate::ledger::CreatorIntent {
                        canonical_name: row["entity"]["canonical_name"].as_str().unwrap().into(),
                        normalized_name: normalized.into(),
                        links: vec![],
                    });
            for link in row["after_links"].as_array().unwrap() {
                if row["links"].as_array().unwrap().contains(link) {
                    continue;
                }
                let mut link = link.clone();
                link.as_object_mut().unwrap().remove("entity_id");
                intent.links.push(link);
            }
        } else {
            after_links.extend(row["after_links"].as_array().unwrap().clone());
            entity_ids.insert(db::integer(&row["entity"]["id"])?);
        }
        let old = row["old_path_assets"].as_array().unwrap().clone();
        let mut old_after = old.clone();
        old_after.retain(|a| a["id"] != row["asset"]["id"]);
        let new = row["new_path_assets"].as_array().unwrap().clone();
        let mut new_after = new.clone();
        new_after.push(row["after_asset"].clone());
        guards.push(db::Guard::AssetPath {
            path: row["asset"]["path"].as_str().unwrap().into(),
            before: old,
            after: old_after,
        });
        guards.push(db::Guard::AssetPath {
            path: row["after_asset"]["path"].as_str().unwrap().into(),
            before: new,
            after: new_after,
        });
        guards.push(db::Guard::AssetLinks {
            asset_id: row["asset"]["id"].clone(),
            before: row["links"].as_array().unwrap().clone(),
            after: if pending {
                row["links"].as_array().unwrap().clone()
            } else {
                row["after_links"].as_array().unwrap().clone()
            },
        });
        let matches = row["entity_matches"].as_array().unwrap().clone();
        let after = matches.clone();
        let guard = db::Guard::CreatorName {
            canonical: row["requested_name"].as_str().unwrap().into(),
            normalized: row["entity"]["normalized_name"].as_str().unwrap().into(),
            before: matches,
            after,
        };
        if !guards.contains(&guard) {
            guards.push(guard)
        }
    }
    let selected = |rows: &Value| -> Result<Vec<Value>> {
        rows.as_array()
            .unwrap()
            .iter()
            .filter(|r| !r["id"].is_null())
            .filter_map(|r| match db::integer(&r["id"]) {
                Ok(id) if entity_ids.contains(&id) => Some(Ok(r.clone())),
                Ok(_) => None,
                Err(e) => Some(Err(e)),
            })
            .collect()
    };
    Ok(crate::ledger::Delta {
        creators: creators.into_values().collect(),
        changes: db::Delta {
            guards,
            tables: vec![
                table(
                    db::Table::Entity,
                    serde_json::from_value(prepared["entity_keys"].clone())?,
                    selected(&prepared["entities_before"])?,
                    selected(&prepared["entities_after"])?,
                ),
                table(db::Table::Asset, vec!["id".into()], assets, after_assets),
                table(
                    db::Table::AssetEntity,
                    serde_json::from_value(prepared["link_keys"].clone())?,
                    links,
                    after_links,
                ),
            ],
        },
    })
}
