#!/usr/bin/env python3
"""Tests for integrations/harbor/audit.py."""

from __future__ import annotations

import contextlib
import importlib.util
import io
import json
from pathlib import Path
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("harbor_audit", ROOT / "integrations" / "harbor" / "audit.py")
audit = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(audit)

PYTEST_PASS = "============================== 9 passed in 0.58s ===============================\n"
PYTEST_FAIL = "=================== 1 failed, 2 passed, 3 warnings in 0.60s ====================\n"


def request(status: str) -> str:
    return json.dumps(
        {"kind": "privacy.request.finished", "payload": {"request": {"status": status}}}
    )


class AuditTests(unittest.TestCase):
    def setUp(self) -> None:
        self.job = Path(tempfile.mkdtemp(prefix="harbor-audit-job."))

    def trial(
        self,
        name: str,
        *,
        reward: float | None = 1.0,
        exception: tuple[str, str] | None = None,
        verifier_output: str | None = PYTEST_PASS,
        requests: tuple[str, ...] = ("accepted",),
        cleanup: str = "succeeded",
        finished: bool = True,
    ) -> Path:
        trial = self.job / name
        (trial / "agent").mkdir(parents=True)
        (trial / "verifier").mkdir()
        (trial / "trial.log").write_text("log\n", encoding="utf-8")
        if finished:
            result = {
                "finished_at": "2026-10-18T21:00:00",
                "exception_info": None,
                "verifier_result": None if reward is None else {"rewards": {"reward": reward}},
            }
            if exception is not None:
                result["exception_info"] = {
                    "exception_type": exception[0],
                    "exception_message": exception[1],
                }
            (trial / "result.json").write_text(json.dumps(result), encoding="utf-8")
        if verifier_output is not None:
            (trial / "verifier" / "test-stdout.txt").write_text(verifier_output, encoding="utf-8")
        (trial / "agent" / "events.jsonl").write_text(
            "".join(request(status) + "\n" for status in requests), encoding="utf-8"
        )
        (trial / "agent" / "provenance.json").write_text(
            json.dumps(
                {
                    "result": {"status": "completed", "failure_kind": None},
                    "execution": {"post_run_cleanup_status": cleanup},
                }
            ),
            encoding="utf-8",
        )
        return trial

    def verdict(self, trial: Path) -> dict:
        return audit.audit_trial(trial)

    def test_genuine_results_pass(self) -> None:
        self.assertFalse(self.verdict(self.trial("solved"))["flagged"])
        failed = self.verdict(
            self.trial(
                "agent-failed",
                reward=0.0,
                exception=("NonZeroAgentExitCodeError", "Command failed (exit 1)"),
                verifier_output=PYTEST_FAIL,
            )
        )
        self.assertFalse(failed["flagged"], failed)
        timed_out = self.verdict(
            self.trial("timed-out", reward=0.0, exception=("AgentTimeoutError", "timed out"), verifier_output=PYTEST_FAIL)
        )
        self.assertFalse(timed_out["flagged"], timed_out)

    def test_false_zero_from_a_broken_verifier_is_flagged(self) -> None:
        verdict = self.verdict(
            self.trial(
                "broken-verifier",
                reward=0.0,
                verifier_output="E: Failed to fetch x.deb  Hash Sum mismatch\n/tests/test.sh: line 23: uv: command not found\n",
            )
        )
        self.assertEqual(verdict["reasons"], ["the verifier's tests never ran (Hash Sum mismatch)"])
        missing = self.verdict(self.trial("no-summary", reward=0.0, verifier_output="done\n"))
        self.assertEqual(missing["reasons"], ["the verifier output has no test summary"])

    def test_infrastructure_failures_are_flagged(self) -> None:
        cases = {
            "build": (("RuntimeError", "failed to solve: process did not complete successfully"), "the task image failed to build"),
            "unreachable": (("NetworkConnectionError", "no response"), "S-Code could not reach the model endpoint"),
            "rejected": (("UnknownApiError", "rejected"), "the model endpoint rejected S-Code's requests"),
            "cancelled": (("CancelledError", ""), "the trial was cancelled"),
            "adapter": (("ValueError", "bad bundle\nmore detail"), "ValueError: bad bundle"),
        }
        for name, (exception, reason) in cases.items():
            with self.subTest(name=name):
                verdict = self.verdict(self.trial(name, reward=None, exception=exception))
                self.assertTrue(verdict["flagged"])
                self.assertIn(reason, verdict["reasons"])

    def test_unfinished_trials_and_unanswered_models_are_flagged(self) -> None:
        unfinished = self.verdict(self.trial("unfinished", finished=False))
        self.assertIn("Harbor did not finish the trial", unfinished["reasons"])
        unanswered = self.verdict(
            self.trial(
                "unanswered",
                reward=0.0,
                exception=("NonZeroAgentExitCodeError", "turn failed"),
                verifier_output=PYTEST_FAIL,
                requests=("connection_error",) * 3,
            )
        )
        self.assertEqual(unanswered["reasons"], ["S-Code never got a model response"])
        retried = self.verdict(self.trial("retried", requests=("connection_error", "accepted")))
        self.assertFalse(retried["flagged"])
        self.assertEqual(retried["warnings"], ["1 model request(s) failed before succeeding"])

    def test_cleanup_failure_is_flagged(self) -> None:
        verdict = self.verdict(self.trial("cleanup", cleanup="failed"))
        self.assertEqual(verdict["reasons"], ["S-Code's cleanup did not confirm that it stopped"])

    def test_exit_status_and_json_output(self) -> None:
        self.trial("solved")
        with contextlib.redirect_stdout(io.StringIO()) as output:
            self.assertEqual(audit.main(["--json", str(self.job)]), 0)
        report = json.loads(output.getvalue())
        self.assertEqual((len(report["trials"]), report["flagged"]), (1, 0))

        self.trial("broken", reward=0.0, verifier_output="uv: command not found\n")
        with contextlib.redirect_stdout(io.StringIO()) as output:
            self.assertEqual(audit.main([str(self.job)]), 1)
        self.assertIn("2 trial(s), 1 flagged", output.getvalue())


if __name__ == "__main__":
    unittest.main()
