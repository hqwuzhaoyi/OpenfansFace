#!/usr/bin/env python3
"""Fast incremental Docker context and thin-layer overlay builder.

Eliminates 250-300 MB `docker save | ssh` full-image transfers by producing:
1. `web-context`: a ~1.3 MB deterministic source tarball + incremental Dockerfile
   that reuses a lockfile-keyed `face-admin-next:build-deps-<hash>` image and the
   existing runtime base image on the target host.
2. `rust-overlay`: a ~3.5 MB compressed binary overlay context that layers updated
   `face-file-operations` (and optional `storage-probe`) binaries onto the existing
   runtime image without re-exporting the 290 MB Debian/FFmpeg base layers.
3. `python-overlay`: a ~50 KB script overlay context for `worker` or `calibrator`
   when Python package requirements have not changed.
"""
from __future__ import annotations

import argparse
import gzip
import hashlib
import io
import json
import os
from pathlib import Path
import tarfile

REPO_ROOT = Path(__file__).resolve().parent.parent

WEB_INCLUDE_FILES = (
    "package.json",
    "pnpm-lock.yaml",
    "pnpm-workspace.yaml",
    "next.config.ts",
    "next-env.d.ts",
    "tsconfig.json",
    "Dockerfile",
    ".dockerignore",
)
WEB_INCLUDE_DIRS = (
    "app",
    "components",
    "lib",
    "public",
)
EXCLUDED_NAMES = {
    ".DS_Store",
    "__pycache__",
    "node_modules",
    ".next",
    ".git",
    ".local",
    "tsconfig.tsbuildinfo",
}


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(65536), b""):
            h.update(chunk)
    return h.hexdigest()


def compute_web_deps_key(root: Path = REPO_ROOT) -> str:
    """Compute a 12-char cache key from lockfile and dependency manifests."""
    h = hashlib.sha256()
    for rel in ("package.json", "pnpm-lock.yaml", "pnpm-workspace.yaml"):
        p = root / rel
        h.update(rel.encode("utf-8") + b"\0")
        h.update(p.read_bytes() + b"\0")
    return h.hexdigest()[:12]


def iter_web_files(root: Path = REPO_ROOT) -> list[tuple[str, Path]]:
    collected: list[tuple[str, Path]] = []
    for rel in WEB_INCLUDE_FILES:
        p = root / rel
        if p.is_file():
            collected.append((rel, p))
    for d in WEB_INCLUDE_DIRS:
        base = root / d
        if not base.is_dir():
            continue
        for p in sorted(base.rglob("*")):
            if any(part in EXCLUDED_NAMES or part.startswith(".env") for part in p.relative_to(root).parts):
                continue
            if p.is_file() and not p.is_symlink():
                collected.append((p.relative_to(root).as_posix(), p))
    return sorted(collected, key=lambda item: item[0])


def render_incremental_web_dockerfile(
    deps_image: str,
    runtime_base_image: str,
    skip_typecheck: bool = True,
) -> str:
    skip_flag = "1" if skip_typecheck else "0"
    return "\n".join(
        [
            f"FROM {deps_image} AS build",
            "WORKDIR /app",
            f"ENV NEXT_TELEMETRY_DISABLED=1 NEXT_SKIP_TYPECHECK={skip_flag}",
            "COPY . .",
            "RUN pnpm build",
            "",
            f"FROM {runtime_base_image} AS runtime",
            "WORKDIR /app",
            "ENV NODE_ENV=production HOSTNAME=0.0.0.0 NEXT_TELEMETRY_DISABLED=1 FACES_BASE=/data/base MEDIA2_ROOT=/mnt/media2 DOCKER_SOCK=/var/run/docker.sock",
            "RUN rm -rf /app/.next /app/public",
            "COPY --from=build /app/.next/standalone ./",
            "COPY --from=build /app/.next/static ./.next/static",
            "COPY --from=build /app/public ./public",
            'CMD ["node", "server.js"]',
            "",
        ]
    )


def add_bytes_to_tar(tar: tarfile.TarFile, arcname: str, data: bytes, mode: int = 0o644) -> None:
    info = tarfile.TarInfo(name=arcname)
    info.size = len(data)
    info.mode = mode
    info.mtime = 0
    info.uid = 0
    info.gid = 0
    info.uname = "root"
    info.gname = "root"
    tar.addfile(info, io.BytesIO(data))


def build_web_context(
    output_tar_gz: Path,
    root: Path = REPO_ROOT,
    deps_image: str | None = None,
    runtime_base_image: str = "face-admin-prod:current",
    skip_typecheck: bool = True,
) -> dict:
    deps_key = compute_web_deps_key(root)
    resolved_deps_image = deps_image or f"face-admin-next:build-deps-{deps_key}"
    files = iter_web_files(root)
    file_hashes: dict[str, str] = {}

    output_tar_gz.parent.mkdir(parents=True, exist_ok=True)
    with output_tar_gz.open("wb") as raw_out:
        with gzip.GzipFile(filename="", mode="wb", fileobj=raw_out, mtime=0) as gz:
            with tarfile.open(mode="w", fileobj=gz) as tar:
                for rel, path in files:
                    data = path.read_bytes()
                    file_hashes[rel] = sha256_bytes(data)
                    mode = 0o755 if os.access(path, os.X_OK) else 0o644
                    add_bytes_to_tar(tar, rel, data, mode=mode)
                inc_dockerfile = render_incremental_web_dockerfile(
                    deps_image=resolved_deps_image,
                    runtime_base_image=runtime_base_image,
                    skip_typecheck=skip_typecheck,
                ).encode("utf-8")
                add_bytes_to_tar(tar, "Dockerfile.incremental", inc_dockerfile, mode=0o644)

    return {
        "kind": "web-context",
        "deps_key": deps_key,
        "deps_image": resolved_deps_image,
        "runtime_base_image": runtime_base_image,
        "skip_typecheck": skip_typecheck,
        "file_count": len(files),
        "archive_path": str(output_tar_gz),
        "archive_bytes": output_tar_gz.stat().st_size,
        "archive_sha256": sha256_file(output_tar_gz),
        "files_sha256": file_hashes,
    }


def build_web_overlay(
    output_tar_gz: Path,
    base_image: str,
    payload_tar_gz: Path,
) -> dict:
    if not payload_tar_gz.is_file():
        raise FileNotFoundError(f"Web payload archive not found: {payload_tar_gz}")
    payload_bytes = payload_tar_gz.read_bytes()
    dockerfile_lines = [
        f"FROM {base_image}",
        "WORKDIR /app",
        "ENV NODE_ENV=production HOSTNAME=0.0.0.0 NEXT_TELEMETRY_DISABLED=1 FACES_BASE=/data/base MEDIA2_ROOT=/mnt/media2 DOCKER_SOCK=/var/run/docker.sock",
        "COPY web-app-payload.tar.gz /tmp/web-app-payload.tar.gz",
        "RUN rm -rf /app/.next /app/public && tar -xzf /tmp/web-app-payload.tar.gz -C /app && rm -f /tmp/web-app-payload.tar.gz",
        'CMD ["node", "server.js"]',
        "",
    ]
    output_tar_gz.parent.mkdir(parents=True, exist_ok=True)
    with output_tar_gz.open("wb") as raw_out:
        with gzip.GzipFile(filename="", mode="wb", fileobj=raw_out, mtime=0) as gz:
            with tarfile.open(mode="w", fileobj=gz) as tar:
                add_bytes_to_tar(tar, "web-app-payload.tar.gz", payload_bytes, mode=0o644)
                add_bytes_to_tar(tar, "Dockerfile", "\n".join(dockerfile_lines).encode("utf-8"), mode=0o644)

    return {
        "kind": "web-overlay",
        "base_image": base_image,
        "archive_path": str(output_tar_gz),
        "archive_bytes": output_tar_gz.stat().st_size,
        "archive_sha256": sha256_file(output_tar_gz),
        "payload_sha256": sha256_bytes(payload_bytes),
    }


def build_rust_overlay(
    output_tar_gz: Path,
    base_image: str,
    binary_path: Path,
    probe_binary_path: Path | None = None,
) -> dict:
    if not binary_path.is_file():
        raise FileNotFoundError(f"Rust binary not found: {binary_path}")
    bin_bytes = binary_path.read_bytes()
    if not bin_bytes.startswith(b"\x7fELF"):
        raise ValueError(f"Expected Linux ELF binary for face-file-operations: {binary_path}")

    lines = [
        f"FROM {base_image}",
        "USER root",
        "COPY face-file-operations /usr/local/bin/face-file-operations",
    ]
    hashes = {"face-file-operations": sha256_bytes(bin_bytes)}

    probe_bytes = None
    if probe_binary_path is not None:
        if not probe_binary_path.is_file():
            raise FileNotFoundError(f"Probe binary not found: {probe_binary_path}")
        probe_bytes = probe_binary_path.read_bytes()
        if not probe_bytes.startswith(b"\x7fELF"):
            raise ValueError(f"Expected Linux ELF binary for storage-probe: {probe_binary_path}")
        lines.append("COPY storage-probe /usr/local/bin/storage-probe")
        hashes["storage-probe"] = sha256_bytes(probe_bytes)

    lines.extend(
        [
            "RUN chmod 0755 /usr/local/bin/face-file-operations /usr/local/bin/storage-probe",
            "USER 1000:1000",
            'ENTRYPOINT ["face-file-operations"]',
            "",
        ]
    )
    dockerfile_bytes = "\n".join(lines).encode("utf-8")

    output_tar_gz.parent.mkdir(parents=True, exist_ok=True)
    with output_tar_gz.open("wb") as raw_out:
        with gzip.GzipFile(filename="", mode="wb", fileobj=raw_out, mtime=0) as gz:
            with tarfile.open(mode="w", fileobj=gz) as tar:
                add_bytes_to_tar(tar, "face-file-operations", bin_bytes, mode=0o755)
                if probe_bytes is not None:
                    add_bytes_to_tar(tar, "storage-probe", probe_bytes, mode=0o755)
                add_bytes_to_tar(tar, "Dockerfile", dockerfile_bytes, mode=0o644)

    return {
        "kind": "rust-overlay",
        "base_image": base_image,
        "archive_path": str(output_tar_gz),
        "archive_bytes": output_tar_gz.stat().st_size,
        "archive_sha256": sha256_file(output_tar_gz),
        "binaries_sha256": hashes,
    }


def build_python_overlay(
    output_tar_gz: Path,
    role: str,
    base_image: str,
    root: Path = REPO_ROOT,
) -> dict:
    if role == "worker":
        entries: list[tuple[str, Path]] = []
        for p in sorted((root / "maintenance").glob("*.py")):
            entries.append((f"maintenance/{p.name}", p))
        for name in ("__init__.py", "client.py", "protocol.py"):
            p = root / "services" / "gpu_inference" / name
            if p.is_file():
                entries.append((f"services/gpu_inference/{name}", p))
        dockerfile_lines = [
            f"FROM {base_image}",
            "COPY maintenance/*.py /opt/",
            "COPY services/gpu_inference/__init__.py services/gpu_inference/client.py services/gpu_inference/protocol.py /opt/services/gpu_inference/",
            'CMD ["python3", "/opt/processing_worker.py"]',
            "",
        ]
    elif role == "calibrator":
        entries = [
            ("calibrator.py", root / "sidecar" / "calibrator.py"),
            ("tplgroups.py", root / "sidecar" / "tplgroups.py"),
            ("native_metadata.py", root / "sidecar" / "native_metadata.py"),
        ]
        dockerfile_lines = [
            f"FROM {base_image}",
            "COPY calibrator.py tplgroups.py native_metadata.py /opt/",
            'CMD ["python3", "/opt/calibrator.py"]',
            "",
        ]
    else:
        raise ValueError(f"Unsupported python overlay role: {role}")

    hashes: dict[str, str] = {}
    output_tar_gz.parent.mkdir(parents=True, exist_ok=True)
    with output_tar_gz.open("wb") as raw_out:
        with gzip.GzipFile(filename="", mode="wb", fileobj=raw_out, mtime=0) as gz:
            with tarfile.open(mode="w", fileobj=gz) as tar:
                for rel, path in entries:
                    data = path.read_bytes()
                    hashes[rel] = sha256_bytes(data)
                    add_bytes_to_tar(tar, rel, data, mode=0o644)
                add_bytes_to_tar(tar, "Dockerfile", "\n".join(dockerfile_lines).encode("utf-8"), mode=0o644)

    return {
        "kind": "python-overlay",
        "role": role,
        "base_image": base_image,
        "archive_path": str(output_tar_gz),
        "archive_bytes": output_tar_gz.stat().st_size,
        "archive_sha256": sha256_file(output_tar_gz),
        "files_sha256": hashes,
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)

    p_key = sub.add_parser("deps-key", help="Print the 12-char Web build-deps cache key")
    p_key.add_argument("--root", type=Path, default=REPO_ROOT)

    p_web = sub.add_parser("web-context", help="Build incremental Web source context tar.gz")
    p_web.add_argument("--output", type=Path, required=True)
    p_web.add_argument("--root", type=Path, default=REPO_ROOT)
    p_web.add_argument("--deps-image", default=None)
    p_web.add_argument("--runtime-base-image", default="face-admin-prod:current")
    p_web.add_argument("--run-typecheck-in-docker", action="store_true")

    p_wov = sub.add_parser("web-overlay", help="Build thin-layer Web standalone overlay tar.gz")
    p_wov.add_argument("--output", type=Path, required=True)
    p_wov.add_argument("--base-image", required=True)
    p_wov.add_argument("--payload", type=Path, required=True)

    p_rust = sub.add_parser("rust-overlay", help="Build thin-layer Rust binary overlay tar.gz")
    p_rust.add_argument("--output", type=Path, required=True)
    p_rust.add_argument("--base-image", required=True)
    p_rust.add_argument("--binary", type=Path, required=True)
    p_rust.add_argument("--probe-binary", type=Path, default=None)

    p_py = sub.add_parser("python-overlay", help="Build thin-layer Python overlay tar.gz")
    p_py.add_argument("--output", type=Path, required=True)
    p_py.add_argument("--role", choices=("worker", "calibrator"), required=True)
    p_py.add_argument("--base-image", required=True)
    p_py.add_argument("--root", type=Path, default=REPO_ROOT)

    args = parser.parse_args(argv)
    if args.command == "deps-key":
        print(compute_web_deps_key(args.root))
        return 0
    if args.command == "web-context":
        result = build_web_context(
            output_tar_gz=args.output,
            root=args.root,
            deps_image=args.deps_image,
            runtime_base_image=args.runtime_base_image,
            skip_typecheck=not args.run_typecheck_in_docker,
        )
    elif args.command == "web-overlay":
        result = build_web_overlay(
            output_tar_gz=args.output,
            base_image=args.base_image,
            payload_tar_gz=args.payload,
        )
    elif args.command == "rust-overlay":
        result = build_rust_overlay(
            output_tar_gz=args.output,
            base_image=args.base_image,
            binary_path=args.binary,
            probe_binary_path=args.probe_binary,
        )
    elif args.command == "python-overlay":
        result = build_python_overlay(
            output_tar_gz=args.output,
            role=args.role,
            base_image=args.base_image,
            root=args.root,
        )
    else:
        raise ValueError(f"Unknown command: {args.command}")

    print(json.dumps(result, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
