//! Internal, exact-row deltas. The caller owns the transaction and its receipt and
//! MUST roll it back on any error. These helpers never commit or infer a receipt.
use crate::{
    db,
    model::{Error, Result},
};
use rusqlite::{Connection, params_from_iter};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum Table {
    Videos,
    Tasks,
    Settings,
    Entity,
    Asset,
    AssetEntity,
    EntityAlias,
}
impl Table {
    fn name(self) -> &'static str {
        match self {
            Self::Videos => "videos",
            Self::Tasks => "tasks",
            Self::Settings => "settings",
            Self::Entity => "entity",
            Self::Asset => "asset",
            Self::AssetEntity => "asset_entity",
            Self::EntityAlias => "entity_alias",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Delta {
    pub tables: Vec<TableDelta>,
    #[serde(default)]
    pub guards: Vec<Guard>,
}

/// Fixed predicates protect domain lookups against newly inserted matching rows.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub enum Guard {
    EntityLinks {
        entity_id: Value,
        before: Vec<Value>,
        after: Vec<Value>,
    },
    EntityAliases {
        entity_id: Value,
        before: Vec<Value>,
        after: Vec<Value>,
    },
    AssetPrefix {
        prefix: String,
        before: Vec<Value>,
        after: Vec<Value>,
    },
    LocalIdentity {
        table: Table,
        vids: Vec<String>,
        paths: Vec<String>,
        prefixes: Vec<String>,
        names: Vec<String>,
        before: Vec<Value>,
        after: Vec<Value>,
    },
    AssetPath {
        path: String,
        before: Vec<Value>,
        after: Vec<Value>,
    },
    CreatorName {
        canonical: String,
        normalized: String,
        before: Vec<Value>,
        after: Vec<Value>,
    },
    CreatorCanonical {
        canonical: String,
        before: Vec<Value>,
        after: Vec<Value>,
    },
    AssetLinks {
        asset_id: Value,
        before: Vec<Value>,
        after: Vec<Value>,
    },
    LocalFiles {
        table: Table,
        vids: Vec<String>,
        paths: Vec<String>,
        before: Vec<Value>,
        after: Vec<Value>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TableDelta {
    pub table: Table,
    pub key_columns: Vec<String>,
    pub scope: Vec<Value>,
    pub before: Vec<Value>,
    pub after: Vec<Value>,
}

struct Schema {
    columns: Vec<String>,
    primary: Vec<String>,
}
fn invalid(message: impl Into<String>) -> Error {
    Error::conflict(message)
}
fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn schema(conn: &Connection, table: &str) -> Result<Schema> {
    let info = db::rows(conn, &format!("PRAGMA table_xinfo({})", quoted(table)), [])?;
    if info.is_empty()
        || info
            .iter()
            .any(|r| db::integer(&r["hidden"]).unwrap_or(-1) != 0)
    {
        return Err(invalid("行变更表不存在或包含不支持的生成列"));
    }
    let columns = info
        .iter()
        .map(|r| r["name"].as_str().unwrap().to_owned())
        .collect();
    let mut primary = info
        .iter()
        .filter_map(|r| {
            let order = db::integer(&r["pk"]).ok()?;
            (order > 0).then(|| (order, r["name"].as_str().unwrap().to_owned()))
        })
        .collect::<Vec<_>>();
    primary.sort();
    Ok(Schema {
        columns,
        primary: primary.into_iter().map(|(_, name)| name).collect(),
    })
}

fn same_columns(a: &[String], b: &[String]) -> bool {
    !a.is_empty() && a.len() == b.len() && a.iter().collect::<BTreeSet<_>>() == b.iter().collect()
}

fn unique_selector(conn: &Connection, table: Table, key: &[String], schema: &Schema) -> Result<()> {
    if key.is_empty()
        || key.iter().collect::<BTreeSet<_>>().len() != key.len()
        || key.iter().any(|k| !schema.columns.contains(k))
    {
        return Err(invalid("行选择器列无效"));
    }
    if same_columns(key, &schema.primary) {
        return Ok(());
    }
    for index in db::rows(
        conn,
        &format!("PRAGMA index_list({})", quoted(table.name())),
        [],
    )? {
        if db::integer(&index["unique"])? != 1 || db::integer(&index["partial"])? != 0 {
            continue;
        }
        let name = index["name"]
            .as_str()
            .ok_or_else(|| invalid("索引名称无效"))?;
        let columns = db::rows(conn, &format!("PRAGMA index_info({})", quoted(name)), [])?;
        let names = columns
            .iter()
            .filter_map(|r| r["name"].as_str().map(str::to_owned))
            .collect::<Vec<_>>();
        if names.len() == columns.len() && same_columns(key, &names) {
            return Ok(());
        }
    }
    Err(invalid("行选择器必须匹配实际主键或完整唯一约束"))
}

// Require exactly the durable encoding produced by db::rows, including INTEGER
// versus REAL and byte-for-byte BLOB data. Do not accept ambiguous tagged objects.
fn valid_value(value: &Value) -> Result<()> {
    match value {
        Value::Null | Value::String(_) | Value::Number(_) => {}
        Value::Object(map) if map.len() == 1 && map.contains_key("$integer") => {
            let number = db::integer(value)?;
            if value["$integer"].as_str() != Some(number.to_string().as_str()) {
                return Err(invalid("非规范整数编码"));
            }
        }
        Value::Object(map) if map.len() == 1 && map.contains_key("$blob") => {
            let _: Vec<u8> = serde_json::from_value(value["$blob"].clone())?;
        }
        _ => return Err(invalid("数据库快照值编码无效")),
    }
    db::sql_value(value)?;
    Ok(())
}

fn object_columns(row: &Value, columns: &[String]) -> Result<()> {
    let object = row
        .as_object()
        .ok_or_else(|| invalid("数据库行必须为对象"))?;
    if object.len() != columns.len() || columns.iter().any(|c| !object.contains_key(c)) {
        return Err(invalid("数据库行必须包含全部且仅有实际列"));
    }
    for value in object.values() {
        valid_value(value)?;
    }
    Ok(())
}

fn key(row: &Value, columns: &[String]) -> Result<String> {
    let mut map = serde_json::Map::new();
    for column in columns {
        let value = row.get(column).ok_or_else(|| invalid("行键缺少列"))?;
        if value.is_null() {
            return Err(invalid("行键不允许 NULL"));
        }
        valid_value(value)?;
        map.insert(column.clone(), value.clone());
    }
    Ok(serde_json::to_string(&map)?)
}

fn indexed<'a>(rows: &'a [Value], columns: &[String]) -> Result<BTreeMap<String, &'a Value>> {
    let mut result = BTreeMap::new();
    for row in rows {
        if result.insert(key(row, columns)?, row).is_some() {
            return Err(invalid("数据库快照包含重复行键"));
        }
    }
    Ok(result)
}

fn validate(conn: &Connection, delta: &Delta) -> Result<BTreeMap<Table, Schema>> {
    let mut schemas = BTreeMap::new();
    for change in &delta.tables {
        let structure = schema(conn, change.table.name())?;
        unique_selector(conn, change.table, &change.key_columns, &structure)?;
        for row in change.before.iter().chain(&change.after) {
            object_columns(row, &structure.columns)?;
        }
        for row in &change.scope {
            object_columns(row, &change.key_columns)?;
        }
        let before = indexed(&change.before, &change.key_columns)?;
        let after = indexed(&change.after, &change.key_columns)?;
        let scope = indexed(&change.scope, &change.key_columns)?;
        let union = before.keys().chain(after.keys()).collect::<BTreeSet<_>>();
        if union != scope.keys().collect() {
            return Err(invalid("行范围必须精确等于变更前后键的并集"));
        }
        if !structure.primary.is_empty() {
            indexed(&change.before, &structure.primary)?;
            indexed(&change.after, &structure.primary)?;
        }
        if schemas.insert(change.table, structure).is_some() {
            return Err(invalid("同一表只能有一个行变更集"));
        }
    }
    for guard in &delta.guards {
        let (table, before, after) = match guard {
            Guard::EntityLinks {
                entity_id,
                before,
                after,
            } => {
                db::integer(entity_id)?;
                ("asset_entity", before, after)
            }
            Guard::EntityAliases {
                entity_id,
                before,
                after,
            } => {
                db::integer(entity_id)?;
                ("entity_alias", before, after)
            }
            Guard::AssetPath { before, after, .. } | Guard::AssetPrefix { before, after, .. } => {
                ("asset", before, after)
            }
            Guard::CreatorName { before, after, .. }
            | Guard::CreatorCanonical { before, after, .. } => ("entity", before, after),
            Guard::AssetLinks {
                asset_id,
                before,
                after,
            } => {
                valid_value(asset_id)?;
                db::integer(asset_id)?;
                ("asset_entity", before, after)
            }
            Guard::LocalFiles {
                table,
                before,
                after,
                ..
            }
            | Guard::LocalIdentity {
                table,
                before,
                after,
                ..
            } => (local_table(*table)?, before, after),
        };
        let structure = schema(conn, table)?;
        for row in before.iter().chain(after) {
            object_columns(row, &structure.columns)?;
        }
        // Guard snapshots use the actual primary identity, including Peach's
        // four-column relation key, and may never hide duplicate rows.
        if !structure.primary.is_empty() {
            indexed(before, &structure.primary)?;
            indexed(after, &structure.primary)?;
        }
    }
    Ok(schemas)
}

fn predicate(columns: &[String]) -> String {
    columns
        .iter()
        .map(|c| format!("{} IS ?", quoted(c)))
        .collect::<Vec<_>>()
        .join(" AND ")
}
fn values(row: &Value, columns: &[String]) -> Result<Vec<rusqlite::types::Value>> {
    columns.iter().map(|c| db::sql_value(&row[c])).collect()
}

fn local_table(table: Table) -> Result<&'static str> {
    match table {
        Table::Videos | Table::Tasks => Ok(table.name()),
        _ => Err(invalid("本地文件查询保护只允许 videos 或 tasks")),
    }
}

/// Union the fixed vid/path predicates in bounded chunks so large accepted file
/// sets do not depend on SQLite's build-specific parameter limit. Rows matching
/// both selectors occur only once in the resulting snapshot.
fn local_files(
    conn: &Connection,
    table: Table,
    vids: &[String],
    paths: &[String],
) -> Result<Vec<Value>> {
    let table = local_table(table)?;
    let mut found = BTreeMap::new();
    for (column, selectors) in [("vid", vids), ("path", paths)] {
        for chunk in selectors.chunks(400) {
            let sql = format!(
                "SELECT * FROM {} WHERE {} IN ({})",
                quoted(table),
                quoted(column),
                vec!["?"; chunk.len()].join(",")
            );
            for row in db::rows(conn, &sql, params_from_iter(chunk))? {
                found.insert(serde_json::to_string(&row)?, row);
            }
        }
    }
    Ok(found.into_values().collect())
}

fn inspect_rows(conn: &Connection, delta: &Delta, after: bool) -> Result<()> {
    for change in &delta.tables {
        let expected = indexed(
            if after { &change.after } else { &change.before },
            &change.key_columns,
        )?;
        let sql = format!(
            "SELECT * FROM {} WHERE {}",
            quoted(change.table.name()),
            predicate(&change.key_columns)
        );
        for selector in &change.scope {
            let actual = db::rows(
                conn,
                &sql,
                params_from_iter(values(selector, &change.key_columns)?),
            )?;
            let selected = expected.get(&key(selector, &change.key_columns)?);
            if match selected {
                Some(row) => actual.len() != 1 || actual[0] != **row,
                None => !actual.is_empty(),
            } {
                return Err(invalid(format!(
                    "{} 的作用范围记录已变化",
                    change.table.name()
                )));
            }
        }
    }
    for guard in &delta.guards {
        let (actual, expected, table) = match guard {
            Guard::EntityLinks {
                entity_id,
                before,
                after: next,
            } => (
                db::rows(
                    conn,
                    "SELECT * FROM asset_entity WHERE entity_id=?",
                    [db::sql_value(entity_id)?],
                )?,
                if after { next } else { before },
                "asset_entity",
            ),
            Guard::EntityAliases {
                entity_id,
                before,
                after: next,
            } => (
                db::rows(
                    conn,
                    "SELECT * FROM entity_alias WHERE entity_id=?",
                    [db::sql_value(entity_id)?],
                )?,
                if after { next } else { before },
                "entity_alias",
            ),
            Guard::AssetPrefix {
                prefix,
                before,
                after: next,
            } => (
                db::rows(
                    conn,
                    "SELECT * FROM asset WHERE substr(path,1,length(?))=? ORDER BY id",
                    [prefix, prefix],
                )?,
                if after { next } else { before },
                "asset",
            ),
            Guard::LocalIdentity {
                table,
                vids,
                paths,
                prefixes,
                names,
                before,
                after: next,
            } => (
                local_identity(conn, *table, vids, paths, prefixes, names)?,
                if after { next } else { before },
                local_table(*table)?,
            ),
            Guard::AssetPath {
                path,
                before,
                after: next,
            } => (
                db::rows(conn, "SELECT * FROM asset WHERE path=? ORDER BY id", [path])?,
                if after { next } else { before },
                "asset",
            ),
            Guard::CreatorName {
                canonical,
                normalized,
                before,
                after: next,
            } => (
                db::rows(
                    conn,
                    "SELECT * FROM entity WHERE kind='creator' AND (canonical_name=? OR normalized_name=?) ORDER BY id",
                    [canonical, normalized],
                )?,
                if after { next } else { before },
                "entity",
            ),
            Guard::CreatorCanonical {
                canonical,
                before,
                after: next,
            } => (
                db::rows(
                    conn,
                    "SELECT * FROM entity WHERE kind='creator' AND canonical_name=? ORDER BY id",
                    [canonical],
                )?,
                if after { next } else { before },
                "entity",
            ),
            Guard::AssetLinks {
                asset_id,
                before,
                after: next,
            } => {
                db::integer(asset_id)?;
                (
                    db::rows(
                        conn,
                        "SELECT * FROM asset_entity WHERE asset_id=? ORDER BY entity_id,role,source",
                        [db::sql_value(asset_id)?],
                    )?,
                    if after { next } else { before },
                    "asset_entity",
                )
            }
            Guard::LocalFiles {
                table,
                vids,
                paths,
                before,
                after: next,
            } => (
                local_files(conn, *table, vids, paths)?,
                if after { next } else { before },
                local_table(*table)?,
            ),
        };
        let structure = schema(conn, table)?;
        for row in expected {
            object_columns(row, &structure.columns)?;
        }
        let canonical = |rows: &[Value]| -> Result<BTreeSet<String>> {
            let sorted = rows
                .iter()
                .map(serde_json::to_string)
                .collect::<std::result::Result<BTreeSet<_>, _>>()?;
            if sorted.len() != rows.len() {
                return Err(invalid("查询快照包含重复行"));
            }
            Ok(sorted)
        };
        if canonical(&actual)? != canonical(expected)? {
            return Err(invalid("数据库查询范围出现新增、缺失或外部修改的记录"));
        }
    }
    Ok(())
}

/// Check full scoped rows and allowlisted phantom guards; does not mutate.
pub fn inspect(conn: &Connection, delta: &Delta, after: bool) -> Result<()> {
    validate(conn, delta)?;
    inspect_rows(conn, delta, after)
}

/// Reject any parent-key mutation which would implicitly mutate surviving child
/// rows. Explicit child deletions run first. Unknown Peach relationship tables
/// are checked too, so ON DELETE CASCADE cannot silently erase their evidence.
pub(crate) fn protect_references(
    conn: &Connection,
    table: Table,
    before: &Value,
    after: Option<&Value>,
    primary: &[String],
) -> Result<()> {
    let tables = db::rows(
        conn,
        "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
        [],
    )?;
    for child in tables {
        let name = child["name"].as_str().unwrap();
        let mut groups = BTreeMap::<i64, Vec<Value>>::new();
        for fk in db::rows(
            conn,
            &format!("PRAGMA foreign_key_list({})", quoted(name)),
            [],
        )? {
            if fk["table"]
                .as_str()
                .is_some_and(|t| t.eq_ignore_ascii_case(table.name()))
            {
                groups.entry(db::integer(&fk["id"])?).or_default().push(fk);
            }
        }
        for (_, mut group) in groups {
            group.sort_by_key(|fk| db::integer(&fk["seq"]).unwrap_or(-1));
            let mut columns = vec![];
            let mut args = vec![];
            let mut changed = after.is_none();
            for (i, fk) in group.iter().enumerate() {
                let parent = fk["to"]
                    .as_str()
                    .or_else(|| primary.get(i).map(String::as_str))
                    .ok_or_else(|| invalid("外键父列无法确定"))?;
                let value = before
                    .get(parent)
                    .ok_or_else(|| invalid("外键父列不存在"))?;
                changed |= after.is_some_and(|row| row.get(parent) != Some(value));
                // SQLite foreign keys compare using the parent's affinity and
                // collation, not the child's. Keep the parent on the left.
                columns.push(format!(
                    "p.{} = c.{} AND p.{} = ?",
                    quoted(parent),
                    quoted(fk["from"].as_str().ok_or_else(|| invalid("外键子列无效"))?),
                    quoted(parent)
                ));
                args.push(db::sql_value(value)?);
            }
            if changed {
                let exists: bool = conn.query_row(
                    &format!(
                        "SELECT EXISTS(SELECT 1 FROM {} AS c JOIN {} AS p ON {})",
                        quoted(name),
                        quoted(table.name()),
                        columns.join(" AND ")
                    ),
                    params_from_iter(args),
                    |row| row.get(0),
                )?;
                if exists {
                    return Err(invalid(format!(
                        "{} 仍有 {} 外键关联，拒绝隐式级联变更",
                        table.name(),
                        name
                    )));
                }
            }
        }
    }
    Ok(())
}

fn mutate(conn: &Connection, delta: &Delta, reverse: bool) -> Result<()> {
    if conn.is_autocommit() {
        return Err(invalid("行变更必须由调用方持有事务"));
    }
    let foreign_keys: bool = conn.query_row("PRAGMA foreign_keys", [], |row| row.get(0))?;
    if !foreign_keys {
        return Err(invalid("行变更要求启用外键约束"));
    }
    let schemas = validate(conn, delta)?;
    inspect_rows(conn, delta, reverse)?;
    // Parents precede children on insertion. Inverse images use the same safe
    // dependency order, which reverses the effects of the forward operation.
    let mut changes = delta.tables.iter().collect::<Vec<_>>();
    changes.sort_by_key(|change| change.table);
    for change in &changes {
        let structure = &schemas[&change.table];
        let identity = if structure.primary.is_empty() {
            &change.key_columns
        } else {
            &structure.primary
        };
        let (source, target) = if reverse {
            (&change.after, &change.before)
        } else {
            (&change.before, &change.after)
        };
        let old = indexed(source, identity)?;
        for row in target {
            if old.contains_key(&key(row, identity)?) {
                continue;
            }
            let columns = structure
                .columns
                .iter()
                .map(|c| quoted(c))
                .collect::<Vec<_>>()
                .join(",");
            let placeholders = vec!["?"; structure.columns.len()].join(",");
            conn.execute(
                &format!(
                    "INSERT INTO {} ({columns}) VALUES ({placeholders})",
                    quoted(change.table.name())
                ),
                params_from_iter(values(row, &structure.columns)?),
            )?;
        }
    }
    for change in &changes {
        let structure = &schemas[&change.table];
        let identity = if structure.primary.is_empty() {
            &change.key_columns
        } else {
            &structure.primary
        };
        let (source, target) = if reverse {
            (&change.after, &change.before)
        } else {
            (&change.before, &change.after)
        };
        let old = indexed(source, identity)?;
        for row in target {
            let Some(previous) = old.get(&key(row, identity)?) else {
                continue;
            };
            let changed = structure
                .columns
                .iter()
                .filter(|column| previous[*column] != row[*column])
                .cloned()
                .collect::<Vec<_>>();
            if changed.is_empty() {
                continue;
            }
            protect_references(conn, change.table, previous, Some(row), &structure.primary)?;
            let assignments = changed
                .iter()
                .map(|c| format!("{}=?", quoted(c)))
                .collect::<Vec<_>>()
                .join(",");
            let mut args = values(row, &changed)?;
            args.extend(values(previous, identity)?);
            if conn.execute(
                &format!(
                    "UPDATE {} SET {assignments} WHERE {}",
                    quoted(change.table.name()),
                    predicate(identity)
                ),
                params_from_iter(args),
            )? != 1
            {
                return Err(invalid("更新行数量发生变化"));
            }
        }
    }
    for change in changes.iter().rev() {
        let structure = &schemas[&change.table];
        let identity = if structure.primary.is_empty() {
            &change.key_columns
        } else {
            &structure.primary
        };
        let (source, target) = if reverse {
            (&change.after, &change.before)
        } else {
            (&change.before, &change.after)
        };
        let next = indexed(target, identity)?;
        for row in source.iter().rev() {
            if next.contains_key(&key(row, identity)?) {
                continue;
            }
            protect_references(conn, change.table, row, None, &structure.primary)?;
            if conn.execute(
                &format!(
                    "DELETE FROM {} WHERE {}",
                    quoted(change.table.name()),
                    predicate(identity)
                ),
                params_from_iter(values(row, identity)?),
            )? != 1
            {
                return Err(invalid("删除行数量发生变化"));
            }
        }
    }
    inspect_rows(conn, delta, !reverse)
}

/// Validate before, perform only the explicit changes, and validate after.
/// Repeated application is a conflict; receipt-based replay belongs to caller.
pub fn apply(conn: &Connection, delta: &Delta) -> Result<()> {
    mutate(conn, delta, false)
}
/// Validate after before restoring. External changes block compensation.
pub fn restore(conn: &Connection, delta: &Delta) -> Result<()> {
    mutate(conn, delta, true)
}

/// Native identity predicate. Raw OCR, identifiers and notes are never identity matches.
fn identity_reference(
    value: &Value,
    names: &[String],
    paths: &[String],
    prefixes: &[String],
) -> bool {
    match value {
        Value::Array(rows) => rows
            .iter()
            .any(|v| identity_reference(v, names, paths, prefixes)),
        Value::Object(map) => map.iter().any(|(key, v)| {
            v.as_str().is_some_and(|s| {
                ([
                    "path",
                    "src",
                    "dst",
                    "win_dst",
                    "win_src",
                    "original_dst",
                    "canonical_dst",
                    "collection",
                    "root",
                ]
                .contains(&key.as_str())
                    && (paths.iter().any(|p| p == s)
                        || prefixes
                            .iter()
                            .any(|p| s.starts_with(p) || s == p.trim_end_matches(['/', '\\']))))
                    || ((["name", "creator", "suggested"].contains(&key.as_str())
                        || key == "source_id" && value["source"] == "person")
                        && names.iter().any(|n| n == s))
                    || (key == "return_href"
                        && names
                            .iter()
                            .any(|n| s == format!("/persons/{}", encode_identity(n))))
            }) || identity_reference(v, names, paths, prefixes)
        }),
        _ => false,
    }
}
fn encode_identity(s: &str) -> String {
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
pub fn local_identity(
    conn: &Connection,
    table: Table,
    vids: &[String],
    paths: &[String],
    prefixes: &[String],
    names: &[String],
) -> Result<Vec<Value>> {
    let table_name = local_table(table)?;
    let mut found = BTreeMap::new();
    for row in local_files(conn, table, vids, paths)? {
        found.insert(row["vid"].as_str().unwrap().to_owned(), row);
    }
    // Stream to keep memory bounded by the relevant result, even during unrelated worker progress.
    let mut stmt = conn.prepare(&format!(
        "SELECT vid,path{} FROM {table_name}",
        if table == Table::Tasks { ",data" } else { "" }
    ))?;
    let mut rows = stmt.query([])?;
    let mut related = Vec::new();
    while let Some(row) = rows.next()? {
        let vid: String = row.get(0)?;
        let path: Option<String> = row.get(1)?;
        let path_related = path
            .as_ref()
            .is_some_and(|p| prefixes.iter().any(|prefix| p.starts_with(prefix)));
        let data_related = if table == Table::Tasks {
            let raw: String = row.get(2)?;
            let data: Value = serde_json::from_str(&raw)?;
            identity_reference(&data, names, paths, prefixes)
        } else {
            false
        };
        if path_related || data_related {
            related.push(vid)
        }
    }
    drop(rows);
    drop(stmt);
    for chunk in related.chunks(400) {
        for row in db::rows(
            conn,
            &format!(
                "SELECT * FROM {table_name} WHERE vid IN ({})",
                vec!["?"; chunk.len()].join(",")
            ),
            params_from_iter(chunk),
        )? {
            found.insert(row["vid"].as_str().unwrap().to_owned(), row);
        }
    }
    Ok(found.into_values().collect())
}
