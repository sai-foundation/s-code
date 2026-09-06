#!/usr/bin/env python3
"""Regression coverage for affected-area routing and fail-closed CI gates."""

import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("ci_plan", ROOT / "scripts/ci-plan.py")
ci = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ci)


def outcomes(plan):
    return {job: {"result": "success" if job == "checks" or plan[job] else "skipped"}
            for job in ("checks", *ci.JOBS)}


def workflow(name):
    # Ruby is already a documented contributor/CLI-test prerequisite; no extra
    # Python packages or dependency installation are needed in the fast job.
    output = subprocess.check_output(["ruby", "-ryaml", "-rjson", "-e",
        "puts JSON.generate(YAML.safe_load(File.read(ARGV[0])))", str(ROOT / ".github/workflows" / name)])
    value = json.loads(output)
    if "true" in value:  # YAML 1.1 interprets the unquoted Actions 'on' key.
        value["on"] = value.pop("true")
    return value


class RoutingTests(unittest.TestCase):
    def test_readme_only_is_lightweight(self):
        p = ci.plan(["README.md", "GOVERNANCE.md", ".github/ISSUE_TEMPLATE/bug_report.yml"])
        self.assertFalse(any(p.values()))

    def test_documentation_and_plugin_changes_are_isolated(self):
        for path, expected in [("docs/guides/configuration.md", "docs"),
                               ("docs-site/app/page.tsx", "docs"),
                               ("clients/vscode/extension.js", "vscode"),
                               ("clients/jetbrains/build.gradle.kts", "jetbrains"),
                               ("tests/benchmarks/runner/package-lock.json", "benchmarks")]:
            with self.subTest(path=path):
                p = ci.plan([path])
                self.assertEqual({key for key in ci.JOBS if p[key]}, {expected})

    def test_learning_bridge_changes_run_real_daemon_only_on_linux(self):
        for path in ci.LEARNING_BRIDGE_FILES:
            with self.subTest(path=path):
                p = ci.plan([path])
                self.assertTrue(p["learning_e2e"])
                self.assertEqual({job for job in ci.JOBS if p[job]}, {"linux", "benchmarks"})
                self.assertFalse(p["runtime"] or p["rust"] or p["install"])
                ci.validate_gate(p, outcomes(p), public=False)
                missing = outcomes(p)
                missing["linux"]["result"] = "skipped"
                with self.assertRaises(ValueError):
                    ci.validate_gate(p, missing, public=False)
        for path in ("tests/benchmarks/self-evolving/results/quality08/quality08.json",
                     "tests/benchmarks/self-evolving/README.md"):
            p = ci.plan([path])
            self.assertFalse(p["learning_e2e"] or p["linux"] or p["macos"])
        linux = workflow("ci.yml")["jobs"]["linux"]
        step = next(step for step in linux["steps"] if step.get("name") == "Project learning through real daemon and local provider")
        self.assertEqual(step["if"], "needs.checks.outputs.learning_e2e == 'true'")
        self.assertEqual(step["env"]["S_CODE_REQUIRE_LEARNING_E2E"], "1")
        self.assertIn("cargo build --locked -p s-code-daemon", step["run"])
        sandbox = next(step for step in linux["steps"] if step.get("name") == "Install Linux sandbox backend")
        self.assertIn("needs.checks.outputs.learning_e2e == 'true'", sandbox["if"])

    def test_runtime_changes_cover_both_supported_platforms(self):
        for path in ["crates/daemon/src/lib.rs", "crates/platform-runtime/src/lib.rs", "crates/storage/migrations/new.sql", "crates/model-gateway/src/lib.rs", "crates/agent-adapter/src/lib.rs", "crates/context-engine/src/lib.rs", "crates/new-runtime/src/lib.rs", "tests/model_fixture.py", "scripts/s-code"]:
            with self.subTest(path=path):
                p = ci.plan([path])
                self.assertTrue(p["linux"] and p["macos"] and p["rust"] and p["runtime"])
                self.assertFalse(p["windows"])
        self.assertFalse(ci.plan(["crates/daemon/src/lib.rs"])["install"])
        self.assertTrue(ci.plan(["scripts/s-code"])["install"])

    def test_dependency_bootstrap_runs_installed_release_tests(self):
        for path in ("scripts/source-dependencies.sh", "tests/test-source-dependencies.py"):
            p = ci.plan([path])
            self.assertTrue(p["install"] and p["linux"] and p["macos"])
            self.assertFalse(p["windows"] or p["jetbrains"] or p["benchmarks"])

    def test_shared_protocol_and_build_inputs_expand_coverage(self):
        p = ci.plan(["crates/protocol/src/lib.rs"])
        for area in ("protocol", "rust", "runtime", "web", "vscode", "jetbrains", "linux", "macos"):
            self.assertTrue(p[area], area)
        for path in ["Cargo.lock", "Cargo.toml", "rust-toolchain.toml", ".github/workflows/ci.yml", "scripts/ci-plan.py", "crates/config/Cargo.toml", "new-component/input.bin"]:
            self.assertTrue(all(ci.plan([path])[area] for area in ci.AREAS), path)

    def test_public_javascript_gets_codeql_and_full_includes_windows(self):
        self.assertTrue(ci.plan(["web/src/main.ts"], public=True)["codeql"])
        self.assertFalse(ci.plan(["web/src/main.ts"])["codeql"])
        self.assertFalse(ci.plan(["README.md"], public=True)["codeql"])
        self.assertTrue(all(ci.plan([], full=True, public=True).values()))

    def test_dependency_updates_get_online_audits(self):
        for area in ("web", "docs-site", "clients/vscode", "tests/benchmarks/runner"):
            for filename in ("package.json", "package-lock.json", ".npmrc"):
                self.assertTrue(ci.plan([f"{area}/{filename}"])["audit"])
        self.assertFalse(ci.plan(["web/src/main.ts"])["audit"])
        self.assertFalse(ci.plan(["docs/guides/configuration.md"])["audit"])

    def test_privacy_runner_and_cases_execute_the_changed_suite(self):
        for path in ("tests/test-privacy-security-use-cases.sh", "tests/cases/privacy-security-use-cases.jsonl"):
            p = ci.plan([path])
            self.assertTrue(p["privacy"] and p["runtime"] and p["linux"] and p["macos"])
        self.assertFalse(ci.plan(["crates/daemon/src/lib.rs"])["privacy"])
        for platform in ("linux", "macos"):
            step = next(step for step in workflow("ci.yml")["jobs"][platform]["steps"]
                        if step.get("run") == "tests/test-privacy-security-use-cases.sh")
            self.assertEqual(step["if"], "needs.checks.outputs.privacy == 'true'")

    def test_git_diff_keeps_deleted_renamed_and_many_paths(self):
        (ROOT / ".work").mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(dir=ROOT / ".work") as directory:
            root = Path(directory)
            def git(*args):
                return subprocess.check_output(["git", "-C", str(root), *args], stderr=subprocess.DEVNULL).decode().strip()
            git("init", "-q")
            (root / "crates/daemon").mkdir(parents=True)
            (root / "crates/daemon/fixture.md").write_text("fixture")
            git("add", ".")
            git("-c", "user.name=CI Test", "-c", "user.email=test@example.invalid", "commit", "-qm", "base")
            base = git("rev-parse", "HEAD")
            (root / "docs").mkdir()
            (root / "crates/daemon/fixture.md").rename(root / "docs/renamed\nfixture.md")
            for index in range(350):
                (root / f"docs/{index}.md").write_text("new")
            git("add", "-A")
            git("-c", "user.name=CI Test", "-c", "user.email=test@example.invalid", "commit", "-qm", "rename")
            head = git("rev-parse", "HEAD")
            cwd = Path.cwd()
            try:
                os.chdir(root)
                paths = ci.changed_paths(base, head)
            finally:
                os.chdir(cwd)
            self.assertIn("crates/daemon/fixture.md", paths)
            self.assertIn("docs/renamed\nfixture.md", paths)
            self.assertEqual(len(paths), 352)
            self.assertTrue(ci.plan(paths)["runtime"])


class GateTests(unittest.TestCase):
    def test_only_selected_success_and_explicit_skips_pass(self):
        for p in [ci.plan(["README.md"]), ci.plan(["crates/daemon/src/lib.rs"]), ci.plan([], full=True)]:
            ci.validate_gate(p, outcomes(p), public=False, full=p["full"])
            for job in outcomes(p):
                for state in ("success", "failure", "cancelled", "skipped", None):
                    if state == outcomes(p)[job]["result"]:
                        continue
                    broken = outcomes(p)
                    broken[job]["result"] = state
                    with self.assertRaises(ValueError, msg=f"{job}: {state}"):
                        ci.validate_gate(p, broken, public=False, full=p["full"])
        p = ci.plan([], full=True, public=True)
        ci.validate_gate(p, outcomes(p), public=True, full=True)

    def test_missing_malformed_and_false_full_plans_fail(self):
        p = ci.plan(["README.md"])
        for bad in [None, {}, {**p, "linux": "false"}, {**p, "extra": False}]:
            with self.assertRaises(ValueError):
                ci.validate_gate(bad, outcomes(p), public=False)
        for bad in [{}, {"checks": {"result": "success"}}]:
            with self.assertRaises(ValueError):
                ci.validate_gate(p, bad, public=False)
        with self.assertRaises(ValueError):
            ci.validate_gate(p, outcomes(p), public=False, full=True)
        full = ci.plan([], full=True)
        full["windows"] = False
        with self.assertRaises(ValueError):
            ci.validate_gate(full, outcomes(full), public=False, full=True)

    def test_platform_and_public_scan_cannot_be_silently_skipped(self):
        for key in ("linux", "macos", "codeql", "learning_e2e"):
            p = ci.plan(["crates/protocol/src/lib.rs"], public=True)
            p[key] = False
            with self.assertRaises(ValueError):
                ci.validate_gate(p, outcomes(p), public=True)


class WorkflowTests(unittest.TestCase):
    def test_pr_gate_is_always_present_and_matches_job_graph(self):
        w = workflow("ci.yml")
        self.assertIn("pull_request", w["on"])
        self.assertFalse(w["on"]["pull_request"])
        self.assertNotIn("pull_request_target", w["on"])
        jobs = w["jobs"]
        self.assertEqual(set(jobs), {"checks", "source-gate", *ci.JOBS})
        self.assertEqual(set(jobs["source-gate"]["needs"]), {"checks", *ci.JOBS})
        self.assertEqual(jobs["source-gate"]["if"], "always()")
        for job in ci.JOBS:
            self.assertEqual(jobs[job]["if"], f"needs.checks.outputs.{job} == 'true'")
        gate = jobs["source-gate"]["steps"][-1]
        self.assertIn("inputs.full", gate["env"]["FULL"])
        self.assertIn("args+=(--full)", gate["run"])
        self.assertFalse(any("community-candidate.py qualify" in step.get("run", "")
                             for job in jobs.values() for step in job.get("steps", [])))

    def test_full_runs_and_release_evidence_are_separate_from_prs(self):
        w = workflow("rc.yml")
        self.assertEqual(set(w["on"]), {"schedule", "workflow_dispatch"})
        self.assertEqual(w["jobs"]["verify"]["with"], {"full": True})
        self.assertEqual(w["jobs"]["verify"]["uses"], "./.github/workflows/ci.yml")
        self.assertEqual(w["jobs"]["qualify"]["if"], "github.event_name == 'workflow_dispatch'")
        self.assertEqual(set(w["jobs"]["qualify"]["needs"]), {"source", "verify"})
        original_pr = next(step for step in w["jobs"]["source"]["steps"]
                           if step.get("with", {}).get("ref", "").startswith("refs/pull/"))
        self.assertEqual(original_pr["with"]["fetch-depth"], 0)
        self.assertIs(original_pr["with"]["persist-credentials"], False)
        self.assertIn('test "$(git rev-parse HEAD)" = "$HEAD"', w["jobs"]["source"]["steps"][-1]["run"])
        self.assertIn("github.run_attempt", w["jobs"]["qualify"]["steps"][-1]["with"]["name"])
        release = workflow("release.yml")
        scripts = "\n".join(step.get("run", "") for step in release["jobs"]["validate-source-release"]["steps"])
        self.assertIn('.event == "workflow_dispatch"', scripts)
        self.assertIn(".github/workflows/rc.yml", scripts)
        self.assertIn('--workflow-run-id "$candidate_run_id"', scripts)
        self.assertIn('--workflow-run-attempt "$candidate_run_attempt"', scripts)
        self.assertIn('"community-release-ready-$GITHUB_SHA-$candidate_run_attempt"', scripts)
        self.assertIn("--require-codeql", scripts)
        self.assertNotIn("S_CODE_SKIP_NETWORK_AUDIT", str(release))


if __name__ == "__main__":
    unittest.main()
