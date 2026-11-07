#![allow(dead_code)]
#[path = "../src/db.rs"]
mod db;
#[path = "../src/delta.rs"]
mod delta;
#[path = "../src/model.rs"]
mod model;

use delta::{Delta, Guard, Table, TableDelta};
use rusqlite::Connection;
use serde_json::{Value, json};

fn fixture() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(include_str!(
        "../../../tests/fixtures/peach-contract-schema.sql"
    ))
    .unwrap();
    conn.execute_batch("CREATE TABLE videos(vid TEXT PRIMARY KEY,path TEXT UNIQUE,payload BLOB,size INTEGER); CREATE TABLE tasks(vid TEXT PRIMARY KEY,path TEXT UNIQUE,data TEXT); INSERT INTO entity(id,kind,canonical_name,normalized_name,created_at,updated_at) VALUES(1,'creator','Alice','alice','fixture','fixture'),(2,'creator','Bob','bob','fixture','fixture'); INSERT INTO asset(id,location,path) VALUES(1,'fixture','old.mp4'); INSERT INTO asset_entity(asset_id,entity_id,role,source) VALUES(1,1,'creator','manual'),(1,1,'creator','import');").unwrap();
    conn
}
fn rows(conn: &Connection, table: &str) -> Vec<Value> {
    db::rows(conn, &format!("SELECT * FROM {table}"), []).unwrap()
}
fn change(table: Table, keys: &[&str], before: Vec<Value>, after: Vec<Value>) -> TableDelta {
    let key_columns = keys.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let mut scope = vec![];
    for row in before.iter().chain(&after) {
        let selector = Value::Object(
            keys.iter()
                .map(|k| (k.to_string(), row[k].clone()))
                .collect(),
        );
        if !scope.contains(&selector) {
            scope.push(selector);
        }
    }
    TableDelta {
        table,
        key_columns,
        scope,
        before,
        after,
    }
}
fn plan(tables: Vec<TableDelta>) -> Delta {
    Delta {
        tables,
        guards: vec![],
    }
}

#[test]
fn stable_primary_key_update_preserves_all_relationships_and_integer_blob() {
    let mut conn = fixture();
    conn.execute(
        "INSERT INTO videos VALUES('v','old',X'00FF7F',9223372036854775807)",
        [],
    )
    .unwrap();
    let assets = rows(&conn, "asset");
    let links = rows(&conn, "asset_entity");
    let mut next = assets.clone();
    next[0]["path"] = json!("new.mp4");
    let videos = rows(&conn, "videos");
    let mut next_videos = videos.clone();
    next_videos[0]["path"] = json!("new");
    // Changed unique selector still identifies the same immutable primary row.
    let delta = plan(vec![
        change(Table::Asset, &["location", "path"], assets.clone(), next),
        change(Table::Videos, &["vid"], videos.clone(), next_videos),
    ]);
    let tx = conn.transaction().unwrap();
    delta::apply(&tx, &delta).unwrap();
    assert_eq!(rows(&tx, "asset_entity"), links);
    assert_eq!(
        rows(&tx, "videos")[0]["size"],
        json!({"$integer":"9223372036854775807"})
    );
    assert_eq!(
        rows(&tx, "videos")[0]["payload"],
        json!({"$blob":[0,255,127]})
    );
    delta::restore(&tx, &delta).unwrap();
    assert_eq!(rows(&tx, "videos"), videos);
    assert_eq!(rows(&tx, "asset"), assets);
    tx.commit().unwrap();
}

#[test]
fn composite_relation_reassignment_and_new_parent_restore() {
    let mut conn = fixture();
    let entities = rows(&conn, "entity");
    let links = rows(&conn, "asset_entity");
    let mut new_entity = entities[0].clone();
    new_entity["id"] = json!({"$integer":"3"});
    new_entity["canonical_name"] = json!("Carol");
    new_entity["normalized_name"] = json!("carol");
    let mut after_links = links.clone();
    for row in &mut after_links {
        row["entity_id"] = json!({"$integer":"3"});
    }
    let delta = plan(vec![
        change(
            Table::AssetEntity,
            &["asset_id", "entity_id", "role", "source"],
            links.clone(),
            after_links,
        ),
        change(Table::Entity, &["id"], vec![], vec![new_entity]),
    ]);
    let tx = conn.transaction().unwrap();
    delta::apply(&tx, &delta).unwrap();
    delta::inspect(&tx, &delta, true).unwrap();
    assert!(delta::apply(&tx, &delta).is_err());
    delta::restore(&tx, &delta).unwrap();
    assert_eq!(rows(&tx, "entity"), entities);
    assert_eq!(rows(&tx, "asset_entity"), links);
    assert!(delta::restore(&tx, &delta).is_err());
    tx.commit().unwrap();
}

#[test]
fn scoped_child_delete_before_parent_and_inverse_insert_order() {
    let mut conn = fixture();
    let entity = vec![rows(&conn, "entity")[0].clone()];
    let links = rows(&conn, "asset_entity");
    let delta = plan(vec![
        change(Table::Entity, &["id"], entity, vec![]),
        change(
            Table::AssetEntity,
            &["asset_id", "entity_id", "role", "source"],
            links.clone(),
            vec![],
        ),
    ]);
    let tx = conn.transaction().unwrap();
    delta::apply(&tx, &delta).unwrap();
    delta::restore(&tx, &delta).unwrap();
    assert_eq!(rows(&tx, "asset_entity"), links);
    tx.commit().unwrap();
}

#[test]
fn unknown_cascading_references_block_deletion_and_are_preserved() {
    let mut conn = fixture();
    conn.execute("INSERT INTO entity_alias VALUES(1,'A','a','manual',1)", [])
        .unwrap();
    let before = rows(&conn, "entity");
    let links = rows(&conn, "asset_entity");
    let delta = plan(vec![
        change(Table::Entity, &["id"], vec![before[0].clone()], vec![]),
        change(
            Table::AssetEntity,
            &["asset_id", "entity_id", "role", "source"],
            links.clone(),
            vec![],
        ),
    ]);
    {
        let tx = conn.transaction().unwrap();
        assert!(
            delta::apply(&tx, &delta)
                .unwrap_err()
                .error
                .contains("entity_alias")
        );
    }
    assert_eq!(rows(&conn, "entity"), before);
    assert_eq!(rows(&conn, "asset_entity"), links);
    assert_eq!(rows(&conn, "entity_alias").len(), 1);
}

#[test]
fn malformed_scopes_selectors_and_incomplete_images_rejected() {
    let conn = fixture();
    let before = rows(&conn, "asset");
    let base = plan(vec![change(Table::Asset, &["id"], before.clone(), before)]);
    let mut d = base.clone();
    d.tables[0].key_columns = vec!["path".into()];
    assert!(delta::inspect(&conn, &d, false).is_err()); // only (location,path) is unique
    let mut d = base.clone();
    d.tables[0].scope.clear();
    assert!(delta::inspect(&conn, &d, false).is_err());
    let mut d = base.clone();
    d.tables[0].scope.push(json!({"id":{"$integer":"2"}}));
    assert!(delta::inspect(&conn, &d, false).is_err());
    let mut d = base.clone();
    d.tables[0].before[0]
        .as_object_mut()
        .unwrap()
        .remove("name");
    assert!(delta::inspect(&conn, &d, false).is_err());
    let mut d = base.clone();
    let duplicate = d.tables[0].before[0].clone();
    d.tables[0].before.push(duplicate);
    assert!(delta::inspect(&conn, &d, false).is_err());
    let mut d = base.clone();
    d.tables[0].before[0]["id"] = json!({"$integer":"01"});
    assert!(delta::inspect(&conn, &d, false).is_err());
    let mut d = base.clone();
    d.tables[0].before[0]["id"] = json!({"$integer":"1","other":0});
    assert!(delta::inspect(&conn, &d, false).is_err());
    assert!(serde_json::from_str::<Table>("\"entity_alias\"").is_err());
}

#[test]
fn missing_and_foreign_rows_and_external_after_change_block_mutations() {
    let mut conn = fixture();
    let assets = rows(&conn, "asset");
    let mut after = assets.clone();
    after[0]["path"] = json!("new");
    let delta = plan(vec![change(Table::Asset, &["id"], assets.clone(), after)]);
    {
        let tx = conn.transaction().unwrap();
        delta::apply(&tx, &delta).unwrap();
        tx.execute("UPDATE asset SET name='external' WHERE id=1", [])
            .unwrap();
        assert!(delta::restore(&tx, &delta).is_err());
        assert_eq!(rows(&tx, "asset")[0]["name"], json!("external"));
    }
    let missing = plan(vec![change(Table::Asset, &["id"], vec![], assets.clone())]);
    assert!(delta::inspect(&conn, &missing, false).is_err());
    conn.execute("DELETE FROM asset WHERE id=1", []).unwrap();
    assert!(delta::inspect(&conn, &delta, false).is_err());
}

#[test]
fn transaction_rollback_on_late_constraint_failure() {
    let mut conn = fixture();
    let old = rows(&conn, "asset");
    let mut asset = old[0].clone();
    asset["id"] = json!({"$integer":"2"});
    asset["path"] = json!("inserted");
    let links = rows(&conn, "asset_entity");
    let mut bad = links[0].clone();
    bad["entity_id"] = json!({"$integer":"999"});
    let delta = plan(vec![
        change(Table::Asset, &["id"], vec![], vec![asset]),
        change(
            Table::AssetEntity,
            &["asset_id", "entity_id", "role", "source"],
            vec![],
            vec![bad],
        ),
    ]);
    {
        let tx = conn.transaction().unwrap();
        assert!(delta::apply(&tx, &delta).is_err());
    }
    assert_eq!(rows(&conn, "asset"), old);
    assert_eq!(rows(&conn, "asset_entity"), links);
    assert!(delta::apply(&conn, &delta).is_err());
}

#[test]
fn phantom_guards_detect_foreign_assets_creators_and_links() {
    let conn = fixture();
    let assets = rows(&conn, "asset");
    let creators = vec![rows(&conn, "entity")[0].clone()];
    let links = rows(&conn, "asset_entity");
    let delta = Delta {
        tables: vec![],
        guards: vec![
            Guard::AssetPath {
                path: "old.mp4".into(),
                before: assets.clone(),
                after: assets,
            },
            Guard::CreatorName {
                canonical: "Alice".into(),
                normalized: "alice".into(),
                before: creators.clone(),
                after: creators.clone(),
            },
            Guard::CreatorCanonical {
                canonical: "Alice".into(),
                before: creators.clone(),
                after: creators,
            },
            Guard::AssetLinks {
                asset_id: json!({"$integer":"1"}),
                before: links.clone(),
                after: links,
            },
        ],
    };
    delta::inspect(&conn, &delta, false).unwrap();
    conn.execute(
        "INSERT INTO asset(id,location,path,disposal) VALUES(2,'elsewhere','old.mp4','gone')",
        [],
    )
    .unwrap();
    assert!(delta::inspect(&conn, &delta, false).is_err());
    conn.execute("DELETE FROM asset WHERE id=2", []).unwrap();
    conn.execute("INSERT INTO entity(id,kind,canonical_name,normalized_name,created_at,updated_at) VALUES(3,'creator','Alice','alternate','fixture','fixture')",[]).unwrap();
    assert!(delta::inspect(&conn, &delta, false).is_err());
    conn.execute("DELETE FROM entity WHERE id=3", []).unwrap();
    conn.execute("INSERT INTO asset_entity(asset_id,entity_id,role,source) VALUES(1,2,'appearance','external')",[]).unwrap();
    assert!(delta::inspect(&conn, &delta, false).is_err());
}

#[test]
fn guards_validate_after_and_restore_directions() {
    let mut conn = fixture();
    let before = rows(&conn, "asset");
    let mut after = before.clone();
    after[0]["path"] = json!("new.mp4");
    let delta = Delta {
        tables: vec![change(Table::Asset, &["id"], before.clone(), after.clone())],
        guards: vec![
            Guard::AssetPath {
                path: "old.mp4".into(),
                before: before.clone(),
                after: vec![],
            },
            Guard::AssetPath {
                path: "new.mp4".into(),
                before: vec![],
                after,
            },
        ],
    };
    let tx = conn.transaction().unwrap();
    delta::apply(&tx, &delta).unwrap();
    delta::restore(&tx, &delta).unwrap();
    assert_eq!(rows(&tx, "asset"), before);
}

#[test]
fn partial_unique_and_nullable_keys_are_not_trusted() {
    let conn = fixture();
    conn.execute_batch("CREATE UNIQUE INDEX partial_name ON asset(name) WHERE name IS NOT NULL;")
        .unwrap();
    let before = rows(&conn, "asset");
    let mut row = before[0].clone();
    row["name"] = json!("name");
    assert!(
        delta::inspect(
            &conn,
            &plan(vec![change(
                Table::Asset,
                &["name"],
                vec![row.clone()],
                vec![row]
            )]),
            false
        )
        .is_err()
    );
    let mut d = plan(vec![change(Table::Asset, &["id"], before.clone(), before)]);
    d.tables[0].scope[0]["id"] = Value::Null;
    assert!(delta::inspect(&conn, &d, false).is_err());
}

#[test]
fn parent_collation_is_used_when_protecting_foreign_children() {
    let mut conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE videos(vid TEXT PRIMARY KEY COLLATE NOCASE,path TEXT); CREATE TABLE unknown_child(vid TEXT REFERENCES videos(vid) ON DELETE CASCADE); INSERT INTO videos VALUES('CaseID','fixture'); INSERT INTO unknown_child VALUES('caseid');").unwrap();
    let delta = plan(vec![change(
        Table::Videos,
        &["vid"],
        rows(&conn, "videos"),
        vec![],
    )]);
    {
        let tx = conn.transaction().unwrap();
        assert!(
            delta::apply(&tx, &delta)
                .unwrap_err()
                .error
                .contains("unknown_child")
        );
    }
    assert_eq!(rows(&conn, "unknown_child").len(), 1);
}

#[test]
fn parent_unique_key_update_cannot_cascade_outside_scope() {
    let mut conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE videos(vid TEXT PRIMARY KEY,path TEXT UNIQUE); CREATE TABLE unknown_child(path TEXT REFERENCES videos(path) ON UPDATE CASCADE); INSERT INTO videos VALUES('id','old'); INSERT INTO unknown_child VALUES('old');").unwrap();
    let before = rows(&conn, "videos");
    let mut after = before.clone();
    after[0]["path"] = json!("new");
    let delta = plan(vec![change(Table::Videos, &["vid"], before.clone(), after)]);
    {
        let tx = conn.transaction().unwrap();
        assert!(
            delta::apply(&tx, &delta)
                .unwrap_err()
                .error
                .contains("unknown_child")
        );
    }
    assert_eq!(rows(&conn, "videos"), before);
    assert_eq!(rows(&conn, "unknown_child")[0]["path"], json!("old"));
}

#[test]
fn post_guard_failure_rolls_back_and_disabled_foreign_keys_reject() {
    let mut conn = fixture();
    let before = rows(&conn, "asset");
    let mut after = before.clone();
    after[0]["path"] = json!("new");
    let mut delta = plan(vec![change(Table::Asset, &["id"], before.clone(), after)]);
    delta.guards.push(Guard::AssetPath {
        path: "old.mp4".into(),
        before: before.clone(),
        after: before.clone(),
    });
    {
        let tx = conn.transaction().unwrap();
        assert!(delta::apply(&tx, &delta).is_err());
    }
    assert_eq!(rows(&conn, "asset"), before);
    conn.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
    let tx = conn.transaction().unwrap();
    assert!(
        delta::apply(&tx, &delta)
            .unwrap_err()
            .error
            .contains("外键约束")
    );
}

#[test]
fn processing_discovery_setting_restores_without_touching_other_settings() {
    let mut conn = fixture();
    conn.execute_batch("CREATE TABLE settings(key TEXT PRIMARY KEY,value TEXT NOT NULL); INSERT INTO settings VALUES('unrelated','keep');").unwrap();
    let delta = plan(vec![change(
        Table::Settings,
        &["key"],
        vec![],
        vec![json!({"key":"discovery_requested","value":"fixture-timestamp"})],
    )]);
    let tx = conn.transaction().unwrap();
    delta::apply(&tx, &delta).unwrap();
    assert_eq!(rows(&tx, "settings").len(), 2);
    delta::restore(&tx, &delta).unwrap();
    assert_eq!(
        rows(&tx, "settings"),
        vec![json!({"key":"unrelated","value":"keep"})]
    );
    tx.commit().unwrap();
}

#[test]
fn local_file_guards_detect_late_vid_and_path_inserts_but_allow_unrelated_rows() {
    for table in [Table::Videos, Table::Tasks] {
        let conn = fixture();
        let table_name = if table == Table::Videos {
            "videos"
        } else {
            "tasks"
        };
        let delta = Delta {
            tables: vec![],
            guards: vec![Guard::LocalFiles {
                table,
                vids: vec!["selected".into()],
                paths: vec!["source.mp4".into(), "destination.mp4".into()],
                before: vec![],
                after: vec![],
            }],
        };
        conn.execute(
            &format!("INSERT INTO {table_name}(vid,path) VALUES('unrelated','elsewhere.mp4')"),
            [],
        )
        .unwrap();
        delta::inspect(&conn, &delta, false).unwrap();
        for (vid, path) in [
            ("selected", "other.mp4"),
            ("new-source", "source.mp4"),
            ("new-destination", "destination.mp4"),
        ] {
            conn.execute(
                &format!("INSERT INTO {table_name}(vid,path) VALUES(?,?)"),
                [vid, path],
            )
            .unwrap();
            assert!(delta::inspect(&conn, &delta, false).is_err());
            conn.execute(&format!("DELETE FROM {table_name} WHERE vid=?"), [vid])
                .unwrap();
            delta::inspect(&conn, &delta, false).unwrap();
        }
    }
}

#[test]
fn local_file_guard_tracks_after_and_restore_for_registration_and_move() {
    let mut conn = fixture();
    let inserted = json!({"vid":"selected","path":"destination.mp4","payload":null,"size":null});
    let mut delta = plan(vec![change(
        Table::Videos,
        &["vid"],
        vec![],
        vec![inserted.clone()],
    )]);
    delta.guards.push(Guard::LocalFiles {
        table: Table::Videos,
        vids: vec!["selected".into()],
        paths: vec!["source.mp4".into(), "destination.mp4".into()],
        before: vec![],
        after: vec![inserted],
    });
    let tx = conn.transaction().unwrap();
    delta::apply(&tx, &delta).unwrap();
    delta::inspect(&tx, &delta, true).unwrap();
    delta::restore(&tx, &delta).unwrap();
    delta::inspect(&tx, &delta, false).unwrap();
    tx.commit().unwrap();

    conn.execute(
        "INSERT INTO tasks VALUES('selected','source.mp4','original')",
        [],
    )
    .unwrap();
    let before = rows(&conn, "tasks");
    let mut after = before.clone();
    after[0]["path"] = json!("destination.mp4");
    let mut delta = plan(vec![change(
        Table::Tasks,
        &["vid"],
        before.clone(),
        after.clone(),
    )]);
    delta.guards.push(Guard::LocalFiles {
        table: Table::Tasks,
        vids: vec!["selected".into()],
        paths: vec!["source.mp4".into(), "destination.mp4".into()],
        before,
        after,
    });
    let tx = conn.transaction().unwrap();
    delta::apply(&tx, &delta).unwrap();
    tx.execute(
        "INSERT INTO tasks VALUES('foreign-source','source.mp4','new')",
        [],
    )
    .unwrap();
    assert!(delta::restore(&tx, &delta).is_err());
    tx.execute("DELETE FROM tasks WHERE vid='foreign-source'", [])
        .unwrap();
    delta::restore(&tx, &delta).unwrap();
    tx.execute(
        "INSERT INTO tasks VALUES('foreign','destination.mp4','new')",
        [],
    )
    .unwrap();
    assert!(delta::apply(&tx, &delta).is_err());
}

#[test]
fn local_file_guard_rejects_other_tables_and_incomplete_snapshots() {
    let conn = fixture();
    for table in [
        Table::Asset,
        Table::Entity,
        Table::AssetEntity,
        Table::Settings,
    ] {
        let delta = Delta {
            tables: vec![],
            guards: vec![Guard::LocalFiles {
                table,
                vids: vec![],
                paths: vec![],
                before: vec![],
                after: vec![],
            }],
        };
        assert!(
            delta::inspect(&conn, &delta, false)
                .unwrap_err()
                .error
                .contains("只允许")
        );
    }
    let delta = Delta {
        tables: vec![],
        guards: vec![Guard::LocalFiles {
            table: Table::Videos,
            vids: vec!["selected".into()],
            paths: vec![],
            before: vec![],
            after: vec![json!({"vid":"selected"})],
        }],
    };
    assert!(delta::inspect(&conn, &delta, false).is_err());
}

#[test]
fn local_file_guard_unions_duplicates_and_large_selector_chunks() {
    let conn = fixture();
    conn.execute(
        "INSERT INTO videos(vid,path) VALUES('selected','source.mp4')",
        [],
    )
    .unwrap();
    let mut vids = (0..1500).map(|n| format!("absent-{n}")).collect::<Vec<_>>();
    vids.extend(["selected".into(), "selected".into()]);
    let snapshot = rows(&conn, "videos");
    let delta = Delta {
        tables: vec![],
        guards: vec![Guard::LocalFiles {
            table: Table::Videos,
            vids,
            paths: vec!["source.mp4".into(), "source.mp4".into()],
            before: snapshot.clone(),
            after: snapshot,
        }],
    };
    delta::inspect(&conn, &delta, false).unwrap();
}

#[test]
fn identity_scope_ignores_raw_evidence_and_catches_new_related_tasks() {
    let mut conn = fixture();
    conn.execute(
        "INSERT INTO tasks VALUES('related','outside.mp4',?)",
        [r#"{"candidates":[{"name":"Old"}],"ocr":"Old"}"#],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO tasks VALUES('unrelated','other.mp4',?)",
        [r#"{"ocr":"Old","vid":"Old","note":"Old"}"#],
    )
    .unwrap();
    let before = delta::local_identity(
        &conn,
        Table::Tasks,
        &[],
        &[],
        &["/source/".into()],
        &["Old".into()],
    )
    .unwrap();
    assert_eq!(before.len(), 1);
    let mut after = before.clone();
    after[0]["data"] = json!(r#"{"candidates":[{"name":"New"}],"ocr":"Old"}"#);
    let d = Delta {
        tables: vec![change(
            Table::Tasks,
            &["vid"],
            before.clone(),
            after.clone(),
        )],
        guards: vec![Guard::LocalIdentity {
            table: Table::Tasks,
            vids: vec!["related".into()],
            paths: vec![],
            prefixes: vec!["/source/".into()],
            names: vec!["Old".into()],
            before,
            after,
        }],
    };
    conn.execute("UPDATE tasks SET data='{}' WHERE vid='unrelated'", [])
        .unwrap();
    let tx = conn.transaction().unwrap();
    delta::apply(&tx, &d).unwrap();
    tx.commit().unwrap();
    delta::inspect(&conn, &d, true).unwrap();
    conn.execute(
        "UPDATE tasks SET data=? WHERE vid='unrelated'",
        [r#"{"suggested":"Old"}"#],
    )
    .unwrap();
    assert!(delta::inspect(&conn, &d, true).is_err());
    let tx = conn.transaction().unwrap();
    assert!(delta::restore(&tx, &d).is_err());
}

#[test]
fn asset_prefix_guard_is_literal_including_percent_and_underscore() {
    let conn = fixture();
    conn.execute("UPDATE asset SET path='/Old%_/one' WHERE id=1", [])
        .unwrap();
    let before = db::rows(&conn, "SELECT * FROM asset WHERE id=1", []).unwrap();
    let d = Delta {
        tables: vec![],
        guards: vec![Guard::AssetPrefix {
            prefix: "/Old%_/".into(),
            before: before.clone(),
            after: before,
        }],
    };
    conn.execute(
        "INSERT INTO asset(location,path) VALUES('fixture','/OldAB/other')",
        [],
    )
    .unwrap();
    delta::inspect(&conn, &d, false).unwrap();
    conn.execute(
        "INSERT INTO asset(location,path) VALUES('fixture','/Old%_/late')",
        [],
    )
    .unwrap();
    assert!(delta::inspect(&conn, &d, false).is_err());
}
