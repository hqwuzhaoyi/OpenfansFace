#![allow(dead_code)]
#[path = "../src/creator_delta.rs"]
mod creator_delta;
#[path = "../src/db.rs"]
mod db;
#[path = "../src/delta.rs"]
mod delta;
#[path = "../src/model.rs"]
mod model;
use creator_delta::CreatorIntent;
use delta::{Delta, Guard, Table, TableDelta};
use rusqlite::Connection;
use serde_json::{Value, json};

fn fixture() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(include_str!(
        "../../../tests/fixtures/peach-contract-schema.sql"
    ))
    .unwrap();
    conn.execute_batch("INSERT INTO entity(id,kind,canonical_name,normalized_name,created_at,updated_at) VALUES(1,'creator','Alice','alice','fixture','fixture'); INSERT INTO asset(id,location,path) VALUES(1,'fixture','old'); INSERT INTO asset_entity(asset_id,entity_id,role,source) VALUES(1,1,'creator','manual'),(1,1,'creator','import');").unwrap();
    conn
}
fn rows(conn: &Connection, table: &str) -> Vec<Value> {
    db::rows(conn, &format!("SELECT * FROM {table}"), []).unwrap()
}
fn pending() -> Value {
    json!({"asset_id":{"$integer":"1"},"role":"creator","source":"manual","confidence":1.0,"metadata_json":"{}","first_seen_at":null,"last_seen_at":null})
}
fn intent() -> CreatorIntent {
    CreatorIntent {
        aliases: vec![],
        metadata_json: None,
        canonical_name: "Bob".into(),
        normalized_name: "bob".into(),
        links: vec![pending()],
    }
}
fn empty() -> Delta {
    Delta {
        tables: vec![],
        guards: vec![],
    }
}
fn guarded(conn: &Connection) -> Delta {
    Delta {
        tables: vec![],
        guards: vec![
            Guard::CreatorName {
                canonical: "Bob".into(),
                normalized: "bob".into(),
                before: vec![],
                after: vec![],
            },
            Guard::AssetLinks {
                asset_id: json!({"$integer":"1"}),
                before: rows(conn, "asset_entity"),
                after: rows(conn, "asset_entity"),
            },
        ],
    }
}

#[test]
fn native_id_allocation_survives_unrelated_insert_and_restores_complete_rows() {
    let mut conn = fixture();
    let changes = guarded(&conn);
    let creators = vec![intent()];
    creator_delta::inspect_before(&conn, &changes, &creators).unwrap();
    conn.execute("INSERT INTO entity(id,kind,canonical_name,normalized_name,created_at,updated_at) VALUES(700,'creator','Unrelated','unrelated','fixture','fixture')",[]).unwrap();
    let entities = rows(&conn, "entity");
    let links = rows(&conn, "asset_entity");
    let tx = conn.transaction().unwrap();
    let realized = creator_delta::apply(&tx, &changes, &creators).unwrap();
    let created = realized
        .tables
        .iter()
        .find(|t| t.table == Table::Entity)
        .unwrap()
        .after[0]
        .clone();
    assert_eq!(created["id"], json!({"$integer":"701"}));
    assert_eq!(created["metadata_json"], json!("{}"));
    assert_eq!(created["region"], Value::Null);
    assert!(created["created_at"].as_str().unwrap().ends_with('Z'));
    let link = realized
        .tables
        .iter()
        .find(|t| t.table == Table::AssetEntity)
        .unwrap()
        .after[0]
        .clone();
    assert_eq!(link["entity_id"], created["id"]);
    assert_eq!(link["confidence"], json!(1.0));
    delta::inspect(&tx, &realized, true).unwrap();
    delta::restore(&tx, &realized).unwrap();
    assert_eq!(rows(&tx, "entity"), entities);
    assert_eq!(rows(&tx, "asset_entity"), links);
    tx.commit().unwrap();
}

#[test]
fn base_changes_and_created_rows_share_one_realized_delta() {
    let mut conn = fixture();
    let links = rows(&conn, "asset_entity");
    let entities = rows(&conn, "entity");
    let mut changes = guarded(&conn);
    let keys = vec![
        "asset_id".into(),
        "entity_id".into(),
        "role".into(),
        "source".into(),
    ];
    changes.tables.push(TableDelta{table:Table::AssetEntity,key_columns:keys,scope:links.iter().map(|r| json!({"asset_id":r["asset_id"],"entity_id":r["entity_id"],"role":r["role"],"source":r["source"]})).collect(),before:links.clone(),after:vec![]});
    if let Guard::AssetLinks { after, .. } = &mut changes.guards[1] {
        *after = vec![];
    }
    let tx = conn.transaction().unwrap();
    let realized = creator_delta::apply(&tx, &changes, &[intent()]).unwrap();
    assert_eq!(rows(&tx, "asset_entity").len(), 1);
    delta::restore(&tx, &realized).unwrap();
    assert_eq!(rows(&tx, "entity"), entities);
    assert_eq!(rows(&tx, "asset_entity"), links);
}

#[test]
fn missing_guard_is_created_from_original_scope_plus_base_effects() {
    let mut conn = fixture();
    let links = rows(&conn, "asset_entity");
    let mut changes = empty();
    changes.tables.push(TableDelta{table:Table::AssetEntity,key_columns:vec!["asset_id".into(),"entity_id".into(),"role".into(),"source".into()],scope:links.iter().map(|r| json!({"asset_id":r["asset_id"],"entity_id":r["entity_id"],"role":r["role"],"source":r["source"]})).collect(),before:links.clone(),after:vec![]});
    let tx = conn.transaction().unwrap();
    let realized = creator_delta::apply(&tx, &changes, &[intent()]).unwrap();
    assert!(
        realized
            .guards
            .iter()
            .any(|g| matches!(g, Guard::EntityLinks { .. }))
    );
    delta::restore(&tx, &realized).unwrap();
    assert_eq!(rows(&tx, "asset_entity"), links);
}

#[test]
fn receipt_replay_uses_realized_snapshot_without_allocating_again() {
    let mut conn = fixture();
    let changes = guarded(&conn);
    let creators = vec![intent()];
    let tx = conn.transaction().unwrap();
    db::receipt_table(&tx).unwrap();
    let realized = creator_delta::apply(&tx, &changes, &creators).unwrap();
    let data = serde_json::to_string(&realized).unwrap();
    tx.execute(
        "INSERT INTO _face_admin_rust_commits VALUES('fixture-op','fixture-signature',?)",
        [&data],
    )
    .unwrap();
    tx.commit().unwrap();
    let receipt = db::receipt(&conn, "fixture-op", "fixture-signature")
        .unwrap()
        .unwrap();
    let resolved: Delta = serde_json::from_value(receipt).unwrap();
    delta::inspect(&conn, &resolved, true).unwrap();
    assert_eq!(rows(&conn, "entity").len(), 2);
    {
        let tx = conn.transaction().unwrap();
        assert!(creator_delta::apply(&tx, &changes, &creators).is_err());
    }
    assert_eq!(rows(&conn, "entity").len(), 2);
}

#[test]
fn phantom_creator_or_link_blocks_before_mutation() {
    let conn = fixture();
    let changes = guarded(&conn);
    conn.execute("INSERT INTO entity(id,kind,canonical_name,normalized_name,created_at,updated_at) VALUES(2,'creator','Other','bob','fixture','fixture')",[]).unwrap();
    assert!(creator_delta::inspect_before(&conn, &changes, &[intent()]).is_err());
    conn.execute("DELETE FROM entity WHERE id=2", []).unwrap();
    conn.execute("INSERT INTO asset_entity(asset_id,entity_id,role,source) VALUES(1,1,'appearance','foreign')",[]).unwrap();
    assert!(creator_delta::inspect_before(&conn, &changes, &[intent()]).is_err());
}

#[test]
fn late_link_failure_rolls_back_creator_and_base_changes_atomically() {
    let mut conn = fixture();
    let entities = rows(&conn, "entity");
    let assets = rows(&conn, "asset");
    let mut after = assets.clone();
    after[0]["path"] = json!("new");
    let changes = Delta {
        tables: vec![TableDelta {
            table: Table::Asset,
            key_columns: vec!["id".into()],
            scope: vec![json!({"id":{"$integer":"1"}})],
            before: assets.clone(),
            after,
        }],
        guards: vec![],
    };
    let mut creator = intent();
    creator.links[0]["asset_id"] = json!({"$integer":"999"});
    {
        let tx = conn.transaction().unwrap();
        assert!(creator_delta::apply(&tx, &changes, &[creator]).is_err());
    }
    assert_eq!(rows(&conn, "entity"), entities);
    assert_eq!(rows(&conn, "asset"), assets);
}

#[test]
fn malformed_or_duplicate_intents_and_partial_links_are_rejected() {
    let conn = fixture();
    let mut creator = intent();
    creator.links[0].as_object_mut().unwrap().remove("source");
    assert!(creator_delta::inspect_before(&conn, &empty(), &[creator]).is_err());
    let mut creator = intent();
    creator.links[0]["entity_id"] = json!({"$integer":"2"});
    assert!(creator_delta::inspect_before(&conn, &empty(), &[creator]).is_err());
    let mut creator = intent();
    creator.links.push(pending());
    assert!(creator_delta::inspect_before(&conn, &empty(), &[creator]).is_err());
    assert!(creator_delta::inspect_before(&conn, &empty(), &[intent(), intent()]).is_err());
    let mut creator = intent();
    creator.links[0]["asset_id"] = json!({"$integer":"01"});
    assert!(creator_delta::inspect_before(&conn, &empty(), &[creator]).is_err());
}

#[test]
fn restore_blocks_foreign_changes_in_created_scope_and_unknown_cascades() {
    let mut conn = fixture();
    let tx = conn.transaction().unwrap();
    let realized = creator_delta::apply(&tx, &empty(), &[intent()]).unwrap();
    tx.execute(
        "UPDATE entity SET metadata_json='external' WHERE normalized_name='bob'",
        [],
    )
    .unwrap();
    assert!(delta::restore(&tx, &realized).is_err());
    tx.rollback().unwrap();
    let tx = conn.transaction().unwrap();
    let realized = creator_delta::apply(&tx, &empty(), &[intent()]).unwrap();
    let id = db::integer(
        &realized
            .tables
            .iter()
            .find(|t| t.table == Table::Entity)
            .unwrap()
            .after[0]["id"],
    )
    .unwrap();
    tx.execute(
        "INSERT INTO entity_alias VALUES(?,'B','b','foreign',1)",
        [id],
    )
    .unwrap();
    assert!(
        delta::restore(&tx, &realized)
            .unwrap_err()
            .error
            .contains("entity_alias")
    );
}

#[test]
fn unexpected_trigger_link_is_not_laundered_into_guard_after_image() {
    let mut conn = fixture();
    conn.execute_batch("CREATE TRIGGER unexpected_link AFTER INSERT ON entity WHEN new.normalized_name='bob' BEGIN INSERT INTO asset_entity(asset_id,entity_id,role,source) VALUES(1,new.id,'appearance','trigger'); END;").unwrap();
    {
        let tx = conn.transaction().unwrap();
        assert!(creator_delta::apply(&tx, &empty(), &[intent()]).is_err());
    }
    assert_eq!(rows(&conn, "entity").len(), 1);
    assert_eq!(rows(&conn, "asset_entity").len(), 2);
}

#[test]
fn database_defaults_are_retained_in_full_realized_creator_snapshot() {
    let mut conn = fixture();
    conn.execute_batch("ALTER TABLE entity ADD COLUMN native_counter INTEGER NOT NULL DEFAULT 9223372036854775807; ALTER TABLE entity ADD COLUMN native_payload BLOB DEFAULT X'00FF';").unwrap();
    let tx = conn.transaction().unwrap();
    let realized = creator_delta::apply(&tx, &empty(), &[intent()]).unwrap();
    let row = &realized
        .tables
        .iter()
        .find(|t| t.table == Table::Entity)
        .unwrap()
        .after[0];
    assert_eq!(
        row["native_counter"],
        json!({"$integer":"9223372036854775807"})
    );
    assert_eq!(row["native_payload"], json!({"$blob":[0,255]}));
    delta::restore(&tx, &realized).unwrap();
    assert_eq!(rows(&tx, "entity").len(), 1);
}
