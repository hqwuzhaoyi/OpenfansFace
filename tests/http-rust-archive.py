"""#5 native confirmation/undo/archive contract against isolated real files and SQLite.
Python seeds and asserts only. Rust service runs with PATH=/no-executables.
"""

import argparse, concurrent.futures, http.client, importlib.util, json, os
from pathlib import Path
import signal, sqlite3, subprocess, tempfile, time, urllib.error, urllib.request, uuid

REPO = Path(__file__).resolve().parents[1]
p = argparse.ArgumentParser()
p.add_argument(
    "--binary",
    type=Path,
    default=REPO / "services/file-operations/target/debug/face-file-operations",
)
p.add_argument("--port", type=int, default=18981)
args = p.parse_args()
SCHEMA = (REPO / "tests/fixtures/peach-contract-schema.sql").read_text()
spec = importlib.util.spec_from_file_location(
    "processing_store", REPO / "maintenance/processing_store.py"
)
processing = importlib.util.module_from_spec(spec)
spec.loader.exec_module(processing)
BYTES = b"Rust archive isolated bytes\0\xff" * 2048


def write(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, ensure_ascii=False))


class Fixture:
    def __init__(self, count=2):
        self.root = Path(
            tempfile.mkdtemp(prefix="face-admin-person-fileops-archive-", dir="/tmp")
        ).resolve()
        (self.root / ".fileops-fixture").write_text("person file operations")
        self.base = self.root / "base"
        self.media = self.root / "media"
        self.base.mkdir()
        self.creators = self.media / "pikpak/creators"
        self.creators.mkdir(parents=True)
        self.ledger = self.root / "ledger.db"
        self.sources = []
        self.ids = []
        self.process = None
        with sqlite3.connect(self.base / "faces.db") as db:
            db.executescript(
                "CREATE TABLE videos(vid TEXT PRIMARY KEY,path TEXT,name TEXT,collection TEXT);CREATE TABLE faces(vid TEXT,thumb TEXT,score REAL);"
            )
        with sqlite3.connect(self.ledger) as db:
            db.executescript(SCHEMA)
            db.execute(
                "INSERT INTO entity(id,kind,canonical_name,normalized_name,created_at,updated_at) VALUES(7,'creator','momoyih','momoyih','fixture','fixture')"
            )
        with processing.connect(self.base) as db:
            pass
        for i in range(count):
            self.add(i)
        self.env = {
            **os.environ,
            "FACES_BASE": str(self.base),
            "MEDIA2_ROOT": str(self.media),
            "PEACH_LEDGER_PATH": str(self.ledger),
            "FILE_OPERATIONS_BIND": f"127.0.0.1:{args.port}",
            "FILE_OPERATIONS_FIXTURE_ROOT": str(self.root),
            "FILE_OPERATIONS_RUST_TOKEN": "archive-fixture",
            "PATH": "/no-executables",
        }
        self.url = f"http://127.0.0.1:{args.port}"
        self.start()

    def add(self, i):
        source = self.media / f"pikpak/My Telegram/folder-{i}/video-{i}.mp4"
        source.parent.mkdir(parents=True, exist_ok=True)
        source.write_bytes(BYTES + str(i).encode())
        vid = self.win(source)
        self.sources.append(source)
        self.ids.append(vid)
        with sqlite3.connect(self.base / "faces.db") as db:
            db.execute(
                "INSERT INTO videos VALUES(?,?,?,?)",
                (vid, str(source), source.name, str(source.parent)),
            )
        with sqlite3.connect(self.ledger) as db:
            db.execute(
                "INSERT INTO asset(id,location,path,name) VALUES(?,?,?,?)",
                (i + 1, "fixture", vid, "original ledger title"),
            )
        with processing.connect(self.base) as db:
            db.execute(
                "INSERT INTO tasks(vid,path,name,size,mtime_ns,stage,batch_id,created_at,updated_at,data,note) VALUES(?,?,?,?,?,'review','fixture',0,0,?,'preserved note')",
                (
                    vid,
                    str(source),
                    source.name,
                    source.stat().st_size,
                    str(source.stat().st_mtime_ns),
                    json.dumps(
                        {
                            "ready": True,
                            "kind": "suggested",
                            "suggested": "Momoyih",
                            "watermark": {
                                "candidates": [
                                    {
                                        "key": "X:fixture",
                                        "kind": "account",
                                        "platform": "X",
                                        "text": "@fixture",
                                        "token": "fixture",
                                    }
                                ]
                            },
                        }
                    ),
                ),
            )

    def win(self, path):
        return "R:\\media\\pikpak\\" + str(
            path.relative_to(self.media / "pikpak")
        ).replace("/", "\\")

    def start(self):
        self.log = (self.root / "service.log").open("ab")
        self.process = subprocess.Popen(
            [str(args.binary.resolve())], env=self.env, stdout=self.log, stderr=self.log
        )
        for _ in range(400):
            if self.process.poll() is not None:
                raise AssertionError(
                    (self.process.returncode, (self.root / "service.log").read_text())
                )
            try:
                urllib.request.urlopen(self.url + "/health", timeout=0.2).close()
                return
            except OSError:
                time.sleep(0.02)
        raise AssertionError("start timeout")

    def stop(self):
        if self.process and self.process.poll() is None:
            self.process.send_signal(signal.SIGINT)
            self.process.wait(timeout=10)
        self.log.close()

    def call(self, command=None, body=None, status=200, id=None):
        route = "/v1/archive" + (("?id=" + id) if id else "")
        data = (
            None
            if command is None
            else json.dumps({"command": command, "body": body or {}}).encode()
        )
        req = urllib.request.Request(
            self.url + route,
            data=data,
            headers={
                "Content-Type": "application/json",
                "Authorization": "Bearer archive-fixture",
            },
        )
        try:
            with urllib.request.urlopen(req, timeout=20) as r:
                code, result = r.status, json.load(r)
        except urllib.error.HTTPError as e:
            code, result = e.code, json.load(e)
        assert code == status, (code, status, result, self.root)
        return result

    def confirm(self, ids=None, status=200, **extra):
        ids = ids or self.ids
        fingerprints = {
            vid: {
                "path": str(self.sources[self.ids.index(vid)]),
                "size": self.sources[self.ids.index(vid)].stat().st_size,
                "mtime_ns": str(self.sources[self.ids.index(vid)].stat().st_mtime_ns),
            }
            for vid in ids
        }
        return self.call(
            "queue-confirm",
            {
                "creator": "Momoyih",
                "vids": ids,
                "source": "review",
                "fingerprints": fingerprints,
                "review_group": {"key": "person:Momoyih", "deferred": False},
                **extra,
            },
            status=status,
        )

    def preview(self):
        return self.call("archive-preview")

    def apply(self, preview=None, ids=None, op=None):
        return self.call(
            "archive-apply",
            {
                "vids": ids or self.ids,
                "version": (preview or self.preview())["version"],
                "operation_id": op or str(uuid.uuid4()),
            },
        )

    def queue(self):
        return (
            json.loads((self.base / "integration-queue.json").read_text())
            if (self.base / "integration-queue.json").exists()
            else []
        )

    def fault(self, phase, mode="pause", **extra):
        write(self.root / "rust-fault.json", {"phase": phase, "mode": mode, **extra})


def confirmation_undo():
    f = Fixture()
    try:
        op = str(uuid.uuid4())
        assert f.confirm(operation_id=op)["added"] == 2
        assert f.confirm(operation_id=op)["added"] == 2
        assert all(p.exists() for p in f.sources)
        q = f.queue()
        assert (
            q[0]["source"] == "review"
            and q[0]["return_href"] == "/review"
            and q[0]["note"] == "preserved note"
        )
        assert f.call("queue-confirm", {"creator": "Other", "vids": f.ids}, status=409)[
            "error"
        ]
        with processing.connect(f.base) as db:
            db.execute(
                "UPDATE tasks SET stage='excluded',deferred=1 WHERE vid=?", (f.ids[0],)
            )
        undone = f.call("queue-undo", {"vids": [f.ids[0]]})
        assert undone["removed"][0]["return_href"] == "/review"
        assert len(f.queue()) == 1
        with processing.connect(f.base) as db:
            assert tuple(
                db.execute(
                    "SELECT stage,deferred,note FROM tasks WHERE vid=?", (f.ids[0],)
                ).fetchone()
            ) == ("review", 0, "preserved note")
        f.confirm([f.ids[0]], watermark_key="X:fixture")
        assert (
            json.loads((f.base / "watermarks/mappings.json").read_text())["X:fixture"][
                "creator"
            ]
            == "Momoyih"
        )
        with processing.connect(f.base) as db:
            assert [
                tuple(r) for r in db.execute("SELECT DISTINCT stage FROM tasks")
            ] == [("queued",)]
        assert all(
            p.read_bytes() == BYTES + str(i).encode() for i, p in enumerate(f.sources)
        )
        print(
            "PASS review queue, idempotent receipt, provenance, undo, optional rule, no media moves"
        )
    finally:
        f.stop()


def ordinary_archive():
    f = Fixture()
    try:
        f.confirm()
        preview = f.preview()
        op = str(uuid.uuid4())
        result = f.apply(preview, op=op)
        assert result["ok"] and result["moved"] == 2, result
        assert f.apply(preview, op=op) == result
        assert not f.queue()
        history = json.loads((f.base / "archive-history.json").read_text())
        assert len(history) == 2
        for i, source in enumerate(f.sources):
            dst = f.creators / "Momoyih" / source.name
            assert not source.exists() and dst.read_bytes() == BYTES + str(i).encode()
            with sqlite3.connect(f.base / "faces.db") as db:
                assert db.execute(
                    "SELECT path FROM videos WHERE vid=?", (f.ids[i],)
                ).fetchone()[0] == str(dst)
            with sqlite3.connect(f.ledger) as db:
                assert db.execute(
                    "SELECT path,name FROM asset WHERE id=?", (i + 1,)
                ).fetchone() == (f.win(dst), "original ledger title")
        with sqlite3.connect(f.ledger) as db:
            assert db.execute(
                "SELECT id FROM entity WHERE kind='creator'"
            ).fetchall() == [(7,)]
            assert db.execute(
                "SELECT asset_id,entity_id,role FROM asset_entity ORDER BY asset_id"
            ).fetchall() == [(1, 7, "creator"), (2, 7, "creator")]
            assert not db.execute("PRAGMA foreign_key_check").fetchall()
        assert f.call(id=op)["result"] == result
        assert f.call()["job"]["id"] == op
        print(
            "PASS native archive full bytes, stable IDs, normalized creator reuse, receipts, progress"
        )
    finally:
        f.stop()


def preflight():
    for fault in ("missing", "duplicate", "ambiguous", "fingerprint", "group"):
        f = Fixture(1)
        try:
            if fault == "fingerprint":
                f.call(
                    "queue-confirm",
                    {
                        "creator": "A",
                        "vids": f.ids,
                        "source": "review",
                        "fingerprints": {},
                    },
                    status=409,
                )
                assert not f.queue()
                continue
            if fault == "group":
                f.confirm(review_group={"key": "none", "deferred": False}, status=409)
                assert not f.queue()
                continue
            f.confirm()
            with sqlite3.connect(f.ledger) as db:
                if fault == "missing":
                    db.execute("DELETE FROM asset")
                if fault == "duplicate":
                    db.execute(
                        "INSERT INTO asset(location,path) VALUES(?,?)",
                        ("second", f.ids[0]),
                    )
                if fault == "ambiguous":
                    db.execute(
                        "INSERT INTO entity(kind,canonical_name,normalized_name,created_at,updated_at) VALUES('creator','Momoyih','different','fixture','fixture')"
                    )
            p = f.preview()
            f.call(
                "archive-apply", {"vids": f.ids, "version": p["version"]}, status=409
            )
            assert f.sources[0].exists() and len(f.queue()) == 1
        finally:
            f.stop()
    print("PASS missing/duplicate assets and creator ambiguity blocked before move")


def ledger_failure():
    f = Fixture()
    try:
        f.confirm()
        preview = f.preview()
        with sqlite3.connect(f.ledger) as db:
            db.execute(
                "CREATE TRIGGER archive_failure BEFORE UPDATE OF path ON asset BEGIN SELECT RAISE(ABORT,'native fixture ledger failure'); END"
            )
        result = f.apply(preview)
        assert not result["ok"] and result["rolled_back"], result
        assert len(f.queue()) == 2 and all(path.exists() for path in f.sources)
        for i, path in enumerate(f.sources):
            assert path.read_bytes() == BYTES + str(i).encode()
            assert not (f.creators / "Momoyih" / path.name).exists()
        with sqlite3.connect(f.ledger) as db:
            assert db.execute("SELECT path FROM asset ORDER BY id").fetchall() == [
                (x,) for x in f.ids
            ]
        with sqlite3.connect(f.base / "faces.db") as db:
            assert db.execute("SELECT path FROM videos ORDER BY path").fetchall() == [
                (str(x),) for x in f.sources
            ]
        assert not (f.base / ".review-write.lock").exists()
        print(
            "PASS actual native ledger failure rolls whole successful-file subset back"
        )
    finally:
        f.stop()


def wait_fault(f):
    for _ in range(300):
        if (f.root / "rust-fault-reached.json").exists():
            return
        time.sleep(0.01)
    raise AssertionError(("fault not reached", f.root))


def partial_and_concurrent():
    f = Fixture(3)
    try:
        f.confirm(f.ids[:2])
        preview = f.preview()
        op = str(uuid.uuid4())
        f.fault("destination_publish_intent:0", milliseconds=900)
        with concurrent.futures.ThreadPoolExecutor(max_workers=3) as pool:
            apply = pool.submit(f.apply, preview, f.ids[:2], op)
            wait_fault(f)
            progress = f.call(id=op)
            assert (
                progress["job"]["stage"] == "移动文件" and progress["job"]["current"]
            ), progress
            late = f.creators / "Momoyih" / f.sources[1].name
            late.parent.mkdir(parents=True, exist_ok=True)
            late.write_bytes(b"external late target")
            duplicate = pool.submit(f.apply, preview, f.ids[:2], op)
            confirmation = pool.submit(f.confirm, [f.ids[2]])
            result = apply.result()
            assert duplicate.result() == result
            confirmed = confirmation.result()
        assert not result["ok"] and result["moved"] == 1, result
        assert confirmed["added"] == 1 and {r["vid"] for r in f.queue()} == set(
            f.ids[1:]
        )
        assert (
            late.read_bytes() == b"external late target"
            and f.sources[1].exists()
            and f.sources[2].exists()
        )
        assert not f.sources[0].exists()
        with sqlite3.connect(f.ledger) as db:
            assert db.execute("SELECT path FROM asset ORDER BY id").fetchall() == [
                (f.win(f.creators / "Momoyih" / f.sources[0].name),),
                (f.ids[1],),
                (f.ids[2],),
            ]
        assert f.apply(preview, f.ids[:2], op) == result
        print(
            "PASS successful subset, late target safety, serialized unrelated confirmation, duplicate apply"
        )
    finally:
        f.stop()


def optional_failure():
    for action in ("rule", "undo"):
        f = Fixture(1)
        try:
            if action == "undo":
                f.confirm()
                with processing.connect(f.base) as db:
                    db.execute(
                        "CREATE TRIGGER restore_failure BEFORE UPDATE ON tasks BEGIN SELECT RAISE(ABORT,'native restore fixture failure'); END"
                    )
                result = f.call(
                    "queue-undo", {"vids": f.ids, "operation_id": str(uuid.uuid4())}
                )
                assert result["ok"] and result["warning"] and not f.queue(), result
            else:
                with processing.connect(f.base) as db:
                    db.execute(
                        "CREATE TRIGGER rule_failure BEFORE UPDATE OF stage ON tasks BEGIN SELECT RAISE(ABORT,'native rule fixture failure'); END"
                    )
                op = str(uuid.uuid4())
                result = f.confirm(operation_id=op, watermark_key="X:fixture")
                assert (
                    result["ok"] and result["rule_warning"] and len(f.queue()) == 1
                ), result
                assert f.confirm(operation_id=op, watermark_key="X:fixture") == result
            assert f.sources[0].exists()
            assert not (f.base / ".review-write.lock").exists()
        finally:
            f.stop()
    print(
        "PASS optional native rule/restore transaction failure retains human decision and durable warning"
    )


def new_creator():
    f = Fixture(2)
    try:
        f.confirm(creator="新人物")
        p = f.preview()
        # An unrelated creator inserted after preview must not consume a predicted MAX(id)+1.
        with sqlite3.connect(f.ledger) as db:
            db.execute(
                "INSERT INTO entity(kind,canonical_name,normalized_name,created_at,updated_at) VALUES('creator','unrelated','unrelated','fixture','fixture')"
            )
        r = f.apply(p)
        assert r["ok"] and r["moved"] == 2, r
        with sqlite3.connect(f.ledger) as db:
            entities = db.execute(
                "SELECT id FROM entity WHERE canonical_name='新人物'"
            ).fetchall()
            assert len(entities) == 1
            assert db.execute(
                "SELECT entity_id FROM asset_entity ORDER BY asset_id"
            ).fetchall() == [entities[0], entities[0]]
        print(
            "PASS new native creator allocated once under actual uniqueness/FK constraints"
        )
    finally:
        f.stop()


def restart_optional():
    for phase in ("optional_parent_committed", "optional_rule_committed"):
        f = Fixture(1)
        try:
            op = str(uuid.uuid4())
            f.fault(phase, "exit")
            try:
                f.confirm(operation_id=op, watermark_key="X:fixture")
            except (OSError, http.client.RemoteDisconnected):
                pass
            f.process.wait(timeout=10)
            assert f.process.returncode == 86
            assert len(f.queue()) == 1
            f.stop()
            (f.root / "rust-fault.json").unlink()
            f.start()
            result = f.confirm(operation_id=op, watermark_key="X:fixture")
            assert result["ok"] and "rule_warning" not in result, result
            assert f.confirm(operation_id=op, watermark_key="X:fixture") == result
            with sqlite3.connect(
                f.base / "file-operations-rust/operations.sqlite3"
            ) as db:
                assert (
                    db.execute(
                        "SELECT COUNT(*) FROM operations WHERE json_extract(plan,'$.action')='queue-rule'"
                    ).fetchone()[0]
                    == 1
                )
            assert f.sources[0].exists()
            print("PASS optional receipt restart", phase)
        finally:
            f.stop()


def archive_restart():
    for phase in (
        "source_removed:1",
        "successful_file_subset",
        "commit_scope",
        "ledger_committed_unrecorded",
        "final_receipt_unreleased",
    ):
        f = Fixture(2)
        try:
            f.confirm()
            preview = f.preview()
            op = str(uuid.uuid4())
            f.fault(phase, "exit")
            try:
                f.apply(preview, op=op)
            except (OSError, http.client.RemoteDisconnected):
                pass
            f.process.wait(timeout=10)
            assert f.process.returncode == 86
            f.stop()
            (f.root / "rust-fault.json").unlink()
            f.start()
            progress = f.call(id=op)
            result = progress.get("result")
            assert result and not result.get("recovery_needed"), (phase, progress)
            if result["ok"]:
                assert result["moved"] == 2 and not f.queue()
                assert all(
                    (f.creators / "Momoyih" / p.name).exists() and not p.exists()
                    for p in f.sources
                )
                assert f.apply(preview, op=op) == result
            else:
                assert result["rolled_back"] and len(f.queue()) == 2
                assert all(
                    p.exists() and not (f.creators / "Momoyih" / p.name).exists()
                    for p in f.sources
                )
            for i, p in enumerate(f.sources):
                assert (
                    p if p.exists() else f.creators / "Momoyih" / p.name
                ).read_bytes() == BYTES + str(i).encode()
            assert not (f.base / ".review-write.lock").exists()
            print("PASS multi-file archive restart", phase)
        finally:
            f.stop()


def claim_and_fence():
    f = Fixture(1)
    try:
        write(f.base / "claims-detail.json", [{"vid": f.ids[0], "name": "Momoyih"}])
        assert (
            f.call("claim-decide", {"vid": f.ids[0], "approve": False})["bucket"]
            == "rejected"
        )
        assert f.call("claim-decide", {"vid": f.ids[0], "approve": True})["queued"]
        assert f.queue()[0]["source"] == "approval"
        undone = f.call("queue-undo", {"vids": f.ids})
        assert undone["removed"][0]["return_href"].startswith("/approvals?vid=")
        assert json.loads((f.base / "review-decisions.json").read_text()) == {
            "approved": [],
            "rejected": [],
        }
        f.confirm()
        preview = f.preview()
        op = str(uuid.uuid4())
        assert f.call("archive-reconcile", {"id": op})["result"]["moved"] == 0
        f.call(
            "archive-apply",
            {"vids": f.ids, "version": preview["version"], "operation_id": op},
            status=409,
        )
        assert f.sources[0].exists()
        print("PASS claims/undo provenance and never-started late-request fencing")
    finally:
        f.stop()


def subset_ledger_failure():
    f = Fixture(2)
    try:
        f.confirm()
        preview = f.preview()
        f.fault("destination_publish_intent:0", milliseconds=700)
        with sqlite3.connect(f.ledger) as db:
            db.execute(
                "CREATE TRIGGER subset_failure BEFORE UPDATE OF path ON asset BEGIN SELECT RAISE(ABORT,'subset ledger failure'); END"
            )
        with concurrent.futures.ThreadPoolExecutor() as pool:
            future = pool.submit(f.apply, preview)
            wait_fault(f)
            late = f.creators / "Momoyih" / f.sources[1].name
            late.write_bytes(b"late target bytes")
            result = future.result()
        assert not result["ok"] and result["rolled_back"], result
        assert len(f.queue()) == 2 and all(p.exists() for p in f.sources)
        assert (
            not (f.creators / "Momoyih" / f.sources[0].name).exists()
            and late.read_bytes() == b"late target bytes"
        )
        with sqlite3.connect(f.ledger) as db:
            assert db.execute("SELECT path FROM asset ORDER BY id").fetchall() == [
                (v,) for v in f.ids
            ]
        print(
            "PASS ledger failure compensates only successful subset while preserving late target"
        )
    finally:
        f.stop()


def preview_scope():
    for kind in ("faces", "tasks", "ledger", "unrelated"):
        f = Fixture(2)
        try:
            f.confirm([f.ids[0]])
            preview = f.preview()
            if kind == "faces":
                with sqlite3.connect(f.base / "faces.db") as db:
                    db.execute(
                        "UPDATE videos SET name='changed' WHERE vid=?", (f.ids[0],)
                    )
            if kind == "tasks":
                with sqlite3.connect(f.base / "processing/processing.db") as db:
                    db.execute(
                        "UPDATE tasks SET note='new human note' WHERE vid=?",
                        (f.ids[0],),
                    )
            if kind == "ledger":
                with sqlite3.connect(f.ledger) as db:
                    db.execute("UPDATE asset SET name='new ledger title' WHERE id=1")
            if kind == "unrelated":
                with sqlite3.connect(f.base / "processing/processing.db") as db:
                    db.execute(
                        "UPDATE tasks SET updated_at=0.0021650249602265926,stage='face',data='{}' WHERE vid=?",
                        (f.ids[1],),
                    )
                assert f.apply(preview, [f.ids[0]])["ok"]
            else:
                f.call(
                    "archive-apply",
                    {"vids": [f.ids[0]], "version": preview["version"]},
                    status=409,
                )
                assert f.sources[0].exists()
        finally:
            f.stop()
    print(
        "PASS scoped preview versions reject related rows and tolerate unrelated analysis"
    )


def request_boundaries_and_late_rule_write():
    f = Fixture(1)
    try:
        assert (
            f.call("queue-confirm", {"creator": "Momoyih", "vids": f.ids * 5000})[
                "added"
            ]
            == 1
        )
        f.call(
            "queue-confirm", {"creator": "Momoyih", "vids": f.ids * 5001}, status=400
        )
        f.call("queue-undo", {"vids": f.ids})
        folder = f.base / "watermarks"
        folder.mkdir(exist_ok=True)
        f.fault("json_replace_intent:watermarks/mappings.json", milliseconds=700)
        op = str(uuid.uuid4())
        with concurrent.futures.ThreadPoolExecutor() as pool:
            future = pool.submit(f.confirm, operation_id=op, watermark_key="X:fixture")
            wait_fault(f)
            folder.chmod(0o555)
            try:
                result = future.result()
            finally:
                folder.chmod(0o755)
        assert result["ok"] and result["rule_warning"] and len(f.queue()) == 1, result
        assert (
            not (folder / "mappings.json").exists()
            and not (f.base / ".review-write.lock").exists()
        )
        with sqlite3.connect(f.base / "processing/processing.db") as db:
            assert db.execute("SELECT stage FROM tasks").fetchone()[0] == "review"
        assert f.confirm(operation_id=op, watermark_key="X:fixture") == result
        print(
            "PASS supported 5000-ID body, explicit overflow, late actual permission failure retains confirmation"
        )
    finally:
        f.stop()


def local_constraints_and_missing_ledger():
    for kind in ("videos", "tasks", "disposed-ledger", "missing-ledger"):
        f = Fixture(1)
        try:
            f.confirm()
            dst = f.creators / "Momoyih" / f.sources[0].name
            if kind == "videos":
                with sqlite3.connect(f.base / "faces.db") as db:
                    db.execute(
                        "INSERT INTO videos VALUES(?,?,?,?)",
                        ("foreign", str(dst), dst.name, str(dst.parent)),
                    )
            elif kind == "tasks":
                with sqlite3.connect(f.base / "processing/processing.db") as db:
                    db.execute(
                        "INSERT INTO tasks(vid,path,name,size,mtime_ns,batch_id,created_at,updated_at) VALUES('foreign',?,'foreign',1,'0','fixture',0,0)",
                        (str(dst),),
                    )
            elif kind == "disposed-ledger":
                with sqlite3.connect(f.ledger) as db:
                    db.execute(
                        "INSERT INTO asset(location,path,disposal) VALUES('fixture',?,'disposed')",
                        (f.win(dst),),
                    )
            else:
                f.ledger.unlink()
                f.call("archive-preview", status=503)
                assert f.call("queue-undo", {"vids": f.ids})["ok"]
                assert not f.queue()
                assert f.sources[0].exists()
                continue
            preview = f.preview()
            f.call(
                "archive-apply",
                {"vids": f.ids, "version": preview["version"]},
                status=409,
            )
            assert f.sources[0].exists() and not dst.exists() and len(f.queue()) == 1
        finally:
            f.stop()
    print(
        "PASS native local destination constraints, disposed asset conflict, missing ledger and local-only undo"
    )


if __name__ == "__main__":
    confirmation_undo()
    ordinary_archive()
    preflight()
    ledger_failure()
    partial_and_concurrent()
    optional_failure()
    new_creator()
    restart_optional()
    archive_restart()
    claim_and_fence()
    subset_ledger_failure()
    preview_scope()
    request_boundaries_and_late_rule_write()
    local_constraints_and_missing_ledger()
