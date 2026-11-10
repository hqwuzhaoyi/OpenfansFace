//! Whole-person Peach merge as a typed aggregate delta, using actual schema rows and keys.
use crate::{
    config::Config,
    db, ledger, metadata,
    model::{Error, Plan, Result},
};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}
fn key(row: &Value, keys: &[String]) -> Value {
    Value::Object(keys.iter().map(|k| (k.clone(), row[k].clone())).collect())
}
fn primary(conn: &Connection, table: &str) -> Result<Vec<String>> {
    let mut keys = db::rows(conn, &format!("PRAGMA table_info({})", quote(table)), [])?
        .into_iter()
        .filter_map(|r| {
            let n = db::integer(&r["pk"]).ok()?;
            (n > 0).then(|| (n, r["name"].as_str().unwrap().to_owned()))
        })
        .collect::<Vec<_>>();
    keys.sort();
    if keys.is_empty() {
        return Err(Error::conflict("Peach 关联表缺少明确主键"));
    }
    Ok(keys.into_iter().map(|(_, s)| s).collect())
}
fn table(
    conn: &Connection,
    table: db::Table,
    name: &str,
    before: Vec<Value>,
    after: Vec<Value>,
) -> Result<db::TableDelta> {
    let keys = primary(conn, name)?;
    let mut scope = vec![];
    for r in before.iter().chain(&after) {
        let k = key(r, &keys);
        if !scope.contains(&k) {
            scope.push(k);
        }
    }
    Ok(db::TableDelta {
        table,
        key_columns: keys,
        scope,
        before,
        after,
    })
}
/// Match all actual unique constraints, not only the row primary key. Some
/// supported ledgers use an integer id plus UNIQUE(asset_id,entity_id,role).
fn unique_constraints(conn: &Connection, name: &str) -> Result<Vec<Vec<(String, String)>>> {
    let mut constraints = vec![
        primary(conn, name)?
            .into_iter()
            .map(|k| (k, "BINARY".into()))
            .collect(),
    ];
    for index in db::rows(conn, &format!("PRAGMA index_list({})", quote(name)), [])? {
        if db::integer(&index["unique"])? == 0 {
            continue;
        }
        if db::integer(&index["partial"])? != 0 {
            return Err(Error::conflict("Peach 关联表存在需核对的条件唯一索引").at(name));
        }
        let mut keys = vec![];
        for column in db::rows(
            conn,
            &format!(
                "PRAGMA index_xinfo({})",
                quote(index["name"].as_str().unwrap())
            ),
            [],
        )? {
            if db::integer(&column["key"])? == 0 {
                continue;
            }
            let field = column["name"]
                .as_str()
                .ok_or_else(|| Error::conflict("Peach 关联表存在需核对的表达式唯一索引"))?;
            keys.push((
                field.into(),
                column["coll"].as_str().unwrap_or("BINARY").into(),
            ));
        }
        constraints.push(keys);
    }
    Ok(constraints)
}
fn collides(
    conn: &Connection,
    old: &Value,
    row: &Value,
    constraints: &[Vec<(String, String)>],
) -> Result<bool> {
    for keys in constraints {
        // SQLite unique indexes permit multiple NULLs (including newly allocated IDs).
        if keys
            .iter()
            .any(|(k, _)| old[k].is_null() || row[k].is_null())
        {
            continue;
        }
        let sql = format!(
            "SELECT {}",
            keys.iter()
                .map(|(_, coll)| format!("(? COLLATE {}) = ?", quote(coll)))
                .collect::<Vec<_>>()
                .join(" AND ")
        );
        let values = keys
            .iter()
            .flat_map(|(k, _)| [&old[k], &row[k]])
            .map(db::sql_value)
            .collect::<Result<Vec<_>>>()?;
        if conn.query_row(&sql, rusqlite::params_from_iter(values), |r| {
            r.get::<_, bool>(0)
        })? {
            return Ok(true);
        }
    }
    Ok(false)
}
fn push_unique(
    conn: &Connection,
    rows: &mut Vec<Value>,
    row: Value,
    constraints: &[Vec<(String, String)>],
) -> Result<()> {
    for old in rows.iter() {
        if collides(conn, old, &row, constraints)? {
            return Ok(());
        }
    }
    rows.push(row);
    Ok(())
}
fn creator(conn: &Connection, name: &str) -> Result<Vec<Value>> {
    let rows = db::rows(
        conn,
        "SELECT * FROM entity WHERE kind='creator' AND (canonical_name=? OR normalized_name=?) ORDER BY id",
        params![name, name.to_lowercase()],
    )?;
    if rows.len() > 1 {
        return Err(Error::conflict("Peach 人物记录不唯一"));
    }
    Ok(rows)
}
fn audit_value(value: &Value) -> Result<Value> {
    if value
        .as_object()
        .is_some_and(|v| v.len() == 1 && v.contains_key("$integer"))
    {
        return Ok(json!(db::integer(value)?));
    }
    Ok(match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| Ok((k.clone(), audit_value(v)?)))
                .collect::<Result<_>>()?,
        ),
        Value::Array(rows) => Value::Array(rows.iter().map(audit_value).collect::<Result<_>>()?),
        _ => value.clone(),
    })
}
fn person_metadata(row: &Value) -> Result<Value> {
    let value: Value = serde_json::from_str(row["metadata_json"].as_str().unwrap_or("{}"))?;
    if !value.is_object()
        || value
            .get("face_admin_merged_people")
            .is_some_and(|v| !v.is_array())
    {
        return Err(Error::conflict("Peach 人物附加资料格式异常"));
    }
    Ok(value)
}
fn reject_unknown(conn: &Connection, id: &Value) -> Result<()> {
    for table in db::rows(
        conn,
        "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
        [],
    )? {
        let name = table["name"].as_str().unwrap();
        if ["asset_entity", "entity_alias"].contains(&name) {
            continue;
        }
        let mut groups = BTreeMap::<i64, Vec<Value>>::new();
        for fk in db::rows(
            conn,
            &format!("PRAGMA foreign_key_list({})", quote(name)),
            [],
        )? {
            if fk["table"]
                .as_str()
                .is_some_and(|s| s.eq_ignore_ascii_case("entity"))
            {
                groups.entry(db::integer(&fk["id"])?).or_default().push(fk);
            }
        }
        for (_, mut group) in groups {
            group.sort_by_key(|fk| db::integer(&fk["seq"]).unwrap_or(-1));
            let predicates = group
                .iter()
                .map(|fk| {
                    let parent = fk["to"].as_str().unwrap_or("id");
                    let child = fk["from"]
                        .as_str()
                        .ok_or_else(|| Error::conflict("Peach 外键字段无效"))?;
                    Ok(format!("p.{} = c.{}", quote(parent), quote(child)))
                })
                .collect::<Result<Vec<_>>>()?;
            // Keep the parent operand on the left: SQLite applies its affinity
            // and collation to foreign-key comparisons, including composite keys.
            if !db::rows(
                conn,
                &format!(
                    "SELECT 1 AS occupied FROM {} AS c JOIN entity AS p ON {} WHERE p.id=? LIMIT 1",
                    quote(name),
                    predicates.join(" AND ")
                ),
                [db::sql_value(id)?],
            )?
            .is_empty()
            {
                return Err(Error::conflict("Peach 还有需核对的人物关联").at(name));
            }
        }
    }
    Ok(())
}
fn default_link(
    conn: &Connection,
    asset: &Value,
    entity: &Value,
    timestamp: &Value,
) -> Result<Value> {
    let fields = json!({"asset_id":asset,"entity_id":entity,"role":"creator","source":"face-persons","confidence":1.0,"metadata_json":"{}","first_seen_at":timestamp,"last_seen_at":timestamp});
    let mut row = serde_json::Map::new();
    for col in db::rows(conn, "PRAGMA table_info(asset_entity)", [])? {
        let name = col["name"].as_str().unwrap();
        let value = if let Some(value) = fields.get(name) {
            value.clone()
        } else if let Some(default) = col["dflt_value"].as_str() {
            db::rows(conn, &format!("SELECT {default} AS value"), [])?[0]["value"].clone()
        } else if db::integer(&col["notnull"])? != 0 {
            return Err(Error::conflict("Peach 新关联有未支持必填字段").at(name));
        } else {
            Value::Null
        };
        row.insert(name.into(), value);
    }
    Ok(Value::Object(row))
}
pub fn snapshot(config: &Config, plan: &Plan) -> Result<ledger::Delta> {
    let mut conn = db::open(&config.ledger)?;
    let tx = conn.transaction()?;
    let mut source = creator(&tx, &plan.person)?;
    let target = creator(&tx, &plan.target_person)?;
    let mut source_id = source.first().map(|r| r["id"].clone());
    let target_id = target.first().map(|r| r["id"].clone());
    let mut shared_entity = false;
    if source_id.is_some() && source_id == target_id {
        // Peach matches names case-insensitively: a spelling variant already
        // shares the kept person's entity, so there is no entity to merge.
        if !crate::person_registration::same_spelling(&plan.person, &plan.target_person) {
            return Err(Error::conflict("Peach 中所选名字已指向同一人物"));
        }
        source.clear();
        source_id = None;
        shared_entity = true;
    }
    for row in source.iter().chain(&target) {
        person_metadata(row)?;
    }
    if let Some(id) = &source_id {
        reject_unknown(&tx, id)?;
    }
    let prefix = |person: &str| {
        format!(
            "{}\\",
            metadata::win(
                config,
                &config.creators().join(person).display().to_string()
            )
        )
    };
    let old = prefix(&plan.person);
    let new = prefix(&plan.target_person);
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
    let mut paths = BTreeSet::new();
    let mut destinations = BTreeSet::new();
    let mut after_assets = vec![];
    let mut replaced_stubs = vec![];
    let mut replaced_stub_ids = BTreeSet::new();
    for asset in &assets {
        let path = asset["path"]
            .as_str()
            .ok_or_else(|| Error::conflict("Peach 资产路径无效"))?;
        let dst = metadata::mapped(config, path, plan);
        if !paths.insert(path.to_owned()) || !destinations.insert(dst.clone()) {
            return Err(Error::conflict("Peach 来源或目标资产路径存在冲突").at(dst));
        }
        let matching_occupied: Vec<&Value> =
            occupied.iter().filter(|r| r["path"] == dst).collect();
        if !matching_occupied.is_empty() {
            if !shared_entity || matching_occupied.len() > 1 {
                return Err(Error::conflict("Peach 来源或目标资产路径存在冲突").at(dst));
            }
            let stub = matching_occupied[0];
            crate::archive_cancel::is_pure_scanner_stub(
                &tx,
                stub,
                asset,
                asset["name"].as_str(),
                db::integer(&asset["size"]).ok(),
            )
            .map_err(|_| Error::conflict("Peach 来源或目标资产路径存在冲突").at(dst.clone()))?;
            replaced_stub_ids.insert(db::integer(&stub["id"])?);
            replaced_stubs.push(stub.clone());
        }
        let mut next = asset.clone();
        next["path"] = json!(dst);
        after_assets.push(next);
    }
    let mut by_asset = BTreeMap::<i64, Vec<Value>>::new();
    let mut source_links = vec![];
    let mut target_links = vec![];
    for (entity, dest) in [
        (&source_id, &mut source_links),
        (&target_id, &mut target_links),
    ] {
        if let Some(id) = entity {
            *dest = db::rows(
                &tx,
                "SELECT * FROM asset_entity WHERE entity_id=?",
                [db::sql_value(id)?],
            )?;
        }
    }
    for id in assets
        .iter()
        .map(|r| db::integer(&r["id"]))
        .chain(replaced_stub_ids.iter().copied().map(Ok))
        .chain(
            source_links
                .iter()
                .chain(&target_links)
                .map(|r| db::integer(&r["asset_id"])),
        )
    {
        let id = id?;
        if let std::collections::btree_map::Entry::Vacant(entry) = by_asset.entry(id) {
            entry.insert(db::rows(
                &tx,
                "SELECT * FROM asset_entity WHERE asset_id=?",
                [id],
            )?);
        }
    }
    let links = by_asset.values().flatten().cloned().collect::<Vec<_>>();
    let link_keys = unique_constraints(&tx, "asset_entity")?;
    // Existing target rows win collisions on any actual unique key; other roles/sources transfer.
    let mut after_links = links
        .iter()
        .filter(|r| Some(&r["entity_id"]) != source_id.as_ref())
        .cloned()
        .collect::<Vec<_>>();
    let mut pending = vec![];
    for source in &source_links {
        let mut row = source.clone();
        row["entity_id"] = target_id.clone().unwrap_or(Value::Null);
        if target_id.is_some() {
            push_unique(&tx, &mut after_links, row, &link_keys)?;
        } else {
            pending.push(row)
        }
    }
    let timestamp = &plan.execution.as_ref().unwrap().data["timestamp"];
    for asset in &assets {
        let rows = if target_id.is_some() {
            &mut after_links
        } else {
            &mut pending
        };
        if !rows.iter().any(|r| {
            r["asset_id"] == asset["id"]
                && r["entity_id"] == target_id.clone().unwrap_or(Value::Null)
                && r["role"] == "creator"
        }) {
            rows.push(default_link(
                &tx,
                &asset["id"],
                &target_id.clone().unwrap_or(Value::Null),
                timestamp,
            )?);
        }
    }
    let aliases_for = |id: &Option<Value>| -> Result<Vec<Value>> {
        id.as_ref()
            .map(|id| {
                db::rows(
                    &tx,
                    "SELECT * FROM entity_alias WHERE entity_id=?",
                    [db::sql_value(id)?],
                )
            })
            .transpose()
            .map(|v| v.unwrap_or_default())
    };
    let source_aliases = aliases_for(&source_id)?;
    let target_aliases = aliases_for(&target_id)?;
    let aliases = source_aliases
        .iter()
        .chain(&target_aliases)
        .cloned()
        .collect::<Vec<_>>();
    let alias_keys = unique_constraints(&tx, "entity_alias")?;
    let mut after_aliases = target_aliases.clone();
    let mut pending_aliases = vec![];
    for alias in &source_aliases {
        let mut row = alias.clone();
        row["entity_id"] = target_id.clone().unwrap_or(Value::Null);
        if target_id.is_some() {
            push_unique(&tx, &mut after_aliases, row, &alias_keys)?;
        } else {
            pending_aliases.push(row)
        }
    }
    let mut metadata = target
        .first()
        .map(person_metadata)
        .transpose()?
        .unwrap_or_else(|| json!({}));
    if let Some(source) = source.first() {
        if metadata.get("face_admin_merged_people").is_none() {
            metadata["face_admin_merged_people"] = json!([]);
        }
        metadata["face_admin_merged_people"]
            .as_array_mut()
            .unwrap()
            .push(json!({"operation":plan.id,"entity":audit_value(source)?,"aliases":audit_value(&json!(source_aliases))?}));
    }
    let entities = source.iter().chain(&target).cloned().collect::<Vec<_>>();
    let mut after_entities = target.clone();
    if source_id.is_some()
        && let Some(row) = after_entities.first_mut()
    {
        row["metadata_json"] = json!(serde_json::to_string(&metadata)?);
        row["updated_at"] = timestamp.clone();
    }
    let need_creator = target_id.is_none() && (!assets.is_empty() || source_id.is_some());
    let creators = if need_creator {
        for row in pending.iter_mut().chain(&mut pending_aliases) {
            row.as_object_mut().unwrap().remove("entity_id");
        }
        vec![ledger::CreatorIntent {
            canonical_name: plan.target_person.clone(),
            normalized_name: plan.target_person.to_lowercase(),
            links: pending,
            aliases: pending_aliases,
            metadata_json: Some(serde_json::to_string(&metadata)?),
        }]
    } else {
        vec![]
    };
    let mut guards = vec![
        db::Guard::CreatorName {
            canonical: plan.person.clone(),
            normalized: plan.person.to_lowercase(),
            before: source.clone(),
            after: vec![],
        },
    ];
    if shared_entity {
        // The variant's name query returns the kept entity, which the target
        // guard below already pins exactly.
        guards.clear();
    }
    guards.extend([
        db::Guard::CreatorName {
            canonical: plan.target_person.clone(),
            normalized: plan.target_person.to_lowercase(),
            before: target.clone(),
            after: after_entities.clone(),
        },
        db::Guard::AssetPrefix {
            prefix: old,
            before: assets.clone(),
            after: vec![],
        },
        db::Guard::AssetPrefix {
            prefix: new,
            before: occupied.clone(),
            after: occupied
                .into_iter()
                .filter(|r| {
                    db::integer(&r["id"])
                        .ok()
                        .is_none_or(|id| !replaced_stub_ids.contains(&id))
                })
                .chain(after_assets.clone())
                .collect(),
        },
    ]);
    for (asset_id, before) in by_asset {
        let id = json!({"$integer":asset_id.to_string()});
        guards.push(db::Guard::AssetLinks {
            asset_id: id.clone(),
            before,
            after: after_links
                .iter()
                .filter(|r| r["asset_id"] == id)
                .cloned()
                .collect(),
        });
    }
    for id in source_id.iter().chain(&target_id) {
        guards.push(db::Guard::EntityLinks {
            entity_id: id.clone(),
            before: links
                .iter()
                .filter(|r| r["entity_id"] == *id)
                .cloned()
                .collect(),
            after: after_links
                .iter()
                .filter(|r| r["entity_id"] == *id)
                .cloned()
                .collect(),
        });
        guards.push(db::Guard::EntityAliases {
            entity_id: id.clone(),
            before: aliases
                .iter()
                .filter(|r| r["entity_id"] == *id)
                .cloned()
                .collect(),
            after: after_aliases
                .iter()
                .filter(|r| r["entity_id"] == *id)
                .cloned()
                .collect(),
        });
    }
    let mut before_assets = assets;
    before_assets.extend(replaced_stubs);
    Ok(ledger::Delta {
        changes: db::Delta {
            tables: vec![
                table(&tx, db::Table::Entity, "entity", entities, after_entities)?,
                table(&tx, db::Table::Asset, "asset", before_assets, after_assets)?,
                table(
                    &tx,
                    db::Table::AssetEntity,
                    "asset_entity",
                    links,
                    after_links,
                )?,
                table(
                    &tx,
                    db::Table::EntityAlias,
                    "entity_alias",
                    aliases,
                    after_aliases,
                )?,
            ],
            guards,
        },
        creators,
    })
}
