#!/usr/bin/env python3
"""Regression coverage for the S-Code harness benchmark run collector."""

from __future__ import annotations

import importlib.util
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[3]
SCRIPT = ROOT / "tests/benchmarks/harness/run.py"
WORK_ROOT = ROOT / ".work"
spec = importlib.util.spec_from_file_location("harness_run", SCRIPT)
harness_run = importlib.util.module_from_spec(spec)
spec.loader.exec_module(harness_run)

FAKE_SCRIPT = """#!/bin/sh
if [ "$1" = "--version" ]; then
  echo "s-code 0.0.0-fake"
  exit 0
fi
printf 'cwd=%s\\n' "$PWD" >&2
printf 'workspace=%s\\n' "${S_CODE_WORKSPACE:-}" >&2
printf 'args=%s\\n' "$*" >&2
"""
PROCESS_OK = {"exit_code": 0, "timed_out": False, "events_truncated": False, "stderr_truncated": False}


def raw(kind: str, payload: dict | None = None, *, sequence: int = 1, timestamp: str = "2026-09-09T10:00:00.000000000Z") -> str:
    return json.dumps(
        {
            "schema_version": "1",
            "sequence": sequence,
            "timestamp": timestamp,
            "kind": kind,
            "session_id": "ses_1",
            "turn_id": "turn_1",
            "item_id": None,
            "payload": payload or {},
        }
    )


def typed(kind: str, **fields: object) -> str:
    return json.dumps({"schema_version": "1", "type": kind, "session_id": "ses_1", "turn_id": "turn_1", **fields})


def completed_turn(*, input_units: int = 300, output_units: int = 30, calls: int = 2, model: str = "vendor/model-a") -> list[str]:
    """A consistent stream: per-call usage events whose sum equals the turn total."""

    per_call = [(input_units // calls, output_units // calls)] * (calls - 1)
    per_call.append((input_units - sum(units[0] for units in per_call), output_units - sum(units[1] for units in per_call)))
    lines = [typed("turn.started"), raw("model.route.selected", {"model_id": model}, sequence=2)]
    for index, (input_tokens, output_tokens) in enumerate(per_call):
        lines.append(raw("model.usage", {"input_tokens": input_tokens, "output_tokens": output_tokens}, sequence=10 + index))
    lines.append(raw("tool.completed", {"tool": "read_file"}, sequence=20))
    lines.append(
        raw(
            "turn.usage",
            {"model": model, "input_units": input_units, "output_units": output_units, "total_units": input_units + output_units, "model_calls": calls, "tool_calls": 1},
            sequence=30,
        )
    )
    lines.append(raw("turn.completed", {"status": "completed"}, sequence=31, timestamp="2026-09-09T10:00:01.500000000Z"))
    return lines


class TimestampTests(unittest.TestCase):
    def test_nanosecond_utc_and_offset_timestamps_parse(self):
        base = harness_run.parse_timestamp("2026-09-09T10:00:00Z")
        self.assertEqual(harness_run.parse_timestamp("2026-09-09T10:00:00.123456789Z"), base + 0.123456)
        self.assertEqual(harness_run.parse_timestamp("2026-09-09T12:00:00+02:00"), base)
        self.assertEqual(harness_run.parse_timestamp("2026-09-09T08:30:00.5-01:30"), base + 0.5)

    def test_invalid_timestamps_are_ignored(self):
        for value in (None, 12, "", "2026-09-09 10:00:00Z", "2026-13-01T00:00:00Z", "yesterday"):
            with self.subTest(value=value):
                self.assertIsNone(harness_run.parse_timestamp(value))


class EventSummaryTests(unittest.TestCase):
    def test_turn_usage_is_authoritative_and_cross_checked_against_per_call_events(self):
        lines = completed_turn()
        lines.insert(3, raw("context.compacted", {"omitted_messages": 3}, sequence=3))
        summary = harness_run.summarize_events(lines)
        usage = harness_run.usage_record(summary)
        self.assertEqual(
            usage,
            {
                "accounting": "sum_of_provider_usage_events",
                "source": "turn.usage",
                "input_units": 300,
                "output_units": 30,
                "model_calls": 2,
                "tool_calls": 1,
                "total_units": 330,
                "model_usage_events": 2,
                "model_usage_sum": {"input_units": 300, "output_units": 30},
                "matches_model_usage_sum": True,
            },
        )
        self.assertEqual(summary["status"], "completed")
        self.assertEqual(summary["model"], "vendor/model-a")
        self.assertEqual(summary["routed_models"], ["vendor/model-a"])
        self.assertEqual(harness_run.effective_model(summary), "vendor/model-a")
        self.assertEqual(summary["context_compactions"], 1)
        self.assertEqual((summary["session_id"], summary["turn_id"]), ("ses_1", "turn_1"))
        self.assertEqual(summary["total"], len(lines))
        self.assertEqual(summary["malformed_lines"], 0)
        self.assertEqual(summary["by_kind"]["model.usage"], 2)
        self.assertAlmostEqual(summary["last_timestamp"] - summary["first_timestamp"], 1.5)
        self.assertIsNone(harness_run.exclusion_reason(True, PROCESS_OK, summary, usage))

    def test_turn_total_that_disagrees_with_per_call_events_is_flagged(self):
        lines = completed_turn()
        del lines[2]  # one per-call usage event is missing from the evidence
        summary = harness_run.summarize_events(lines)
        usage = harness_run.usage_record(summary)
        self.assertEqual(usage["source"], "turn.usage")
        self.assertFalse(usage["matches_model_usage_sum"])
        self.assertIn("does not match", harness_run.exclusion_reason(True, PROCESS_OK, summary, usage))

    def test_failed_turn_falls_back_to_summed_model_usage(self):
        lines = [
            typed("turn.started"),
            raw("model.usage", {"input_tokens": 50, "output_tokens": 5}),
            raw("model.usage", {"input_tokens": 60, "output_tokens": 6}, sequence=2),
            typed("turn.failed", error_code="agent_failed"),
        ]
        summary = harness_run.summarize_events(lines)
        usage = harness_run.usage_record(summary)
        self.assertEqual(summary["status"], "failed")
        self.assertEqual(summary["error_code"], "agent_failed")
        self.assertEqual(usage["source"], "model.usage")
        self.assertEqual((usage["input_units"], usage["output_units"], usage["total_units"]), (110, 11, 121))
        self.assertIsNone(usage["model_calls"])
        self.assertIsNone(usage["tool_calls"])
        self.assertIsNone(usage["matches_model_usage_sum"])

    def test_malformed_rows_are_counted_and_usage_can_be_missing(self):
        lines = [
            "not json",
            "",
            json.dumps({"kind": "model.usage", "payload": {"input_tokens": 1, "output_tokens": 1}}),
            json.dumps(["schema_version", "1"]),
            json.dumps({"schema_version": "2", "kind": "turn.completed"}),
            json.dumps({"schema_version": "1", "payload": {}}),
        ]
        summary = harness_run.summarize_events(lines)
        self.assertEqual(summary["total"], 0)
        self.assertEqual(summary["malformed_lines"], 5)
        self.assertEqual(summary["status"], "unknown")
        usage = harness_run.usage_record(summary)
        self.assertEqual(usage["source"], "missing")
        self.assertIsNone(usage["total_units"])
        self.assertIsNone(harness_run.effective_model(summary))

    def test_invalid_turn_usage_counts_do_not_masquerade_as_provider_usage(self):
        lines = [
            raw("model.usage", {"input_tokens": 7, "output_tokens": 3}),
            raw("turn.usage", {"model": "m", "input_units": -1, "output_units": "3", "model_calls": True, "tool_calls": 1}),
            raw("turn.completed"),
        ]
        summary = harness_run.summarize_events(lines)
        self.assertIsNone(summary["turn_usage"])
        self.assertEqual(summary["model"], "m")
        self.assertEqual(harness_run.usage_record(summary)["source"], "model.usage")

    def test_approval_and_cancellation_rows_are_recorded(self):
        lines = [
            typed("turn.started"),
            typed("approval.required", item_id="item_1", payload={"approval_id": "apr_1"}),
            raw("model.route.selected", {"model_id": "a"}),
            raw("model.route.fallback", {"from_model_id": "a", "to_model_id": "b"}, sequence=2),
            raw("model.route.selected", {"model_id": "b"}, sequence=3),
            typed("turn.cancelled"),
        ]
        summary = harness_run.summarize_events(lines)
        self.assertEqual(summary["approvals_requested"], 1)
        self.assertEqual(summary["route_fallbacks"], 1)
        self.assertEqual(summary["routed_models"], ["a", "b"])
        self.assertIsNone(harness_run.effective_model(summary))
        self.assertEqual(summary["status"], "cancelled")


class ComparabilityTests(unittest.TestCase):
    def setUp(self):
        self.summary = harness_run.summarize_events(completed_turn())
        self.usage = harness_run.usage_record(self.summary)

    def reason(self, *, passed=True, process=None, summary=None, usage=None):
        return harness_run.exclusion_reason(passed, {**PROCESS_OK, **(process or {})}, {**self.summary, **(summary or {})}, {**self.usage, **(usage or {})})

    def test_outcome_is_checked_before_turn_process_and_usage(self):
        self.assertIsNone(self.reason())
        self.assertIn("grader rejected", self.reason(passed=False, process={"timed_out": True}))
        self.assertIn("timeout", self.reason(process={"timed_out": True, "exit_code": -9}))
        self.assertIn("status failed", self.reason(summary={"status": "failed"}, process={"exit_code": 1}))
        self.assertIn("exited with status 1", self.reason(process={"exit_code": 1}))

    def test_incomplete_or_inconsistent_evidence_is_never_comparable(self):
        self.assertIn("incomplete", self.reason(process={"events_truncated": True}))
        self.assertIn("incomplete", self.reason(summary={"malformed_lines": 1}))
        self.assertIn("no turn usage", self.reason(usage={"source": "model.usage"}))
        self.assertIn("no turn usage", self.reason(usage={"source": "missing"}))
        self.assertIn("provider reported no usage", self.reason(usage={"model_usage_events": 0}))
        self.assertIn("provider reported no usage", self.reason(usage={"total_units": 0}))
        self.assertIn("does not match", self.reason(usage={"matches_model_usage_sum": False}))

    def test_route_changes_are_never_silently_compared(self):
        self.assertIn("route changed", self.reason(summary={"route_fallbacks": 1}))
        self.assertIn("route changed", self.reason(summary={"routed_models": ["a", "b"]}))


class PromptTests(unittest.TestCase):
    def setUp(self):
        self.manifest = harness_run.load_manifest()

    def test_project_prompt_points_at_the_frozen_readme_and_paths(self):
        task = harness_run.find_task(self.manifest, "project", "durable-task-queue")
        prompt = harness_run.compose_prompt("project", task)
        self.assertEqual(prompt, harness_run.compose_prompt("project", task))
        self.assertIn("README.md in the current workspace", prompt)
        self.assertIn("Only create or modify these paths: durable_queue.", prompt)
        self.assertIn("including: README.md, tests.", prompt)
        self.assertIn("`python3 -m unittest discover -s tests -v`", prompt)
        self.assertTrue(prompt.endswith("\n"))

    def test_algorithm_prompt_names_the_implementation_and_test_file(self):
        task = harness_run.find_task(self.manifest, "algorithm", "wordy")
        prompt = harness_run.compose_prompt("algorithm", task)
        self.assertIn("Implement wordy.py", prompt)
        self.assertIn("every test in wordy_test.py passes", prompt)
        self.assertIn("`python3 -m unittest -v wordy_test.py`", prompt)
        self.assertNotIn("README.md", prompt)


class DrainTests(unittest.TestCase):
    def test_output_is_bounded_and_the_pipe_is_still_drained(self):
        with tempfile.TemporaryDirectory(dir=WORK_ROOT) as directory:
            destination = Path(directory) / "out"
            result: dict = {"truncated": False, "bytes": 0}
            harness_run.drain(io.BytesIO(b"x" * 100_000), destination, 10, result)
            self.assertEqual(destination.read_bytes(), b"x" * 10)
            self.assertEqual(result, {"truncated": True, "bytes": 10})
            result = {"truncated": False, "bytes": 0}
            harness_run.drain(io.BytesIO(b"y" * 100), destination, 1_000, result)
            self.assertEqual(destination.read_bytes(), b"y" * 100)
            self.assertEqual(result, {"truncated": False, "bytes": 100})


class BytecodeCacheTests(unittest.TestCase):
    def test_only_pure_bytecode_caches_are_removed(self):
        with tempfile.TemporaryDirectory(dir=WORK_ROOT) as directory:
            workspace = Path(directory)
            for cache in ("tests/__pycache__", "pkg/__pycache__", ".git/__pycache__", "mixed/__pycache__", "nested/__pycache__"):
                (workspace / cache).mkdir(parents=True)
                (workspace / cache / "module.cpython-312.pyc").write_bytes(b"\x00")
            (workspace / "mixed/__pycache__/notes.txt").write_text("keep")
            (workspace / "nested/__pycache__/inner").mkdir()
            (workspace / "pkg/module.py").write_text("SOURCE = 1\n")
            (workspace / "not_a_cache").mkdir()
            (workspace / "not_a_cache/data.pyc").write_bytes(b"\x00")
            removed = harness_run.remove_bytecode_caches(workspace)
            self.assertEqual(removed, ["pkg/__pycache__", "tests/__pycache__"])
            for kept in (".git/__pycache__", "mixed/__pycache__/notes.txt", "nested/__pycache__/inner", "pkg/module.py", "not_a_cache/data.pyc"):
                self.assertTrue((workspace / kept).exists(), kept)
            self.assertFalse((workspace / "tests/__pycache__").exists())


class RunIntegrationTests(unittest.TestCase):
    """Drive the script as a subprocess against a fake S-Code executable."""

    def setUp(self):
        WORK_ROOT.mkdir(exist_ok=True)
        self.task = Path(tempfile.mkdtemp(prefix="harness-run-test.", dir=WORK_ROOT))
        self.addCleanup(shutil.rmtree, self.task, True)

    def fake_binary(self, body: str) -> Path:
        path = self.task / "s-code"
        path.write_text(FAKE_SCRIPT + body, encoding="utf-8")
        path.chmod(0o755)
        return path

    def run_script(self, *arguments: str, env: dict | None = None) -> subprocess.CompletedProcess:
        environment = dict(os.environ)
        environment.update(env or {})
        return subprocess.run(
            [sys.executable, str(SCRIPT), *arguments],
            check=False,
            text=True,
            capture_output=True,
            env=environment,
            cwd=self.task,
        )

    def common_arguments(self, name: str, *extra: str) -> list[str]:
        return [
            "--track", "project", "--task", "durable-task-queue",
            "--s-code", str(self.task / "s-code"), "--output", str(self.task / name),
            "--timeout", "30", "--grace-seconds", "1", "--grader-timeout", "60", *extra,
        ]

    def test_completed_turn_is_recorded_and_graded_by_outcome(self):
        events = "\n".join(completed_turn(input_units=120, output_units=30, calls=1)) + "\n"
        (self.task / "events.txt").write_text(events, encoding="utf-8")
        self.fake_binary(
            "printf 'VERSION = \"fake\"\\n' > durable_queue/candidate.py\n"
            "mkdir -p tests/__pycache__ && printf 'x' > tests/__pycache__/test_durable_queue.cpython-312.pyc\n"
            'cat "$FAKE_EVENTS"\n'
        )
        completed = self.run_script(*self.common_arguments("run-1", "--model", "vendor/model-a"), env={"FAKE_EVENTS": str(self.task / "events.txt")})
        self.assertEqual(completed.returncode, 2, completed.stderr)
        output = self.task / "run-1"
        record = json.loads((output / "run.json").read_text(encoding="utf-8"))
        summary = json.loads(completed.stdout)
        self.assertEqual(summary["record"], str(output / "run.json"))
        self.assertEqual(summary["total_units"], 150)
        self.assertEqual(record["schema_version"], 1)
        self.assertEqual(record["kind"], "harness_benchmark_run")
        self.assertEqual(record["harness"]["name"], "s-code")
        self.assertEqual(record["harness"]["version"], "s-code 0.0.0-fake")
        task = harness_run.find_task(harness_run.load_manifest(), "project", "durable-task-queue")
        self.assertEqual(record["task"], {"track": "project", "id": "durable-task-queue", "protected_sha256": task["protected_sha256"]})
        self.assertEqual(record["configuration"]["model"], "vendor/model-a")
        self.assertEqual(record["configuration"]["permission_mode"], "workspace")
        self.assertEqual(record["configuration"]["timeout_seconds"], 30)
        self.assertEqual(record["process"], PROCESS_OK)
        self.assertEqual(record["turn"]["status"], "completed")
        self.assertEqual(record["turn"]["model"], "vendor/model-a")
        self.assertEqual(record["turn"]["effective_model"], "vendor/model-a")
        self.assertEqual(record["turn"]["elapsed_seconds"], 1.5)
        self.assertEqual(record["usage"]["source"], "turn.usage")
        self.assertEqual((record["usage"]["input_units"], record["usage"]["output_units"], record["usage"]["total_units"]), (120, 30, 150))
        self.assertTrue(record["usage"]["matches_model_usage_sum"])
        self.assertEqual(record["events"]["total"], 6)
        self.assertEqual(record["events"]["malformed_lines"], 0)
        self.assertEqual(record["workspace_normalization"], {"removed_bytecode_caches": ["tests/__pycache__"]})
        # Outcome is primary: the fake candidate does not implement the task, so the
        # unchanged grader ran the protected tests and rejected the workspace.
        self.assertFalse(record["passed"])
        self.assertFalse(record["comparable"])
        self.assertIn("grader rejected", record["exclusion_reason"])
        self.assertEqual(record["grader"]["observed_tests"], record["grader"]["expected_tests"])
        self.assertEqual(record["grader"]["trust_model"], "trusted_workspace_only")
        self.assertEqual(json.loads((output / "grade.json").read_text(encoding="utf-8")), record["grader"])
        self.assertEqual((output / "events.jsonl").read_text(encoding="utf-8"), events)
        self.assertEqual((output / "prompt.txt").read_text(encoding="utf-8"), harness_run.compose_prompt("project", task))
        self.assertTrue((output / "grade.log").is_file())
        self.assertTrue((output / "workspace/.git").is_dir())
        stderr = (output / "s-code.stderr.log").read_text(encoding="utf-8")
        self.assertIn(f"cwd={(output / 'workspace').resolve()}\n", stderr)
        self.assertIn(f"workspace={(output / 'workspace').resolve().as_uri()}\n", stderr)
        self.assertIn("args=exec --stream-json --ephemeral --permission-mode workspace --timeout 30 --model vendor/model-a -- Complete the task described in README.md", stderr)
        self.assertNotIn(str(self.task), json.dumps({key: value for key, value in record.items() if key != "grader"}))
        self.assertNotIn(str(self.task), json.dumps(record["grader"]))

    def test_failed_turn_keeps_partial_usage_and_is_not_comparable(self):
        events = "\n".join(
            [
                typed("turn.started"),
                raw("model.usage", {"input_tokens": 40, "output_tokens": 4}),
                typed("turn.failed", error_code="turn_elapsed_timeout"),
            ]
        ) + "\n"
        (self.task / "events.txt").write_text(events, encoding="utf-8")
        self.fake_binary('cat "$FAKE_EVENTS"\nexit 1\n')
        completed = self.run_script(*self.common_arguments("run-2"), env={"FAKE_EVENTS": str(self.task / "events.txt")})
        self.assertEqual(completed.returncode, 2, completed.stderr)
        record = json.loads((self.task / "run-2/run.json").read_text(encoding="utf-8"))
        self.assertEqual(record["process"]["exit_code"], 1)
        self.assertEqual(record["turn"]["status"], "failed")
        self.assertEqual(record["turn"]["error_code"], "turn_elapsed_timeout")
        self.assertEqual(record["usage"]["source"], "model.usage")
        self.assertEqual(record["usage"]["total_units"], 44)
        self.assertIsNone(record["configuration"]["model"])
        self.assertFalse(record["comparable"])
        self.assertEqual(record["workspace_normalization"]["removed_bytecode_caches"], [])

    def test_hung_process_is_killed_after_the_grace_period(self):
        self.fake_binary("exec sleep 60\n")
        completed = self.run_script(*self.common_arguments("run-3", "--timeout", "1"))
        self.assertEqual(completed.returncode, 2, completed.stderr)
        record = json.loads((self.task / "run-3/run.json").read_text(encoding="utf-8"))
        self.assertTrue(record["process"]["timed_out"])
        self.assertLess(record["elapsed_seconds"], 20)
        self.assertNotEqual(record["process"]["exit_code"], 0)
        self.assertEqual(record["turn"]["status"], "unknown")
        self.assertEqual(record["usage"]["source"], "missing")
        self.assertFalse(record["comparable"])

    def test_invalid_invocations_fail_before_touching_the_workspace(self):
        self.fake_binary("exit 0\n")
        outside = Path(tempfile.gettempdir()) / "harness-run-outside"
        cases = [
            (["--output", str(outside)], "beneath .work/"),
            (["--s-code", str(self.task / "missing")], "not an executable file"),
            (["--task", "no-such-task"], "unknown project task"),
        ]
        for override, message in cases:
            with self.subTest(message=message):
                completed = self.run_script(*self.common_arguments("run-invalid"), *override)
                self.assertEqual(completed.returncode, 2)
                self.assertIn(message, completed.stderr)
                self.assertFalse((self.task / "run-invalid").exists())
        self.assertFalse(outside.exists())


if __name__ == "__main__":
    unittest.main()
