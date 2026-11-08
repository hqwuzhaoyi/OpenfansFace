"""#6 Next resolution transport, Range and reviewable UI fixture (18982/18983).
Set ARCHIVE_UI_HOLD=1 to keep this marked fixture running for browser verification.
"""

import importlib.util
import json
import os
from pathlib import Path
import signal
import subprocess
import time
import urllib.parse
import urllib.request

spec = importlib.util.spec_from_file_location(
    "resolution", Path(__file__).with_name("http-rust-archive-resolution.py")
)
a = importlib.util.module_from_spec(spec)
spec.loader.exec_module(a)
n = a.n
f = n.Fixture(4)
next_process = None
log = None
try:
    sample = Path(
        os.environ.get("ARCHIVE_VIDEO_SAMPLE", "/tmp/persons-flower.mp4")
    ).read_bytes()
    for source in f.sources:
        source.write_bytes(sample)
    targets = [a.canonical(f, i) for i in range(4)]
    targets[3].write_bytes(sample[:-1] + bytes([sample[-1] ^ 1]))
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
            "18982",
        ],
        cwd=n.REPO,
        env=env,
        stdout=log,
        stderr=log,
        start_new_session=True,
    )
    base = "http://127.0.0.1:18982"
    for _ in range(600):
        if next_process.poll() is not None:
            raise AssertionError((f.root / "next.log").read_text())
        try:
            urllib.request.urlopen(base + "/api/plan/preview", timeout=2).close()
            break
        except OSError:
            time.sleep(0.1)
    else:
        raise AssertionError(("Next not ready", f.root))

    def call(path, body=None):
        req = urllib.request.Request(
            base + path,
            data=None if body is None else json.dumps(body).encode(),
            headers={"Content-Type": "application/json"},
        )
        with urllib.request.urlopen(req, timeout=60) as r:
            return json.load(r)

    for vid in f.ids:
        call("/api/plan/queue", dict(creator="Momoyih", vids=[vid], source="all"))
    p = call("/api/plan/preview")
    assert len(p["rows"]) == 4
    r = call(
        "/api/plan/resolve",
        dict(vids=f.ids[:1], version=p["version"], kind="stage-duplicate"),
    )
    assert r["prepared"] == 1
    p = call("/api/plan/preview")
    assert p["rows"][0]["status"] == "ok"
    result = call("/api/plan/apply", dict(vids=f.ids[:1], version=p["version"]))
    assert result["ok"] and result["staged"] == 1, result
    assert call("/api/plan/progress?id=" + result["id"])["result"] == result
    for path in [f.ids[0], str(targets[0]), result["locations"][0]["path"]]:
        req = urllib.request.Request(
            base + "/api/video?p=" + urllib.parse.quote(path),
            headers={"Range": "bytes=0-63"},
        )
        with urllib.request.urlopen(req, timeout=10) as r:
            assert r.status == 206 and r.read() == sample[:64]
    with urllib.request.urlopen(base + "/integration", timeout=60) as r:
        assert r.status == 200
    print(
        "PASS Next native resolve/preview/apply/progress; source stable-ID, canonical and staged Range",
        flush=True,
    )
    print("fixture:", f.root, flush=True)
    (Path("/tmp/openfansface-rust-implementation/context/ticket06-ui.json")).write_text(
        json.dumps(
            {
                "root": str(f.root),
                "url": base,
                "ids": f.ids,
                "targets": [str(p) for p in targets],
            }
        )
    )
    if os.environ.get("ARCHIVE_UI_HOLD") == "1":
        while True:
            time.sleep(1)
finally:
    if next_process and next_process.poll() is None:
        os.killpg(next_process.pid, signal.SIGTERM)
        next_process.wait(timeout=15)
    if log:
        log.close()
    f.stop()
