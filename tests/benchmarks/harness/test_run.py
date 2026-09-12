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

INSTANCE = "inst_fake_0123456789abcdef"
FAKE_SCRIPT = """#!/bin/sh
if [ "$1" = "--version" ]; then
  echo "s-code 0.0.0-fake"
  exit 0
fi
printf 'cwd=%s\\n' "$PWD" >&2
printf 'workspace=%s\\n' "${S_CODE_WORKSPACE:-}" >&2
printf 'home=%s\\nruntime=%s\\nstate=%s\\n' "${S_CODE_HOME:-unset}" "${S_CODE_RUNTIME_DIR:-unset}" "${S_CODE_STATE_DIR:-unset}" >&2
printf 'url=%s\\ntoken=%s\\nautostart=%s\\nconfig=%s\\nlisten=%s\\n' "${S_CODE_URL:-unset}" "${S_CODE_TOKEN:-unset}" "${S_CODE_NO_AUTOSTART:-unset}" "${S_CODE_CONFIG:-unset}" "${S_CODE_DAEMON_LISTEN:-unset}" >&2
printf 'args=%s\\n' "$*" >&2
printf 'policy=%s budget=%s\\n' "${S_CODE_DAEMON_TOOL_HISTORY_POLICY:-unset}" "${S_CODE_DAEMON_TOOL_HISTORY_BUDGET_TOKENS:-unset}" >&2
if [ "${FAKE_CONNECTION:-1}" = "1" ] && [ -n "${S_CODE_RUNTIME_DIR:-}" ]; then
  printf '{"schema_version":1,"daemon_url":"http://127.0.0.1:1","token":"fake","instance_id":"%s","pid":%s,"started_at":"%s"}\\n' \\
    "${FAKE_INSTANCE:-inst_fake_0123456789abcdef}" "${FAKE_SERVICE_PID:-$$}" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" > "$S_CODE_RUNTIME_DIR/daemon.json"
fi
"""
PROCESS_OK = {"exit_code": 0, "timed_out": False, "events_truncated": False, "stderr_truncated": False}
SERVICE_OK = {
    "isolated": True,
    "connection_file": True,
    "instance_id": INSTANCE,
    "started_at": "2026-09-09T09:59:59Z",
    "pid_alive_after_run": False,
    "stopped": None,
    "reason": None,
}
HARNESS_OK = {
    "name": "s-code",
    "version": "s-code 0.0.0-fake",
    "expected_daemon_version": "0.0.0-fake",
    "source_revision": None,
    "source_dirty": None,
}
NO_ARM = {"tool_history_policy": None, "tool_history_budget_tokens": None}
FIXED_ARM = {"tool_history_policy": "fixed-count", "tool_history_budget_tokens": None}
BUDGET_ARM = {"tool_history_policy": "token-budget", "tool_history_budget_tokens": 12000}


def raw(
    kind: str,
    payload: dict | None = None,
    *,
    sequence: int = 1,
    timestamp: str = "2026-09-09T10:00:00.000000000Z",
    session_id: str = "ses_1",
    turn_id: str = "turn_1",
) -> str:
    return json.dumps(
        {
            "schema_version": "1",
            "sequence": sequence,
            "timestamp": timestamp,
            "kind": kind,
            "session_id": session_id,
            "turn_id": turn_id,
            "item_id": None,
            "payload": payload or {},
        }
    )


def typed(kind: str, **fields: object) -> str:
    return json.dumps({"schema_version": "1", "type": kind, "session_id": "ses_1", "turn_id": "turn_1", **fields})


def created(
    *,
    instance_id: str | None = INSTANCE,
    version: str | None = "0.0.0-fake",
    identity: bool = True,
    policy: str | None = "fixed-count",
    budget: int | None = None,
) -> str:
    """The daemon's turn.created row: identity from PR1 and the tool-history report side by side."""

    payload = {"status": "running", "item_id": "item_user"}
    if identity:
        payload["daemon"] = {"version": version, "instance_id": instance_id}
    if policy is not None:
        payload.update({"tool_history_policy": policy, "tool_history_budget_tokens": budget})
    return raw("turn.created", payload, sequence=1)


def call_completed(call: int, usage_events: int, input_units: int, output_units: int, *, sequence: int, outcome: str = "completed") -> str:
    return raw(
        "model.call.completed",
        {
            "model_call": call,
            "usage_events": usage_events,
            "input_tokens": input_units,
            "output_tokens": output_units,
            "accounted": usage_events > 0,
            "outcome": outcome,
        },
        sequence=sequence,
    )


def completed_turn(
    *, input_units: int = 300, output_units: int = 30, calls: int = 2, model: str = "vendor/model-a",
    policy: str | None = "fixed-count", budget: int | None = None,
) -> list[str]:
    """A consistent stream: one accounted usage event per call whose sum equals the turn total."""

    per_call = [(input_units // calls, output_units // calls)] * (calls - 1)
    per_call.append((input_units - sum(units[0] for units in per_call), output_units - sum(units[1] for units in per_call)))
    lines = [typed("turn.started"), created(policy=policy, budget=budget), raw("model.route.selected", {"model_id": model}, sequence=2)]
    for index, (input_tokens, output_tokens) in enumerate(per_call):
        call = index + 1
        lines.append(raw("model.usage", {"model_call": call, "input_tokens": input_tokens, "output_tokens": output_tokens}, sequence=10 + 2 * index))
        lines.append(call_completed(call, 1, input_tokens, output_tokens, sequence=11 + 2 * index))
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


def verdict(
    lines: list[str], *, passed: bool = True, process: dict | None = None, service: dict | None = None, harness: dict | None = None,
    requested: dict | None = None,
):
    """Reduce a synthetic stream exactly as the runner does and return (summary, accounting, reason)."""

    summary = harness_run.summarize_events(lines)
    usage = harness_run.usage_record(summary)
    accounting = harness_run.accounting_record(summary)
    reason = harness_run.exclusion_reason(
        passed, {**PROCESS_OK, **(process or {})}, summary, usage, accounting, {**SERVICE_OK, **(service or {})}, {**HARNESS_OK, **(harness or {})},
        requested or NO_ARM,
    )
    return summary, accounting, reason


class WorkRootTestCase(unittest.TestCase):
    """Every test that writes beneath ``.work`` creates it first, so a fresh checkout passes."""

    def setUp(self):
        WORK_ROOT.mkdir(parents=True, exist_ok=True)


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
        summary, accounting, reason = verdict(lines)
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
        self.assertEqual(summary["daemon"], {"version": "0.0.0-fake", "instance_id": INSTANCE})
        self.assertEqual(summary["tool_history_policy"], "fixed-count")
        self.assertIsNone(summary["tool_history_budget_tokens"])
        self.assertEqual(summary["total"], len(lines))
        self.assertEqual(summary["malformed_lines"], 0)
        self.assertEqual(summary["by_kind"]["model.usage"], 2)
        self.assertAlmostEqual(summary["last_timestamp"] - summary["first_timestamp"], 1.5)
        self.assertTrue(summary["lifecycle"]["valid"])
        self.assertTrue(accounting["verified"])
        self.assertIsNone(reason)

    def test_turn_total_that_disagrees_with_per_call_events_is_flagged(self):
        lines = completed_turn()
        del lines[next(index for index, line in enumerate(lines) if json.loads(line).get("kind") == "model.usage")]
        summary, accounting, reason = verdict(lines)
        usage = harness_run.usage_record(summary)
        self.assertEqual(usage["source"], "turn.usage")
        self.assertFalse(usage["matches_model_usage_sum"])
        self.assertFalse(accounting["verified"])
        self.assertIn("model call 1 reports 1 usage events but 0 were streamed", reason)

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
        self.assertEqual(summary["model_usage"]["unattributed_events"], 2)

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
            typed("turn.started"),
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


class AccountingTests(unittest.TestCase):
    """Comparable means every counted model call has complete, valid usage evidence."""

    def test_all_calls_completely_accounted_are_comparable(self):
        summary, accounting, reason = verdict(completed_turn(calls=3))
        self.assertTrue(accounting["verified"])
        self.assertEqual(accounting["model_calls"], 3)
        self.assertEqual([call["call"] for call in accounting["calls"]], [1, 2, 3])
        self.assertTrue(all(call["accounted"] and call["usage_events"] == 1 for call in accounting["calls"]))
        self.assertIsNone(reason)

    def test_two_calls_with_only_one_accounted_are_not_comparable(self):
        # The reviewer's reproduction: two calls, one usage event, a turn total
        # equal to that partial subtotal. The subtotal is kept, comparability is not.
        lines = [
            typed("turn.started"),
            created(),
            raw("model.usage", {"model_call": 1, "input_tokens": 200, "output_tokens": 20}, sequence=2),
            call_completed(1, 1, 200, 20, sequence=3),
            call_completed(2, 0, 0, 0, sequence=4),
            raw("turn.usage", {"model": "m", "input_units": 200, "output_units": 20, "total_units": 220, "model_calls": 2, "tool_calls": 1}, sequence=5),
            raw("turn.completed", sequence=6),
        ]
        summary, accounting, reason = verdict(lines)
        usage = harness_run.usage_record(summary)
        self.assertTrue(usage["matches_model_usage_sum"])
        self.assertEqual(usage["total_units"], 220)
        self.assertFalse(accounting["verified"])
        self.assertEqual(accounting["reasons"], ["model call 2 has no provider usage"])
        self.assertIn("accounting is incomplete", reason)
        self.assertIn("model call 2 has no provider usage", reason)

    def test_malformed_usage_counters_are_invalid_not_zero(self):
        lines = completed_turn(calls=2)
        bad = json.loads(lines[3])
        bad["payload"]["input_tokens"] = "150"
        lines[3] = json.dumps(bad)
        summary, accounting, reason = verdict(lines)
        self.assertEqual(summary["model_usage"]["invalid_events"], 1)
        self.assertEqual(summary["model_usage"]["events"], 1)
        self.assertEqual(summary["model_usage"]["input_units"], 150)
        self.assertIn("1 usage rows carry missing or invalid counters", accounting["reasons"])
        self.assertIn("model call 1 reports 1 usage events but 0 were streamed", accounting["reasons"])
        self.assertIn("accounting is incomplete", reason)

    def test_malformed_accounting_records_are_rejected(self):
        for field, value in (("input_tokens", None), ("usage_events", -1), ("accounted", "yes"), ("outcome", 3), ("model_call", 0)):
            with self.subTest(field=field):
                lines = completed_turn(calls=1)
                record = json.loads(lines[4])
                record["payload"][field] = value
                lines[4] = json.dumps(record)
                summary, accounting, reason = verdict(lines)
                self.assertEqual(summary["invalid_call_records"], 1)
                self.assertFalse(accounting["verified"])
                self.assertIn("accounting is incomplete", reason)

    def test_several_usage_objects_for_one_call_are_accounted(self):
        # An Anthropic-style stream reports usage at message_start and message_delta.
        lines = [
            typed("turn.started"),
            created(),
            raw("model.usage", {"model_call": 1, "input_tokens": 120, "output_tokens": 1}, sequence=2),
            raw("model.usage", {"model_call": 1, "input_tokens": 0, "output_tokens": 29}, sequence=3),
            call_completed(1, 2, 120, 30, sequence=4),
            raw("turn.usage", {"model": "m", "input_units": 120, "output_units": 30, "total_units": 150, "model_calls": 1, "tool_calls": 0}, sequence=5),
            raw("turn.completed", sequence=6),
        ]
        summary, accounting, reason = verdict(lines)
        self.assertTrue(accounting["verified"])
        self.assertEqual(accounting["calls"][0]["usage_events"], 2)
        self.assertIsNone(reason)

    def test_unattributed_duplicate_or_missing_records_are_not_verified(self):
        lines = completed_turn(calls=2)
        unattributed = json.loads(lines[3])
        del unattributed["payload"]["model_call"]
        lines[3] = json.dumps(unattributed)
        summary, accounting, _ = verdict(lines)
        self.assertIn("1 usage rows are not attributed to a model call", accounting["reasons"])

        lines = completed_turn(calls=2)
        lines.append(call_completed(2, 1, 150, 15, sequence=40))
        summary, accounting, _ = verdict(lines)
        self.assertIn("model call 2 has 2 accounting records", accounting["reasons"])

        lines = completed_turn(calls=2)
        total = json.loads(lines[-2])
        total["payload"]["model_calls"] = 3
        lines[-2] = json.dumps(total)
        summary, accounting, reason = verdict(lines)
        self.assertIn("the daemon counted 3 model calls but accounting records cover calls [1, 2]", accounting["reasons"])
        self.assertIn("accounting is incomplete", reason)

    def test_per_call_records_must_sum_to_the_turn_total(self):
        lines = completed_turn(calls=2)
        record = json.loads(lines[4])
        record["payload"]["input_tokens"] = 1
        lines[4] = json.dumps(record)
        summary, accounting, _ = verdict(lines)
        self.assertIn("the usage rows of model call 1 do not sum to its accounting record", accounting["reasons"])
        self.assertIn("the per-call accounting does not sum to the turn usage", accounting["reasons"])


class LifecycleTests(unittest.TestCase):
    """The reducer binds every piece of evidence to one started, terminated turn."""

    def assert_unbound(self, lines: list[str], fragment: str):
        summary, _, reason = verdict(lines)
        self.assertFalse(summary["lifecycle"]["valid"])
        self.assertIn("event lifecycle is not bound to one turn", reason)
        self.assertIn(fragment, reason)
        return summary

    def test_missing_turn_started_anchor_is_not_comparable(self):
        lines = [line for line in completed_turn() if json.loads(line).get("type") != "turn.started"]
        summary = self.assert_unbound(lines, "expected exactly one turn.started anchor, saw 0")
        # Nothing is folded without an anchor, but every row is still counted.
        self.assertEqual(summary["total"], len(lines))
        self.assertEqual(summary["lifecycle"]["unbound_rows"], len(lines))
        self.assertIsNone(summary["turn_usage"])

    def test_two_anchors_or_a_late_anchor_are_not_comparable(self):
        lines = completed_turn()
        lines.insert(4, typed("turn.started"))
        self.assert_unbound(lines, "saw 2")
        lines = completed_turn()
        lines.append(lines.pop(0))
        summary = self.assert_unbound(lines, "rows precede the turn.started anchor")
        self.assertEqual(summary["lifecycle"]["unbound_rows"], len(lines) - 1)

    def test_usage_rows_from_another_turn_are_rejected(self):
        lines = completed_turn(calls=1)
        lines[3] = raw("model.usage", {"model_call": 1, "input_tokens": 300, "output_tokens": 30}, sequence=10, turn_id="turn_2")
        summary = self.assert_unbound(lines, "1 rows carry another session or turn")
        self.assertEqual(summary["model_usage"]["events"], 0)

    def test_usage_rows_from_another_session_are_rejected(self):
        lines = completed_turn(calls=1)
        lines[3] = raw("model.usage", {"model_call": 1, "input_tokens": 300, "output_tokens": 30}, sequence=10, session_id="ses_2")
        summary = self.assert_unbound(lines, "1 rows carry another session or turn")
        self.assertEqual(summary["model_usage"]["events"], 0)

    def test_completion_row_for_another_turn_is_rejected(self):
        lines = completed_turn(calls=1)
        lines[-1] = raw("turn.completed", {"status": "completed"}, sequence=31, turn_id="turn_2")
        summary = self.assert_unbound(lines, "expected exactly one terminal turn event, saw 0")
        self.assertEqual(summary["status"], "unknown")
        lines = completed_turn(calls=1)
        lines[-2] = raw("turn.usage", {"model": "m", "input_units": 300, "output_units": 30, "model_calls": 1, "tool_calls": 1}, sequence=30, session_id="ses_2")
        summary = self.assert_unbound(lines, "1 rows carry another session or turn")
        self.assertIsNone(summary["turn_usage"])

    def test_contradictory_or_repeated_terminal_events_are_rejected(self):
        lines = completed_turn(calls=1)
        lines.append(typed("turn.failed", error_code="late_failure"))
        summary = self.assert_unbound(lines, "expected exactly one terminal turn event, saw 2 (turn.completed, turn.failed)")
        self.assertEqual(summary["status"], "conflicting")
        lines = completed_turn(calls=1)
        lines.append(raw("turn.completed", sequence=32))
        summary = self.assert_unbound(lines, "saw 2 (turn.completed, turn.completed)")
        self.assertEqual(summary["status"], "completed")

    def test_consistent_lifecycle_is_comparable_when_every_other_gate_passes(self):
        summary, accounting, reason = verdict(completed_turn())
        self.assertTrue(summary["lifecycle"]["valid"])
        self.assertEqual(summary["lifecycle"]["terminal"], ["turn.completed"])
        self.assertIsNone(reason)

    def test_unrelated_rows_from_another_turn_never_feed_counters(self):
        lines = completed_turn(calls=1)
        lines.insert(5, raw("context.compacted", {"omitted_messages": 3}, sequence=12, turn_id="turn_9"))
        summary = self.assert_unbound(lines, "1 rows carry another session or turn")
        self.assertEqual(summary["context_compactions"], 0)


class ComparabilityTests(unittest.TestCase):
    def setUp(self):
        self.summary = harness_run.summarize_events(completed_turn())
        self.usage = harness_run.usage_record(self.summary)
        self.accounting = harness_run.accounting_record(self.summary)

    def reason(self, *, passed=True, process=None, summary=None, usage=None, accounting=None, service=None, harness=None, requested=None):
        return harness_run.exclusion_reason(
            passed,
            {**PROCESS_OK, **(process or {})},
            {**self.summary, **(summary or {})},
            {**self.usage, **(usage or {})},
            {**self.accounting, **(accounting or {})},
            {**SERVICE_OK, **(service or {})},
            {**HARNESS_OK, **(harness or {})},
            requested or NO_ARM,
        )

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
        self.assertIn("accounting is incomplete: x", self.reason(accounting={"verified": False, "reasons": ["x"]}))
        self.assertIn("does not match", self.reason(usage={"matches_model_usage_sum": False}))

    def test_route_changes_are_never_silently_compared(self):
        self.assertIn("route changed", self.reason(summary={"route_fallbacks": 1}))
        self.assertIn("route changed", self.reason(summary={"routed_models": ["a", "b"]}))

    def test_daemon_identity_gates_comparability(self):
        self.assertIn("could not be verified: no file", self.reason(service={"reason": "no file"}))
        self.assertIn("carries no daemon identity", self.reason(summary={"daemon": None}))
        self.assertIn("carries no daemon identity", self.reason(summary={"daemon": {"version": "0.0.0-fake", "instance_id": None}}))
        self.assertIn("executed by a daemon other than the isolated service", self.reason(service={"instance_id": "inst_other"}))
        self.assertIn("version 0.0.0-fake does not match", self.reason(harness={"expected_daemon_version": "0.0.1"}))
        self.assertIn("does not match", self.reason(harness={"expected_daemon_version": None}))
        self.assertIsNone(self.reason(service={"pid_alive_after_run": True, "stopped": True}))

    def test_requested_arm_must_match_the_policy_the_service_reported(self):
        effective = {"tool_history_policy": "token-budget", "tool_history_budget_tokens": 12000}
        # Verified isolated daemon plus matching policy: comparable under both arms.
        self.assertIsNone(self.reason(summary=effective, requested=BUDGET_ARM))
        self.assertIsNone(self.reason(summary=FIXED_ARM, requested=FIXED_ARM))
        # Verified isolated daemon but a different policy than requested: excluded.
        self.assertIn("did not report", self.reason(summary={"tool_history_policy": None}, requested=BUDGET_ARM))
        self.assertIn("did not report", self.reason(summary={"tool_history_policy": None}, requested=FIXED_ARM))
        self.assertIn("different tool-history policy", self.reason(summary=FIXED_ARM, requested=BUDGET_ARM))
        self.assertIn("different tool-history policy", self.reason(summary=effective, requested=FIXED_ARM))
        self.assertIn("different tool-history policy", self.reason(summary={**effective, "tool_history_budget_tokens": 8000}, requested=BUDGET_ARM))
        # Without a requested arm the reported policy is recorded but not enforced.
        self.assertIsNone(self.reason(summary={"tool_history_policy": None}))
        self.assertIsNone(self.reason(summary=effective))
        # The policy gate never outranks the daemon identity or accounting gates.
        self.assertIn("executed by a daemon other than", self.reason(summary=FIXED_ARM, service={"instance_id": "inst_other"}, requested=BUDGET_ARM))
        self.assertIn("accounting is incomplete", self.reason(summary=effective, accounting={"verified": False, "reasons": ["x"]}, requested=BUDGET_ARM))


class ToolHistoryArmTests(unittest.TestCase):
    """Both arms are judged by the same lifecycle, accounting and identity gates plus the policy report."""

    def test_fixed_count_arm_is_comparable_with_full_accounting(self):
        summary, accounting, reason = verdict(completed_turn(calls=3), requested=FIXED_ARM)
        self.assertEqual((summary["tool_history_policy"], summary["tool_history_budget_tokens"]), ("fixed-count", None))
        self.assertTrue(accounting["verified"])
        self.assertIsNone(reason)

    def test_token_budget_arm_is_comparable_with_full_accounting(self):
        summary, accounting, reason = verdict(completed_turn(calls=3, policy="token-budget", budget=12000), requested=BUDGET_ARM)
        self.assertEqual((summary["tool_history_policy"], summary["tool_history_budget_tokens"]), ("token-budget", 12000))
        self.assertTrue(accounting["verified"])
        self.assertIsNone(reason)

    def test_policy_mismatch_is_not_comparable_but_keeps_the_evidence(self):
        summary, accounting, reason = verdict(completed_turn(calls=2), requested=BUDGET_ARM)
        self.assertTrue(summary["lifecycle"]["valid"])
        self.assertTrue(accounting["verified"])
        self.assertEqual(reason, "the service ran a different tool-history policy than requested")

    def test_missing_policy_report_fails_an_explicit_arm_only(self):
        lines = completed_turn(calls=2, policy=None)
        _, _, reason = verdict(lines, requested=BUDGET_ARM)
        self.assertEqual(reason, "the service did not report its tool-history policy")
        _, _, reason = verdict(lines, requested=FIXED_ARM)
        self.assertEqual(reason, "the service did not report its tool-history policy")
        _, _, reason = verdict(lines)
        self.assertIsNone(reason)

    def test_partial_accounting_fails_both_arms(self):
        for policy, budget, arm in (("fixed-count", None, FIXED_ARM), ("token-budget", 12000, BUDGET_ARM)):
            with self.subTest(policy=policy):
                lines = completed_turn(calls=2, policy=policy, budget=budget)
                lines.remove(next(line for line in lines if json.loads(line).get("kind") == "model.usage"))
                _, accounting, reason = verdict(lines, requested=arm)
                self.assertFalse(accounting["verified"])
                self.assertIn("accounting is incomplete", reason)


class ServiceTests(WorkRootTestCase):
    """The run owns the daemon that executes it and can tell when it did not."""

    def setUp(self):
        super().setUp()
        self.task = Path(tempfile.mkdtemp(prefix="harness-service-test.", dir=WORK_ROOT))
        self.addCleanup(shutil.rmtree, self.task, True)

    def connection(self, **fields: object) -> dict:
        runtime = self.task / "service/runtime"
        runtime.mkdir(parents=True, exist_ok=True)
        content = {"schema_version": 1, "daemon_url": "http://127.0.0.1:1", "token": "t", "instance_id": INSTANCE, "pid": 2**22 + 1, "started_at": "2026-09-09T10:00:00Z", **fields}
        (runtime / "daemon.json").write_text(json.dumps(content), encoding="utf-8")
        return {"runtime": runtime}

    def test_prepare_service_isolates_the_environment_and_never_copies_config(self):
        config = self.task / "config.toml"
        config.write_text("[model]\nprovider = 'x'\n", encoding="utf-8")
        stale_home = self.task / "stale-home"
        (stale_home / "run").mkdir(parents=True)
        (stale_home / "run/daemon.json").write_text("{}", encoding="utf-8")
        inherited = {"S_CODE_URL": "http://127.0.0.1:9/", "S_CODE_TOKEN": "stale", "S_CODE_NO_AUTOSTART": "1", "S_CODE_HOME": str(stale_home), "S_CODE_RUNTIME_DIR": str(stale_home / "run"), "S_CODE_STATE_DIR": str(stale_home / "state")}
        saved = {name: os.environ.get(name) for name in (*inherited, "S_CODE_CONFIG")}
        os.environ.update(inherited)
        os.environ.pop("S_CODE_CONFIG", None)
        try:
            output = self.task / "run"
            output.mkdir()
            environment, directories, record = harness_run.prepare_service(output, harness_run.service_config(str(config)))
        finally:
            for name, value in saved.items():
                if value is None:
                    os.environ.pop(name, None)
                else:
                    os.environ[name] = value
        for name in ("S_CODE_URL", "S_CODE_TOKEN", "S_CODE_NO_AUTOSTART"):
            self.assertNotIn(name, environment)
        self.assertEqual(environment["S_CODE_HOME"], str(output / "service/home"))
        self.assertEqual(environment["S_CODE_RUNTIME_DIR"], str(output / "service/runtime"))
        self.assertEqual(environment["S_CODE_STATE_DIR"], str(output / "service/state"))
        self.assertEqual(environment["S_CODE_DAEMON_LISTEN"], "127.0.0.1:0")
        self.assertEqual(environment["S_CODE_CONFIG"], str(config))
        self.assertEqual(record, {"source": "argument", "sha256": harness_run.hashlib.sha256(config.read_bytes()).hexdigest()})
        for directory in directories.values():
            self.assertTrue(directory.is_dir())
            self.assertEqual(directory.stat().st_mode & 0o777, 0o700)
            self.assertEqual(list(directory.iterdir()), [])
        self.assertTrue((stale_home / "run/daemon.json").is_file())

    def test_inspect_service_reads_only_a_fresh_connection_from_the_run(self):
        started = harness_run.parse_timestamp("2026-09-09T10:00:00Z")
        service, pid = harness_run.inspect_service({"runtime": self.task / "service/runtime"}, started)
        self.assertEqual(service["reason"], "the isolated service published no connection file")
        self.assertIsNone(pid)
        directories = self.connection()
        (directories["runtime"] / "daemon.json").write_text("not json", encoding="utf-8")
        service, _ = harness_run.inspect_service(directories, started)
        self.assertEqual(service["reason"], "the isolated service connection file is unreadable")
        directories = self.connection(started_at="2026-09-09T09:00:00Z")
        service, _ = harness_run.inspect_service(directories, started)
        self.assertEqual(service["reason"], "the isolated service connection predates this run")
        directories = self.connection(instance_id="")
        service, _ = harness_run.inspect_service(directories, started)
        self.assertEqual(service["reason"], "the isolated service published no instance id")
        directories = self.connection()
        service, pid = harness_run.inspect_service(directories, started)
        self.assertIsNone(service["reason"])
        self.assertEqual(service["instance_id"], INSTANCE)
        self.assertEqual(pid, 2**22 + 1)
        self.assertFalse(service["pid_alive_after_run"])
        self.assertNotIn("pid", service)

    def test_stop_service_terminates_only_a_live_daemon(self):
        self.assertIsNone(harness_run.stop_service(None))
        self.assertIsNone(harness_run.stop_service(2**22 + 1))
        # The launcher detaches the daemon with nohup, so stop it the same way:
        # as a process that is not the runner's own child.
        detached = int(subprocess.run(["sh", "-c", "sleep 60 >/dev/null 2>&1 & echo $!"], check=True, text=True, capture_output=True).stdout.strip())
        try:
            self.assertTrue(harness_run.process_alive(detached))
            self.assertTrue(harness_run.stop_service(detached))
            self.assertFalse(harness_run.process_alive(detached))
        finally:
            harness_run.stop_service(detached)
        # A child of the runner itself is reaped rather than left as a zombie.
        child = subprocess.Popen(["sleep", "60"])
        self.assertTrue(harness_run.stop_service(child.pid))
        self.assertFalse(harness_run.process_alive(child.pid))
        child.poll()  # the runner already reaped it; let Popen notice


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


class DrainTests(WorkRootTestCase):
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


class BytecodeCacheTests(WorkRootTestCase):
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


class RunIntegrationTests(WorkRootTestCase):
    """Drive the script as a subprocess against a fake S-Code executable."""

    def setUp(self):
        super().setUp()
        self.task = Path(tempfile.mkdtemp(prefix="harness-run-test.", dir=WORK_ROOT))
        self.addCleanup(shutil.rmtree, self.task, True)
        # A caller's environment that points at an existing, possibly stale,
        # daemon: the runner must never let the measured turn reach it.
        stale_home = self.task / "stale-home"
        (stale_home / "run").mkdir(parents=True)
        (stale_home / "run/daemon.json").write_text(json.dumps({"instance_id": "inst_stale", "pid": 1}), encoding="utf-8")
        self.stale_environment = {
            "S_CODE_URL": "http://127.0.0.1:9/",
            "S_CODE_TOKEN": "stale-token",
            "S_CODE_NO_AUTOSTART": "1",
            "S_CODE_HOME": str(stale_home),
            "S_CODE_RUNTIME_DIR": str(stale_home / "run"),
            "S_CODE_STATE_DIR": str(stale_home / "state"),
        }

    def fake_binary(self, body: str) -> Path:
        path = self.task / "s-code"
        path.write_text(FAKE_SCRIPT + body, encoding="utf-8")
        path.chmod(0o755)
        return path

    def run_script(self, *arguments: str, env: dict | None = None) -> subprocess.CompletedProcess:
        environment = dict(os.environ)
        environment.pop("S_CODE_CONFIG", None)
        environment.update(self.stale_environment)
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
        events = "\n".join(completed_turn(input_units=120, output_units=30, calls=1, policy="token-budget", budget=12000)) + "\n"
        (self.task / "events.txt").write_text(events, encoding="utf-8")
        self.fake_binary(
            "printf 'VERSION = \"fake\"\\n' > durable_queue/candidate.py\n"
            "mkdir -p tests/__pycache__ && printf 'x' > tests/__pycache__/test_durable_queue.cpython-312.pyc\n"
            'cat "$FAKE_EVENTS"\n'
        )
        completed = self.run_script(
            *self.common_arguments("run-1", "--model", "vendor/model-a", "--tool-history-policy", "token-budget", "--tool-history-budget-tokens", "12000"),
            env={"FAKE_EVENTS": str(self.task / "events.txt")},
        )
        self.assertEqual(completed.returncode, 2, completed.stderr)
        output = self.task / "run-1"
        record = json.loads((output / "run.json").read_text(encoding="utf-8"))
        summary = json.loads(completed.stdout)
        self.assertEqual(summary["record"], str(output / "run.json"))
        self.assertEqual(summary["total_units"], 150)
        self.assertEqual(record["schema_version"], 2)
        self.assertEqual(record["kind"], "harness_benchmark_run")
        self.assertEqual(record["harness"]["name"], "s-code")
        self.assertEqual(record["harness"]["version"], "s-code 0.0.0-fake")
        self.assertEqual(record["harness"]["expected_daemon_version"], "0.0.0-fake")
        task = harness_run.find_task(harness_run.load_manifest(), "project", "durable-task-queue")
        self.assertEqual(record["task"], {"track": "project", "id": "durable-task-queue", "protected_sha256": task["protected_sha256"]})
        self.assertEqual(record["configuration"]["model"], "vendor/model-a")
        self.assertEqual(record["configuration"]["permission_mode"], "workspace")
        self.assertEqual(record["configuration"]["tool_history_policy"], "token-budget")
        self.assertEqual(record["configuration"]["tool_history_budget_tokens"], 12000)
        self.assertEqual(record["turn"]["tool_history_policy"], "token-budget")
        self.assertEqual(record["turn"]["tool_history_budget_tokens"], 12000)
        self.assertEqual(record["configuration"]["timeout_seconds"], 30)
        self.assertEqual(record["configuration"]["service_config"], {"source": "none", "sha256": None})
        self.assertEqual(record["process"], PROCESS_OK)
        self.assertEqual(record["turn"]["status"], "completed")
        self.assertEqual(record["turn"]["model"], "vendor/model-a")
        self.assertEqual(record["turn"]["effective_model"], "vendor/model-a")
        self.assertEqual(record["turn"]["elapsed_seconds"], 1.5)
        self.assertEqual(record["usage"]["source"], "turn.usage")
        self.assertEqual((record["usage"]["input_units"], record["usage"]["output_units"], record["usage"]["total_units"]), (120, 30, 150))
        self.assertTrue(record["usage"]["matches_model_usage_sum"])
        self.assertTrue(record["usage"]["accounting_verified"])
        self.assertTrue(record["accounting"]["verified"])
        self.assertEqual(record["accounting"]["calls"], [{"call": 1, "usage_events": 1, "input_units": 120, "output_units": 30, "accounted": True, "outcome": "completed"}])
        self.assertTrue(record["lifecycle"]["valid"])
        self.assertEqual(record["events"]["total"], 8)
        self.assertEqual(record["events"]["malformed_lines"], 0)
        self.assertEqual(record["workspace_normalization"], {"removed_bytecode_caches": ["tests/__pycache__"]})
        # The fake published its own connection file into the run's runtime
        # directory and stamped the same instance into turn.created, so the
        # daemon identity is verified even though the outcome fails the run.
        service = record["service"]
        self.assertTrue(service["isolated"])
        self.assertTrue(service["connection_file"])
        self.assertEqual(service["instance_id"], INSTANCE)
        self.assertEqual(service["daemon_instance_id"], INSTANCE)
        self.assertEqual(service["daemon_version"], "0.0.0-fake")
        self.assertTrue(service["identity_verified"])
        self.assertIsNone(service["reason"])
        self.assertFalse(service["pid_alive_after_run"])
        self.assertIsNone(service["stopped"])
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
        # Isolation: the measured process saw only the run's own service
        # namespace, never the caller's daemon, token or autostart override.
        self.assertIn(f"home={(output / 'service/home').resolve()}\n", stderr)
        self.assertIn(f"runtime={(output / 'service/runtime').resolve()}\n", stderr)
        self.assertIn(f"state={(output / 'service/state').resolve()}\n", stderr)
        self.assertIn("url=unset\ntoken=unset\nautostart=unset\nconfig=unset\nlisten=127.0.0.1:0\n", stderr)
        self.assertIn("args=exec --stream-json --ephemeral --permission-mode workspace --timeout 30 --model vendor/model-a -- Complete the task described in README.md", stderr)
        # The requested arm reached the environment the isolated service starts from.
        self.assertIn("policy=token-budget budget=12000\n", stderr)
        self.assertEqual(json.loads((self.task / "stale-home/run/daemon.json").read_text(encoding="utf-8"))["instance_id"], "inst_stale")
        self.assertNotIn(str(self.task), json.dumps({key: value for key, value in record.items() if key != "grader"}))
        self.assertNotIn(str(self.task), json.dumps(record["grader"]))
        self.assertNotIn("stale-token", json.dumps(record))

    def test_turn_run_by_another_daemon_is_recorded_but_not_verified(self):
        cases = {
            "mismatched": (completed_turn(calls=1), {"FAKE_INSTANCE": "inst_other_0123456789abcdef"}, "executed by a daemon other than the isolated service"),
            "missing-identity": ([typed("turn.started"), created(identity=False), *completed_turn(calls=1)[2:]], {}, "carries no daemon identity"),
            "no-connection": (completed_turn(calls=1), {"FAKE_CONNECTION": "0"}, "published no connection file"),
            "wrong-version": ([typed("turn.started"), created(version="9.9.9"), *completed_turn(calls=1)[2:]], {}, "version 9.9.9 does not match"),
        }
        for name, (lines, env, fragment) in cases.items():
            with self.subTest(name=name):
                (self.task / f"{name}.txt").write_text("\n".join(lines) + "\n", encoding="utf-8")
                self.fake_binary('cat "$FAKE_EVENTS"\n')
                completed = self.run_script(*self.common_arguments(name), env={"FAKE_EVENTS": str(self.task / f"{name}.txt"), **env})
                self.assertEqual(completed.returncode, 2, completed.stderr)
                record = json.loads((self.task / name / "run.json").read_text(encoding="utf-8"))
                self.assertFalse(record["service"]["identity_verified"])
                self.assertFalse(record["comparable"])
                self.assertTrue(record["lifecycle"]["valid"])
                self.assertEqual(record["usage"]["total_units"], 330)
                self.assertIn("grader rejected", record["exclusion_reason"])
                summary = harness_run.summarize_events(lines)
                self.assertIn(fragment, harness_run.service_reason(record["service"], summary, record["harness"]))

    def test_requested_arm_reaches_the_isolated_service_and_is_verified(self):
        """requested arm -> isolated service environment -> turn.created report -> runner verdict."""

        cases = {
            "budget": (["--tool-history-policy", "token-budget", "--tool-history-budget-tokens", "12000"], "token-budget", 12000, "policy=token-budget budget=12000\n"),
            "fixed": (["--tool-history-policy", "fixed-count"], "fixed-count", None, "policy=fixed-count budget=unset\n"),
        }
        for name, (arguments, policy, budget, environment_line) in cases.items():
            with self.subTest(arm=name):
                matching = self.task / f"{name}-events.txt"
                matching.write_text("\n".join(completed_turn(calls=2, policy=policy, budget=budget)) + "\n", encoding="utf-8")
                self.fake_binary('cat "$FAKE_EVENTS"\n')
                # A budget inherited from the caller never leaks into a fixed-count arm.
                completed = self.run_script(*self.common_arguments(name, *arguments), env={"FAKE_EVENTS": str(matching), "S_CODE_DAEMON_TOOL_HISTORY_BUDGET_TOKENS": "999"})
                self.assertEqual(completed.returncode, 2, completed.stderr)
                record = json.loads((self.task / name / "run.json").read_text(encoding="utf-8"))
                self.assertIn(environment_line, (self.task / name / "s-code.stderr.log").read_text(encoding="utf-8"))
                self.assertEqual(record["configuration"]["tool_history_policy"], policy)
                self.assertEqual(record["configuration"]["tool_history_budget_tokens"], budget)
                self.assertEqual(record["turn"]["tool_history_policy"], policy)
                self.assertEqual(record["turn"]["tool_history_budget_tokens"], budget)
                self.assertTrue(record["service"]["identity_verified"])
                self.assertTrue(record["lifecycle"]["valid"])
                self.assertTrue(record["accounting"]["verified"])
                # Every gate but the outcome passes: the fake never solved the task.
                self.assertIn("grader rejected", record["exclusion_reason"])

                # The same request against a service that reports another policy is excluded.
                other = self.task / f"{name}-other-events.txt"
                other.write_text("\n".join(completed_turn(calls=2, policy="token-budget" if policy == "fixed-count" else "fixed-count", budget=None)) + "\n", encoding="utf-8")
                summary = harness_run.summarize_events(other.read_text(encoding="utf-8").splitlines())
                self.assertEqual(harness_run.tool_history_reason(summary, record["configuration"]), "the service ran a different tool-history policy than requested")

    def test_isolated_service_left_running_is_stopped_after_the_run(self):
        (self.task / "events.txt").write_text("\n".join(completed_turn(calls=1)) + "\n", encoding="utf-8")
        self.fake_binary(
            "sleep 300 >/dev/null 2>&1 &\n"
            'printf \'{"schema_version":1,"daemon_url":"http://127.0.0.1:1","token":"fake","instance_id":"%s","pid":%s,"started_at":"%s"}\\n\' '
            '"inst_fake_0123456789abcdef" "$!" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" > "$S_CODE_RUNTIME_DIR/daemon.json"\n'
            'printf \'%s\\n\' "$!" > "$S_CODE_RUNTIME_DIR/child.pid"\n'
            'cat "$FAKE_EVENTS"\n'
        )
        completed = self.run_script(*self.common_arguments("run-stop"), env={"FAKE_EVENTS": str(self.task / "events.txt")})
        self.assertEqual(completed.returncode, 2, completed.stderr)
        record = json.loads((self.task / "run-stop/run.json").read_text(encoding="utf-8"))
        self.assertTrue(record["service"]["identity_verified"])
        self.assertTrue(record["service"]["pid_alive_after_run"])
        self.assertTrue(record["service"]["stopped"])
        child = int((self.task / "run-stop/service/runtime/child.pid").read_text().strip())
        self.assertFalse(harness_run.process_alive(child))

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
        completed = self.run_script(
            *self.common_arguments("run-2", "--tool-history-policy", "fixed-count"),
            env={"FAKE_EVENTS": str(self.task / "events.txt"), "S_CODE_DAEMON_TOOL_HISTORY_BUDGET_TOKENS": "999"},
        )
        self.assertEqual(completed.returncode, 2, completed.stderr)
        record = json.loads((self.task / "run-2/run.json").read_text(encoding="utf-8"))
        self.assertEqual(record["process"]["exit_code"], 1)
        self.assertEqual(record["turn"]["status"], "failed")
        self.assertEqual(record["turn"]["error_code"], "turn_elapsed_timeout")
        self.assertEqual(record["usage"]["source"], "model.usage")
        self.assertEqual(record["usage"]["total_units"], 44)
        self.assertFalse(record["accounting"]["verified"])
        self.assertIsNone(record["configuration"]["model"])
        self.assertEqual(record["configuration"]["tool_history_policy"], "fixed-count")
        self.assertIsNone(record["configuration"]["tool_history_budget_tokens"])
        self.assertIsNone(record["turn"]["tool_history_policy"])
        # The fixed-count arm never forwards a budget, even one inherited from the environment.
        self.assertIn("policy=fixed-count budget=unset\n", (self.task / "run-2/s-code.stderr.log").read_text(encoding="utf-8"))
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
        self.assertFalse(record["lifecycle"]["valid"])
        self.assertFalse(record["comparable"])

    def test_invalid_invocations_fail_before_touching_the_workspace(self):
        self.fake_binary("exit 0\n")
        outside = Path(tempfile.gettempdir()) / "harness-run-outside"
        cases = [
            (["--output", str(outside)], "beneath .work/"),
            (["--s-code", str(self.task / "missing")], "not an executable file"),
            (["--task", "no-such-task"], "unknown project task"),
            (["--service-config", str(self.task / "missing.toml")], "not a regular file"),
            (["--tool-history-policy", "adaptive"], "invalid choice"),
            (["--tool-history-budget-tokens", "0"], "must be between 1 and"),
            (["--tool-history-policy", "token-budget"], "requires --tool-history-budget-tokens"),
            (["--tool-history-policy", "fixed-count", "--tool-history-budget-tokens", "12000"], "applies only to"),
            (["--tool-history-budget-tokens", "12000"], "applies only to"),
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
