#[allow(unused_imports)]
// Public native planner API; individual integration harnesses use a subset.
pub use crate::delta::{Delta, Guard, Table, TableDelta};
use crate::model::{Error, Result};
use rusqlite::{
    Connection, OpenFlags, Params,
    types::{Type, ValueRef},
};
use serde_json::{Map, Value, json};
use std::{path::Path, time::Duration};

static PARITY_ROOT: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
pub fn configure_parity(root: std::path::PathBuf) -> Result<()> {
    PARITY_ROOT
        .set(root)
        .map_err(|_| Error::new(503, "configuration", "只读对照已初始化"))
}

pub fn open(path: &Path) -> Result<Connection> {
    if let Some(root) = PARITY_ROOT.get() {
        if path.starts_with(root.join("base")) || path == root.join("ledger.db") {
            // SQLite backup snapshots contain no live WAL. immutable avoids SHM creation,
            // recovery, journal deletion, or any other incidental database-file writes.
            let encoded: String = path
                .as_os_str()
                .as_encoded_bytes()
                .iter()
                .map(|b| {
                    if b.is_ascii_alphanumeric() || b"/._-".contains(b) {
                        (*b as char).to_string()
                    } else {
                        format!("%{b:02X}")
                    }
                })
                .collect();
            return Ok(Connection::open_with_flags(
                format!("file:{encoded}?immutable=1"),
                OpenFlags::SQLITE_OPEN_READ_ONLY
                    | OpenFlags::SQLITE_OPEN_URI
                    | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )?);
        }
        if !path.starts_with(root.join("state")) {
            return Err(Error::new(
                403,
                "read_only_parity",
                "只读对照禁止打开快照之外的数据库",
            ));
        }
    }
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
                ValueRef::Real(n) if n.is_finite() => json!(n),
                ValueRef::Real(_) => {
                    return Err(rusqlite::Error::FromSqlConversionFailure(
                        index,
                        Type::Real,
                        Box::new(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "non-finite SQLite REAL cannot be recorded losslessly",
                        )),
                    ));
                }
                ValueRef::Text(s) => json!(std::str::from_utf8(s).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(index, Type::Text, Box::new(error))
                })?),
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

#[cfg(test)]
mod tests {
    #[test]
    fn non_finite_sqlite_real_is_rejected_instead_of_recorded_as_null() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE fixture_values(value REAL)")
            .unwrap();
        for value in [f64::INFINITY, f64::NEG_INFINITY] {
            conn.execute("DELETE FROM fixture_values", []).unwrap();
            conn.execute("INSERT INTO fixture_values VALUES(?)", [value])
                .unwrap();
            assert!(super::rows(&conn, "SELECT * FROM fixture_values", []).is_err());
        }
    }

    #[test]
    fn invalid_utf8_sqlite_text_is_rejected_instead_of_lossily_replaced() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE fixture_values(value TEXT); INSERT INTO fixture_values VALUES(CAST(X'80FF' AS TEXT))").unwrap();
        assert!(super::rows(&conn, "SELECT * FROM fixture_values", []).is_err());
    }

    #[test]
    fn persisted_real_values_keep_exact_sqlite_bits() {
        // Real-valued SQLite timestamps/scores are durable before-images. A JSON
        // roundtrip must not turn an unchanged row into an apparent conflict.
        let mut state = 0x9e3779b97f4a7c15u64;
        for _ in 0..4096 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let value = f64::from_bits(state);
            if value.is_finite() {
                let raw = serde_json::to_string(&value).unwrap();
                let recovered: f64 = serde_json::from_str(&raw).unwrap();
                assert_eq!(value.to_bits(), recovered.to_bits(), "{raw}");
            }
        }
    }
}
