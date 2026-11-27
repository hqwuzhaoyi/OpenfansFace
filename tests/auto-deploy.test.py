"""Unit tests for maintenance/auto_deploy.py."""
from __future__ import annotations

import importlib.util
from pathlib import Path
import unittest

REPO_ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("auto_deploy", REPO_ROOT / "maintenance" / "auto_deploy.py")
auto_deploy = importlib.util.module_from_spec(SPEC)
assert SPEC and SPEC.loader
SPEC.loader.exec_module(auto_deploy)


class AutoDeployTests(unittest.TestCase):
    def test_docs_only_change_skips_deploy(self) -> None:
        plan = auto_deploy.classify_changes(["docs/README.md", "AGENTS.md"], "docs: update notes")
        self.assertEqual(plan["skip"], True)
        self.assertEqual(plan["deploy_web"], False)
        self.assertEqual(plan["deploy_rust"], False)

    def test_web_only_change_deploys_web_only(self) -> None:
        for path in [
            "app/(workbench)/integration/page.tsx",
            "components/review/archive-resolution.tsx",
            "lib/archive-actions.ts",
            "package.json",
        ]:
            plan = auto_deploy.classify_changes([path], "feat: update web")
            self.assertEqual(plan["skip"], False, path)
            self.assertEqual(plan["deploy_web"], True, path)
            self.assertEqual(plan["deploy_rust"], False, path)

    def test_rust_only_change_deploys_rust_only(self) -> None:
        plan = auto_deploy.classify_changes(["services/file-operations/src/archive.rs"], "fix: rust archive")
        self.assertEqual(plan["skip"], False)
        self.assertEqual(plan["deploy_web"], False)
        self.assertEqual(plan["deploy_rust"], True)

    def test_both_web_and_rust_changes_deploy_both(self) -> None:
        plan = auto_deploy.classify_changes(
            ["services/file-operations/src/archive.rs", "lib/archive-actions.ts"],
            "feat: simplify resolution",
        )
        self.assertEqual(plan["skip"], False)
        self.assertEqual(plan["deploy_web"], True)
        self.assertEqual(plan["deploy_rust"], True)

    def test_skip_flag_overrides_code_changes(self) -> None:
        for flag in ("[skip deploy]", "[CI SKIP]"):
            plan = auto_deploy.classify_changes(
                ["services/file-operations/src/archive.rs", "lib/archive-actions.ts"],
                f"chore: cleanup {flag}",
            )
            self.assertEqual(plan["skip"], True)
            self.assertEqual(plan["deploy_web"], False)
            self.assertEqual(plan["deploy_rust"], False)

    def test_deploy_all_flag_forces_both_for_docs_change(self) -> None:
        plan = auto_deploy.classify_changes(["README.md"], "chore: trigger rebuild [Deploy All]")
        self.assertEqual(plan["skip"], False)
        self.assertEqual(plan["deploy_web"], True)
        self.assertEqual(plan["deploy_rust"], True)

    def test_compute_rust_deps_key_is_deterministic_12_hex(self) -> None:
        k1 = auto_deploy.compute_rust_deps_key(REPO_ROOT)
        k2 = auto_deploy.compute_rust_deps_key(REPO_ROOT)
        self.assertEqual(k1, k2)
        self.assertEqual(len(k1), 12)
        int(k1, 16)


if __name__ == "__main__":
    unittest.main()
