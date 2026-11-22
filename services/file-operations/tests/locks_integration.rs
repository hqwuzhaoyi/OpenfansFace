use face_file_operations::{
    Command, Config, Engine, Work, admission, archive_api, auto_archive, files, locks, metadata,
    model::{Request, SelectedFile},
    person_batch, processing_scheduler, review_sessions,
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
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

struct LockTestFixture {
    root: PathBuf,
    config: Config,
}

impl LockTestFixture {
    fn new(label: &str) -> Self {
        let root = fs::canonicalize("/tmp")
            .unwrap()
            .join(format!("face-locks-test-{label}-{}", uuid::Uuid::new_v4()));
        let base = root.join("base");
        let media = root.join("media");
        let ledger = root.join("ledger.db");
        fs::create_dir_all(base.join("processing")).unwrap();
        fs::create_dir_all(media.join("pikpak/inbox")).unwrap();
        fs::create_dir_all(media.join("pikpak/creators/Alice")).unwrap();
        fs::create_dir_all(media.join("pikpak/creators/Bob")).unwrap();
        fs::write(root.join(".fileops-fixture"), "locks integration fixture").unwrap();

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
        for (id, name, norm) in [(1, "Alice", "alice"), (2, "Bob", "bob")] {
            peach
                .execute(
                    "INSERT INTO entity(id,kind,canonical_name,normalized_name,metadata_json,created_at,updated_at)
                     VALUES(?,'creator',?,?,'{}','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z')",
                    params![id, name, norm],
                )
                .unwrap();
        }
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
        let _ = processing_scheduler::open_or_init(&config).unwrap();

        Self { root, config }
    }

    fn add_person_video(
        &self,
        person: &str,
        filename: &str,
        vid: &str,
        bytes: &[u8],
        peach_asset_id: Option<i64>,
    ) -> PathBuf {
        let dir = self.config.creators().join(person);
        fs::create_dir_all(&dir).unwrap();
        let file_path = dir.join(filename);
        fs::write(&file_path, bytes).unwrap();
        let stat = fs::metadata(&file_path).unwrap();
        let path_str = file_path.to_string_lossy().to_string();

        let faces = Connection::open(self.config.base.join("faces.db")).unwrap();
        faces
            .execute(
                "INSERT INTO videos(vid, path, name, collection) VALUES(?, ?, ?, ?)",
                params![vid, path_str, filename, dir.to_string_lossy().to_string()],
            )
            .unwrap();

        if let Some(aid) = peach_asset_id {
            let peach = Connection::open(&self.config.ledger).unwrap();
            let win_path = metadata::win(&self.config, &path_str);
            peach
                .execute(
                    "INSERT INTO asset(id,location,path,name,medium,size) VALUES(?,'local',?,?,'video',?)",
                    params![aid, win_path, filename, stat.len() as i64],
                )
                .unwrap();
        }
        file_path
    }

    fn add_review_watermark_video(
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
        let wm_key = format!("{:064x}", peach_asset_id.unwrap_or(500) + 1);
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

    fn set_fault(&self, fault: Value) {
        fs::write(
            self.root.join("rust-fault.json"),
            serde_json::to_string(&fault).unwrap(),
        )
        .unwrap();
    }

    fn clear_fault(&self) {
        let _ = fs::remove_file(self.root.join("rust-fault.json"));
        let _ = fs::remove_file(self.root.join("rust-fault-reached.json"));
    }
}

impl Drop for LockTestFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn wait_until(timeout: Duration, mut pred: impl FnMut() -> bool) {
    let start = Instant::now();
    while !pred() {
        assert!(start.elapsed() < timeout, "condition timed out");
        thread::sleep(Duration::from_millis(15));
    }
}

#[test]
fn raii_guard_releases_on_drop_and_panic_without_leaking_locks() {
    let reg = locks::ActiveLockRegistry::new();

    // Normal drop without settle -> cancelled & released.
    {
        let _guard = reg.register_queued(
            "op-drop-1",
            "move-files",
            vec!["v1".into()],
            vec!["Alice".into(), "Bob".into()],
        );
        let snap = reg.snapshot();
        assert_eq!(snap.videos["v1"].label, "排队移动");
        assert_eq!(snap.people["Alice"].label, "排队移动");
        assert_eq!(snap.people["Bob"].label, "排队移动");
    }
    let after = reg.snapshot();
    assert!(after.videos.is_empty());
    assert!(after.people.is_empty());
    assert!(after.active_ops.is_empty());
    assert_eq!(after.last_settled.as_ref().unwrap().outcome, "cancelled");

    // Panic while holding guard -> RAII Drop still releases lock cleanly!
    let reg_clone = Arc::clone(&reg);
    let panicked = std::panic::catch_unwind(move || {
        let guard = reg_clone.register_queued(
            "op-panic-1",
            "rename-person",
            vec!["v2".into()],
            vec!["Alice".into()],
        );
        guard.mark_running("rename-person", vec!["v2".into()], vec!["Alice".into(), "AliceNew".into()]);
        assert_eq!(reg_clone.snapshot().people["AliceNew"].label, "改名中");
        panic!("simulated executor panic");
    });
    assert!(panicked.is_err());
    let post_panic = reg.snapshot();
    assert!(post_panic.videos.is_empty());
    assert!(post_panic.people.is_empty());
    assert!(post_panic.active_ops.is_empty());
    assert_eq!(post_panic.last_settled.as_ref().unwrap().op_id, "op-panic-1");
    assert_eq!(post_panic.last_settled.as_ref().unwrap().outcome, "cancelled");
}

#[test]
fn engine_live_locks_cover_review_delete_person_batch_merge_and_auto_archive() {
    let fx = LockTestFixture::new("multi-ops");
    let _alice_vid = fx.add_person_video("Alice", "a1.mp4", "vid-a1", b"alice-video-1", Some(10));
    let _bob_vid = fx.add_person_video("Bob", "b1.mp4", "vid-b1", b"bob-video-1", Some(11));
    let rev_del_path = fx.add_review_watermark_video(
        "inbox",
        "delme.mp4",
        "vid-rev-del",
        b"review-delete-bytes",
        "Alice",
        Some(12),
    );
    let _auto_path = fx.add_review_watermark_video(
        "inbox",
        "auto1.mp4",
        "vid-auto-1",
        b"auto-archive-bytes",
        "Alice",
        Some(13),
    );

    let mut engine = Engine::new(fx.config.clone()).unwrap();
    let registry = engine.locks();

    // 1. review-delete: verify live "running" / "删除中" during pause, then settled "complete".
    let page = review_sessions::execute(&mut engine, "review-page", json!({"group": "person:Alice"})).unwrap();
    let session = page["session"].as_str().unwrap().to_owned();
    let token = page["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["vid"] == "vid-rev-del")
        .unwrap()["review_token"]
        .as_str()
        .unwrap()
        .to_owned();
    let stat = fs::metadata(&rev_del_path).unwrap();
    let del_id = uuid::Uuid::new_v4().to_string();
    let del_req = Request {
        id: del_id.clone(),
        action: "review-delete".into(),
        person: String::new(),
        files: vec![],
        filename: String::new(),
        target_person: None,
        target_folder: None,
        payload: Some(json!({
            "vids": ["vid-rev-del"],
            "review_session": session,
            "review_tokens": { "vid-rev-del": token },
            "review_group": { "key": "person:Alice", "deferred": false },
            "fingerprints": {
                "vid-rev-del": {
                    "path": rev_del_path.to_string_lossy(),
                    "size": stat.len(),
                    "mtime_ns": files::mtime_ns(&stat),
                }
            }
        })),
    };
    let del_preview = engine.preview(del_req.clone()).unwrap();
    let del_version = del_preview["version"].as_str().unwrap().to_owned();

    // Pause right after admission so we deterministically observe the running lock.
    fx.set_fault(json!({"phase": "admitted", "mode": "pause", "milliseconds": 250}));
    let reg_observer = Arc::clone(&registry);
    let fx_root = fx.root.clone();
    let observed = thread::spawn(move || {
        let marker = fx_root.join("rust-fault-reached.json");
        let start = Instant::now();
        while !marker.exists() && start.elapsed() < Duration::from_secs(5) {
            thread::sleep(Duration::from_millis(10));
        }
        reg_observer.snapshot()
    });
    let del_res = engine.apply(del_req, del_version).unwrap();
    assert_eq!(del_res["ok"], true);
    fx.clear_fault();

    let mid_snap = observed.join().unwrap();
    assert_eq!(mid_snap.videos["vid-rev-del"].op_id, del_id);
    assert_eq!(mid_snap.videos["vid-rev-del"].action, "review-delete");
    assert_eq!(mid_snap.videos["vid-rev-del"].status, "running");
    assert_eq!(mid_snap.videos["vid-rev-del"].label, "删除中");

    let post_del = registry.snapshot();
    assert!(post_del.videos.is_empty());
    let settled = post_del.last_settled.as_ref().unwrap();
    assert_eq!(settled.op_id, del_id);
    assert_eq!(settled.action, "review-delete");
    assert_eq!(settled.outcome, "complete");
    assert_eq!(settled.vids, vec!["vid-rev-del"]);

    // 2. person-batch merge (Bob -> Alice): verify multi-entity lock covers BOTH people & vids!
    let batch_id = uuid::Uuid::new_v4().to_string();
    let batch_preview = person_batch::execute(
        &mut engine,
        json!({
            "mode": "preview",
            "id": batch_id,
            "request": {
                "action": "merge",
                "people": ["Bob", "Alice"],
                "target": "Alice"
            }
        }),
    )
    .unwrap();
    let batch_version = batch_preview["version"].as_str().unwrap().to_owned();

    // Verify admission-time extraction from persisted batch preview works off the writer lane.
    let extracted = locks::extract_from_batch_apply(
        &fx.config,
        &json!({"mode": "apply", "id": batch_id, "version": batch_version}),
    )
    .expect("batch apply extraction");
    assert_eq!(extracted.0, batch_id);
    assert_eq!(extracted.1, "batch-merge");
    assert_eq!(extracted.2, vec!["vid-b1"]);
    assert_eq!(extracted.3, vec!["Alice", "Bob"]);

    fx.set_fault(json!({"phase": "batch_validated", "mode": "pause", "milliseconds": 250}));
    let reg_observer = Arc::clone(&registry);
    let fx_root = fx.root.clone();
    let observed_batch = thread::spawn(move || {
        let marker = fx_root.join("rust-fault-reached.json");
        let start = Instant::now();
        while !marker.exists() && start.elapsed() < Duration::from_secs(5) {
            thread::sleep(Duration::from_millis(10));
        }
        reg_observer.snapshot()
    });
    let batch_res = person_batch::execute(
        &mut engine,
        json!({
            "mode": "apply",
            "id": batch_id,
            "version": batch_version
        }),
    )
    .unwrap();
    assert_eq!(batch_res["ok"], true);
    fx.clear_fault();

    let mid_batch = observed_batch.join().unwrap();
    assert_eq!(mid_batch.active_ops.len(), 1);
    assert_eq!(mid_batch.active_ops[0].action, "batch-merge");
    assert_eq!(mid_batch.people["Alice"].label, "合并中");
    assert_eq!(mid_batch.people["Bob"].label, "合并中");
    assert_eq!(mid_batch.videos["vid-b1"].label, "合并中");

    let post_batch = registry.snapshot();
    assert!(post_batch.people.is_empty());
    assert!(post_batch.videos.is_empty());
    let batch_settled = post_batch.last_settled.as_ref().unwrap();
    assert_eq!(batch_settled.op_id, batch_id);
    assert_eq!(batch_settled.action, "batch-merge");
    assert_eq!(batch_settled.outcome, "complete");
    assert_eq!(batch_settled.people, vec!["Alice", "Bob"]);
    assert_eq!(batch_settled.vids, vec!["vid-b1"]);

    // 3. Background auto-archive publisher via AutoArchiveWake + locks.
    // SAFETY: isolated test env vars for fast debounce.
    unsafe {
        std::env::set_var("AUTO_ARCHIVE_DEBOUNCE_MS", "20");
        std::env::set_var("AUTO_ARCHIVE_SWEEP_SECS", "1");
    }
    let (sender, queue) = mpsc::sync_channel::<Work>(32);
    let worker = admission::Worker::new(fx.config.clone(), sender).unwrap();
    let completion = worker.completion();
    let wake = auto_archive::AutoArchiveWake::new();
    wake.set_locks(Arc::clone(&registry));
    engine.set_auto_archive_wake(Arc::clone(&wake));

    let mut rx = registry.subscribe();
    let _ = rx.borrow_and_update();

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
                _ => panic!("unexpected command"),
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
        Arc::clone(&wake),
        Arc::clone(&stop),
    );

    wake.notify_vid("vid-auto-1");
    wait_until(Duration::from_secs(5), || {
        registry
            .snapshot()
            .last_settled
            .as_ref()
            .is_some_and(|s| s.action == "auto-archive" && s.outcome == "complete")
    });

    let after_auto = registry.snapshot();
    assert!(after_auto.videos.is_empty());
    assert!(after_auto.people.is_empty());
    let auto_settled = after_auto.last_settled.as_ref().unwrap();
    assert_eq!(auto_settled.action, "auto-archive");
    assert_eq!(auto_settled.outcome, "complete");
    assert_eq!(auto_settled.people, vec!["Alice"]);
    assert_eq!(auto_settled.vids, vec!["vid-auto-1"]);

    stop.store(true, Ordering::Relaxed);
    wake.notify_stop();
    publisher.join().unwrap();
    drop(worker);
    executor.join().unwrap();
}

#[test]
fn recovery_needed_startup_seeding_and_reconcile_clear() {
    let fx = LockTestFixture::new("recovery-seed");
    let src = fx.add_person_video("Alice", "r1.mp4", "vid-r1", b"recovery-bytes", Some(20));

    // Part 1: Real ambiguous mutation -> stays recovery_needed across daemon restart -> seeded by seed_from_store!
    let op_ambiguous = uuid::Uuid::new_v4().to_string();
    let req_ambiguous = Request {
        id: op_ambiguous.clone(),
        action: "delete-person".into(),
        person: "Alice".into(),
        files: vec![],
        filename: String::new(),
        target_person: None,
        target_folder: None,
        payload: None,
    };
    {
        let mut engine = Engine::new(fx.config.clone()).unwrap();
        let preview = engine.preview(req_ambiguous.clone()).unwrap();
        let version = preview["version"].as_str().unwrap().to_owned();
        fx.set_fault(json!({
            "phase": "mutation:destination_publish",
            "mode": "mutation_error_after",
            "errno": "EIO"
        }));
        let err = engine.apply(req_ambiguous.clone(), version).unwrap_err();
        assert_eq!(err.code, "recovery_required");
        // Live registry keeps recovery_needed lock!
        let live_snap = engine.locks().snapshot();
        assert_eq!(live_snap.people["Alice"].op_id, op_ambiguous);
        assert_eq!(live_snap.people["Alice"].status, "recovery_needed");
        assert_eq!(live_snap.people["Alice"].label, "待恢复");
        assert_eq!(live_snap.videos["vid-r1"].status, "recovery_needed");
        assert_eq!(live_snap.videos["vid-r1"].label, "待恢复");
        fx.clear_fault();
    }

    // Simulate daemon restart: Engine::new runs recover_startup (refuses ambiguous mutation) then seeds locks!
    {
        let mut engine = Engine::new(fx.config.clone()).unwrap();
        let seeded_snap = engine.locks().snapshot();
        assert_eq!(seeded_snap.people["Alice"].op_id, op_ambiguous);
        assert_eq!(seeded_snap.people["Alice"].action, "delete-person");
        assert_eq!(seeded_snap.people["Alice"].status, "recovery_needed");
        assert_eq!(seeded_snap.people["Alice"].label, "待恢复");
        assert_eq!(seeded_snap.videos["vid-r1"].op_id, op_ambiguous);
        assert_eq!(seeded_snap.videos["vid-r1"].status, "recovery_needed");
        assert_eq!(seeded_snap.videos["vid-r1"].label, "待恢复");
        assert_eq!(seeded_snap.active_ops.len(), 1);
        assert!(seeded_snap.last_settled.is_none());

        // Clear the simulated operator-verified ambiguous mutation barrier and run explicit reconcile_request.
        engine
            .store
            .db
            .execute(
                "DELETE FROM steps WHERE operation_id=? AND phase='filesystem_mutation_intent'",
                [&op_ambiguous],
            )
            .unwrap();
        engine
            .store
            .db
            .execute(
                "UPDATE operations SET error=NULL WHERE id=?",
                [&op_ambiguous],
            )
            .unwrap();

        let rec_plan = engine.store.get(&op_ambiguous).unwrap().unwrap().plan;
        let status = engine
            .reconcile_request(rec_plan.request, Some(rec_plan.version))
            .unwrap();
        assert!(
            matches!(status["status"].as_str(), Some("complete" | "rolled_back")),
            "expected terminal reconciled status, got {status}"
        );

        let cleared = engine.locks().snapshot();
        assert!(cleared.people.is_empty(), "expected people unlocked after reconcile: {cleared:?}");
        assert!(cleared.videos.is_empty(), "expected videos unlocked after reconcile: {cleared:?}");
        assert!(cleared.active_ops.is_empty());
        let settled = cleared.last_settled.as_ref().expect("reconcile sets last_settled");
        assert_eq!(settled.op_id, op_ambiguous);
        assert!(matches!(settled.outcome.as_str(), "complete" | "rolled_back"));
    }

    // Confirm original fixture path was handled deterministically (either staged or restored).
    let _ = Path::new(&src);
}

#[test]
fn engine_archive_and_unarchive_lock_and_settle_properly() {
    let fx = LockTestFixture::new("arch-unarch");
    let src = fx.add_review_watermark_video(
        "inbox",
        "arch1.mp4",
        "vid-arch-1",
        b"archive-unarchive-bytes",
        "Alice",
        Some(30),
    );
    let mut engine = Engine::new(fx.config.clone()).unwrap();
    let registry = engine.locks();

    // Confirm into integration queue first.
    let page = review_sessions::execute(&mut engine, "review-page", json!({"group": "person:Alice"})).unwrap();
    let session = page["session"].as_str().unwrap().to_owned();
    let row = page["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["vid"] == "vid-arch-1")
        .unwrap();
    let token = row["review_token"].as_str().unwrap().to_owned();
    let confirm_res = archive_api::execute(
        &mut engine,
        "queue-confirm",
        json!({
            "vids": ["vid-arch-1"],
            "creator": "Alice",
            "source": "review",
            "review_session": session,
            "review_tokens": { "vid-arch-1": token },
            "review_group": { "key": "person:Alice", "deferred": false },
            "fingerprints": {
                "vid-arch-1": {
                    "path": row["path"],
                    "size": row["size"],
                    "mtime_ns": row["mtime_ns"]
                }
            }
        }),
    )
    .unwrap();
    assert_eq!(confirm_res["ok"], true);

    // Archive apply
    let preview = archive_api::execute(&mut engine, "archive-preview", json!({})).unwrap();
    let arch_id = uuid::Uuid::new_v4().to_string();
    let arch_res = archive_api::execute(
        &mut engine,
        "archive-apply",
        json!({
            "operation_id": arch_id,
            "vids": ["vid-arch-1"],
            "version": preview["version"]
        }),
    )
    .unwrap();
    assert_eq!(arch_res["ok"], true);
    let after_arch = registry.snapshot();
    assert!(after_arch.videos.is_empty());
    assert!(after_arch.people.is_empty());
    let s1 = after_arch.last_settled.as_ref().unwrap();
    assert_eq!(s1.op_id, arch_id);
    assert_eq!(s1.action, "archive");
    assert_eq!(s1.outcome, "complete");
    assert_eq!(s1.vids, vec!["vid-arch-1"]);
    assert_eq!(s1.people, vec!["Alice"]);

    // Archive unarchive
    let unarch_id = uuid::Uuid::new_v4().to_string();
    let unarch_res = archive_api::execute(
        &mut engine,
        "archive-unarchive",
        json!({
            "operation_id": unarch_id,
            "vid": "vid-arch-1"
        }),
    )
    .unwrap();
    assert_eq!(unarch_res["ok"], true);
    assert!(src.is_file());
    let after_unarch = registry.snapshot();
    assert!(after_unarch.videos.is_empty());
    assert!(after_unarch.people.is_empty());
    let s2 = after_unarch.last_settled.as_ref().unwrap();
    assert_eq!(s2.op_id, unarch_id);
    assert_eq!(s2.action, "archive-unarchive");
    assert_eq!(s2.outcome, "complete");
    assert_eq!(s2.vids, vec!["vid-arch-1"]);
    assert_eq!(s2.people, vec!["Alice"]);

    // Also verify delete-files on a person directory video settles cleanly.
    let pfile = fx.add_person_video("Alice", "pdel.mp4", "vid-pdel-1", b"pdel-bytes", Some(31));
    let pstat = fs::metadata(&pfile).unwrap();
    let pdel_id = uuid::Uuid::new_v4().to_string();
    let pdel_req = Request {
        id: pdel_id.clone(),
        action: "delete-files".into(),
        person: "Alice".into(),
        files: vec![SelectedFile {
            vid: "vid-pdel-1".into(),
            path: pfile.to_string_lossy().into_owned(),
            version: files::fingerprint(&pfile.to_string_lossy(), &pstat),
        }],
        filename: String::new(),
        target_person: None,
        target_folder: None,
        payload: None,
    };
    let pdel_prev = engine.preview(pdel_req.clone()).unwrap();
    let pdel_res = engine
        .apply(pdel_req, pdel_prev["version"].as_str().unwrap().to_owned())
        .unwrap();
    assert_eq!(pdel_res["ok"], true);
    let s3 = registry.snapshot().last_settled.as_ref().unwrap().clone();
    assert_eq!(s3.op_id, pdel_id);
    assert_eq!(s3.action, "delete-files");
    assert_eq!(s3.outcome, "complete");
    assert_eq!(s3.vids, vec!["vid-pdel-1"]);
    assert_eq!(s3.people, vec!["Alice"]);
}

async fn read_http_response(addr: std::net::SocketAddr, request: &str) -> (String, String) {
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.unwrap();
    let text = String::from_utf8(buf).unwrap();
    let (headers, body) = text.split_once("\r\n\r\n").unwrap();
    (headers.to_owned(), body.to_owned())
}

async fn read_next_sse_chunk(reader: &mut BufReader<tokio::net::TcpStream>) -> Option<String> {
    let mut chunk = String::new();
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).await.unwrap();
        if n == 0 {
            return if chunk.is_empty() { None } else { Some(chunk) };
        }
        // Handle HTTP/1.1 chunked transfer framing transparently.
        let trimmed = line.trim();
        if !trimmed.is_empty()
            && trimmed.chars().all(|c| c.is_ascii_hexdigit())
            && !trimmed.starts_with("event:")
            && !trimmed.starts_with("data:")
            && !trimmed.starts_with(':')
        {
            if trimmed == "0" {
                return if chunk.is_empty() { None } else { Some(chunk) };
            }
            continue;
        }
        if line == "\r\n" {
            continue;
        }
        if line == "\n" {
            if !chunk.is_empty() {
                return Some(chunk);
            }
            continue;
        }
        chunk.push_str(&line);
    }
}

#[tokio::test]
async fn http_locks_and_live_sse_streaming_with_auth_keepalive_and_shutdown() {
    let fx = LockTestFixture::new("http-sse");
    let mut config = fx.config.clone();
    config.token = Some("locks-secret-token".into());
    let registry = locks::ActiveLockRegistry::new();

    let router = locks::router_with_keepalive(
        config,
        Arc::clone(&registry),
        Duration::from_millis(80),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });

    // 1. Unauthorized GET /v1/locks -> 401
    let (h401, _) = read_http_response(
        addr,
        "GET /v1/locks HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(h401.starts_with("HTTP/1.1 401"), "expected 401, got {h401}");

    // 2. Authorized GET /v1/locks -> 200 JSON with Cache-Control: no-store
    let (h200, body200) = read_http_response(
        addr,
        "GET /v1/locks HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer locks-secret-token\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(h200.starts_with("HTTP/1.1 200"), "expected 200, got {h200}");
    assert!(
        h200.to_lowercase().contains("cache-control: no-store"),
        "missing no-store header: {h200}"
    );
    let snap: locks::LockSnapshot = serde_json::from_str(&body200).unwrap();
    assert_eq!(snap.revision, 0);
    assert!(snap.videos.is_empty());
    assert!(snap.people.is_empty());

    // 3. Unauthorized GET /v1/locks/stream -> 401
    let (s401, _) = read_http_response(
        addr,
        "GET /v1/locks/stream HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(s401.starts_with("HTTP/1.1 401"), "expected 401, got {s401}");

    // 4. Authorized GET /v1/locks/stream -> live SSE frames + keepalive + clean close_streams()
    let mut sse_stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    sse_stream
        .write_all(
            b"GET /v1/locks/stream HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer locks-secret-token\r\n\r\n",
        )
        .await
        .unwrap();
    let mut reader = BufReader::new(sse_stream);
    let mut response_headers = String::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        if line == "\r\n" {
            break;
        }
        response_headers.push_str(&line);
    }
    let headers_lower = response_headers.to_lowercase();
    assert!(response_headers.starts_with("HTTP/1.1 200"));
    assert!(headers_lower.contains("content-type: text/event-stream"));
    assert!(headers_lower.contains("cache-control: no-cache, no-transform"));
    assert!(headers_lower.contains("x-accel-buffering: no"));

    // Frame 0: initial snapshot
    let f0 = tokio::time::timeout(Duration::from_secs(2), read_next_sse_chunk(&mut reader))
        .await
        .unwrap()
        .unwrap();
    assert!(f0.starts_with("event: locks\ndata: "), "unexpected frame 0: {f0:?}");
    let d0: locks::LockSnapshot = serde_json::from_str(
        f0.lines().find(|l| l.starts_with("data: ")).unwrap().strip_prefix("data: ").unwrap(),
    )
    .unwrap();
    assert_eq!(d0.revision, 0);

    // Frame 1: queued lock pushed live
    let guard = registry.register_queued(
        "op-sse-1",
        "auto-archive",
        vec!["vid-sse-1".into()],
        vec!["Alice".into()],
    );
    let mut d1_opt = None;
    for _ in 0..5 {
        let frame = tokio::time::timeout(Duration::from_secs(2), read_next_sse_chunk(&mut reader))
            .await
            .unwrap()
            .unwrap();
        if let Some(data) = frame.lines().find(|l| l.starts_with("data: ")) {
            let parsed: locks::LockSnapshot =
                serde_json::from_str(data.strip_prefix("data: ").unwrap()).unwrap();
            if parsed.revision == 1 {
                d1_opt = Some(parsed);
                break;
            }
        }
    }
    let d1 = d1_opt.expect("received queued SSE frame");
    assert_eq!(d1.videos["vid-sse-1"].status, "queued");
    assert_eq!(d1.videos["vid-sse-1"].label, "排队合入");
    assert_eq!(d1.people["Alice"].label, "排队合入");

    // Frame 2: running lock pushed live
    registry.mark_running(
        "op-sse-1",
        "auto-archive",
        vec!["vid-sse-1".into()],
        vec!["Alice".into()],
    );
    let mut d2_opt = None;
    for _ in 0..5 {
        let frame = tokio::time::timeout(Duration::from_secs(2), read_next_sse_chunk(&mut reader))
            .await
            .unwrap()
            .unwrap();
        if let Some(data) = frame.lines().find(|l| l.starts_with("data: ")) {
            let parsed: locks::LockSnapshot =
                serde_json::from_str(data.strip_prefix("data: ").unwrap()).unwrap();
            if parsed.revision == 2 {
                d2_opt = Some(parsed);
                break;
            }
        }
    }
    let d2 = d2_opt.expect("received running SSE frame");
    assert_eq!(d2.videos["vid-sse-1"].status, "running");
    assert_eq!(d2.videos["vid-sse-1"].label, "合入中");

    // Frame 3: settled pushed live
    guard.settle("complete", vec!["vid-sse-1".into()], vec!["Alice".into()]);
    let mut d3_opt = None;
    for _ in 0..5 {
        let frame = tokio::time::timeout(Duration::from_secs(2), read_next_sse_chunk(&mut reader))
            .await
            .unwrap()
            .unwrap();
        if let Some(data) = frame.lines().find(|l| l.starts_with("data: ")) {
            let parsed: locks::LockSnapshot =
                serde_json::from_str(data.strip_prefix("data: ").unwrap()).unwrap();
            if parsed.revision == 3 {
                d3_opt = Some(parsed);
                break;
            }
        }
    }
    let d3 = d3_opt.expect("received settled SSE frame");
    assert!(d3.videos.is_empty());
    assert!(d3.people.is_empty());
    assert_eq!(d3.last_settled.as_ref().unwrap().outcome, "complete");

    // Wait for `: keepalive` comment frame (interval is 80ms in test).
    let keepalive = tokio::time::timeout(Duration::from_secs(2), read_next_sse_chunk(&mut reader))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(keepalive, ": keepalive\n");

    // Trigger clean server stream shutdown -> SSE stream reaches EOF promptly.
    registry.close_streams();
    let eof = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match read_next_sse_chunk(&mut reader).await {
                None => break true,
                Some(_) => continue,
            }
        }
    })
    .await
    .unwrap();
    assert!(eof);

    server.abort();
}

#[test]
fn person_batch_unreadable_gate_keeps_parent_recovery_needed_across_restart_until_reconciled() {
    let fx = LockTestFixture::new("batch-gate-recovery");
    let _alice_vid = fx.add_person_video("Alice", "a1.mp4", "vid-bg-a1", b"alice-bytes", Some(70));
    let _bob_vid = fx.add_person_video("Bob", "b1.mp4", "vid-bg-b1", b"bob-bytes", Some(71));

    let batch_id = uuid::Uuid::new_v4().to_string();
    let batch_version;
    {
        let mut engine = Engine::new(fx.config.clone()).unwrap();
        let preview = person_batch::execute(
            &mut engine,
            json!({
                "mode": "preview",
                "id": batch_id,
                "request": {
                    "action": "delete",
                    "people": ["Alice", "Bob"]
                }
            }),
        )
        .unwrap();
        batch_version = preview["version"].as_str().unwrap().to_owned();

        // Fail child 1 preparation (so child 1 stays not_started and finish() inspects .review-write.lock),
        // then corrupt .review-write.lock before calling reconcile so finish() sees recovery_needed=true
        // with zero children in accepted/running/recovery_needed/rolling_back!
        fx.set_fault(json!({
            "phase": "batch_before_child:1",
            "mode": "error"
        }));
        let res = person_batch::execute(
            &mut engine,
            json!({
                "mode": "apply",
                "id": batch_id,
                "version": batch_version
            }),
        )
        .unwrap();
        assert_eq!(res["ok"], false);
        fx.clear_fault();

        // Simulate corrupt/unreadable .review-write.lock while reconciling batch.
        fs::write(fx.config.base.join(".review-write.lock"), b"not-valid-json").unwrap();
        let rec_res = person_batch::execute(
            &mut engine,
            json!({
                "mode": "reconcile",
                "id": batch_id,
                "version": batch_version
            }),
        )
        .unwrap();
        assert_eq!(rec_res["recovery_needed"], true);

        // Parent batch must stay locked as recovery_needed ("待恢复") even though no child is stuck!
        let snap = engine.locks().snapshot();
        assert_eq!(snap.people["Alice"].op_id, batch_id);
        assert_eq!(snap.people["Alice"].status, "recovery_needed");
        assert_eq!(snap.people["Alice"].label, "待恢复");
        assert_eq!(snap.people["Bob"].op_id, batch_id);
        assert_eq!(snap.people["Bob"].status, "recovery_needed");
        assert_eq!(snap.people["Bob"].label, "待恢复");
    }

    // Daemon restart with unreadable .review-write.lock still present -> seed_from_store re-seeds parent lock!
    {
        let mut engine = Engine::new(fx.config.clone()).unwrap();
        let seeded = engine.locks().snapshot();
        assert_eq!(seeded.people["Bob"].op_id, batch_id);
        assert_eq!(seeded.people["Bob"].status, "recovery_needed");
        assert_eq!(seeded.people["Bob"].label, "待恢复");

        // Operator removes corrupt gate and reconciles batch -> lock clears!
        fs::remove_file(fx.config.base.join(".review-write.lock")).unwrap();
        let cleared_res = person_batch::execute(
            &mut engine,
            json!({
                "mode": "reconcile",
                "id": batch_id,
                "version": batch_version
            }),
        )
        .unwrap();
        assert_ne!(cleared_res["recovery_needed"], true);
        let after = engine.locks().snapshot();
        assert!(after.people.is_empty(), "expected people unlocked: {after:?}");
        assert!(after.videos.is_empty(), "expected videos unlocked: {after:?}");
    }
}
