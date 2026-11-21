#!/usr/bin/env python3
"""
Executable E2E Test Suite Runner for OpenfansFace Multi-Agent Registration & Operations Tri-View (#20–#28).
Runs all 4 tiers of opaque-box tests, validates isolation, and generates compliance summaries.

Usage:
    python3 tests/e2e/run_e2e_tests.py
    python3 tests/e2e/run_e2e_tests.py --tier 1
    python3 tests/e2e/run_e2e_tests.py --verbose
"""
import argparse
import os
from pathlib import Path
import sys
import time
import unittest
import warnings

# Suppress ResourceWarning for cleaner E2E reporting
warnings.simplefilter("ignore", ResourceWarning)

REPO_ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO_ROOT))

TIER_MODULES = {
    1: "tests.e2e.test_tier1_feature_coverage",
    2: "tests.e2e.test_tier2_boundary_corner",
    3: "tests.e2e.test_tier3_cross_feature",
    4: "tests.e2e.test_tier4_workload_scenarios",
}

TIER_NAMES = {
    1: "Tier 1: Feature Coverage (28 Features, Isolated Happy Paths)",
    2: "Tier 2: Boundary & Corner Cases (Limits, Timeouts, Tolerances)",
    3: "Tier 3: Cross-Feature Combinations (Pairwise & Pipeline Lifecycles)",
    4: "Tier 4: Real-World Workload Scenarios (Heterogeneous Cluster, Failovers)",
}


def run_tier(tier_num: int, module_name: str, verbose: bool) -> tuple[int, int, float]:
    loader = unittest.TestLoader()
    suite = loader.loadTestsFromName(module_name)
    total_tests = suite.countTestCases()
    
    tier_title = TIER_NAMES.get(tier_num, f"Tier {tier_num}")
    print(f"\n{'='*75}")
    print(f"▶ Running {tier_title}")
    print(f"  Module: {module_name} ({total_tests} test cases)")
    print(f"{'='*75}")

    runner = unittest.TextTestRunner(verbosity=2 if verbose else 1)
    start_time = time.time()
    result = runner.run(suite)
    duration = time.time() - start_time

    passed = total_tests - len(result.failures) - len(result.errors)
    failed = len(result.failures) + len(result.errors)

    print(f"✔ Completed {tier_title} in {duration:.3f}s: {passed} passed, {failed} failed.")
    return passed, failed, duration


def main():
    parser = argparse.ArgumentParser(description="OpenfansFace Multi-Agent E2E Test Suite Runner")
    parser.add_argument("--tier", type=int, choices=[1, 2, 3, 4], help="Run specific tier only")
    parser.add_argument("--verbose", "-v", action="store_true", help="Verbose test execution")
    args = parser.parse_args()

    print("===========================================================================")
    print(" OpenfansFace Multi-Agent Registration & Tri-View E2E Test Suite Runner")
    print(" Reference: PROJECT.md § Feature Inventory (28 Features)")
    print(" Mode: Opaque-Box Hermetic Execution with Isolated Fixtures")
    print("===========================================================================")

    tiers_to_run = [args.tier] if args.tier else [1, 2, 3, 4]

    overall_passed = 0
    overall_failed = 0
    start_all = time.time()
    tier_stats = {}

    for t in tiers_to_run:
        mod = TIER_MODULES[t]
        passed, failed, duration = run_tier(t, mod, args.verbose)
        overall_passed += passed
        overall_failed += failed
        tier_stats[t] = (passed, failed, duration)

    total_time = time.time() - start_all

    print("\n" + "="*75)
    print("                 E2E TEST SUITE EXECUTION SUMMARY")
    print("="*75)
    for t in tiers_to_run:
        p, f, d = tier_stats[t]
        status = "PASSED" if f == 0 else "FAILED"
        print(f"  Tier {t}: {status:6} | {p:3d} passed, {f:2d} failed | {d:6.3f}s | {TIER_NAMES[t]}")
    print("-" * 75)
    total_tests = overall_passed + overall_failed
    pass_pct = (overall_passed / total_tests * 100.0) if total_tests > 0 else 0.0
    print(f"  Total:  {overall_passed}/{total_tests} passed ({pass_pct:.1f}%) in {total_time:.3f}s")
    print("="*75)

    if overall_failed > 0:
        print("\n❌ E2E TEST SUITE FAILED!")
        sys.exit(1)
    else:
        print("\n✅ ALL E2E TEST TIERS PASSED PERFECTLY!")
        sys.exit(0)


if __name__ == "__main__":
    main()
