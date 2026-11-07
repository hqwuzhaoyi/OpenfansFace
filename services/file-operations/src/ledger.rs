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
