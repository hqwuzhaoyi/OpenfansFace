"""Thin Next gateway contract using the marked native archive fixture and reserved ports."""

import importlib.util, json, os, signal, subprocess, time, urllib.error, urllib.parse, urllib.request
from pathlib import Path

spec = importlib.util.spec_from_file_location(
    "archive_fixture", Path(__file__).with_name("http-rust-archive.py")
)
native = importlib.util.module_from_spec(spec)
spec.loader.exec_module(native)
f = native.Fixture(1)
next_process = None
log = None
try:
    env = {
        **f.env,
        "PATH": os.environ["PATH"],
        "FILE_OPERATIONS_BACKEND": "rust",
        "FILE_OPERATIONS_ENABLE_RUST": "1",
        "FILE_OPERATIONS_RUST_URL": f.url,
        "NEXT_TELEMETRY_DISABLED": "1",
    }
    log = (f.root / "next.log").open("wb")
    next_process = subprocess.Popen(
        [
            "node",
            "node_modules/next/dist/bin/next",
            "dev",
            "--hostname",
            "127.0.0.1",
            "--port",
            "18980",
        ],
        cwd=native.REPO,
        env=env,
        stdout=log,
        stderr=log,
        start_new_session=True,
    )
    base = "http://127.0.0.1:18980"
    for _ in range(600):
        if next_process.poll() is not None:
            raise AssertionError((f.root / "next.log").read_text())
        try:
            with urllib.request.urlopen(base + "/api/plan/preview", timeout=2) as r:
                json.load(r)
            break
        except OSError:
            time.sleep(0.1)
    else:
        raise AssertionError(("next not ready", f.root))

    def call(path, body=None, method=None):
        req = urllib.request.Request(
            base + path,
            data=None if body is None else json.dumps(body).encode(),
            headers={"Content-Type": "application/json"},
            method=method,
        )
        try:
            with urllib.request.urlopen(req, timeout=60) as r:
                return json.load(r)
        except urllib.error.HTTPError as e:
            raise AssertionError((e.code, e.read().decode(), f.root))

    confirmed = call(
        "/api/plan/queue", {"creator": "Momoyih", "vids": f.ids, "source": "all"}
    )
    assert confirmed["added"] == 1 and f.sources[0].exists()
    removed = call("/api/plan/queue", {"vids": f.ids}, "DELETE")
    assert removed["removed"][0]["return_href"] == "/unclassified"
    native.write(f.base / "claims-detail.json", [{"vid": f.ids[0], "name": "Momoyih"}])
    assert call("/api/claims/decide", {"vid": f.ids[0], "approve": True})["queued"]
    preview = call("/api/plan/preview")
    assert preview["n_ok"] == 1
    op = str(native.uuid.uuid4())
    result = call(
        "/api/plan/apply",
        {"vids": f.ids, "version": preview["version"], "operation_id": op},
    )
    assert result["ok"] and result["moved"] == 1, result
    assert call("/api/plan/progress?id=" + op)["result"] == result
    req = urllib.request.Request(
        base + "/api/video?p=" + urllib.parse.quote(f.ids[0]),
        headers={"Range": "bytes=0-15"},
    )
    with urllib.request.urlopen(req, timeout=10) as r:
        assert r.status == 206 and r.read() == native.BYTES[:16]
    print(
        "PASS Next queue/undo/claim/preview/apply/progress shapes and stable video Range after native archive"
    )
    print("fixture:", f.root)
finally:
    if next_process and next_process.poll() is None:
        os.killpg(next_process.pid, signal.SIGTERM)
        next_process.wait(timeout=15)
    if log:
        log.close()
    f.stop()
