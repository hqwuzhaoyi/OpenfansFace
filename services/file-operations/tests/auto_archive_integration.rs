use face_file_operations::{
    Command, Config, Engine, Work, admission, archive_api, auto_archive, auto_watermark, db, files,
    metadata, model::Request, processing_scheduler, review_sessions,
};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

struct TestFixture {
    root: PathBuf,
    config: Config,
}

impl TestFixture {
    fn new(label: &str) -> Self {
        let root = fs::canonicalize("/tmp")
            .unwrap()
            .join(format!("face-admin-auto-archive-{label}-{}", uuid::Uuid::new_v4()));
        let base = root.join("base");
        let media = root.join("media");
        let ledger = root.join("ledger.db");
        fs::create_dir_all(base.join("processing")).unwrap();
        fs::create_dir_all(media.join("pikpak/inbox")).unwrap();
        fs::create_dir_all(media.join("pikpak/creators/Alice")).unwrap();
        fs::write(root.join(".fileops-fixture"), "auto-archive integration fixture").unwrap();

        // Initialize faces.db
        let faces = Connection::open(base.join("faces.db")).unwrap();
        faces
            .execute_batch(
                "CREATE TABLE videos (vid TEXT PRIMARY KEY, path TEXT NOT NULL, name TEXT, collection TEXT);
                 CREATE TABLE faces (vid TEXT NOT NULL, emb BLOB, thumb TEXT);",
            )
            .unwrap();
        drop(faces);

        // Initialize Peach ledger.db using exact contract schema
        let peach = Connection::open(&ledger).unwrap();
        peach
            .execute_batch(include_str!(
                "../../../tests/fixtures/peach-contract-schema.sql"
            ))
            .unwrap();
        peach
            .execute(
                "INSERT INTO entity(id,kind,canonical_name,normalized_name,metadata_json,created_at,updated_at)
                 VALUES(1,'creator','Alice','alice','{}','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z')",
                [],
            )
            .unwrap();
        peach
            .execute(
                "INSERT INTO entity(id,kind,canonical_name,normalized_name,metadata_json,created_at,updated_at)
                 VALUES(2,'tag','FavTag','favtag','{}','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z')",
                [],
            )
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

    fn add_ready_watermark_video(
        &self,
        rel_dir: &str,
        filename: &str,
        vid: &str,
        bytes: &[u8],
        handle_token: &str,
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

        let faces = Connection::open(self.config.base.join("faces.db")).unwrap();
        faces
            .execute(
                "INSERT INTO videos(vid, path, name, collection) VALUES(?, ?, ?, ?)",
                params![vid, path_str, filename, dir.to_string_lossy().to_string()],
            )
            .unwrap();

        let proc_conn = processing_scheduler::open_or_init(&self.config).unwrap();
        let wm_key = format!("{:064x}", peach_asset_id.unwrap_or(100) + 1);
        let wm_text = format!("OnlyFans.com/{handle_token}");
        let data = json!({
            "kind": "suggested",
            "suggested": handle_token,
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
                "candidates": [{
                    "key": wm_key,
                    "kind": "account",
                    "platform": "OnlyFans",
                    "text": wm_text,
                    "token": handle_token,
                    "frames": 6,
                    "score": 0.99
                }]
            },
            "face": {
                "version": "v1",
                "reference_version": "r1",
                "matches": [],
                "thumbs": []
            },
            "folder": [],
            "text_candidates": [{
                "name": handle_token,
                "source": "watermark",
                "confirmed": false,
                "key": wm_key,
                "detail": wm_text
            }],
            "candidates": [{
                "name": handle_token,
                "source": "watermark",
                "confirmed": false,
                "key": wm_key,
                "detail": wm_text
            }]
        });
        proc_conn
            .execute(
                "INSERT INTO tasks(vid,path,name,size,mtime_ns,stage,retry_stage,batch_id,created_at,updated_at,data,auto_opt_out)
                 VALUES(?,?,?,?,?,'review','face','batch-1',1000.0,1000.0,?,0)",
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
    let raw = fs::read_to_string(path).unwrap();
    serde_json::from_str(&raw).unwrap()
}

#[test]
fn direct_auto_archive_keep_both_and_peach_consistency() {
    let fx = TestFixture::new("direct-keep-both");
    let existing_alice_clip = fx.config.creators().join("Alice/clip.mp4");
    fs::write(&existing_alice_clip, b"existing-alice-clip").unwrap();

    let src1 = fx.add_ready_watermark_video("inbox", "clip.mp4", "vid-1", b"video-bytes-1", "Alice", Some(10));
    let src2 = fx.add_ready_watermark_video("inbox2", "clip.mp4", "vid-2", b"video-bytes-2", "Alice", Some(11));

    let mut engine = Engine::new(fx.config.clone()).unwrap();
    let preview = archive_api::execute(&mut engine, "auto-watermark-preview", json!({})).unwrap();
    assert_eq!(preview["ok"], true);
    assert_eq!(preview["eligible_count"], 2);
    assert_eq!(preview["group_count"], 1);
    assert_eq!(preview["groups"][0]["person"], "Alice");
    assert_eq!(preview["groups"][0]["existing_files"], 1);

    let items: Vec<Value> = preview["groups"][0]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|it| {
            assert_eq!(it["renamed_keep_both"], true);
            json!({
                "vid": it["vid"],
                "person": "Alice",
                "path": it["path"],
                "size": it["size"],
                "mtime_ns": it["mtime_ns"],
            })
        })
        .collect();

    let op_id = uuid::Uuid::new_v4().to_string();
    let res = archive_api::execute(
        &mut engine,
        "auto-watermark-apply",
        json!({
            "operation_id": op_id,
            "version": preview["version"],
            "items": items,
            "async": true
        }),
    )
    .unwrap();
    assert_eq!(res["ok"], true);
    assert_eq!(res["archived"], 2);
    assert_eq!(res["moved"], 2);
    assert_eq!(res["fallback_queued"], 0);
    assert_eq!(res["skipped"], 0);

    // Existing file in creator dir MUST NEVER be overwritten.
    assert_eq!(fs::read(&existing_alice_clip).unwrap(), b"existing-alice-clip");
    assert!(!src1.exists());
    assert!(!src2.exists());

    let dst1 = res["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["vid"] == "vid-1")
        .unwrap()["dst"]
        .as_str()
        .unwrap()
        .to_owned();
    let dst2 = res["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["vid"] == "vid-2")
        .unwrap()["dst"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(dst1, dst2);
    assert!(dst1.contains("保留两份-"));
    assert!(dst2.contains("保留两份-"));
    assert_eq!(fs::read(&dst1).unwrap(), b"video-bytes-1");
    assert_eq!(fs::read(&dst2).unwrap(), b"video-bytes-2");

    // Verify archive-history.json records audit metadata + original_path.
    let history = read_json(&fx.config.base.join("archive-history.json"));
    let hist_arr = history.as_array().unwrap();
    assert_eq!(hist_arr.len(), 2);
    for (vid, orig, dst) in [
        ("vid-1", src1.to_string_lossy().to_string(), &dst1),
        ("vid-2", src2.to_string_lossy().to_string(), &dst2),
    ] {
        let entry = hist_arr.iter().find(|r| r["vid"] == vid).unwrap();
        assert_eq!(entry["original_path"], orig);
        assert_eq!(entry["dst"], *dst);
        assert_eq!(entry["source"], "auto-watermark");
        assert_eq!(entry["auto_reason"], "direct_handle");
        assert_eq!(entry["renamed_keep_both"], true);
        assert_eq!(entry["operation_id"], op_id);
        assert!(entry["size_gb"].is_number());
        assert!(entry["watermark_text"].as_str().unwrap().contains("Alice"));
    }

    // Verify faces.db & processing.db updated + auto_archived events logged.
    let faces = db::open(&fx.config.base.join("faces.db")).unwrap();
    let v1_path: String = faces
        .query_row("SELECT path FROM videos WHERE vid='vid-1'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v1_path, dst1);

    let proc_conn = db::open(&fx.config.base.join("processing/processing.db")).unwrap();
    let (stage, task_path): (String, String) = proc_conn
        .query_row("SELECT stage, path FROM tasks WHERE vid='vid-1'", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(stage, "excluded");
    assert_eq!(task_path, dst1);
    let event_count: i64 = proc_conn
        .query_row(
            "SELECT COUNT(*) FROM events WHERE kind='auto_archived'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(event_count, 2);

    // Verify Peach ledger updated asset paths and added creator links.
    let peach = db::open(&fx.config.ledger).unwrap();
    let p1: String = peach
        .query_row("SELECT path FROM asset WHERE id=10", [], |r| r.get(0))
        .unwrap();
    assert_eq!(p1, metadata::win(&fx.config, &dst1));
    let link_cnt: i64 = peach
        .query_row(
            "SELECT COUNT(*) FROM asset_entity WHERE asset_id IN (10,11) AND entity_id=1 AND role='creator'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(link_cnt, 2);

    // Verify progress polling compatibility for auto-archive.
    let prog = archive_api::progress(&fx.config, Some(&op_id)).unwrap();
    assert_eq!(prog["job"]["kind"], "auto-archive");
    assert_eq!(prog["job"]["status"], "complete");
    assert_eq!(prog["result"]["archived"], 2);
}

#[test]
fn preflight_peach_conflict_falls_back_to_queue_without_blocking_valid_items() {
    let fx = TestFixture::new("preflight-fallback");
    let src_ok = fx.add_ready_watermark_video("inbox", "ok.mp4", "vid-ok", b"ok-bytes", "Alice", Some(20));
    let src_blocked = fx.add_ready_watermark_video(
        "inbox",
        "blocked.mp4",
        "vid-blocked",
        b"blocked-bytes",
        "Alice",
        Some(21),
    );

    // Insert a real (non-stub) Peach asset already occupying target `creators/Alice/blocked.mp4`.
    let dst_blocked_win = metadata::win(
        &fx.config,
        &fx.config.creators().join("Alice/blocked.mp4").to_string_lossy(),
    );
    {
        let peach = Connection::open(&fx.config.ledger).unwrap();
        peach
            .execute(
                "INSERT INTO asset(id,location,path,name,medium,size,play_count)
                 VALUES(999,'local',?,'blocked.mp4','video',1234,5)",
                [&dst_blocked_win],
            )
            .unwrap();
    }

    let mut engine = Engine::new(fx.config.clone()).unwrap();
    let preview = archive_api::execute(&mut engine, "auto-watermark-preview", json!({})).unwrap();
    let items: Vec<Value> = preview["groups"][0]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|it| {
            json!({
                "vid": it["vid"],
                "person": "Alice",
                "path": it["path"],
                "size": it["size"],
                "mtime_ns": it["mtime_ns"],
            })
        })
        .collect();

    let res = archive_api::execute(
        &mut engine,
        "auto-watermark-apply",
        json!({ "version": preview["version"], "items": items }),
    )
    .unwrap();
    assert_eq!(res["ok"], true);
    assert_eq!(res["archived"], 1);
    assert_eq!(res["fallback_queued"], 1);
    assert_eq!(res["skipped"], 0);

    // Valid item archived directly; blocked item remained untouched on disk and fell back to queue.
    assert!(!src_ok.exists());
    assert!(fx.config.creators().join("Alice/ok.mp4").is_file());
    assert!(src_blocked.is_file());
    assert!(!fx.config.creators().join("Alice/blocked.mp4").exists());

    let queue = read_json(&fx.config.base.join("integration-queue.json"));
    let q_arr = queue.as_array().unwrap();
    assert_eq!(q_arr.len(), 1);
    assert_eq!(q_arr[0]["vid"], "vid-blocked");
    assert_eq!(q_arr[0]["source"], "auto-watermark");
    assert!(
        q_arr[0]["auto_fallback_reason"]
            .as_str()
            .is_some_and(|s| s.contains("Peach"))
    );

    // Real Peach asset #999 MUST be completely untouched.
    let peach = db::open(&fx.config.ledger).unwrap();
    let play_count: i64 = peach
        .query_row("SELECT play_count FROM asset WHERE id=999", [], |r| r.get(0))
        .unwrap();
    assert_eq!(play_count, 5);
    assert!(!fx.config.base.join(".review-write.lock").exists());
}

#[test]
fn strict_peach_commit_failure_rolls_back_without_best_effort_drift() {
    let fx = TestFixture::new("strict-peach-rollback");
    let src = fx.add_ready_watermark_video("inbox", "race.mp4", "vid-race", b"race-bytes", "Alice", Some(30));
    let stat = fs::metadata(&src).unwrap();

    let mut engine = Engine::new(fx.config.clone()).unwrap();
    let op_id = uuid::Uuid::new_v4().to_string();
    let req = Request {
        id: op_id.clone(),
        action: "auto-archive".into(),
        person: String::new(),
        files: vec![],
        filename: String::new(),
        target_person: None,
        target_folder: None,
        payload: Some(json!({
            "at": "2026-11-22T06:00:00.000Z",
            "items": [{
                "vid": "vid-race",
                "person": "Alice",
                "path": src.to_string_lossy().to_string(),
                "size": stat.len(),
                "mtime_ns": files::mtime_ns(&stat),
            }]
        })),
    };
    let preview_plan: Value = engine.preview(req.clone()).unwrap();
    let plan_version = preview_plan["version"].as_str().unwrap().to_owned();

    // Simulate a late real Peach conflict (non-stub with play_count=7 and link) inserted
    // between preview and apply. Strict ledger mode must NOT call best_effort_sync_peach.
    let dst_win = metadata::win(
        &fx.config,
        &fx.config.creators().join("Alice/race.mp4").to_string_lossy(),
    );
    {
        let peach = Connection::open(&fx.config.ledger).unwrap();
        peach
            .execute(
                "INSERT INTO asset(id,location,path,name,medium,size,play_count)
                 VALUES(888,'local',?,'race.mp4','video',9999,7)",
                [&dst_win],
            )
            .unwrap();
        peach
            .execute(
                "INSERT INTO asset_entity(asset_id,entity_id,role,source) VALUES(888,2,'tag','external')",
                [],
            )
            .unwrap();
    }

    let err = engine.apply(req, plan_version).unwrap_err();
    assert!(err.status >= 400);

    // Engine rolled back cleanly without leaving stuck lock or deleting real asset #888.
    let record = engine.store.get(&op_id).unwrap().unwrap();
    assert_eq!(record.status, "rolled_back");
    assert!(src.is_file());
    assert!(!fx.config.creators().join("Alice/race.mp4").exists());
    assert!(!fx.config.base.join(".review-write.lock").exists());

    let peach = db::open(&fx.config.ledger).unwrap();
    let play_count: i64 = peach
        .query_row("SELECT play_count FROM asset WHERE id=888", [], |r| r.get(0))
        .unwrap();
    assert_eq!(play_count, 7);
    let orig_path: String = peach
        .query_row("SELECT path FROM asset WHERE id=30", [], |r| r.get(0))
        .unwrap();
    assert_eq!(orig_path, metadata::win(&fx.config, &src.to_string_lossy()));
}

#[test]
fn unarchive_restores_all_stores_sets_auto_opt_out_and_restores_review_visibility() {
    let fx = TestFixture::new("unarchive-full");
    let orig_path = fx.add_ready_watermark_video(
        "inbox",
        "restore_me.mp4",
        "vid-u1",
        b"unarchive-video-payload",
        "Alice",
        Some(40),
    );
    // Add an unrelated tag link in Peach to verify unarchive ONLY removes creator link.
    {
        let peach = Connection::open(&fx.config.ledger).unwrap();
        peach
            .execute(
                "INSERT INTO asset_entity(asset_id,entity_id,role,source) VALUES(40,2,'tag','manual-tag')",
                [],
            )
            .unwrap();
    }

    let mut engine = Engine::new(fx.config.clone()).unwrap();
    let preview = archive_api::execute(&mut engine, "auto-watermark-preview", json!({})).unwrap();
    assert_eq!(preview["eligible_count"], 1);
    let item = &preview["groups"][0]["items"][0];
    let apply_res = archive_api::execute(
        &mut engine,
        "auto-watermark-apply",
        json!({
            "version": preview["version"],
            "items": [{
                "vid": item["vid"],
                "person": "Alice",
                "path": item["path"],
                "size": item["size"],
                "mtime_ns": item["mtime_ns"],
            }]
        }),
    )
    .unwrap();
    assert_eq!(apply_res["archived"], 1);
    let archived_path = PathBuf::from(apply_res["results"][0]["dst"].as_str().unwrap());
    assert!(archived_path.is_file());
    assert!(!orig_path.exists());

    // While archived, vid-u1 is hidden from /review and auto-watermark-preview.
    let review_before = review_sessions::execute(&mut engine, "review-page", json!({})).unwrap();
    assert_eq!(review_before["total"], 0);
    assert_eq!(
        auto_watermark::preview_auto_watermark_matches(&fx.config).unwrap()["eligible_count"],
        0
    );

    // Execute native archive-unarchive.
    let undo_res = archive_api::execute(
        &mut engine,
        "archive-unarchive",
        json!({ "vid": "vid-u1" }),
    )
    .unwrap();
    assert_eq!(undo_res["ok"], true);
    assert_eq!(undo_res["vid"], "vid-u1");
    assert_eq!(undo_res["person"], "Alice");
    assert_eq!(undo_res["restored_path"], orig_path.to_string_lossy().to_string());
    assert_eq!(undo_res["renamed_keep_both"], false);
    assert_eq!(undo_res["auto_opt_out"], true);

    // File moved back physically.
    assert!(orig_path.is_file());
    assert_eq!(fs::read(&orig_path).unwrap(), b"unarchive-video-payload");
    assert!(!archived_path.exists());

    // faces.db & video-path-overrides.json restored.
    let faces = db::open(&fx.config.base.join("faces.db")).unwrap();
    let v_path: String = faces
        .query_row("SELECT path FROM videos WHERE vid='vid-u1'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v_path, orig_path.to_string_lossy().to_string());
    let overrides = read_json(&fx.config.base.join("video-path-overrides.json"));
    assert_eq!(overrides["vid-u1"], orig_path.to_string_lossy().to_string());

    // archive-history.json entry preserved with undone=true.
    let history = read_json(&fx.config.base.join("archive-history.json"));
    let h = &history.as_array().unwrap()[0];
    assert_eq!(h["vid"], "vid-u1");
    assert_eq!(h["undone"], true);
    assert!(h["undone_at"].as_str().is_some());
    assert_eq!(h["restored_path"], orig_path.to_string_lossy().to_string());

    // Peach ledger restored asset.path, removed Alice creator link, preserved unrelated tag link.
    let peach = db::open(&fx.config.ledger).unwrap();
    let asset_path: String = peach
        .query_row("SELECT path FROM asset WHERE id=40", [], |r| r.get(0))
        .unwrap();
    assert_eq!(asset_path, metadata::win(&fx.config, &orig_path.to_string_lossy()));
    let creator_links: i64 = peach
        .query_row(
            "SELECT COUNT(*) FROM asset_entity WHERE asset_id=40 AND entity_id=1 AND role='creator'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(creator_links, 0);
    let tag_links: i64 = peach
        .query_row(
            "SELECT COUNT(*) FROM asset_entity WHERE asset_id=40 AND entity_id=2 AND role='tag'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(tag_links, 1);

    // processing.db restored to stage='review', auto_opt_out=1, and logged auto_unarchived event.
    let proc_conn = db::open(&fx.config.base.join("processing/processing.db")).unwrap();
    let (stage, path, opt_out): (String, String, i64) = proc_conn
        .query_row(
            "SELECT stage, path, auto_opt_out FROM tasks WHERE vid='vid-u1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(stage, "review");
    assert_eq!(path, orig_path.to_string_lossy().to_string());
    assert_eq!(opt_out, 1);
    let unarchive_events: i64 = proc_conn
        .query_row(
            "SELECT COUNT(*) FROM events WHERE vid='vid-u1' AND kind='auto_unarchived'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(unarchive_events, 1);

    // auto_opt_out=1 prevents re-triggering auto-archive.
    assert_eq!(
        auto_watermark::preview_auto_watermark_matches(&fx.config).unwrap()["eligible_count"],
        0
    );
    assert!(auto_watermark::evaluate_for_vids(&fx.config, None).unwrap().is_empty());

    // undone=true restores visibility in /review!
    let review_after = review_sessions::execute(&mut engine, "review-page", json!({})).unwrap();
    assert_eq!(review_after["total"], 1);
    assert_eq!(review_after["rows"][0]["vid"], "vid-u1");

    // Second unarchive must fail with 409 conflict.
    let second = archive_api::execute(&mut engine, "archive-unarchive", json!({ "vid": "vid-u1" }));
    assert_eq!(second.unwrap_err().status, 409);
}

#[test]
fn auto_archive_adopts_pure_scanner_stub_in_peach_strictly() {
    let fx = TestFixture::new("pure-stub-adopt");
    let src = fx.add_ready_watermark_video(
        "inbox",
        "stubbed.mp4",
        "vid-stub",
        b"exact-stub-bytes",
        "Alice",
        Some(70),
    );
    let stat = fs::metadata(&src).unwrap();

    // Insert a pure unlinked scanner stub (play_count=0, same size/location/name, no links)
    // at target path BEFORE apply — auto-archive should adopt it cleanly instead of failing.
    let dst_win = metadata::win(
        &fx.config,
        &fx.config.creators().join("Alice/stubbed.mp4").to_string_lossy(),
    );
    let mut engine = Engine::new(fx.config.clone()).unwrap();
    let op_id = uuid::Uuid::new_v4().to_string();
    let req = Request {
        id: op_id.clone(),
        action: "auto-archive".into(),
        person: String::new(),
        files: vec![],
        filename: String::new(),
        target_person: None,
        target_folder: None,
        payload: Some(json!({
            "at": "2026-11-22T06:00:00.000Z",
            "items": [{
                "vid": "vid-stub",
                "person": "Alice",
                "path": src.to_string_lossy().to_string(),
                "size": stat.len(),
                "mtime_ns": files::mtime_ns(&stat),
            }]
        })),
    };
    let preview_plan: Value = engine.preview(req.clone()).unwrap();
    let plan_version = preview_plan["version"].as_str().unwrap().to_owned();

    // Simulate Peach scanner inserting a pure stub after preview and before apply.
    {
        let peach = Connection::open(&fx.config.ledger).unwrap();
        peach
            .execute(
                "INSERT INTO asset(id,location,path,name,medium,size,play_count,mutation_revision)
                 VALUES(777,'local',?,'stubbed.mp4','video',?,0,0)",
                params![dst_win, stat.len() as i64],
            )
            .unwrap();
    }

    let res = engine.apply(req, plan_version).unwrap();
    assert_eq!(res["ok"], true);
    assert_eq!(res["archived"], 1);
    assert!(fx.config.creators().join("Alice/stubbed.mp4").is_file());
    assert!(!src.exists());

    // Original asset #70 moved to dst, pure stub #777 adopted/deleted, Alice creator link present.
    let peach = db::open(&fx.config.ledger).unwrap();
    let stub_exists: i64 = peach
        .query_row("SELECT COUNT(*) FROM asset WHERE id=777", [], |r| r.get(0))
        .unwrap();
    assert_eq!(stub_exists, 0);
    let moved_path: String = peach
        .query_row("SELECT path FROM asset WHERE id=70", [], |r| r.get(0))
        .unwrap();
    assert_eq!(moved_path, dst_win);
    let creator_links: i64 = peach
        .query_row(
            "SELECT COUNT(*) FROM asset_entity WHERE asset_id=70 AND entity_id=1 AND role='creator'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(creator_links, 1);
}

#[test]
fn unarchive_uses_keep_both_when_original_path_is_occupied() {
    let fx = TestFixture::new("unarchive-keep-both");
    let orig_path = fx.add_ready_watermark_video(
        "inbox",
        "occupied.mp4",
        "vid-occ",
        b"original-archived-bytes",
        "Alice",
        Some(50),
    );

    let mut engine = Engine::new(fx.config.clone()).unwrap();
    let preview = archive_api::execute(&mut engine, "auto-watermark-preview", json!({})).unwrap();
    let item = &preview["groups"][0]["items"][0];
    archive_api::execute(
        &mut engine,
        "auto-watermark-apply",
        json!({
            "version": preview["version"],
            "items": [{
                "vid": item["vid"],
                "person": "Alice",
                "path": item["path"],
                "size": item["size"],
                "mtime_ns": item["mtime_ns"],
            }]
        }),
    )
    .unwrap();

    // Another file now occupies the original path.
    fs::write(&orig_path, b"new-occupant-bytes").unwrap();

    let undo_res = archive_api::execute(
        &mut engine,
        "archive-unarchive",
        json!({ "vid": "vid-occ" }),
    )
    .unwrap();
    assert_eq!(undo_res["ok"], true);
    assert_eq!(undo_res["renamed_keep_both"], true);
    let restored = PathBuf::from(undo_res["restored_path"].as_str().unwrap());
    assert_ne!(restored, orig_path);
    assert!(restored.to_string_lossy().contains("保留两份-"));
    assert_eq!(fs::read(&orig_path).unwrap(), b"new-occupant-bytes");
    assert_eq!(fs::read(&restored).unwrap(), b"original-archived-bytes");

    let peach = db::open(&fx.config.ledger).unwrap();
    let (asset_path, asset_name): (String, String) = peach
        .query_row("SELECT path, name FROM asset WHERE id=50", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(asset_path, metadata::win(&fx.config, &restored.to_string_lossy()));
    assert_eq!(
        asset_name,
        restored.file_name().unwrap().to_string_lossy().to_string()
    );
}

#[test]
fn background_publisher_respects_pause_toggle_admission_and_wakes_for_new_tasks() {
    let fx = TestFixture::new("bg-publisher");
    let src1 = fx.add_ready_watermark_video("inbox", "bg1.mp4", "vid-bg-1", b"bg-bytes-1", "Alice", Some(60));
    let src2 = fx.add_ready_watermark_video("inbox", "bg2.mp4", "vid-bg-2", b"bg-bytes-2", "Alice", Some(61));

    // SAFETY: single-test env override for fast debounce/sweep in isolated test.
    unsafe {
        std::env::set_var("AUTO_ARCHIVE_DEBOUNCE_MS", "20");
        std::env::set_var("AUTO_ARCHIVE_SWEEP_SECS", "1");
    }

    let (sender, queue) = mpsc::sync_channel::<Work>(32);
    let worker = admission::Worker::new(fx.config.clone(), sender).unwrap();
    let completion = worker.completion();
    let wake = auto_archive::AutoArchiveWake::new();
    let mut engine = Engine::new(fx.config.clone()).unwrap();
    engine.set_auto_archive_wake(wake.clone());

    let executor = thread::spawn(move || {
        for Work {
            command,
            reply,
            mut lock,
        } in queue
        {
            let res = match command {
                Command::Archive { command, body } => {
                    archive_api::execute(&mut engine, &command, body)
                }
                _ => panic!("unexpected command in bg publisher test"),
            };
            if let Some(guard) = lock.take() {
                guard.finish_with_result(&res);
            }
            completion.complete();
            let _ = reply.send(res);
        }
    });

    let stop = Arc::new(AtomicBool::new(false));
    let publisher = auto_archive::spawn_auto_archive_publisher(
        fx.config.clone(),
        worker.clone(),
        wake.clone(),
        stop.clone(),
    );

    // 1. When paused=true, waking must NOT auto-archive.
    {
        let conn = processing_scheduler::open_or_init(&fx.config).unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO settings(key,value) VALUES('paused','true')",
            [],
        )
        .unwrap();
    }
    wake.notify_vid("vid-bg-1");
    thread::sleep(Duration::from_millis(200));
    assert!(src1.is_file());

    // 2. Unpause, but set auto_watermark_merge=false -> must NOT auto-archive.
    {
        let conn = processing_scheduler::open_or_init(&fx.config).unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO settings(key,value) VALUES('paused','false')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO settings(key,value) VALUES('auto_watermark_merge','false')",
            [],
        )
        .unwrap();
    }
    wake.notify_vid("vid-bg-1");
    thread::sleep(Duration::from_millis(200));
    assert!(src1.is_file());

    // 3. Re-enable auto_watermark_merge=true -> waking archives vid-bg-1 automatically!
    {
        let conn = processing_scheduler::open_or_init(&fx.config).unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO settings(key,value) VALUES('auto_watermark_merge','true')",
            [],
        )
        .unwrap();
    }
    wake.notify_vid("vid-bg-1");
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && src1.exists() {
        thread::sleep(Duration::from_millis(40));
    }
    assert!(!src1.exists(), "background publisher should have archived vid-bg-1");
    assert!(fx.config.creators().join("Alice/bg1.mp4").is_file());
    // vid-bg-2 was NOT woken yet, so it stays untouched!
    assert!(src2.is_file());

    // 4. Close admission -> waking vid-bg-2 must NOT auto-archive.
    worker.set_accepting(false).unwrap();
    wake.notify_vid("vid-bg-2");
    thread::sleep(Duration::from_millis(200));
    assert!(src2.is_file());

    // Clean shutdown.
    stop.store(true, Ordering::Relaxed);
    wake.notify_stop();
    publisher.join().unwrap();
    drop(worker);
    executor.join().unwrap();
}

#[test]
fn face_queued_auto_archive_unarchive_survives_discover_and_keeps_review_visibility() {
    let fx = TestFixture::new("face-queued-discover");
    let orig_path = fx.add_ready_watermark_video(
        "inbox",
        "dreamy_clip.mp4",
        "vid-fq",
        b"face-queued-bytes",
        "Alice",
        Some(80),
    );
    // Put task into `stage = 'face_queued'` (with `data.ready = true`, matching live dreamymoo tasks).
    {
        let proc_conn = processing_scheduler::open_or_init(&fx.config).unwrap();
        proc_conn
            .execute("UPDATE tasks SET stage='face_queued' WHERE vid='vid-fq'", [])
            .unwrap();
    }

    let mut engine = Engine::new(fx.config.clone()).unwrap();
    let preview = archive_api::execute(&mut engine, "auto-watermark-preview", json!({})).unwrap();
    assert_eq!(preview["eligible_count"], 1);
    let item = &preview["groups"][0]["items"][0];

    // 1. Auto-archive directly from face_queued.
    let apply = archive_api::execute(
        &mut engine,
        "auto-watermark-apply",
        json!({
            "version": preview["version"],
            "items": [{
                "vid": item["vid"],
                "person": "Alice",
                "path": item["path"],
                "size": item["size"],
                "mtime_ns": item["mtime_ns"],
            }]
        }),
    )
    .unwrap();
    assert_eq!(apply["archived"], 1);
    assert!(!orig_path.exists());

    // 2. Unarchive back to original unorganized path.
    let undo = archive_api::execute(&mut engine, "archive-unarchive", json!({ "vid": "vid-fq" }))
        .unwrap();
    assert_eq!(undo["ok"], true);
    assert_eq!(undo["auto_opt_out"], true);
    assert!(orig_path.is_file());

    // 3. Run processing_scheduler::discover with matching pipeline after unarchive;
    //    auto_opt_out=1 and stage='review' must survive!
    let _discovered = processing_scheduler::discover(
        &fx.config,
        &json!({ "pipeline": "paddleocr-test-v2" }),
    )
    .unwrap();

    let proc_conn = db::open(&fx.config.base.join("processing/processing.db")).unwrap();
    let (stage, opt_out, path): (String, i64, String) = proc_conn
        .query_row(
            "SELECT stage, auto_opt_out, path FROM tasks WHERE vid='vid-fq'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(stage, "review");
    assert_eq!(opt_out, 1);
    assert_eq!(path, orig_path.to_string_lossy().to_string());

    // Must remain blocked from auto-watermark-preview, yet visible in /review!
    assert_eq!(
        auto_watermark::preview_auto_watermark_matches(&fx.config).unwrap()["eligible_count"],
        0
    );
    let review_page = review_sessions::execute(&mut engine, "review-page", json!({})).unwrap();
    assert_eq!(review_page["total"], 1);
    assert_eq!(review_page["rows"][0]["vid"], "vid-fq");

    // 4. Even if discover bumps face reference version (transitioning review -> face_queued),
    //    auto_opt_out=1 must STILL survive and block auto-watermark-preview!
    let _discovered2 = processing_scheduler::discover(
        &fx.config,
        &json!({
            "pipeline": "paddleocr-test-v2",
            "references": { "version": "r2-updated" }
        }),
    )
    .unwrap();
    let (stage2, opt_out2): (String, i64) = proc_conn
        .query_row(
            "SELECT stage, auto_opt_out FROM tasks WHERE vid='vid-fq'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(stage2, "face_queued");
    assert_eq!(opt_out2, 1);
    assert_eq!(
        auto_watermark::preview_auto_watermark_matches(&fx.config).unwrap()["eligible_count"],
        0
    );
}

#[test]
fn unarchive_non_ready_task_restores_queued_stage_and_failure_result_marks_uncommitted_fallbacks_failed() {
    let fx = TestFixture::new("non-ready-and-failure");
    let orig_path = fx.add_ready_watermark_video(
        "inbox",
        "not_ready.mp4",
        "vid-nr",
        b"non-ready-bytes",
        "Alice",
        Some(90),
    );
    // Simulate a manually archived video whose task data did NOT have ready=true.
    let archived_path = fx.config.creators().join("Alice/not_ready.mp4");
    fs::rename(&orig_path, &archived_path).unwrap();
    let stat = fs::metadata(&archived_path).unwrap();
    let archived_str = archived_path.to_string_lossy().to_string();
    let orig_str = orig_path.to_string_lossy().to_string();
    {
        let proc_conn = processing_scheduler::open_or_init(&fx.config).unwrap();
        proc_conn
            .execute(
                "UPDATE tasks SET path=?, stage='excluded', data='{\"ready\":false}' WHERE vid='vid-nr'",
                [&archived_str],
            )
            .unwrap();
        let faces = Connection::open(fx.config.base.join("faces.db")).unwrap();
        faces
            .execute(
                "UPDATE videos SET path=? WHERE vid='vid-nr'",
                [&archived_str],
            )
            .unwrap();
        let peach = Connection::open(&fx.config.ledger).unwrap();
        peach
            .execute(
                "UPDATE asset SET path=? WHERE id=90",
                [metadata::win(&fx.config, &archived_str)],
            )
            .unwrap();
        peach
            .execute(
                "INSERT INTO asset_entity(asset_id,entity_id,role,source) VALUES(90,1,'creator','face-curation-8919')",
                [],
            )
            .unwrap();
        fs::write(
            fx.config.base.join("archive-history.json"),
            serde_json::to_string_pretty(&json!([{
                "vid": "vid-nr",
                "creator": "Alice",
                "original_path": orig_str,
                "src": orig_str,
                "dst": archived_str,
                "win_dst": metadata::win(&fx.config, &archived_str),
                "size": stat.len(),
                "operation": "archive",
                "operation_id": "manual-op-1",
                "manifest": "rust:manual-op-1"
            }]))
            .unwrap(),
        )
        .unwrap();
    }

    let mut engine = Engine::new(fx.config.clone()).unwrap();
    let undo = archive_api::execute(&mut engine, "archive-unarchive", json!({ "vid": "vid-nr" }))
        .unwrap();
    assert_eq!(undo["ok"], true);
    let proc_conn = db::open(&fx.config.base.join("processing/processing.db")).unwrap();
    let (stage, opt_out): (String, i64) = proc_conn
        .query_row(
            "SELECT stage, auto_opt_out FROM tasks WHERE vid='vid-nr'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(stage, "queued");
    assert_eq!(opt_out, 1);

    // Also verify auto_archive::failure_result rewrites uncommitted fallback_queued items to failed.
    let src_ok = fx.add_ready_watermark_video("inbox", "f_ok.mp4", "vid-fok", b"fok-bytes", "Alice", Some(91));
    let _src_fb = fx.add_ready_watermark_video("inbox", "f_fb.mp4", "vid-ffb", b"ffb-bytes", "Alice", Some(92));
    {
        let peach = Connection::open(&fx.config.ledger).unwrap();
        peach
            .execute(
                "INSERT INTO asset(id,location,path,name,medium,size,play_count)
                 VALUES(992,'local',?,'f_fb.mp4','video',100,4)",
                [metadata::win(
                    &fx.config,
                    &fx.config.creators().join("Alice/f_fb.mp4").to_string_lossy(),
                )],
            )
            .unwrap();
    }
    let preview = archive_api::execute(&mut engine, "auto-watermark-preview", json!({})).unwrap();
    let items: Vec<Value> = preview["groups"][0]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|it| {
            json!({
                "vid": it["vid"],
                "person": "Alice",
                "path": it["path"],
                "size": it["size"],
                "mtime_ns": it["mtime_ns"],
            })
        })
        .collect();
    let op_id = uuid::Uuid::new_v4().to_string();
    let req = Request {
        id: op_id.clone(),
        action: "auto-archive".into(),
        person: String::new(),
        files: vec![],
        filename: String::new(),
        target_person: None,
        target_folder: None,
        payload: Some(json!({
            "at": "2026-11-22T06:00:00.000Z",
            "items": items
        })),
    };
    let plan: face_file_operations::model::Plan =
        serde_json::from_value(engine.preview(req).unwrap()).unwrap();
    let failed = auto_archive::failure_result(&plan, "rolled_back", "simulated ledger rollback", &[], &[]);
    assert_eq!(failed["ok"], false);
    assert_eq!(failed["fallback_queued"], 0);
    let results = failed["results"].as_array().unwrap();
    assert!(
        results.iter().all(|r| r["status"] != "fallback_queued"),
        "rolled-back failure_result must never report uncommitted fallback items as fallback_queued: {results:?}"
    );
    let fb_row = results.iter().find(|r| r["vid"] == "vid-ffb").unwrap();
    assert_eq!(fb_row["status"], "failed");
    assert!(src_ok.is_file());
}
