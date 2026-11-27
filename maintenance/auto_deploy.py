#!/usr/bin/env python3
"""Automated LAN CI/CD deployment runner invoked by git post-receive.

Supports selective Web/Rust builds via `fast_deploy.py`, pre-cutover test and
operation-lock gates, health/static-asset verification, and automatic rollback.
"""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time
import urllib.error
import urllib.request

MAINTENANCE_DIR = Path(__file__).resolve().parent
if str(MAINTENANCE_DIR) not in sys.path:
    sys.path.insert(0, str(MAINTENANCE_DIR))

import fast_deploy  # noqa: E402

ZERO_SHA = "0" * 40
WEB_RUNTIME_BASE = "face-admin-next:p1-slim-20261122"
RUST_RUNTIME_BASE = "face-file-operations:p1-slim-62bec82f"
RUST_TOOLCHAIN_IMAGE = "rust:1.95-bookworm"

WEB_TRIGGER_FILES = frozenset(
    {
        "package.json",
        "pnpm-lock.yaml",
        "pnpm-workspace.yaml",
        "next.config.ts",
        "next-env.d.ts",
        "tsconfig.json",
        "Dockerfile",
        ".dockerignore",
    }
)
WEB_TRIGGER_PREFIXES = ("app/", "components/", "lib/", "public/")
RUST_TRIGGER_PREFIX = "services/file-operations/"
WEB_TEST_FILES = (
    "tests/archive-actions.test.mjs",
    "tests/background-operations-model.test.mjs",
    "tests/person-delete-refresh.test.mjs",
    "tests/person-picker-model.test.mjs",
    "tests/locks-model.test.mjs",
)
VERIFY_WEB_ROUTES = ("/", "/review", "/operations", "/persons", "/integration", "/staging")


def log(msg: str) -> None:
    ts = datetime.now().strftime("%H:%M:%S")
    print(f"[auto-deploy {ts}] {msg}", flush=True)


def run_cmd(args: list[str], cwd: Path | None = None, capture: bool = False) -> str:
    if capture:
        return subprocess.check_output(args, cwd=cwd, text=True)
    subprocess.check_call(args, cwd=cwd)
    return ""


def docker_image_exists(image_tag: str) -> bool:
    proc = subprocess.run(
        ["sudo", "-n", "docker", "image", "inspect", image_tag],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    return proc.returncode == 0


def classify_changes(changed_paths: list[str], commit_message: str) -> dict:
    msg_lower = commit_message.lower()
    if "[skip deploy]" in msg_lower or "[ci skip]" in msg_lower:
        return {
            "skip": True,
            "reason": "commit flag",
            "deploy_web": False,
            "deploy_rust": False,
            "changed_count": len(changed_paths),
        }
    if "[deploy all]" in msg_lower:
        return {
            "skip": False,
            "reason": "commit flag [deploy all]",
            "deploy_web": True,
            "deploy_rust": True,
            "changed_count": len(changed_paths),
        }

    deploy_rust = any(path.startswith(RUST_TRIGGER_PREFIX) for path in changed_paths)
    deploy_web = any(
        path.startswith(WEB_TRIGGER_PREFIXES) or path in WEB_TRIGGER_FILES
        for path in changed_paths
    )
    skip = not (deploy_web or deploy_rust)
    reason = "no deployable changes" if skip else "path match"
    return {
        "skip": skip,
        "reason": reason,
        "deploy_web": deploy_web,
        "deploy_rust": deploy_rust,
        "changed_count": len(changed_paths),
    }


def compute_rust_deps_key(root: Path) -> str:
    h = hashlib.sha256()
    for rel in ("services/file-operations/Cargo.toml", "services/file-operations/Cargo.lock"):
        p = root / rel
        h.update(rel.encode("utf-8") + b"\0")
        h.update(p.read_bytes() + b"\0")
    return h.hexdigest()[:12]


def read_changed_paths(git_dir: Path, old_sha: str, new_sha: str, deploy_root: Path) -> tuple[list[str], str]:
    base_sha = old_sha
    last_sha_file = deploy_root / "last-deployed-sha"
    if last_sha_file.is_file():
        candidate = last_sha_file.read_text(encoding="utf-8").strip()
        if re.fullmatch(r"[0-9a-fA-F]{40}", candidate):
            check = subprocess.run(
                ["git", "--git-dir", str(git_dir), "cat-file", "-e", f"{candidate}^{{commit}}"],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
            if check.returncode == 0:
                base_sha = candidate

    if not re.fullmatch(r"[0-9a-fA-F]{40}", base_sha) or base_sha == ZERO_SHA:
        out = run_cmd(
            ["git", "--git-dir", str(git_dir), "show", "--pretty=", "--name-only", new_sha],
            capture=True,
        )
    else:
        out = run_cmd(
            ["git", "--git-dir", str(git_dir), "diff", "--name-only", f"{base_sha}..{new_sha}"],
            capture=True,
        )
    commit_msg = run_cmd(
        ["git", "--git-dir", str(git_dir), "log", "-1", "--pretty=%B", new_sha],
        capture=True,
    )
    paths = [line.strip() for line in out.splitlines() if line.strip()]
    return paths, commit_msg


def get_rust_token() -> str:
    env_raw = run_cmd(
        ["sudo", "-n", "docker", "inspect", "face-file-operations", "--format", "{{json .Config.Env}}"],
        capture=True,
    )
    env_list = json.loads(env_raw)
    for item in env_list:
        if isinstance(item, str) and item.startswith("FILE_OPERATIONS_RUST_TOKEN="):
            return item.split("=", 1)[1]
    raise RuntimeError("FILE_OPERATIONS_RUST_TOKEN not found on running face-file-operations container")


def fetch_locks() -> dict:
    with urllib.request.urlopen("http://127.0.0.1:8919/api/locks", timeout=5) as resp:
        return json.loads(resp.read().decode("utf-8"))


def fetch_rust_readiness(token: str) -> dict:
    req = urllib.request.Request(
        "http://127.0.0.1:8930/v1/readiness",
        headers={"Authorization": f"Bearer {token}"},
    )
    with urllib.request.urlopen(req, timeout=5) as resp:
        return json.loads(resp.read().decode("utf-8"))


def wait_for_idle_locks(wait_seconds: int, require_idle: bool = True) -> tuple[dict, dict]:
    token = get_rust_token()
    readiness = fetch_rust_readiness(token)
    if readiness.get("recovery_required", 0) != 0:
        raise RuntimeError(f"Rust service requires recovery before deploy: {readiness}")

    deadline = time.monotonic() + max(0, wait_seconds)
    while True:
        locks = fetch_locks()
        active_ops = locks.get("active_ops") or []
        if not active_ops or not require_idle:
            return locks, readiness
        if time.monotonic() >= deadline:
            raise TimeoutError(f"Active operations still running after {wait_seconds}s: {active_ops}")
        log(f"waiting for {len(active_ops)} active operation(s) to settle...")
        time.sleep(3)


def ensure_web_deps_image(worktree: Path) -> tuple[str, str]:
    deps_key = fast_deploy.compute_web_deps_key(worktree)
    deps_image = f"face-admin-next:build-deps-{deps_key}"
    if docker_image_exists(deps_image):
        log(f"reusing cached Web deps image {deps_image}")
        return deps_key, deps_image
    log(f"building Web deps image {deps_image} ...")
    run_cmd(["sudo", "-n", "docker", "build", "--target", "deps", "-t", deps_image, str(worktree)])
    return deps_key, deps_image


def run_web_gate_tests(worktree: Path, deps_image: str) -> None:
    existing_tests = [rel for rel in WEB_TEST_FILES if (worktree / rel).is_file()]
    test_cmd = " && ".join(
        [
            "tar -C /src --exclude=node_modules --exclude=.next --exclude=.git --exclude=.local -cf - . | tar -C /app -xf -",
            "pnpm exec tsc --noEmit",
            *(
                [f"node --test {' '.join(existing_tests)}"]
                if existing_tests
                else []
            ),
        ]
    )
    log(f"running Web tsc + {len(existing_tests)} node test file(s) in {deps_image}...")
    run_cmd(
        [
            "sudo",
            "-n",
            "docker",
            "run",
            "--rm",
            "-v",
            f"{worktree}:/src:ro",
            deps_image,
            "sh",
            "-ec",
            test_cmd,
        ]
    )


def build_web_image(worktree: Path, ts: str, short_sha: str, skip_tests: bool) -> dict:
    deps_key, deps_image = ensure_web_deps_image(worktree)
    if not docker_image_exists(WEB_RUNTIME_BASE):
        raise RuntimeError(f"Missing runtime base image {WEB_RUNTIME_BASE}")
    if not skip_tests:
        run_web_gate_tests(worktree, deps_image)

    image_tag = f"face-admin-next:ci-{ts}-{short_sha}"
    with tempfile.TemporaryDirectory(prefix="ci-web-") as tmp:
        tmp_dir = Path(tmp)
        ctx_tar = tmp_dir / "web-context.tar.gz"
        ctx_dir = tmp_dir / "ctx"
        ctx_dir.mkdir()
        meta = fast_deploy.build_web_context(
            output_tar_gz=ctx_tar,
            root=worktree,
            deps_image=deps_image,
            runtime_base_image=WEB_RUNTIME_BASE,
            skip_typecheck=True,
        )
        with tarfile.open(ctx_tar, "r:gz") as tar:
            tar.extractall(ctx_dir)
        log(f"building Web image {image_tag} ({meta['file_count']} files)...")
        run_cmd(
            [
                "sudo",
                "-n",
                "docker",
                "build",
                "-f",
                str(ctx_dir / "Dockerfile.incremental"),
                "-t",
                image_tag,
                str(ctx_dir),
            ]
        )
    return {"image_tag": image_tag, "deps_key": deps_key, "context_bytes": meta["archive_bytes"]}


def ensure_rust_builder_image(worktree: Path) -> tuple[str, str]:
    cargo_key = compute_rust_deps_key(worktree)
    builder_image = f"face-file-operations:build-deps-{cargo_key}"
    if docker_image_exists(builder_image):
        log(f"reusing cached Rust builder image {builder_image}")
        return cargo_key, builder_image

    log(f"building Rust deps cache image {builder_image} ...")
    with tempfile.TemporaryDirectory(prefix="ci-rust-deps-") as tmp:
        tmp_dir = Path(tmp)
        shutil.copy2(worktree / "services/file-operations/Cargo.toml", tmp_dir / "Cargo.toml")
        shutil.copy2(worktree / "services/file-operations/Cargo.lock", tmp_dir / "Cargo.lock")
        dockerfile = "\n".join(
            [
                f"FROM {RUST_TOOLCHAIN_IMAGE}",
                "WORKDIR /build",
                "COPY Cargo.toml Cargo.lock ./",
                "RUN mkdir -p src/bin \\",
                "  && echo 'fn main() {}' > src/main.rs \\",
                "  && echo 'fn main() {}' > src/bin/storage-probe.rs \\",
                "  && cargo build --release --locked --bins \\",
                "  && cargo test --lib --no-run --locked \\",
                "  && rm -rf src target/release/deps/face_file_operations* target/release/deps/storage_probe* target/release/face-file-operations target/release/storage-probe",
                "",
            ]
        )
        (tmp_dir / "Dockerfile").write_text(dockerfile, encoding="utf-8")
        run_cmd(["sudo", "-n", "docker", "build", "-t", builder_image, str(tmp_dir)])
    return cargo_key, builder_image


def build_rust_image(worktree: Path, ts: str, short_sha: str, skip_tests: bool) -> dict:
    if not docker_image_exists(RUST_RUNTIME_BASE):
        raise RuntimeError(f"Missing Rust runtime base image {RUST_RUNTIME_BASE}")
    cargo_key, builder_image = ensure_rust_builder_image(worktree)
    image_tag = f"face-file-operations:ci-{ts}-{short_sha}"
    rust_src = worktree / "services/file-operations"

    with tempfile.TemporaryDirectory(prefix="ci-rust-") as tmp:
        tmp_dir = Path(tmp)
        out_dir = tmp_dir / "out"
        out_dir.mkdir()
        steps = [
            "cp /repo-src/Cargo.toml /repo-src/Cargo.lock /build/",
            "rm -rf /build/src && cp -r /repo-src/src /build/src",
            "touch /build/src/main.rs",
        ]
        if not skip_tests:
            steps.append("cargo test --lib --locked")
        steps.append("cargo build --release --locked --bin face-file-operations")
        steps.append("cp /build/target/release/face-file-operations /out/face-file-operations")
        log(f"compiling Rust binary{' + running cargo test --lib' if not skip_tests else ''}...")
        run_cmd(
            [
                "sudo",
                "-n",
                "docker",
                "run",
                "--rm",
                "-v",
                f"{rust_src}:/repo-src:ro",
                "-v",
                f"{out_dir}:/out",
                builder_image,
                "sh",
                "-ec",
                " && ".join(steps),
            ]
        )

        overlay_tar = tmp_dir / "rust-overlay.tar.gz"
        overlay_dir = tmp_dir / "overlay"
        overlay_dir.mkdir()
        meta = fast_deploy.build_rust_overlay(
            output_tar_gz=overlay_tar,
            base_image=RUST_RUNTIME_BASE,
            binary_path=out_dir / "face-file-operations",
        )
        with tarfile.open(overlay_tar, "r:gz") as tar:
            tar.extractall(overlay_dir)
        log(f"building Rust thin-layer image {image_tag} ...")
        run_cmd(["sudo", "-n", "docker", "build", "-t", image_tag, str(overlay_dir)])

    return {
        "image_tag": image_tag,
        "cargo_key": cargo_key,
        "binaries_sha256": meta["binaries_sha256"],
    }


def verify_rust_after_cutover() -> dict:
    token = get_rust_token()
    last_err: Exception | None = None
    for _ in range(40):
        try:
            readiness = fetch_rust_readiness(token)
            if (
                readiness.get("ready")
                and readiness.get("accepting")
                and readiness.get("authority_valid")
                and readiness.get("recovery_required", 0) == 0
            ):
                return readiness
        except Exception as exc:
            last_err = exc
        time.sleep(0.5)
    raise RuntimeError(f"Rust readiness verification failed: {last_err}")


def verify_web_after_cutover() -> dict:
    last_err: Exception | None = None
    for _ in range(40):
        try:
            locks = fetch_locks()
            if "active_ops" in locks:
                break
        except Exception as exc:
            last_err = exc
        time.sleep(0.5)
    else:
        raise RuntimeError(f"Web /api/locks readiness failed: {last_err}")

    pages = {}
    assets: set[str] = set()
    for route in VERIFY_WEB_ROUTES:
        with urllib.request.urlopen(f"http://127.0.0.1:8919{route}", timeout=10) as resp:
            if resp.status != 200:
                raise RuntimeError(f"Route {route} returned HTTP {resp.status}")
            html = resp.read().decode("utf-8", "replace")
            refs = set(re.findall(r"/_next/static/[^\s\"'<>\\]+", html))
            pages[route] = {"status": resp.status, "static_refs": len(refs)}
            assets.update(refs)

    failed_assets = []
    for asset in sorted(assets):
        with urllib.request.urlopen(f"http://127.0.0.1:8919{asset}", timeout=10) as resp:
            if resp.status != 200:
                failed_assets.append((asset, resp.status))
    if failed_assets:
        raise RuntimeError(f"Static asset check failed: {failed_assets}")

    return {"pages": pages, "static_assets_verified": len(assets)}


def cutover_container(
    container_name: str,
    new_image_tag: str,
    include_user: bool,
    verify_fn,
) -> tuple[dict, dict]:
    inspect_raw = run_cmd(["sudo", "-n", "docker", "inspect", container_name], capture=True)
    info = json.loads(inspect_raw)[0]
    prev_cid = info["Id"]
    prev_image = info["Config"]["Image"]
    rollback_cname = f"{container_name}-rollback"

    env_list = info["Config"].get("Env") or []
    binds = info["HostConfig"].get("Binds") or []
    net_mode = info["HostConfig"].get("NetworkMode") or "host"
    restart_name = (info["HostConfig"].get("RestartPolicy") or {}).get("Name") or "unless-stopped"
    user = info["Config"].get("User") or ""

    # Abort before mutating if a background write operation started mid-build.
    locks = fetch_locks()
    if locks.get("active_ops"):
        raise RuntimeError(f"Active ops appeared before cutover: {locks['active_ops']}")

    subprocess.run(
        ["sudo", "-n", "docker", "rm", "-f", rollback_cname],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    run_cmd(["sudo", "-n", "docker", "stop", "--timeout", "30", container_name])
    run_cmd(["sudo", "-n", "docker", "rename", container_name, rollback_cname])

    fd, env_path = tempfile.mkstemp(prefix=f"{container_name}-env-", text=True)
    os.chmod(env_path, 0o600)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as f:
            for item in env_list:
                f.write(item + "\n")
        cmd = [
            "sudo",
            "-n",
            "docker",
            "run",
            "-d",
            "--name",
            container_name,
            "--network",
            net_mode,
            "--restart",
            restart_name,
            "--env-file",
            env_path,
        ]
        if include_user and user:
            cmd.extend(["--user", user])
        for b in binds:
            cmd.extend(["-v", b])
        cmd.append(new_image_tag)
        new_cid = run_cmd(cmd, capture=True).strip()
    finally:
        try:
            os.unlink(env_path)
        except FileNotFoundError:
            pass

    try:
        verify_result = verify_fn()
    except Exception as exc:
        log(f"ERROR verifying {container_name} ({exc}); rolling back to {rollback_cname}...")
        subprocess.run(["sudo", "-n", "docker", "stop", "--timeout", "15", container_name], check=False)
        subprocess.run(["sudo", "-n", "docker", "rm", "-f", container_name], check=False)
        run_cmd(["sudo", "-n", "docker", "rename", rollback_cname, container_name])
        run_cmd(["sudo", "-n", "docker", "start", container_name])
        raise

    return (
        {
            "container_name": container_name,
            "previous_container_id": prev_cid,
            "previous_image_tag": prev_image,
            "rollback_container_name": rollback_cname,
            "new_container_id": new_cid,
            "new_image_tag": new_image_tag,
        },
        verify_result,
    )


def prune_ci_images(repo: str, keep: int = 3) -> None:
    out = run_cmd(
        ["sudo", "-n", "docker", "images", "--format", "{{.Repository}}:{{.Tag}}", repo],
        capture=True,
    )
    ci_tags = [line.strip() for line in out.splitlines() if ":ci-" in line]
    ci_tags.sort(reverse=True)
    for tag in ci_tags[keep:]:
        subprocess.run(
            ["sudo", "-n", "docker", "rmi", tag],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--git-dir", type=Path, required=True)
    parser.add_argument("--worktree", type=Path, required=True)
    parser.add_argument("--old-sha", default=ZERO_SHA)
    parser.add_argument("--new-sha", required=True)
    parser.add_argument("--deploy-root", type=Path, required=True)
    parser.add_argument("--wait-locks-seconds", type=int, default=120)
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--skip-tests", action="store_true")
    parser.add_argument("--changed-files-override", default=None)
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    args.deploy_root.mkdir(parents=True, exist_ok=True)
    logs_dir = args.deploy_root / "logs"
    logs_dir.mkdir(parents=True, exist_ok=True)

    lock_path = args.deploy_root / "deploy.lock"
    with lock_path.open("w", encoding="utf-8") as lock_file:
        fcntl.flock(lock_file.fileno(), fcntl.LOCK_EX)

        if args.changed_files_override is not None:
            changed_paths = [p.strip() for p in args.changed_files_override.split(",") if p.strip()]
            commit_msg = ""
        else:
            changed_paths, commit_msg = read_changed_paths(
                args.git_dir, args.old_sha, args.new_sha, args.deploy_root
            )

        plan = classify_changes(changed_paths, commit_msg)
        short_sha = args.new_sha[:7]
        ts = datetime.now().strftime("%Y%m%d%H%M%S")

        if args.dry_run:
            locks = fetch_locks()
            readiness = fetch_rust_readiness(get_rust_token())
            web_deps_key = fast_deploy.compute_web_deps_key(args.worktree)
            rust_deps_key = compute_rust_deps_key(args.worktree)
            summary = {
                "dry_run": True,
                "new_sha": args.new_sha,
                "plan": plan,
                "active_ops": locks.get("active_ops") or [],
                "rust_ready": readiness.get("ready"),
                "web_deps_image": f"face-admin-next:build-deps-{web_deps_key}",
                "web_deps_cached": docker_image_exists(f"face-admin-next:build-deps-{web_deps_key}"),
                "web_runtime_base_present": docker_image_exists(WEB_RUNTIME_BASE),
                "rust_deps_key": rust_deps_key,
                "rust_builder_image": f"face-file-operations:build-deps-{rust_deps_key}",
                "rust_builder_cached": docker_image_exists(f"face-file-operations:build-deps-{rust_deps_key}"),
                "rust_runtime_base_present": docker_image_exists(RUST_RUNTIME_BASE),
            }
            print(json.dumps(summary, ensure_ascii=False, indent=2))
            return 0

        if plan["skip"]:
            log(f"skipping deploy for {short_sha}: {plan['reason']}")
            (args.deploy_root / "last-deployed-sha").write_text(args.new_sha + "\n", encoding="utf-8")
            return 0

        log(f"deploy plan for {short_sha}: web={plan['deploy_web']}, rust={plan['deploy_rust']}")
        try:
            wait_for_idle_locks(args.wait_locks_seconds, require_idle=True)
        except TimeoutError as exc:
            log(f"ABORT: {exc}")
            return 2

        rust_build = build_rust_image(args.worktree, ts, short_sha, args.skip_tests) if plan["deploy_rust"] else None
        web_build = build_web_image(args.worktree, ts, short_sha, args.skip_tests) if plan["deploy_web"] else None

        receipt: dict = {
            "release_id": f"ci-{ts}-{short_sha}",
            "deployed_at": datetime.now(timezone.utc).astimezone().isoformat(),
            "old_sha": args.old_sha,
            "new_sha": args.new_sha,
            "plan": plan,
        }

        if rust_build:
            rust_cutover, rust_verify = cutover_container(
                "face-file-operations",
                rust_build["image_tag"],
                include_user=True,
                verify_fn=verify_rust_after_cutover,
            )
            receipt["rust_service"] = {**rust_build, **rust_cutover, "readiness": rust_verify}
            log(f"Rust service updated cleanly -> {rust_build['image_tag']}")

        if web_build:
            web_cutover, web_verify = cutover_container(
                "face-admin-prod",
                web_build["image_tag"],
                include_user=False,
                verify_fn=verify_web_after_cutover,
            )
            receipt["web_service"] = {**web_build, **web_cutover, "verification": web_verify}
            log(f"Web service updated cleanly -> {web_build['image_tag']}")

        (args.deploy_root / "last-deployed-sha").write_text(args.new_sha + "\n", encoding="utf-8")
        receipt_path = logs_dir / f"release-{ts}-{short_sha}.json"
        receipt_path.write_text(json.dumps(receipt, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        prune_ci_images("face-admin-next")
        prune_ci_images("face-file-operations")
        log(f"release complete: {receipt_path}")
        return 0


if __name__ == "__main__":
    raise SystemExit(main())
