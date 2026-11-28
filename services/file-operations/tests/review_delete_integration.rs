use face_file_operations::{
    Config, Engine, archive_api, files, metadata,
    model::Request,
    processing_scheduler, review_sessions, staging,
};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};

struct TestFixture {
    root: PathBuf,
    config: Config,
}

impl TestFixture {
    fn new(label: &str) -> Self {
        let root = fs::canonicalize("/tmp")
            .unwrap()
            .join(format!("face-review-delete-{label}-{}", uuid::Uuid::new_v4()));
        let base = root.join("base");
        let media = root.join("media");
        let ledger = root.join("ledger.db");
        fs::create_dir_all(base.join("processing")).unwrap();
        fs::create_dir_all(media.join("pikpak/My Telegram")).unwrap();
        fs::create_dir_all(media.join("pikpak/creators")).unwrap();
        fs::write(root.join(".fileops-fixture"), "review-delete integration fixture").unwrap();

        let faces = Connection::open(base.join("faces.db")).unwrap();
        faces
            .execute_batch(
                "CREATE TABLE videos (vid TEXT PRIMARY KEY, path TEXT NOT NULL, name TEXT, collection TEXT);
                 CREATE TABLE faces (vid TEXT NOT NULL, emb BLOB, thumb TEXT);",
            )
            .unwrap();
        drop(faces);

        let peach = Connection::open(&ledger).unwrap();
        peach
            .execute_batch(include_str!(
                "../../../tests/fixtures/peach-contract-schema.sql"
            ))
            .unwrap();
        drop(peach);

        let config = Config {
            base,
            media,
            ledger,
            bind: "127.0.0.1:0".into(),
            token: None,
            fixture: Some(root.clone()),
            parity_root: None,
        };
        let proc_conn = processing_scheduler::open_or_init(&config).unwrap();
        drop(proc_conn);

        Self { root, config }
    }

    fn add_review_video(
        &self,
        rel_dir: &str,
        filename: &str,
        vid: &str,
        bytes: &[u8],
        suggested: &str,
        in_faces_db: bool,
        peach_asset_id: Option<i64>,
    ) -> PathBuf {
        let dir = self.config.media.join("pikpak").join(rel_dir);
        fs::create_dir_all(&dir).unwrap();
        let file_path = dir.join(filename);
        fs::write(&file_path, bytes).unwrap();
        let stat = fs::metadata(&file_path).unwrap();
        let path_str = file_path.to_string_lossy().to_string();
        let mtime_ns = files::mtime_ns(&stat);
        let size = stat.len() as i64;

        if in_faces_db {
            let faces = Connection::open(self.config.base.join("faces.db")).unwrap();
            faces
                .execute(
                    "INSERT INTO videos(vid, path, name, collection) VALUES(?, ?, ?, ?)",
                    params![vid, path_str, filename, dir.to_string_lossy().to_string()],
                )
                .unwrap();
        }

        let proc_conn = processing_scheduler::open_or_init(&self.config).unwrap();
        let data = json!({
            "kind": "suggested",
            "suggested": suggested,
            "ready": true,
            "text_complete": true,
            "face_complete": true,
            "face_skipped": false,
            "watermark": {
                "vid": vid,
                "path": path_str,
                "size": stat.len(),
                "mtime_ns": mtime_ns,
                "status": "done",
                "pipeline": "paddleocr-test-v2",
                "candidates": []
            },
            "face": {
                "version": "v1",
                "reference_version": "r1",
                "matches": [],
                "thumbs": []
            },
            "folder": [],
            "text_candidates": [],
            "candidates": [{
                "name": suggested,
                "source": "face",
                "score": 0.88
            }]
        });
        proc_conn
            .execute(
                "INSERT INTO tasks(vid,path,name,size,mtime_ns,stage,retry_stage,batch_id,created_at,updated_at,manual,deferred,data,auto_opt_out)
                 VALUES(?,?,?,?,?,'review','face','batch-1',1000.0,1000.0,0,0,?,0)",
                params![
                    vid,
                    path_str,
                    filename,
                    size,
                    mtime_ns,
                    serde_json::to_string(&data).unwrap()
                ],
            )
            .unwrap();

        if let Some(aid) = peach_asset_id {
            let peach = Connection::open(&self.config.ledger).unwrap();
            let win_path = metadata::win(&self.config, &path_str);
            peach
                .execute(
                    "INSERT INTO asset(id,location,path,name,medium,size) VALUES(?,'local',?,?,'video',?)",
                    params![aid, win_path, filename, size],
                )
                .unwrap();
        }
        file_path
    }
}

impl Drop for TestFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

fn delete_request(id: String, session: &str, group_key: &str, tasks: &[&Value]) -> Request {
    let vids: Vec<String> = tasks
        .iter()
        .map(|t| t["vid"].as_str().unwrap().to_owned())
        .collect();
    let tokens: serde_json::Map<String, Value> = tasks
        .iter()
        .map(|t| (t["vid"].as_str().unwrap().to_owned(), t["review_token"].clone()))
        .collect();
    let fingerprints: serde_json::Map<String, Value> = tasks
        .iter()
        .map(|t| {
            (
                t["vid"].as_str().unwrap().to_owned(),
                json!({
                    "path": t["path"],
                    "size": t["size"],
                    "mtime_ns": t["mtime_ns"],
                }),
            )
        })
        .collect();
    Request {
        id,
        action: "review-delete".into(),
        person: String::new(),
        files: vec![],
        filename: String::new(),
        target_person: None,
        target_folder: None,
        payload: Some(json!({
            "vids": vids,
            "review_session": session,
            "review_tokens": tokens,
            "review_group": { "key": group_key, "deferred": false },
            "fingerprints": fingerprints,
        })),
    }
}

#[test]
fn single_delete_indexed_and_unindexed_syncs_all_stores_and_session() {
    let fx = TestFixture::new("single-indexed-unindexed");
    let src_indexed = fx.add_review_video(
        "My Telegram",
        "beer-1.mp4",
        "vid-indexed",
        b"indexed-video-bytes",
        "菠萝啤beer",
        true,
        Some(101),
    );
    let src_unindexed = fx.add_review_video(
        "My Telegram/sub",
        "beer-2.mp4",
        "vid-unindexed",
        b"unindexed-video-bytes",
        "菠萝啤beer",
        false,
        Some(102),
    );

    let mut engine = Engine::new(fx.config.clone()).unwrap();
    let page = review_sessions::execute(&mut engine, "review-page", json!({"group": "person:菠萝啤beer"})).unwrap();
    let session = page["session"].as_str().unwrap().to_owned();
    let rows = page["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2);

    for (vid, src, expected_bytes, aid) in [
        ("vid-indexed", &src_indexed, b"indexed-video-bytes".as_slice(), 101i64),
        ("vid-unindexed", &src_unindexed, b"unindexed-video-bytes".as_slice(), 102i64),
    ] {
        let task = rows.iter().find(|r| r["vid"] == vid).unwrap();
        let op_id = uuid::Uuid::new_v4().to_string();
        let req = delete_request(op_id.clone(), &session, "person:菠萝啤beer", &[task]);
        let plan_val = engine.preview(req.clone()).unwrap();
        let version = plan_val["version"].as_str().unwrap().to_owned();
        let dst = plan_val["files"][0]["dst"].as_str().unwrap().to_owned();
        assert!(dst.contains("review-deletions"));

        let res = engine.apply(req, version).unwrap();
        assert_eq!(res["ok"], true);
        assert_eq!(res["staged"], true);
        assert!(!src.exists());
        assert_eq!(fs::read(&dst).unwrap(), expected_bytes);

        let origins = read_json(&fx.config.base.join("staging-origins.json"));
        assert_eq!(origins[&dst]["action"], "review-delete");
        assert_eq!(origins[&dst]["original_path"], src.to_string_lossy().to_string());
        assert_eq!(origins[&dst]["person"], "");
        assert!(origins[&dst]["vids"].as_array().unwrap().contains(&json!(vid)));

        let proc_conn = Connection::open(fx.config.base.join("processing/processing.db")).unwrap();
        let (stage, task_path): (String, String) = proc_conn
            .query_row("SELECT stage, path FROM tasks WHERE vid=?", [vid], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(stage, "excluded");
        assert_eq!(task_path, dst);

        let peach = Connection::open(&fx.config.ledger).unwrap();
        let asset_path: String = peach
            .query_row("SELECT path FROM asset WHERE id=?", [aid], |r| r.get(0))
            .unwrap();
        assert_eq!(asset_path, metadata::win(&fx.config, &dst));
    }

    let faces = Connection::open(fx.config.base.join("faces.db")).unwrap();
    let face_path: String = faces
        .query_row("SELECT path FROM videos WHERE vid='vid-indexed'", [], |r| r.get(0))
        .unwrap();
    assert!(face_path.contains("review-deletions"));

    let after_page = review_sessions::execute(
        &mut engine,
        "review-page",
        json!({"session": session, "group": "person:菠萝啤beer"}),
    )
    .unwrap();
    assert_eq!(after_page["rows"].as_array().unwrap().len(), 0);
    assert_eq!(after_page["counts"]["all"], 0);

    let status = review_sessions::execute(
        &mut engine,
        "review-status",
        json!({"session": session}),
    )
    .unwrap();
    assert_eq!(status["has_updates"], false);

    let staged = staging::list(&fx.config, 0, "").unwrap();
    assert_eq!(staged["files"], 2);
    for row in staged["rows"].as_array().unwrap() {
        assert_eq!(row["origin"]["action"], "review-delete");
        assert!(row["origin"]["original_path"].as_str().unwrap().contains("My Telegram"));
    }
}

#[test]
fn batch_delete_preserves_same_filenames_in_distinct_folders() {
    let fx = TestFixture::new("batch-same-name");
    let src_a = fx.add_review_video("My Telegram/dirA", "clip.mp4", "vid-a", b"bytes-a", "菠萝啤beer", true, Some(201));
    let src_b = fx.add_review_video("My Telegram/dirB", "clip.mp4", "vid-b", b"bytes-b", "菠萝啤beer", true, Some(202));

    let mut engine = Engine::new(fx.config.clone()).unwrap();
    let page = review_sessions::execute(&mut engine, "review-page", json!({"group": "person:菠萝啤beer"})).unwrap();
    let session = page["session"].as_str().unwrap().to_owned();
    let rows = page["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2);

    let op_id = uuid::Uuid::new_v4().to_string();
    let req = delete_request(op_id, &session, "person:菠萝啤beer", &[&rows[0], &rows[1]]);
    let plan_val = engine.preview(req.clone()).unwrap();
    let version = plan_val["version"].as_str().unwrap().to_owned();
    let files_arr = plan_val["files"].as_array().unwrap();
    assert_eq!(files_arr.len(), 2);
    let dst_a = files_arr[0]["dst"].as_str().unwrap();
    let dst_b = files_arr[1]["dst"].as_str().unwrap();
    assert_ne!(dst_a, dst_b);

    let res = engine.apply(req, version).unwrap();
    assert_eq!(res["ok"], true);
    assert!(!src_a.exists());
    assert!(!src_b.exists());
    assert!(Path::new(dst_a).is_file());
    assert!(Path::new(dst_b).is_file());

    let after_page = review_sessions::execute(
        &mut engine,
        "review-page",
        json!({"session": session, "group": "person:菠萝啤beer"}),
    )
    .unwrap();
    assert_eq!(after_page["rows"].as_array().unwrap().len(), 0);
}

#[test]
fn restore_to_original_path_returns_video_to_review() {
    let fx = TestFixture::new("restore-review");
    let src = fx.add_review_video("My Telegram", "recover.mp4", "vid-rec", b"recover-bytes", "菠萝啤beer", false, Some(301));

    let mut engine = Engine::new(fx.config.clone()).unwrap();
    let page = review_sessions::execute(&mut engine, "review-page", json!({"group": "person:菠萝啤beer"})).unwrap();
    let session = page["session"].as_str().unwrap().to_owned();
    let task = &page["rows"].as_array().unwrap()[0];

    let del_req = delete_request(uuid::Uuid::new_v4().to_string(), &session, "person:菠萝啤beer", &[task]);
    let del_plan = engine.preview(del_req.clone()).unwrap();
    engine.apply(del_req, del_plan["version"].as_str().unwrap().to_owned()).unwrap();
    assert!(!src.exists());

    let staged = staging::list(&fx.config, 0, "").unwrap();
    let staged_row = &staged["rows"].as_array().unwrap()[0];
    let restore_req: Request = serde_json::from_value(json!({
        "id": uuid::Uuid::new_v4().to_string(),
        "action": "restore-files",
        "person": "",
        "files": [{
            "vid": staged_row["vid"],
            "path": staged_row["path"],
            "version": staged_row["version"],
        }],
    }))
    .unwrap();
    let restore_plan = engine.preview(restore_req.clone()).unwrap();
    let restore_res = engine
        .apply(restore_req, restore_plan["version"].as_str().unwrap().to_owned())
        .unwrap();
    assert_eq!(restore_res["ok"], true);
    assert_eq!(fs::read(&src).unwrap(), b"recover-bytes");

    let proc_conn = Connection::open(fx.config.base.join("processing/processing.db")).unwrap();
    let (stage, task_path): (String, String) = proc_conn
        .query_row("SELECT stage, path FROM tasks WHERE vid='vid-rec'", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(stage, "review");
    assert_eq!(task_path, src.to_string_lossy().to_string());

    let peach = Connection::open(&fx.config.ledger).unwrap();
    let asset_path: String = peach
        .query_row("SELECT path FROM asset WHERE id=301", [], |r| r.get(0))
        .unwrap();
    assert_eq!(asset_path, metadata::win(&fx.config, &src.to_string_lossy()));

    let fresh_page = review_sessions::execute(&mut engine, "review-page", json!({"group": "person:菠萝啤beer"})).unwrap();
    assert_eq!(fresh_page["rows"].as_array().unwrap().len(), 1);
    assert_eq!(fresh_page["rows"][0]["vid"], "vid-rec");
}

#[test]
fn guards_reject_stale_fingerprint_and_cross_action_conflicts() {
    let fx = TestFixture::new("guards");
    let src1 = fx.add_review_video("My Telegram", "one.mp4", "vid-1", b"bytes-1", "菠萝啤beer", true, Some(401));
    let src2 = fx.add_review_video("My Telegram", "two.mp4", "vid-2", b"bytes-2", "菠萝啤beer", true, Some(402));

    let mut engine = Engine::new(fx.config.clone()).unwrap();
    let page = review_sessions::execute(&mut engine, "review-page", json!({"group": "person:菠萝啤beer"})).unwrap();
    let session = page["session"].as_str().unwrap().to_owned();
    let rows = page["rows"].as_array().unwrap();
    let task1 = &rows[0];
    let task2 = &rows[1];

    // 1. Stale fingerprint rejected on preview
    let mut bad_req = delete_request(uuid::Uuid::new_v4().to_string(), &session, "person:菠萝啤beer", &[task1]);
    bad_req.payload.as_mut().unwrap()["fingerprints"]["vid-1"]["size"] = json!(999999);
    let err = engine.preview(bad_req).unwrap_err();
    assert_eq!(err.status, 409);
    assert!(src1.exists());

    // 2. Confirming vid-1 via queue-confirm blocks deleting vid-1
    let confirm_res = archive_api::execute(
        &mut engine,
        "queue-confirm",
        json!({
            "operation_id": uuid::Uuid::new_v4().to_string(),
            "vids": ["vid-1"],
            "creator": "菠萝啤beer",
            "review_tokens": { "vid-1": task1["review_token"] },
            "review_session": session,
            "source": "review",
            "source_id": "person:菠萝啤beer",
            "review_group": { "key": "person:菠萝啤beer", "deferred": false },
            "fingerprints": {
                "vid-1": { "path": task1["path"], "size": task1["size"], "mtime_ns": task1["mtime_ns"] }
            }
        }),
    )
    .unwrap();
    assert_eq!(confirm_res["ok"], true);
    let del_confirmed = delete_request(uuid::Uuid::new_v4().to_string(), &session, "person:菠萝啤beer", &[task1]);
    assert_eq!(engine.preview(del_confirmed).unwrap_err().status, 409);

    // 3. Deleting vid-2 blocks confirming vid-2 in the same session
    let del_2 = delete_request(uuid::Uuid::new_v4().to_string(), &session, "person:菠萝啤beer", &[task2]);
    let del_plan = engine.preview(del_2.clone()).unwrap();
    engine.apply(del_2, del_plan["version"].as_str().unwrap().to_owned()).unwrap();
    assert!(!src2.exists());

    let confirm_deleted = archive_api::execute(
        &mut engine,
        "queue-confirm",
        json!({
            "operation_id": uuid::Uuid::new_v4().to_string(),
            "vids": ["vid-2"],
            "creator": "菠萝啤beer",
            "review_tokens": { "vid-2": task2["review_token"] },
            "review_session": session,
            "source": "review",
            "source_id": "person:菠萝啤beer",
            "review_group": { "key": "person:菠萝啤beer", "deferred": false },
            "fingerprints": {
                "vid-2": { "path": task2["path"], "size": task2["size"], "mtime_ns": task2["mtime_ns"] }
            }
        }),
    )
    .unwrap_err();
    assert_eq!(confirm_deleted.status, 409);
}

#[test]
fn continuous_manual_review_succeeds_across_session_expiry_and_mid_reanalysis_while_preserving_file_and_conflict_guards() {
    let fx = TestFixture::new("continuous-review");
    let src1 = fx.add_review_video("My Telegram", "cont-1.mp4", "vid-c1", b"c1-bytes", "菠萝啤beer", true, Some(501));
    let src2 = fx.add_review_video("My Telegram", "cont-2.mp4", "vid-c2", b"c2-bytes", "菠萝啤beer", true, Some(502));
    let src3 = fx.add_review_video("My Telegram", "cont-3.mp4", "vid-c3", b"c3-bytes", "菠萝啤beer", true, Some(503));

    let mut engine = Engine::new(fx.config.clone()).unwrap();
    let page = review_sessions::execute(&mut engine, "review-page", json!({"group": "person:菠萝啤beer"})).unwrap();
    let expired_session = page["session"].as_str().unwrap().to_owned();
    let rows = page["rows"].as_array().unwrap();
    let t1 = rows.iter().find(|r| r["vid"] == "vid-c1").unwrap().clone();
    let t2 = rows.iter().find(|r| r["vid"] == "vid-c2").unwrap().clone();
    let t3 = rows.iter().find(|r| r["vid"] == "vid-c3").unwrap().clone();

    // 1. Simulate expired/evicted session, changed watermark mapping, AND mid-reanalysis task state (queued/not ready).
    let sc = Connection::open(fx.config.base.join("file-operations-rust/review-sessions.sqlite3")).unwrap();
    sc.execute("UPDATE session_snapshots SET expires = 0 WHERE id = ?", [&expired_session]).unwrap();
    drop(sc);
    fs::create_dir_all(fx.config.base.join("watermarks")).unwrap();
    fs::write(fx.config.base.join("watermarks/mappings.json"), json!({"X:other": {"kind": "source"}}).to_string()).unwrap();

    let proc_conn = processing_scheduler::open_or_init(&fx.config).unwrap();
    let reanalyzing_data = json!({
        "kind": "none",
        "ready": false,
        "text_complete": false,
        "face_complete": false,
        "candidates": []
    });
    proc_conn
        .execute(
            "UPDATE tasks SET stage='queued', retry_stage='text', deferred=1, data=? WHERE vid IN ('vid-c1','vid-c2')",
            [serde_json::to_string(&reanalyzing_data).unwrap()],
        )
        .unwrap();
    drop(proc_conn);

    // 2. queue-confirm succeeds with expired session, missing/invalid tokens, changed review_group, and queued/non-ready task.
    let confirm_res = archive_api::execute(
        &mut engine,
        "queue-confirm",
        json!({
            "operation_id": uuid::Uuid::new_v4().to_string(),
            "vids": ["vid-c1"],
            "creator": "菠萝啤beer",
            "source": "review",
            "source_id": "person:菠萝啤beer",
            "review_session": expired_session,
            "review_tokens": { "vid-c1": "stale-or-wrong-token" },
            "review_group": { "key": "person:菠萝啤beer", "deferred": false },
            "fingerprints": {
                "vid-c1": { "path": t1["path"], "size": t1["size"], "mtime_ns": t1["mtime_ns"] }
            }
        }),
    )
    .unwrap();
    assert_eq!(confirm_res["ok"], true);
    assert_eq!(confirm_res["added"], 1);

    // 3. queue-confirm STILL rejects when assigned to a different creator in the queue.
    let diff_creator = archive_api::execute(
        &mut engine,
        "queue-confirm",
        json!({
            "operation_id": uuid::Uuid::new_v4().to_string(),
            "vids": ["vid-c1"],
            "creator": "AnotherCreator",
            "source": "review",
            "fingerprints": {
                "vid-c1": { "path": t1["path"], "size": t1["size"], "mtime_ns": t1["mtime_ns"] }
            }
        }),
    )
    .unwrap_err();
    assert_eq!(diff_creator.status, 409);

    // 4. queue-undo succeeds even though the session expired.
    let undo_res = archive_api::execute(
        &mut engine,
        "queue-undo",
        json!({
            "vids": ["vid-c1"],
            "review_session": expired_session
        }),
    )
    .unwrap();
    assert_eq!(undo_res["ok"], true);

    // 5. review-note, review-defer, review-restore succeed without an active session.
    for (cmd, extra) in [
        ("review-note", json!({"note": "manual note without session"})),
        ("review-defer", json!({})),
        ("review-restore", json!({})),
    ] {
        let mut payload = json!({
            "vids": ["vid-c2"],
            "review_session": expired_session
        });
        for (k, v) in extra.as_object().unwrap() {
            payload[k] = v.clone();
        }
        let res = archive_api::execute(&mut engine, cmd, payload).unwrap();
        assert_eq!(res["ok"], true);
    }

    // 6. review-delete succeeds without session or review_tokens when fingerprint matches, and rejects mismatched fingerprint.
    let mut bad_del = delete_request(uuid::Uuid::new_v4().to_string(), "", "person:菠萝啤beer", &[&t2]);
    bad_del.payload.as_mut().unwrap().as_object_mut().unwrap().remove("review_session");
    bad_del.payload.as_mut().unwrap().as_object_mut().unwrap().remove("review_tokens");
    bad_del.payload.as_mut().unwrap()["fingerprints"]["vid-c2"]["size"] = json!(999);
    assert_eq!(engine.preview(bad_del).unwrap_err().status, 409);

    let mut ok_del = delete_request(uuid::Uuid::new_v4().to_string(), "", "person:菠萝啤beer", &[&t2]);
    ok_del.payload.as_mut().unwrap().as_object_mut().unwrap().remove("review_session");
    ok_del.payload.as_mut().unwrap().as_object_mut().unwrap().remove("review_tokens");
    let del_plan = engine.preview(ok_del.clone()).unwrap();
    let del_res = engine.apply(ok_del, del_plan["version"].as_str().unwrap().to_owned()).unwrap();
    assert_eq!(del_res["ok"], true);
    assert!(!src2.exists());

    // 7. queue-confirm STILL rejects when the file on disk was modified or deleted.
    fs::write(&src3, b"c3-modified-bytes-longer").unwrap();
    let modified_err = archive_api::execute(
        &mut engine,
        "queue-confirm",
        json!({
            "operation_id": uuid::Uuid::new_v4().to_string(),
            "vids": ["vid-c3"],
            "creator": "菠萝啤beer",
            "source": "review",
            "fingerprints": {
                "vid-c3": { "path": t3["path"], "size": t3["size"], "mtime_ns": t3["mtime_ns"] }
            }
        }),
    )
    .unwrap_err();
    assert_eq!(modified_err.status, 409);
    assert!(src1.exists());
}
