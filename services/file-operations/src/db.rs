use crate::model::{Error, Result};
use rusqlite::{Connection, OpenFlags, Params, types::ValueRef};
use serde_json::{Map, Value, json};
use std::{path::Path, time::Duration};

pub fn open(path: &Path) -> Result<Connection> {
    let db = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    db.busy_timeout(Duration::from_secs(3))?;
    db.execute_batch("PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL;")?;
    Ok(db)
}
// Preserve SQLite integer / blob values in durable before-images without JS numeric loss.
pub fn rows(db: &Connection, sql: &str, args: impl Params) -> Result<Vec<Value>> {
    let mut query = db.prepare(sql)?;
    let names: Vec<String> = query.column_names().map(|s| s.to_string()).collect();
    let mapped = query.query_map(args, |row| {
        let mut result = Map::new();
        for (index, name) in names.iter().enumerate() {
            let value = match row.get_ref(index)? {
                ValueRef::Null => Value::Null,
                ValueRef::Integer(n) => json!({"$integer": n.to_string()}),
                ValueRef::Real(n) => json!(n),
                ValueRef::Text(s) => json!(String::from_utf8_lossy(s)),
                ValueRef::Blob(b) => json!({"$blob":b}),
            };
            result.insert(name.clone(), value);
        }
        Ok(Value::Object(result))
    })?;
    Ok(mapped.collect::<std::result::Result<Vec<_>, _>>()?)
}
pub fn integer(value: &Value) -> Result<i64> {
    value["$integer"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| Error::new(503, "database_contract", "数据库整数格式无效"))
}
pub fn columns(db: &Connection, table: &str) -> Result<Vec<String>> {
    Ok(rows(db, &format!("PRAGMA table_info({table})"), [])?
        .iter()
        .filter_map(|v| v["name"].as_str().map(str::to_owned))
        .collect())
}
pub fn receipt_table(db: &Connection) -> Result<()> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS _face_admin_rust_commits (id TEXT PRIMARY KEY, signature TEXT NOT NULL, data TEXT NOT NULL)")?;
    Ok(())
}

pub fn receipt(db: &Connection, id: &str, signature: &str) -> Result<Option<Value>> {
    use rusqlite::OptionalExtension;
    let exists: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='_face_admin_rust_commits')",
        [],
        |r| r.get(0),
    )?;
    if !exists {
        return Ok(None);
    }
    let row: Option<(String, String)> = db
        .query_row(
            "SELECT signature,data FROM _face_admin_rust_commits WHERE id=?",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    row.map(|(stored, data)| {
        if stored != signature {
            return Err(Error::conflict("数据库回执范围不匹配"));
        }
        Ok(serde_json::from_str(&data)?)
    })
    .transpose()
}
pub fn sql_value(value: &Value) -> Result<rusqlite::types::Value> {
    use rusqlite::types::Value as Sql;
    Ok(match value {
        Value::Null => Sql::Null,
        Value::String(s) => Sql::Text(s.clone()),
        Value::Number(n) => Sql::Real(
            n.as_f64()
                .ok_or_else(|| Error::conflict("无效数据库数字"))?,
        ),
        Value::Object(o) if o.contains_key("$integer") => Sql::Integer(integer(value)?),
        Value::Object(o) if o.contains_key("$blob") => {
            Sql::Blob(serde_json::from_value(value["$blob"].clone())?)
        }
        _ => return Err(Error::conflict("无效数据库值")),
    })
}
