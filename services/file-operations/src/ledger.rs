//! Native Peach transactions. Rename changes only asset.path; all composite-key relations survive.
use crate::{
    config::Config,
    db,
    metadata::win,
    model::{Error, Plan, Result, hash},
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde_json::{Value, json};

fn snapshot_db(db: &Connection, config: &Config, plan: &Plan) -> Result<Value> {
    let old = win(config, &plan.file().src);
    let new = win(config, &plan.file().dst);
    let assets = db::rows(
        db,
        "SELECT * FROM asset WHERE path=? AND disposal IS NULL ORDER BY id",
        [&old],
    )?;
    if assets.len() > 1 {
        return Err(Error::new(
            409,
            "ledger_conflict",
            "Peach 原路径存在多个位置的有效记录，请先核对",
        )
        .at(&old));
    }
    if !db::rows(db, "SELECT id FROM asset WHERE path=?", [&new])?.is_empty() {
        return Err(Error::new(409, "ledger_conflict", "Peach 目标路径已有记录").at(new));
    }
    let entities = db::rows(
        db,
        "SELECT * FROM entity WHERE kind='creator' AND (canonical_name=? OR normalized_name=?) ORDER BY id",
        params![plan.person, plan.person.to_lowercase()],
    )?;
    if entities.len() > 1 {
        return Err(Error::new(409, "ledger_conflict", "Peach 人物记录不唯一"));
    }
    let mut links = vec![];
    for asset in &assets {
        links.extend(db::rows(
            db,
            "SELECT * FROM asset_entity WHERE asset_id=? ORDER BY entity_id,role,source",
            [db::integer(&asset["id"])?],
        )?);
    }
    Ok(json!({"assets":assets,"entities":entities,"links":links,"old":old,"new":new}))
}
pub fn snapshot(config: &Config, plan: &Plan) -> Result<Value> {
    let mut db = db::open(&config.ledger)?;
    let tx = db.transaction()?;
    snapshot_db(&tx, config, plan)
}
pub fn reserve(config: &Config, plan: &Plan) -> Result<()> {
    let mut db = db::open(&config.ledger)?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute_batch("CREATE TABLE IF NOT EXISTS _face_admin_person_ops (id TEXT PRIMARY KEY, signature TEXT NOT NULL, state TEXT NOT NULL, data TEXT NOT NULL)")?;
    let previous: Option<String> = tx
        .query_row(
            "SELECT signature FROM _face_admin_person_ops WHERE id=?",
            [&plan.id],
            |r| r.get(0),
        )
        .optional()?;
    if previous.is_some() {
        return Err(Error::conflict("Peach 操作编号已被使用，需核对原结果"));
    }
    let snapshot = snapshot_db(&tx, config, plan)?;
    if hash(&snapshot) != plan.ledger_version {
        return Err(Error::conflict("Peach 记录已变化，请重新预览"));
    }
    tx.execute(
        "INSERT INTO _face_admin_person_ops VALUES(?,?,'prepared',?)",
        params![plan.id, plan.version, serde_json::to_string(&snapshot)?],
    )?;
    tx.commit()?;
    Ok(())
}
pub fn apply(config: &Config, plan: &Plan) -> Result<Value> {
    let mut db = db::open(&config.ledger)?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let (signature, state): (String, String) = tx.query_row(
        "SELECT signature,state FROM _face_admin_person_ops WHERE id=?",
        [&plan.id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if signature != plan.version || state != "prepared" {
        return Err(Error::conflict("Peach 意图记录不匹配或已提交"));
    }
    let before = snapshot_db(&tx, config, plan)?;
    if hash(&before) != plan.ledger_version {
        return Err(Error::conflict("Peach 记录已变化，停止账本提交"));
    }
    let mut after = vec![];
    for asset in before["assets"].as_array().unwrap() {
        let id = db::integer(&asset["id"])?;
        tx.execute(
            "UPDATE asset SET path=? WHERE id=? AND path=?",
            params![
                win(config, &plan.file().dst),
                id,
                win(config, &plan.file().src)
            ],
        )?;
        after.extend(db::rows(&tx, "SELECT * FROM asset WHERE id=?", [id])?);
    }
    let evidence =
        json!({"backend":"rust","kind":"rename-file","before":before,"after_assets":after});
    tx.execute(
        "UPDATE _face_admin_person_ops SET state='committed',data=? WHERE id=? AND signature=?",
        params![serde_json::to_string(&evidence)?, plan.id, plan.version],
    )?;
    tx.commit()?;
    Ok(evidence)
}

/// Query the transaction receipt and compare the full scoped state before choosing a direction.
pub fn inspect(config: &Config, plan: &Plan) -> Result<String> {
    let mut db = db::open(&config.ledger)?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    inspect_db(&tx, config, plan)
}
fn inspect_db(db: &Connection, config: &Config, plan: &Plan) -> Result<String> {
    let exists: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='_face_admin_person_ops')",
        [],
        |r| r.get(0),
    )?;
    let receipt: Option<(String, String, String)> = if exists {
        db.query_row(
            "SELECT signature,state,data FROM _face_admin_person_ops WHERE id=?",
            [&plan.id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?
    } else {
        None
    };
    let Some((signature, state, data)) = receipt else {
        if hash(&snapshot_db(db, config, plan)?) != plan.ledger_version {
            return Err(Error::conflict("未提交账本已被外部修改"));
        }
        return Ok("absent".into());
    };
    if signature != plan.version {
        return Err(Error::conflict("账本回执范围不匹配"));
    }
    let evidence: Value = serde_json::from_str(&data)?;
    if state == "committed" {
        if hash(&evidence["before"]) != plan.ledger_version {
            return Err(Error::conflict("账本原始证据不匹配"));
        }
        let mut reverse = plan.clone();
        let file = &mut reverse.files[0];
        std::mem::swap(&mut file.src, &mut file.dst);
        let actual = snapshot_db(db, config, &reverse)?;
        if actual["assets"] != evidence["after_assets"]
            || actual["links"] != evidence["before"]["links"]
            || actual["entities"] != evidence["before"]["entities"]
        {
            return Err(Error::conflict("已提交账本被外部修改，保留恢复状态"));
        }
    } else if ["prepared", "cancelled"].contains(&state.as_str()) {
        if hash(&evidence) != plan.ledger_version
            || hash(&snapshot_db(db, config, plan)?) != plan.ledger_version
        {
            return Err(Error::conflict("账本准备记录与实际状态不一致"));
        }
    } else {
        return Err(Error::conflict("账本事务结果未知"));
    }
    Ok(state)
}
/// Fence the reserved transaction before compensation. Never undo an unknown or committed ledger.
pub fn cancel(config: &Config, plan: &Plan) -> Result<()> {
    let mut db = db::open(&config.ledger)?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let state = inspect_db(&tx, config, plan)?;
    if state == "committed" {
        return Err(Error::conflict("账本已经提交，不能按未提交补偿"));
    }
    if state == "cancelled" {
        return Ok(());
    }
    tx.execute_batch("CREATE TABLE IF NOT EXISTS _face_admin_person_ops (id TEXT PRIMARY KEY, signature TEXT NOT NULL, state TEXT NOT NULL, data TEXT NOT NULL)")?;
    if state == "absent" {
        tx.execute(
            "INSERT INTO _face_admin_person_ops VALUES(?,?,'cancelled',?)",
            params![
                plan.id,
                plan.version,
                serde_json::to_string(&snapshot_db(&tx, config, plan)?)?
            ],
        )?;
    } else {
        tx.execute("UPDATE _face_admin_person_ops SET state='cancelled' WHERE id=? AND signature=? AND state='prepared'",params![plan.id,plan.version])?;
    }
    tx.commit()?;
    Ok(())
}

/// Native domain-built row changes, bound to the immutable plan and selected file set.
pub use crate::creator_delta::CreatorIntent;
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Delta {
    pub changes: db::Delta,
    #[serde(default)]
    pub creators: Vec<CreatorIntent>,
}
fn validate_delta(delta: &Delta) -> Result<()> {
    if delta.changes.tables.iter().any(|t| {
        !matches!(
            t.table,
            db::Table::Entity | db::Table::Asset | db::Table::AssetEntity
        )
    }) {
        return Err(Error::conflict("账本事务包含错误的存储表"));
    }
    if delta.changes.guards.iter().any(|guard| {
        !matches!(
            guard,
            db::Guard::AssetPath { .. }
                | db::Guard::AssetPrefix { .. }
                | db::Guard::CreatorName { .. }
                | db::Guard::CreatorCanonical { .. }
                | db::Guard::AssetLinks { .. }
        )
    }) {
        return Err(Error::conflict("账本事务不能包含其他存储域的查询保护"));
    }
    Ok(())
}
pub fn inspect_delta(config: &Config, plan: &Plan, delta: &Delta) -> Result<String> {
    validate_delta(delta)?;
    let mut conn = db::open(&config.ledger)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    inspect_delta_db(&tx, plan, delta)
}
fn inspect_delta_db(conn: &Connection, plan: &Plan, delta: &Delta) -> Result<String> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='_face_admin_person_ops')",
        [],
        |r| r.get(0),
    )?;
    let row: Option<(String, String, String)> = if exists {
        conn.query_row(
            "SELECT signature,state,data FROM _face_admin_person_ops WHERE id=?",
            [&plan.id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?
    } else {
        None
    };
    let Some((signature, state, data)) = row else {
        crate::creator_delta::inspect_before(conn, &delta.changes, &delta.creators)?;
        return Ok("absent".into());
    };
    let evidence: Value = serde_json::from_str(&data)?;
    if signature != plan.version
        || evidence["delta"] != serde_json::to_value(delta)?
        || !["prepared", "committed", "cancelled"].contains(&state.as_str())
    {
        return Err(Error::conflict("账本回执范围或状态不匹配"));
    }
    if state == "committed" {
        let resolved: db::Delta = serde_json::from_value(evidence["resolved"].clone())?;
        crate::delta::inspect(conn, &resolved, true)?;
    } else {
        crate::creator_delta::inspect_before(conn, &delta.changes, &delta.creators)?;
    }
    Ok(state)
}
pub fn reserve_delta(config: &Config, plan: &Plan, delta: &Delta) -> Result<()> {
    validate_delta(delta)?;
    let mut conn = db::open(&config.ledger)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if inspect_delta_db(&tx, plan, delta)? != "absent" {
        return Err(Error::conflict("账本编号已有事务"));
    }
    tx.execute_batch("CREATE TABLE IF NOT EXISTS _face_admin_person_ops (id TEXT PRIMARY KEY, signature TEXT NOT NULL, state TEXT NOT NULL, data TEXT NOT NULL)")?;
    tx.execute(
        "INSERT INTO _face_admin_person_ops VALUES(?,?,'prepared',?)",
        params![
            plan.id,
            plan.version,
            serde_json::to_string(&json!({"backend":"rust","kind":plan.action,"delta":delta}))?
        ],
    )?;
    tx.commit()?;
    Ok(())
}
pub fn apply_delta(config: &Config, plan: &Plan, delta: &Delta) -> Result<Value> {
    validate_delta(delta)?;
    let mut conn = db::open(&config.ledger)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if inspect_delta_db(&tx, plan, delta)? != "prepared" {
        return Err(Error::conflict("账本意图不是待提交状态"));
    }
    let resolved = crate::creator_delta::apply(&tx, &delta.changes, &delta.creators)?;
    let evidence = json!({"backend":"rust","kind":plan.action,"delta":delta,"resolved":resolved});
    tx.execute(
        "UPDATE _face_admin_person_ops SET state='committed',data=? WHERE id=? AND signature=?",
        params![serde_json::to_string(&evidence)?, plan.id, plan.version],
    )?;
    tx.commit()?;
    Ok(evidence)
}
pub fn cancel_delta(config: &Config, plan: &Plan, delta: &Delta) -> Result<()> {
    validate_delta(delta)?;
    let mut conn = db::open(&config.ledger)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    match inspect_delta_db(&tx, plan, delta)?.as_str() {
        "committed" => return Err(Error::conflict("账本已经提交，不能补偿")),
        "cancelled" => return Ok(()),
        "absent" => {
            tx.execute_batch("CREATE TABLE IF NOT EXISTS _face_admin_person_ops (id TEXT PRIMARY KEY, signature TEXT NOT NULL, state TEXT NOT NULL, data TEXT NOT NULL)")?;
            tx.execute(
                "INSERT INTO _face_admin_person_ops VALUES(?,?,'cancelled',?)",
                params![
                    plan.id,
                    plan.version,
                    serde_json::to_string(
                        &json!({"backend":"rust","kind":plan.action,"delta":delta})
                    )?
                ],
            )?;
        }
        _ => {
            tx.execute("UPDATE _face_admin_person_ops SET state='cancelled' WHERE id=? AND signature=? AND state='prepared'",params![plan.id,plan.version])?;
        }
    }
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod guard_domain_tests {
    use super::*;
    #[test]
    fn peach_transactions_reject_local_file_guards() {
        let mut delta = Delta {
            changes: db::Delta {
                tables: vec![],
                guards: vec![db::Guard::AssetPath {
                    path: "a".into(),
                    before: vec![],
                    after: vec![],
                }],
            },
            creators: vec![],
        };
        validate_delta(&delta).unwrap();
        for table in [db::Table::Videos, db::Table::Tasks, db::Table::Settings] {
            delta.changes.guards = vec![db::Guard::LocalFiles {
                table,
                vids: vec![],
                paths: vec![],
                before: vec![],
                after: vec![],
            }];
            assert!(validate_delta(&delta).is_err());
        }
    }
}
