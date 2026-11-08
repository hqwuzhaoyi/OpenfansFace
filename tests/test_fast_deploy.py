import tarfile
import tempfile
import unittest
from pathlib import Path

from maintenance import fast_deploy


class FastDeployTest(unittest.TestCase):
    def test_web_context_excludes_local_and_target_artifacts(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / "web-context.tar.gz"
            meta = fast_deploy.build_web_context(
                output_tar_gz=out,
                runtime_base_image="face-admin-prod:test",
                skip_typecheck=True,
            )
            self.assertEqual(meta["kind"], "web-context")
            self.assertEqual(len(meta["deps_key"]), 12)
            self.assertTrue(out.is_file())
            # Context archive should stay small (< 5 MB)
            self.assertLess(meta["archive_bytes"], 5 * 1024 * 1024)

            with tarfile.open(out, "r:gz") as tar:
                names = set(tar.getnames())
                self.assertIn("Dockerfile.incremental", names)
                self.assertIn("package.json", names)
                self.assertIn("next.config.ts", names)
                self.assertFalse(any(n.startswith("node_modules/") for n in names))
                self.assertFalse(any(n.startswith(".next/") for n in names))
                self.assertFalse(any(n.startswith(".local/") for n in names))
                self.assertFalse(any(n.startswith("services/") for n in names))
                inc = tar.extractfile("Dockerfile.incremental").read().decode("utf-8")
                self.assertIn(f"FROM {meta['deps_image']} AS build", inc)
                self.assertIn("ENV NEXT_TELEMETRY_DISABLED=1 NEXT_SKIP_TYPECHECK=1", inc)
                self.assertIn("FROM face-admin-prod:test AS runtime", inc)
                self.assertIn("RUN rm -rf /app/.next /app/public", inc)

    def test_rust_overlay_validates_elf_and_creates_bundle(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp_dir = Path(tmp)
            fake_bin = tmp_dir / "face-file-operations"
            fake_bin.write_bytes(b"\x7fELF" + b"rust-binary-payload")
            out = tmp_dir / "rust-overlay.tar.gz"

            meta = fast_deploy.build_rust_overlay(
                output_tar_gz=out,
                base_image="sha256:c460984d410b74bd2b62f4922df25f80460c0e6f484ef3e07a9ee006bd94d469",
                binary_path=fake_bin,
            )
            self.assertEqual(meta["kind"], "rust-overlay")
            self.assertIn("face-file-operations", meta["binaries_sha256"])
            with tarfile.open(out, "r:gz") as tar:
                names = set(tar.getnames())
                self.assertEqual(names, {"face-file-operations", "Dockerfile"})
                dockerfile = tar.extractfile("Dockerfile").read().decode("utf-8")
                self.assertIn("USER 1000:1000", dockerfile)
                self.assertIn("COPY face-file-operations /usr/local/bin/face-file-operations", dockerfile)

    def test_rust_overlay_rejects_non_elf(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp_dir = Path(tmp)
            bad_bin = tmp_dir / "face-file-operations"
            bad_bin.write_bytes(b"\xcf\xfa\xed\xfe-mach-o")
            out = tmp_dir / "rust-overlay.tar.gz"
            with self.assertRaises(ValueError):
                fast_deploy.build_rust_overlay(
                    output_tar_gz=out,
                    base_image="face-file-operations:latest",
                    binary_path=bad_bin,
                )

    def test_python_overlay_worker_and_calibrator(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp_dir = Path(tmp)
            worker_tar = tmp_dir / "worker.tar.gz"
            w_meta = fast_deploy.build_python_overlay(
                output_tar_gz=worker_tar,
                role="worker",
                base_image="face-processing:local",
            )
            self.assertEqual(w_meta["role"], "worker")
            self.assertIn("maintenance/processing_worker.py", w_meta["files_sha256"])

            cal_tar = tmp_dir / "calibrator.tar.gz"
            c_meta = fast_deploy.build_python_overlay(
                output_tar_gz=cal_tar,
                role="calibrator",
                base_image="face-calibrator:local",
            )
            self.assertEqual(c_meta["role"], "calibrator")
            self.assertIn("calibrator.py", c_meta["files_sha256"])


if __name__ == "__main__":
    unittest.main()
