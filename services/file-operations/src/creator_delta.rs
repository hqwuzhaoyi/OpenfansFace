//! Transaction-local creator registration. Intent has no predicted entity ID;
//! the returned concrete delta is the only snapshot suitable for a commit receipt.
//! Caller owns the transaction and must roll it back on every error.
use crate::{
    db,
    delta::{self, Delta, Guard, Table, TableDelta},
    model::{Error, Result},
};
use rusqlite::{Connection, params, params_from_iter};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CreatorIntent {
    pub canonical_name: String,
    pub normalized_name: String,
    /// Complete native asset_entity rows except for the not-yet-allocated entity_id.
    pub links: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata_json: Option<String>,
}

fn conflict(message: &str) -> Error {
    Error::conflict(message)
}
fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}
fn creators_named(conn: &Connection, intent: &CreatorIntent) -> Result<Vec<Value>> {
    db::rows(
        conn,
        "SELECT * FROM entity WHERE kind='creator' AND (canonical_name=? OR normalized_name=?) ORDER BY id",
        [&intent.canonical_name, &intent.normalized_name],
    )
}
fn asset_links(conn: &Connection, id: &Value) -> Result<Vec<Value>> {
    db::rows(
        conn,
        "SELECT * FROM asset_entity WHERE asset_id=? ORDER BY entity_id,role,source",
        [db::sql_value(id)?],
    )
}

fn primary_columns(conn: &Connection, table: &str) -> Result<Vec<String>> {
    let mut columns = db::rows(conn, &format!("PRAGMA table_info({})", quote(table)), [])?
        .into_iter()
        .filter_map(|r| {
            let order = db::integer(&r["pk"]).ok()?;
            (order > 0).then(|| (order, r["name"].as_str().unwrap().to_owned()))
        })
        .collect::<Vec<_>>();
    columns.sort();
    if columns.is_empty() {
        return Err(conflict("人物及关联表必须具有实际主键"));
    }
    Ok(columns.into_iter().map(|(_, name)| name).collect())
}

fn validate_intents(conn: &Connection, changes: &Delta, creators: &[CreatorIntent]) -> Result<()> {
    if creators.is_empty() {
        return Ok(());
    }
    let entity_info = db::rows(conn, "PRAGMA table_xinfo(entity)", [])?;
    if primary_columns(conn, "entity")? != ["id"]
        || !entity_info.iter().any(|r| {
            r["name"] == "id"
                && r["type"]
                    .as_str()
                    .is_some_and(|t| t.eq_ignore_ascii_case("INTEGER"))
        })
    {
        return Err(conflict("人物编号必须由 SQLite INTEGER PRIMARY KEY 分配"));
    }
    let link_columns = db::columns(conn, "asset_entity")?;
    if !link_columns.iter().any(|c| c == "entity_id")
        || !link_columns.iter().any(|c| c == "asset_id")
    {
        return Err(conflict("人物关联表结构不匹配"));
    }
    let keys = primary_columns(conn, "asset_entity")?;
    let mut canonical = BTreeSet::new();
    let mut normalized = BTreeSet::new();
    for intent in creators {
        if intent.canonical_name.trim().is_empty()
            || intent.normalized_name.trim().is_empty()
            || !canonical.insert(&intent.canonical_name)
            || !normalized.insert(&intent.normalized_name)
        {
            return Err(conflict("待创建人物名称为空或重复"));
        }
        if !creators_named(conn, intent)?.is_empty() {
            return Err(conflict("待创建人物已出现，请重新核对预览"));
        }
        // The base delta must not create the very entity this helper allocates.
        if changes
            .tables
            .iter()
            .filter(|t| t.table == Table::Entity)
            .flat_map(|t| &t.after)
            .any(|r| {
                r["kind"] == "creator"
                    && (r["canonical_name"] == intent.canonical_name
                        || r["normalized_name"] == intent.normalized_name)
            })
        {
            return Err(conflict("具体行变更与待创建人物意图重叠"));
        }
        if let Some(raw) = &intent.metadata_json {
            let value: Value = serde_json::from_str(raw)?;
            if !value.is_object() {
                return Err(conflict("人物附加资料必须为对象"));
            }
        }
        if !intent.aliases.is_empty() {
            let columns = db::columns(conn, "entity_alias")?;
            let keys = primary_columns(conn, "entity_alias")?;
            let mut seen = BTreeSet::new();
            for alias in &intent.aliases {
                let object = alias
                    .as_object()
                    .ok_or_else(|| conflict("别名必须是完整对象"))?;
                if object.len() + 1 != columns.len()
                    || object.contains_key("entity_id")
                    || columns
                        .iter()
                        .filter(|k| k.as_str() != "entity_id")
                        .any(|k| !object.contains_key(k))
                {
                    return Err(conflict("别名实际列不完整"));
                }
                for value in object.values() {
                    db::sql_value(value)?;
                }
                let key = Value::Object(
                    keys.iter()
                        .filter(|k| k.as_str() != "entity_id")
                        .map(|k| (k.clone(), alias[k].clone()))
                        .collect(),
                );
                if !seen.insert(serde_json::to_string(&key)?) {
                    return Err(conflict("别名主键重复"));
                }
            }
        }
        let mut seen = BTreeSet::new();
        for link in &intent.links {
            let object = link
                .as_object()
                .ok_or_else(|| conflict("待创建人物关联必须是完整对象"))?;
            if object.len() + 1 != link_columns.len()
                || object.contains_key("entity_id")
                || link_columns
                    .iter()
                    .filter(|c| c.as_str() != "entity_id")
                    .any(|c| !object.contains_key(c))
            {
                return Err(conflict(
                    "待创建人物关联必须包含除 entity_id 外的全部实际列",
                ));
            }
            for value in object.values() {
                match value {
                    Value::Null | Value::String(_) | Value::Number(_) => {}
                    Value::Object(o) if o.len() == 1 && o.contains_key("$integer") => {
                        if value["$integer"].as_str()
                            != Some(db::integer(value)?.to_string().as_str())
                        {
                            return Err(conflict("非规范整数编码"));
                        }
                    }
                    Value::Object(o) if o.len() == 1 && o.contains_key("$blob") => {}
                    _ => return Err(conflict("待创建人物关联值编码无效")),
                }
                db::sql_value(value)?;
            }
            db::integer(&link["asset_id"])?;
            let identity = keys
                .iter()
                .filter(|k| k.as_str() != "entity_id")
                .map(|k| {
                    if link[k].is_null() {
                        return Err(conflict("关联主键不允许 NULL"));
                    }
                    Ok((k.clone(), link[k].clone()))
                })
                .collect::<Result<serde_json::Map<String, Value>>>()?;
            if !seen.insert(serde_json::to_string(&identity)?) {
                return Err(conflict("待创建人物关联包含重复主键"));
            }
        }
    }
    Ok(())
}

/// Admission and cancelled-receipt validation, without allocating an entity ID.
pub fn inspect_before(
    conn: &Connection,
    changes: &Delta,
    creators: &[CreatorIntent],
) -> Result<()> {
    delta::inspect(conn, changes, false)?;
    validate_intents(conn, changes, creators)
}

fn selector(row: &Value, keys: &[String]) -> Value {
    Value::Object(keys.iter().map(|k| (k.clone(), row[k].clone())).collect())
}
fn append_insert(
    conn: &Connection,
    changes: &mut Delta,
    table: Table,
    name: &str,
    row: Value,
) -> Result<()> {
    if let Some(change) = changes.tables.iter_mut().find(|t| t.table == table) {
        let scope = selector(&row, &change.key_columns);
        if change
            .after
            .iter()
            .any(|r| selector(r, &change.key_columns) == scope)
        {
            return Err(conflict("新建人物行与具体变更范围重叠"));
        }
        if !change.scope.contains(&scope) {
            change.scope.push(scope);
        }
        change.after.push(row);
    } else {
        let keys = primary_columns(conn, name)?;
        changes.tables.push(TableDelta {
            table,
            scope: vec![selector(&row, &keys)],
            key_columns: keys,
            before: vec![],
            after: vec![row],
        });
    }
    Ok(())
}

// Capture guards before any base mutation, then refresh only the corresponding
// after-image. Existing guards retain their accepted before-image exactly.
fn ensure_guards(
    conn: &Connection,
    resolved: &mut Delta,
    creators: &[CreatorIntent],
) -> Result<()> {
    for creator in creators {
        if !resolved.guards.iter().any(|g| matches!(g, Guard::CreatorName{canonical, normalized,..} if canonical == &creator.canonical_name && normalized == &creator.normalized_name)) {
            resolved.guards.push(Guard::CreatorName { canonical:creator.canonical_name.clone(), normalized:creator.normalized_name.clone(), before:vec![], after:vec![] });
        }
        for link in &creator.links {
            let id = &link["asset_id"];
            if !resolved
                .guards
                .iter()
                .any(|g| matches!(g, Guard::AssetLinks{asset_id,..} if asset_id == id))
            {
                let before = asset_links(conn, id)?;
                let mut after = before.clone();
                if let Some(change) = resolved
                    .tables
                    .iter()
                    .find(|t| t.table == Table::AssetEntity)
                {
                    let keys = primary_columns(conn, "asset_entity")?;
                    after.retain(|row| {
                        !change
                            .before
                            .iter()
                            .any(|old| selector(old, &keys) == selector(row, &keys))
                    });
                    after.extend(
                        change
                            .after
                            .iter()
                            .filter(|row| &row["asset_id"] == id)
                            .cloned(),
                    );
                }
                resolved.guards.push(Guard::AssetLinks {
                    asset_id: id.clone(),
                    after,
                    before,
                });
            }
        }
    }
    Ok(())
}

fn extend_guards(resolved: &mut Delta, table: Table, row: &Value) {
    for guard in &mut resolved.guards {
        match guard {
            Guard::CreatorName {
                canonical,
                normalized,
                after,
                ..
            } if table == Table::Entity
                && row["kind"] == "creator"
                && (row["canonical_name"] == canonical.as_str()
                    || row["normalized_name"] == normalized.as_str()) =>
            {
                after.push(row.clone());
            }
            Guard::CreatorCanonical {
                canonical, after, ..
            } if table == Table::Entity
                && row["kind"] == "creator"
                && row["canonical_name"] == canonical.as_str() =>
            {
                after.push(row.clone());
            }
            Guard::AssetLinks {
                asset_id, after, ..
            } if table == Table::AssetEntity && &row["asset_id"] == asset_id => {
                after.push(row.clone());
            }
            _ => {}
        }
    }
}

/// Apply the concrete changes, allocate creators natively, and return the exact
/// realized delta. Commit this return value with the caller's receipt in the same
/// transaction. Replays must inspect that receipt rather than call this again.
pub fn apply(conn: &Connection, changes: &Delta, creators: &[CreatorIntent]) -> Result<Delta> {
    if conn.is_autocommit() {
        return Err(conflict("人物创建必须由调用方持有事务"));
    }
    inspect_before(conn, changes, creators)?;
    let mut resolved = changes.clone();
    ensure_guards(conn, &mut resolved, creators)?;
    let mut application = changes.clone();
    let mut allocated = vec![];
    for creator in creators {
        if !creators_named(conn, creator)?.is_empty() {
            return Err(conflict("具体变更后人物创建范围出现冲突"));
        }
        conn.execute("INSERT INTO entity(kind,canonical_name,normalized_name,created_at,updated_at) VALUES('creator',?,?,strftime('%Y-%m-%dT%H:%M:%fZ','now'),strftime('%Y-%m-%dT%H:%M:%fZ','now'))", params![creator.canonical_name,creator.normalized_name])?;
        let id = conn.last_insert_rowid();
        if let Some(metadata) = &creator.metadata_json {
            conn.execute(
                "UPDATE entity SET metadata_json=? WHERE id=?",
                params![metadata, id],
            )?;
        }
        let entity = db::rows(conn, "SELECT * FROM entity WHERE id=?", [id])?
            .into_iter()
            .next()
            .ok_or_else(|| conflict("新建人物记录未找到"))?;
        if entity["kind"] != "creator"
            || entity["canonical_name"] != creator.canonical_name
            || entity["normalized_name"] != creator.normalized_name
            || creator
                .metadata_json
                .as_ref()
                .is_some_and(|metadata| entity["metadata_json"] != *metadata)
        {
            return Err(conflict("新建人物记录与接受的意图不一致"));
        }
        // Allocate while source entities still exist so SQLite cannot recycle a
        // deleted source ID. Apply the base delta with this transaction-local,
        // unchanged insertion visible; the durable receipt still has no before row.
        extend_guards(&mut application, Table::Entity, &entity);
        for guard in &mut application.guards {
            match guard {
                Guard::CreatorName {
                    canonical,
                    normalized,
                    before,
                    ..
                } if entity["canonical_name"] == canonical.as_str()
                    || entity["normalized_name"] == normalized.as_str() =>
                {
                    before.push(entity.clone())
                }
                Guard::CreatorCanonical {
                    canonical, before, ..
                } if entity["canonical_name"] == canonical.as_str() => before.push(entity.clone()),
                _ => {}
            }
        }
        append_insert(
            conn,
            &mut application,
            Table::Entity,
            "entity",
            entity.clone(),
        )?;
        application
            .tables
            .iter_mut()
            .find(|t| t.table == Table::Entity)
            .unwrap()
            .before
            .push(entity.clone());
        extend_guards(&mut resolved, Table::Entity, &entity);
        append_insert(conn, &mut resolved, Table::Entity, "entity", entity)?;
        allocated.push((creator, id));
    }
    delta::apply(conn, &application)?;
    for (creator, id) in allocated {
        let mut created_aliases = vec![];
        for pending in &creator.aliases {
            let mut alias = pending.clone();
            alias["entity_id"] = json!({"$integer":id.to_string()});
            let object = alias.as_object().unwrap();
            let columns = object
                .keys()
                .map(|k| quote(k))
                .collect::<Vec<_>>()
                .join(",");
            let placeholders = vec!["?"; object.len()].join(",");
            conn.execute(
                &format!("INSERT INTO entity_alias({columns}) VALUES({placeholders})"),
                params_from_iter(
                    object
                        .values()
                        .map(db::sql_value)
                        .collect::<Result<Vec<_>>>()?,
                ),
            )?;
            created_aliases.push(alias.clone());
            append_insert(
                conn,
                &mut resolved,
                Table::EntityAlias,
                "entity_alias",
                alias,
            )?;
        }
        if !created_aliases.is_empty() {
            resolved.guards.push(Guard::EntityAliases {
                entity_id: json!({"$integer":id.to_string()}),
                before: vec![],
                after: created_aliases,
            });
        }
        let mut created_links = vec![];
        for pending in &creator.links {
            let mut link = pending.clone();
            link["entity_id"] = json!({"$integer":id.to_string()});
            let object = link.as_object().unwrap();
            let columns = object
                .keys()
                .map(|k| quote(k))
                .collect::<Vec<_>>()
                .join(",");
            let placeholders = vec!["?"; object.len()].join(",");
            let values = object
                .values()
                .map(db::sql_value)
                .collect::<Result<Vec<_>>>()?;
            conn.execute(
                &format!("INSERT INTO asset_entity({columns}) VALUES({placeholders})"),
                params_from_iter(values),
            )?;
            // Retain the complete accepted row, not a new read which could hide
            // a trigger changing supplied confidence/source/metadata values.
            created_links.push(link.clone());
            extend_guards(&mut resolved, Table::AssetEntity, &link);
            append_insert(
                conn,
                &mut resolved,
                Table::AssetEntity,
                "asset_entity",
                link,
            )?;
        }
        resolved.guards.push(Guard::EntityLinks {
            entity_id: json!({"$integer":id.to_string()}),
            before: vec![],
            after: created_links,
        });
    }
    delta::inspect(conn, &resolved, true)?;
    Ok(resolved)
}
