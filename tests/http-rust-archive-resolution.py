"""#6 native archive resolution: real isolated bytes, SQLite, fault/restart boundaries.
Extends tests/http-archive-resolution.py semantics; no Python service execution fallback.
"""

import concurrent.futures
import importlib.util
import json
import os
from pathlib import Path
import sqlite3
import time
import uuid

spec = importlib.util.spec_from_file_location(
    "archive_fixture", Path(__file__).with_name("http-rust-archive.py")
)
n = importlib.util.module_from_spec(spec)
spec.loader.exec_module(n)


def canonical(f, i, content=None, ledger=True):
    path = f.creators / "Momoyih" / f.sources[i].name
    path.parent.mkdir(exist_ok=True)
    path.write_bytes(f.sources[i].read_bytes() if content is None else content)
    if ledger:
        with sqlite3.connect(f.ledger) as db:
            db.execute(
                "INSERT INTO asset(id,location,path,name) VALUES(?,?,?,?)",
                (100 + i, "fixture", f.win(path), "canonical preserved"),
            )
        with sqlite3.connect(f.base / "faces.db") as db:
            db.execute(
                "INSERT INTO videos VALUES(?,?,?,?)",
                (f"canonical:{i}", str(path), path.name, str(path.parent)),
            )
    return path


def resolve(f, kind, ids=None, status=200, **kw):
    return f.call(
        "archive-resolve",
        dict(vids=ids or f.ids, version=f.preview()["version"], kind=kind, **kw),
        status=status,
    )


def prepare(count=2):
    f = n.Fixture(count)
    f.confirm()
    targets = [canonical(f, i) for i in range(count)]
    return f, targets


def assert_staged(f, targets, result):
    assert result["ok"] and result["staged"] == len(targets), result
    origins = json.loads((f.base / "staging-origins.json").read_text())
    history = {row["vid"]: row for row in f.preview()["history"]}
    overrides = json.loads((f.base / "video-path-overrides.json").read_text())
    with sqlite3.connect(f.ledger) as db, sqlite3.connect(f.base / "faces.db") as faces:
        for i, target in enumerate(targets):
            staged = Path(history[f.ids[i]]["staged_path"])
            assert not f.sources[i].exists()
            assert (
                staged.read_bytes() == target.read_bytes() == n.BYTES + str(i).encode()
            )
            assert history[f.ids[i]]["win_dst"] == f.win(target)
            assert origins[str(staged)] == dict(
                operation=result["id"],
                action="stage-duplicate",
                original_path=str(f.sources[i]),
                person="",
                vids=[f.ids[i]],
                source="native-receipt",
            )
            assert overrides[f.ids[i]] == str(staged)
            assert faces.execute(
                "SELECT path FROM videos WHERE vid=?", (f.ids[i],)
            ).fetchone()[0] == str(staged)
            assert faces.execute(
                "SELECT path FROM videos WHERE vid=?", (f"canonical:{i}",)
            ).fetchone()[0] == str(target)
            assert db.execute("SELECT path FROM asset WHERE id=?", (i + 1,)).fetchone()[
                0
            ] == f.win(staged)
            assert db.execute(
                "SELECT path,name FROM asset WHERE id=?", (100 + i,)
            ).fetchone() == (f.win(target), "canonical preserved")
            assert (
                db.execute(
                    "SELECT COUNT(*) FROM asset_entity WHERE asset_id IN (?,?) AND entity_id=7 AND role='creator'",
                    (i + 1, 100 + i),
                ).fetchone()[0]
                == 2
            )


def main_cases():
    f, targets = prepare()
    try:
        old = f.preview()
        resolve(f, "rename", ids=f.ids[:1], name="../outside.mp4", status=400)
        resolve(f, "rename", ids=f.ids[:1], name="wrong.mkv", status=400)
        resolve(f, "rename", ids=f.ids[:1])
        p = f.preview()
        row = next(r for r in p["rows"] if r["vid"] == f.ids[0])
        assert row["status"] == "ok" and row["dst"] != str(targets[0])
        assert all(s.exists() for s in f.sources)
        resolve(f, "clear", ids=f.ids[:1])
        assert f.preview()["rows"][0]["status"] == "duplicate"
        resolve(f, "stage-duplicate")
        f.call("archive-apply", dict(vids=f.ids, version=old["version"]), status=409)
        result = f.apply()
        assert_staged(f, targets, result)
        assert f.call(id=result["id"])["result"] == result
    finally:
        f.stop()
    f, targets = prepare()
    try:
        targets[1].write_bytes(b"X" * f.sources[1].stat().st_size)
        resolve(f, "stage-duplicate")
        result = f.apply()
        assert not result["ok"] and result["moved"] == result["staged"] == 1, result
        assert (
            result["failures"][0]["vid"] == f.ids[1]
            and "校验不一致" in result["errors"][0]
        )
        assert f.sources[1].exists() and f.queue()[0]["vid"] == f.ids[1]
        assert f.preview()["n_ok"] == 1
    finally:
        f.stop()
    f, targets = prepare(1)
    try:
        resolve(f, "rename")
        p = f.preview()
        dst = Path(p["rows"][0]["dst"])
        r = f.apply(p)
        assert r["moved"] == 1 and r["staged"] == 0, r
        assert dst.read_bytes() == targets[0].read_bytes() and not f.sources[0].exists()
    finally:
        f.stop()
    print(
        "PASS safe resolution/clear, keep both, bulk exact duplicates, canonical metadata/links, partial content mismatch, stable results"
    )


def faults():
    for fault in [
        "canonical-missing",
        "target-change",
        "source-change",
        "canonical-symlink",
        "restored-mtime",
        "ledger",
    ]:
        f, targets = prepare()
        try:
            resolve(f, "stage-duplicate")
            p = f.preview()
            if fault == "canonical-missing":
                with sqlite3.connect(f.ledger) as db:
                    db.execute("DELETE FROM asset WHERE id=100")
                # Preview includes canonical ledger evidence, stale preview is rejected.
                f.call(
                    "archive-apply", dict(vids=f.ids, version=p["version"]), status=409
                )
            elif fault == "ledger":
                with sqlite3.connect(f.ledger) as db:
                    db.execute(
                        "CREATE TRIGGER fail_path BEFORE UPDATE OF path ON asset BEGIN SELECT RAISE(ABORT,'native ledger failure'); END"
                    )
                result = f.apply()
                assert result["rolled_back"] and result["moved"] == 0, result
                assert not (f.base / "staging-origins.json").exists()
            else:
                f.fault("duplicate_comparison_chunk", milliseconds=600)
                with concurrent.futures.ThreadPoolExecutor() as pool:
                    work = pool.submit(f.apply, p)
                    for _ in range(200):
                        if (f.root / "rust-fault-reached.json").exists():
                            break
                        time.sleep(0.01)
                    assert (f.root / "rust-fault-reached.json").exists()
                    if fault == "target-change":
                        targets[0].write_bytes(b"X" * targets[0].stat().st_size)
                    if fault == "restored-mtime":
                        old = targets[0].stat()
                        targets[0].write_bytes(b"X" * old.st_size)
                        os.utime(targets[0], ns=(old.st_atime_ns, old.st_mtime_ns))
                    if fault == "source-change":
                        f.sources[0].write_bytes(b"X" * f.sources[0].stat().st_size)
                    if fault == "canonical-symlink":
                        keep = f.root / "outside"
                        keep.write_bytes(targets[0].read_bytes())
                        targets[0].unlink()
                        targets[0].symlink_to(keep)
                    result = work.result()
                    assert not result["ok"] and result["moved"] == 1, result
            assert f.sources[0].exists()
        finally:
            f.stop()
    print(
        "PASS source/target changes during comparison, canonical symlink, missing canonical ledger, whole-subset ledger compensation"
    )


def races_and_aliases():
    for symlink in (False, True):
        f, targets = prepare()
        try:
            resolve(f, "stage-duplicate")
            p = f.preview()
            op = str(uuid.uuid4())
            with sqlite3.connect(f.ledger) as db:
                db.execute(
                    "CREATE TRIGGER fail_path BEFORE UPDATE OF path ON asset BEGIN SELECT RAISE(ABORT,'native ledger failure'); END"
                )
            f.fault("duplicate_verified:1", milliseconds=800)
            with concurrent.futures.ThreadPoolExecutor() as pool:
                work = pool.submit(f.apply, p, None, op)
                for _ in range(200):
                    if (f.root / "rust-fault-reached.json").exists():
                        break
                    time.sleep(0.01)
                assert (f.root / "rust-fault-reached.json").exists()
                with sqlite3.connect(
                    f.base / "file-operations-rust/operations.sqlite3"
                ) as db:
                    plan = json.loads(
                        db.execute(
                            "SELECT plan FROM operations WHERE id=?", (op,)
                        ).fetchone()[0]
                    )
                late = Path(plan["files"][1]["dst"])
                if symlink:
                    external = f.root / "external"
                    external.write_bytes(b"external retained")
                    late.symlink_to(external)
                else:
                    late.write_bytes(b"late external bytes")
                r = work.result()
                assert r["rolled_back"] and r["moved"] == 0, r
                assert all(p.exists() for p in f.sources)
                assert (
                    late.is_symlink()
                    if symlink
                    else late.read_bytes() == b"late external bytes"
                )
                assert not Path(plan["files"][0]["dst"]).exists()
                assert (
                    len(f.queue()) == 2
                    and not (f.base / "staging-origins.json").exists()
                )
        finally:
            f.stop()
    f, targets = prepare(1)
    try:
        with sqlite3.connect(f.base / "faces.db") as db:
            db.execute(
                "INSERT INTO videos VALUES(?,?,?,?)",
                (
                    "source-alias",
                    str(f.sources[0]),
                    f.sources[0].name,
                    str(f.sources[0].parent),
                ),
            )
        resolve(f, "stage-duplicate")
        r = f.apply()
        location = r["locations"][0]
        assert set(location["vids"]) == {f.ids[0], "source-alias"}
        with sqlite3.connect(f.base / "faces.db") as db:
            assert (
                db.execute(
                    "SELECT path FROM videos WHERE vid='source-alias'"
                ).fetchone()[0]
                == location["path"]
            )
            assert db.execute(
                "SELECT path FROM videos WHERE vid='canonical:0'"
            ).fetchone()[0] == str(targets[0])
    finally:
        f.stop()
    # Two physical duplicate sources can share one canonical target; its creator
    # association is inserted once even when the creator itself is newly allocated.
    f, targets = prepare(1)
    try:
        second = f.sources[0].parent.parent / "second" / f.sources[0].name
        second.parent.mkdir()
        second.write_bytes(f.sources[0].read_bytes())
        vid = f.win(second)
        with sqlite3.connect(f.base / "faces.db") as db:
            db.execute(
                "INSERT INTO videos VALUES(?,?,?,?)",
                (vid, str(second), second.name, str(second.parent)),
            )
        with sqlite3.connect(f.ledger) as db:
            db.execute(
                "INSERT INTO asset(id,location,path,name) VALUES(2,'fixture',?,'second')",
                (vid,),
            )
            db.execute("DELETE FROM entity WHERE id=7")
        f.call("queue-confirm", dict(creator="Momoyih", vids=[vid], source="all"))
        f.ids.append(vid)
        f.sources.append(second)
        resolve(f, "stage-duplicate")
        r = f.apply()
        assert r["ok"] and r["staged"] == 2, r
        with sqlite3.connect(f.ledger) as db:
            assert (
                db.execute(
                    "SELECT COUNT(*) FROM entity WHERE kind='creator'"
                ).fetchone()[0]
                == 1
            )
            assert (
                db.execute(
                    "SELECT COUNT(*) FROM asset_entity WHERE asset_id=100 AND role='creator'"
                ).fetchone()[0]
                == 1
            )
    finally:
        f.stop()
    # Canonical edits after movement but before aggregate ledger commit compensate
    # the complete successful subset, preserving the external canonical edit.
    f, targets = prepare()
    try:
        resolve(f, "stage-duplicate")
        f.fault("ledger_commit_intent", milliseconds=800)
        with concurrent.futures.ThreadPoolExecutor() as pool:
            work = pool.submit(f.apply)
            for _ in range(200):
                if (f.root / "rust-fault-reached.json").exists():
                    break
                time.sleep(0.01)
            assert (f.root / "rust-fault-reached.json").exists()
            targets[0].write_bytes(b"late canonical edit")
            r = work.result()
            assert r["rolled_back"] and r["moved"] == 0, r
            assert (
                all(p.exists() for p in f.sources)
                and targets[0].read_bytes() == b"late canonical edit"
            )
    finally:
        f.stop()
    print(
        "PASS late staged file/symlink plus subset ledger rollback, source aliases, shared canonical/new creator, late canonical edit"
    )


def late_keep_both():
    for symlink in (False, True):
        f, targets = prepare(1)
        try:
            resolve(f, "rename")
            p = f.preview()
            dst = Path(p["rows"][0]["dst"])
            f.fault("admitted", milliseconds=600)
            with concurrent.futures.ThreadPoolExecutor() as pool:
                work = pool.submit(f.apply, p)
                for _ in range(200):
                    if (f.root / "rust-fault-reached.json").exists():
                        break
                    time.sleep(0.01)
                assert (f.root / "rust-fault-reached.json").exists()
                if symlink:
                    dst.symlink_to(targets[0])
                else:
                    dst.write_bytes(b"late target")
                result = work.result()
                assert result["moved"] == 0 and not result["ok"], result
                assert (
                    f.sources[0].exists() and targets[0].read_bytes() == n.BYTES + b"0"
                )
                assert (
                    dst.is_symlink() if symlink else dst.read_bytes() == b"late target"
                )
                assert len(f.queue()) == 1
        finally:
            f.stop()
    print("PASS late keep-both target and symlink cannot be overwritten")


def special_target():
    f, targets = prepare(1)
    try:
        targets[0].unlink()
        os.mkfifo(targets[0])
        p = f.preview()
        assert p["rows"][0]["status"] == "conflict"
        resolve(f, "stage-duplicate", status=409)
        assert f.sources[0].exists()
    finally:
        f.stop()
    print("PASS non-regular canonical target is rejected without blocking the worker")


def full_content_tail():
    for identical in (False, True):
        f = n.Fixture(1)
        try:
            content = b"same prefix" * (300_000) + b"last byte A"
            f.sources[0].write_bytes(content)
            target = canonical(
                f, 0, content=content if identical else content[:-1] + b"B"
            )
            f.call("queue-confirm", dict(creator="Momoyih", vids=f.ids, source="all"))
            resolve(f, "stage-duplicate")
            result = f.apply()
            if identical:
                assert result["ok"] and result["staged"] == 1, result
                assert Path(result["locations"][0]["path"]).read_bytes() == content
            else:
                assert (
                    not result["ok"]
                    and result["staged"] == 0
                    and "校验不一致" in result["errors"][0]
                ), result
                assert (
                    f.sources[0].read_bytes() == content
                    and target.read_bytes() != content
                )
        finally:
            f.stop()
    print("PASS complete multi-buffer comparison including differing final byte")


def restart():
    for phase in [
        "duplicate_verification_started",
        "duplicate_comparison_chunk",
        "duplicate_verified",
        "destination_published_unrecorded:0",
        "source_removed_unrecorded:0",
        "json_replaced_unrecorded:staging-origins.json",
        "ledger_committed_unrecorded",
        "final_receipt_unreleased",
        "copy_created",
        "copy_verified",
        "copy_publish_intent",
        "copy_published",
    ]:
        f, targets = prepare()
        try:
            resolve(f, "stage-duplicate")
            op = str(uuid.uuid4())
            p = f.preview()
            f.fault(phase, mode="exit", force_copy=phase.startswith("copy_"))
            try:
                f.apply(p, op=op)
            except (OSError, n.http.client.RemoteDisconnected):
                pass
            f.process.wait(timeout=10)
            assert f.process.returncode == 86, (phase, f.process.returncode)
            (f.root / "rust-fault.json").unlink()
            f.stop()
            f.start()
            r = f.call(id=op)["result"]
            assert_staged(f, targets, r)
            assert f.apply(p, op=op) == r
        finally:
            f.stop()
    print(
        "PASS native crashes during full comparison/staging/origin transaction/ledger/final receipt; durable replay"
    )


if __name__ == "__main__":
    main_cases()
    faults()
    races_and_aliases()
    late_keep_both()
    special_target()
    full_content_tail()
    restart()
