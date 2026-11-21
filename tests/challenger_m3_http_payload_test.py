#!/usr/bin/env python3
import json
import os
import subprocess
import tempfile
import time
import urllib.error
import urllib.request

def run_http_payload_test():
    tmp_dir = tempfile.mkdtemp(prefix="face-chal-http-")
    media_dir = os.path.join(tmp_dir, "media")
    os.makedirs(media_dir, exist_ok=True)
    ledger_path = os.path.join(tmp_dir, "ledger.db")

    port = 18933
    bind = f"127.0.0.1:{port}"

    env = os.environ.copy()
    env["FACES_BASE"] = tmp_dir
    env["MEDIA2_ROOT"] = media_dir
    env["PEACH_LEDGER_PATH"] = ledger_path
    env["FILE_OPERATIONS_BIND"] = bind
    env["FILE_OPERATIONS_RUST_TOKEN"] = "test-token"

    binary_path = os.path.abspath("services/file-operations/target/debug/face-file-operations")
    proc = subprocess.Popen([binary_path], env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)

    try:
        # Wait for server to listen
        ready = False
        for _ in range(50):
            try:
                req = urllib.request.Request(f"http://{bind}/health")
                with urllib.request.urlopen(req, timeout=1) as resp:
                    if resp.status == 200:
                        ready = True
                        break
            except Exception:
                time.sleep(0.1)

        if not ready:
            raise RuntimeError("Server failed to start")

        print("Server is listening on", bind)

        # 1. Normal payload test
        small_body = json.dumps({
            "node_id": "test-node",
            "attempt_id": "run-test-1",
            "vid": "vid-1",
            "coordinator_generation": 1,
            "stage": "detect",
        }).encode("utf-8")

        req = urllib.request.Request(
            f"http://{bind}/v1/agent/task-result",
            data=small_body,
            headers={
                "Content-Type": "application/json",
                "Authorization": "Bearer test-node-token"
            },
            method="POST"
        )
        try:
            with urllib.request.urlopen(req) as resp:
                print("Small request status:", resp.status)
        except urllib.error.HTTPError as e:
            print("Small request HTTPError:", e.code, e.read().decode())

        # 2. Oversized payload (>2MB) test: 2.5MB payload
        padding = "X" * (2500 * 1024)
        oversized_body = json.dumps({
            "node_id": "test-node",
            "attempt_id": "run-test-1",
            "vid": "vid-1",
            "coordinator_generation": 1,
            "stage": "detect",
            "padding": padding,
        }).encode("utf-8")

        print(f"Sending oversized payload of size {len(oversized_body)} bytes (>2MB)...")

        req_oversized = urllib.request.Request(
            f"http://{bind}/v1/agent/task-result",
            data=oversized_body,
            headers={
                "Content-Type": "application/json",
                "Authorization": "Bearer test-node-token"
            },
            method="POST"
        )

        try:
            with urllib.request.urlopen(req_oversized) as resp:
                print("Oversized request status:", resp.status)
        except urllib.error.HTTPError as e:
            status_code = e.code
            body = e.read().decode()
            print(f"Oversized payload result: HTTP {status_code}")
            print("Response body:", body)

            if status_code == 413:
                print("PASS: Server returned HTTP 413 Payload Too Large as specified.")
            elif status_code == 400:
                print("FINDING: Server returned HTTP 400 Bad Request instead of HTTP 413 Payload Too Large!")
            else:
                print(f"UNEXPECTED: HTTP {status_code}")

    finally:
        proc.terminate()
        proc.wait()

if __name__ == "__main__":
    run_http_payload_test()
