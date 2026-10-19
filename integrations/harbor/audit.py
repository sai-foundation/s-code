#!/usr/bin/env python3
"""Find Harbor trials whose score does not reflect S-Code.

Usage: audit.py [--json] JOB_DIR [JOB_DIR ...]

A trial passes when Harbor finished it, S-Code reached the model, and the
verifier produced a reward from tests that actually ran. Below full credit,
that means pytest's summary line when the task's test script runs pytest.
Other verifiers print no common summary, so their rewards stand with a warning
unless their output is empty or shows a known setup failure. The script is read
from the task folder recorded in the trial's config.json; if it can't be read,
the trial is flagged. Flagged trials should be rerun rather than scored. The
exit status is 1 when any trial is flagged.

This module uses only the Python standard library.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path
from typing import Any

# Exceptions that record what the agent did rather than a broken harness.
AGENT_OUTCOMES = {"NonZeroAgentExitCodeError", "AgentTimeoutError"}
MODEL_FAILURES = {
    "NetworkConnectionError": "S-Code could not reach the model endpoint",
    "UnknownApiError": "the model endpoint rejected S-Code's requests",
}
# Setup failures seen in verifier output when its tests never started.
VERIFIER_SETUP_FAILURES = (
    "Hash Sum mismatch",
    "Unable to fetch some archives",
    "command not found",
    "Could not resolve host",
    "Temporary failure in name resolution",
    "Could not GET",
    "UnknownHostException",
)
PYTEST_SUMMARY = re.compile(
    r"^=+ .*\b(passed|failed|errors?|no tests ran)\b.* in [0-9.]+s", re.MULTILINE
)


def _read_json(path: Path) -> dict[str, Any] | None:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError):
        return None
    return value if isinstance(value, dict) else None


def _verifier_runs_pytest(trial: Path) -> bool | None:
    """Whether the task's test script runs pytest, or None if it can't be read."""

    config = _read_json(trial / "config.json") or {}
    task = config.get("task")
    path = task.get("path") if isinstance(task, dict) else None
    if not isinstance(path, str) or not path:
        return None
    for name in ("test.sh", "test.bat"):
        try:
            script = (Path(path) / "tests" / name).read_text(encoding="utf-8", errors="replace")
        except (OSError, ValueError):
            continue
        return "pytest" in script
    return None


def _first_line(text: object) -> str:
    lines = [line.strip() for line in str(text or "").splitlines() if line.strip()]
    return lines[0][:200] if lines else ""


def _request_outcomes(events: Path) -> dict[str, int]:
    """Count model request outcomes in S-Code's event log."""

    counts = {"accepted": 0, "rejected": 0, "connection_error": 0}
    try:
        lines = events.read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError:
        return counts
    for line in lines:
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(event, dict) and event.get("kind") == "privacy.request.finished":
            request = (event.get("payload") or {}).get("request") or {}
            status = request.get("status") if isinstance(request, dict) else None
            if status in counts:
                counts[status] += 1
    return counts


def audit_trial(trial: Path) -> dict[str, Any]:
    """Return the verdict for one Harbor trial directory."""

    reasons: list[str] = []
    warnings: list[str] = []
    result = _read_json(trial / "result.json")
    reward = None
    s_code_status = None
    if result is None or not result.get("finished_at"):
        reasons.append("Harbor did not finish the trial")
        result = result or {}

    exception = result.get("exception_info") or {}
    exception_type = exception.get("exception_type")
    message = exception.get("exception_message") or ""
    if exception_type in MODEL_FAILURES:
        reasons.append(MODEL_FAILURES[exception_type])
    elif exception_type == "CancelledError":
        reasons.append("the trial was cancelled")
    elif exception_type and exception_type not in AGENT_OUTCOMES:
        if "failed to solve" in message or "did not complete successfully" in message:
            reasons.append("the task image failed to build")
        else:
            reasons.append(f"{exception_type}: {_first_line(message)}")

    verifier_result = result.get("verifier_result") or {}
    rewards = verifier_result.get("rewards") if isinstance(verifier_result, dict) else None
    if isinstance(rewards, dict) and isinstance(rewards.get("reward"), (int, float)):
        reward = rewards["reward"]
    elif not reasons:
        reasons.append("the verifier produced no reward")

    # A reward below 1 counts only if the tests actually ran; a verifier that
    # could not install its tools also reports 0.
    if reward is not None and reward < 1:
        try:
            output = (trial / "verifier" / "test-stdout.txt").read_text(
                encoding="utf-8", errors="replace"
            )
        except OSError:
            output = ""
        if not PYTEST_SUMMARY.search(output):
            failure = next((s for s in VERIFIER_SETUP_FAILURES if s in output), None)
            if failure:
                reasons.append(f"the verifier's tests never ran ({failure})")
            else:
                runs_pytest = _verifier_runs_pytest(trial)
                if runs_pytest is None:
                    reasons.append(
                        "the verifier output has no test summary, and the task's test script could not be read"
                    )
                elif runs_pytest or not output.strip():
                    reasons.append("the verifier output has no test summary")
                else:
                    warnings.append(
                        "the task's verifier does not use pytest, so its tests could not be confirmed to have run"
                    )

    provenance = _read_json(trial / "agent" / "provenance.json")
    if provenance is None:
        if not reasons:
            reasons.append("S-Code provenance is missing")
    else:
        run = provenance.get("result") or {}
        execution = provenance.get("execution") or {}
        s_code_status = run.get("status")
        failure_kind = run.get("failure_kind")
        if failure_kind == "model_unreachable" and MODEL_FAILURES["NetworkConnectionError"] not in reasons:
            reasons.append(MODEL_FAILURES["NetworkConnectionError"])
        if failure_kind == "model_rejected" and MODEL_FAILURES["UnknownApiError"] not in reasons:
            reasons.append(MODEL_FAILURES["UnknownApiError"])
        if execution.get("post_run_cleanup_status") not in (None, "pending", "succeeded"):
            reasons.append("S-Code's cleanup did not confirm that it stopped")
    # The event log covers every adapter version; provenance counts are newer.
    requests = _request_outcomes(trial / "agent" / "events.jsonl")
    failed_requests = requests["connection_error"] + requests["rejected"]
    if failed_requests and not requests["accepted"]:
        reason = "S-Code never got a model response"
        if not any(r in reasons for r in MODEL_FAILURES.values()):
            reasons.append(reason)
    elif failed_requests:
        warnings.append(f"{failed_requests} model request(s) failed before succeeding")

    return {
        "trial": trial.name,
        "reward": reward,
        "s_code_status": s_code_status,
        "exception": exception_type,
        "flagged": bool(reasons),
        "reasons": reasons,
        "warnings": warnings,
    }


def trial_directories(job: Path) -> list[Path]:
    return sorted(
        path
        for path in job.iterdir()
        if path.is_dir()
        and any((path / name).exists() for name in ("result.json", "config.json", "trial.log"))
    )


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--json", action="store_true", help="print machine-readable results")
    parser.add_argument("jobs", nargs="+", type=Path, help="Harbor job directories")
    args = parser.parse_args(argv)

    verdicts = []
    for job in args.jobs:
        if not job.is_dir():
            parser.error(f"not a directory: {job}")
        for trial in trial_directories(job):
            verdict = audit_trial(trial)
            verdict["job"] = job.name
            verdicts.append(verdict)

    flagged = [verdict for verdict in verdicts if verdict["flagged"]]
    if args.json:
        print(json.dumps({"trials": verdicts, "flagged": len(flagged)}, indent=2))
    else:
        for verdict in verdicts:
            label = "FLAGGED" if verdict["flagged"] else "ok"
            notes = "; ".join(verdict["reasons"] + verdict["warnings"])
            print(
                f"{label:7} {verdict['job']}/{verdict['trial']} reward={verdict['reward']}"
                f" s-code={verdict['s_code_status']}" + (f" - {notes}" if notes else "")
            )
        print(f"{len(verdicts)} trial(s), {len(flagged)} flagged")
    return 1 if flagged else 0


if __name__ == "__main__":
    sys.exit(main())
