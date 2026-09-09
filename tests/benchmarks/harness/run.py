#!/usr/bin/env python3
"""Run one frozen coding-harness benchmark task through an S-Code binary.

The script prepares a frozen task with the existing benchmark tooling, runs
``s-code exec --stream-json --ephemeral`` inside the prepared workspace, keeps
the raw event stream, grades the final workspace with the unchanged protected
grader and writes one structured run record beside the raw artifacts.
"""

from __future__ import annotations

import argparse
from datetime import datetime, timedelta, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import threading
import time
from typing import Any, BinaryIO, Iterable


REPOSITORY = Path(__file__).resolve().parents[3]
BENCHMARK_SCRIPT = REPOSITORY / "tests/test-harness-benchmark.py"
MANIFEST_PATH = REPOSITORY / "tests/benchmarks/harness/manifest.json"
WORK_ROOT = (REPOSITORY / ".work").resolve()
SCHEMA_VERSION = 1
RECORD_KIND = "harness_benchmark_run"
PROMPT_VERSION = 1
# The daemon totals every usage event the provider streamed during the turn.
# The figures are provider usage units under that summation, not an audited
# token bill; see docs/testing/README.md for the per-provider semantics.
USAGE_ACCOUNTING = "sum_of_provider_usage_events"
PERMISSION_MODES = ("workspace", "accept-edits", "manual")
MAX_EVENT_BYTES = 64 * 1024 * 1024
MAX_STDERR_BYTES = 4 * 1024 * 1024
ARTIFACTS = {
    "workspace": "workspace",
    "prompt": "prompt.txt",
    "events": "events.jsonl",
    "stderr": "s-code.stderr.log",
    "grade": "grade.json",
    "grade_log": "grade.log",
}
RECORD_NAME = "run.json"
TIMESTAMP = re.compile(
    r"^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})(?:\.(\d{1,9}))?(Z|[+-]\d{2}:\d{2})$"
)


def load_manifest() -> dict[str, Any]:
    with MANIFEST_PATH.open(encoding="utf-8") as source:
        manifest = json.load(source)
    if manifest.get("schema_version") != 3:
        raise ValueError("unsupported harness benchmark manifest schema")
    return manifest


def find_task(manifest: dict[str, Any], track: str, task_id: str) -> dict[str, Any]:
    for candidate in manifest["tracks"].get(track, []):
        if candidate["id"] == task_id:
            return candidate
    raise ValueError(f"unknown {track} task: {task_id}")


def output_destination(value: str) -> Path:
    destination = Path(value).resolve()
    if destination == WORK_ROOT or WORK_ROOT not in destination.parents:
        raise ValueError("benchmark run output must be beneath .work/")
    if destination.exists():
        raise ValueError(f"output directory already exists: {destination}")
    return destination


def resolve_binary(value: str) -> Path:
    candidate = value if "/" in value else shutil.which(value)
    if candidate is None:
        raise ValueError(f"S-Code binary is not on PATH: {value}")
    path = Path(os.path.abspath(candidate))
    if not path.is_file() or not os.access(path, os.X_OK):
        raise ValueError(f"S-Code binary is not an executable file: {value}")
    return path


def compose_prompt(track: str, task: dict[str, Any]) -> str:
    """Return the fixed instruction text handed to the agent for a task."""

    if track == "algorithm":
        opening = (
            f"Implement {task['implementation']} in the current workspace so that "
            f"every test in {task['test_file']} passes."
        )
    else:
        opening = "Complete the task described in README.md in the current workspace."
    editable = ", ".join(task["editable_paths"])
    protected = ", ".join(task["protected_paths"])
    verification = " ".join(task["grader"])
    return "\n".join(
        [
            opening,
            f"Only create or modify these paths: {editable}.",
            f"Do not modify any other path, including: {protected}.",
            f"Verify your work with `{verification}` before you finish, "
            "and report the observed result.",
        ]
    ) + "\n"


def parse_timestamp(value: Any) -> float | None:
    """Parse the daemon's RFC 3339 timestamps, which may carry nanoseconds."""

    if not isinstance(value, str):
        return None
    match = TIMESTAMP.match(value)
    if match is None:
        return None
    year, month, day, hour, minute, second, fraction, zone = match.groups()
    microsecond = int((fraction or "0")[:6].ljust(6, "0"))
    if zone == "Z":
        offset = timedelta()
    else:
        sign = 1 if zone[0] == "+" else -1
        offset = sign * timedelta(hours=int(zone[1:3]), minutes=int(zone[4:6]))
    try:
        moment = datetime(
            int(year), int(month), int(day), int(hour), int(minute), int(second),
            microsecond, tzinfo=timezone(offset),
        )
    except ValueError:
        return None
    return moment.timestamp()


def as_count(value: Any) -> int | None:
    if isinstance(value, bool) or not isinstance(value, int) or value < 0:
        return None
    return value


def summarize_events(lines: Iterable[str]) -> dict[str, Any]:
    """Reduce the versioned ``--stream-json`` rows of one turn to accounting facts."""

    summary: dict[str, Any] = {
        "total": 0,
        "malformed_lines": 0,
        "by_kind": {},
        "session_id": None,
        "turn_id": None,
        "status": "unknown",
        "error_code": None,
        "model": None,
        "routed_models": [],
        "route_fallbacks": 0,
        "context_compactions": 0,
        "approvals_requested": 0,
        "first_timestamp": None,
        "last_timestamp": None,
        "turn_usage": None,
        "model_usage": {"input_units": 0, "output_units": 0, "events": 0},
    }
    for line in lines:
        text = line.strip()
        if not text:
            continue
        try:
            row = json.loads(text)
        except json.JSONDecodeError:
            summary["malformed_lines"] += 1
            continue
        kind = row.get("kind", row.get("type")) if isinstance(row, dict) else None
        if not isinstance(kind, str) or row.get("schema_version") != "1":
            summary["malformed_lines"] += 1
            continue
        summary["total"] += 1
        summary["by_kind"][kind] = summary["by_kind"].get(kind, 0) + 1
        payload = row.get("payload") if isinstance(row.get("payload"), dict) else {}
        stamp = parse_timestamp(row.get("timestamp"))
        if stamp is not None:
            first = summary["first_timestamp"]
            last = summary["last_timestamp"]
            summary["first_timestamp"] = stamp if first is None else min(first, stamp)
            summary["last_timestamp"] = stamp if last is None else max(last, stamp)
        if kind == "turn.started":
            for field in ("session_id", "turn_id"):
                if isinstance(row.get(field), str):
                    summary[field] = row[field]
        elif kind == "model.usage":
            usage = summary["model_usage"]
            usage["input_units"] += as_count(payload.get("input_tokens")) or 0
            usage["output_units"] += as_count(payload.get("output_tokens")) or 0
            usage["events"] += 1
        elif kind == "turn.usage":
            counts = {
                "input_units": as_count(payload.get("input_units")),
                "output_units": as_count(payload.get("output_units")),
                "model_calls": as_count(payload.get("model_calls")),
                "tool_calls": as_count(payload.get("tool_calls")),
            }
            if all(value is not None for value in counts.values()):
                summary["turn_usage"] = counts
            if isinstance(payload.get("model"), str) and payload["model"]:
                summary["model"] = payload["model"]
        elif kind == "model.route.selected":
            model_id = payload.get("model_id")
            if isinstance(model_id, str) and model_id not in summary["routed_models"]:
                summary["routed_models"].append(model_id)
        elif kind == "model.route.fallback":
            summary["route_fallbacks"] += 1
        elif kind == "context.compacted":
            summary["context_compactions"] += 1
        elif kind == "approval.required":
            summary["approvals_requested"] += 1
        elif kind == "turn.completed":
            summary["status"] = "completed"
        elif kind == "turn.failed":
            summary["status"] = "failed"
            error_code = row.get("error_code", payload.get("error_code"))
            summary["error_code"] = error_code if isinstance(error_code, str) else None
        elif kind == "turn.cancelled":
            summary["status"] = "cancelled"
    return summary


def usage_record(summary: dict[str, Any]) -> dict[str, Any]:
    """Prefer the daemon's turn total and cross-check it against per-call events.

    ``turn.usage`` is the daemon's total for the turn. ``model.usage`` rows are
    the individual provider usage events that total is built from, so their
    sum must match; a summed fallback is labelled and never carries call counts.
    """

    per_call = summary["model_usage"]
    model_usage_sum = {"input_units": per_call["input_units"], "output_units": per_call["output_units"]}
    matches = None
    if summary["turn_usage"] is not None:
        counts = dict(summary["turn_usage"])
        source = "turn.usage"
        matches = (counts["input_units"], counts["output_units"]) == (
            model_usage_sum["input_units"], model_usage_sum["output_units"],
        )
    elif per_call["events"] > 0:
        counts = {**model_usage_sum, "model_calls": None, "tool_calls": None}
        source = "model.usage"
    else:
        counts = {"input_units": None, "output_units": None, "model_calls": None, "tool_calls": None}
        source = "missing"
    total = None
    if counts["input_units"] is not None and counts["output_units"] is not None:
        total = counts["input_units"] + counts["output_units"]
    return {
        "accounting": USAGE_ACCOUNTING,
        "source": source,
        **counts,
        "total_units": total,
        "model_usage_events": per_call["events"],
        "model_usage_sum": model_usage_sum,
        "matches_model_usage_sum": matches,
    }


def effective_model(summary: dict[str, Any]) -> str | None:
    """Name the one model that served the turn, or nothing when that is ambiguous."""

    routed = summary["routed_models"]
    if len(routed) == 1:
        return routed[0]
    return summary["model"] if not routed else None


def exclusion_reason(
    grader_passed: bool, process: dict[str, Any], summary: dict[str, Any], usage: dict[str, Any]
) -> str | None:
    """Explain why a run must stay out of an efficiency comparison, outcome first."""

    if not grader_passed:
        return "the protected grader rejected the final workspace"
    if process["timed_out"]:
        return "the S-Code process exceeded the run timeout"
    if summary["status"] != "completed":
        return f"the turn ended with status {summary['status']}"
    if process["exit_code"] != 0:
        return f"s-code exited with status {process['exit_code']}"
    if process["events_truncated"] or summary["malformed_lines"]:
        return "the event evidence is incomplete"
    if usage["source"] != "turn.usage":
        return "the daemon reported no turn usage"
    if usage["model_usage_events"] == 0 or usage["total_units"] == 0:
        return "the provider reported no usage"
    if not usage["matches_model_usage_sum"]:
        return "the turn usage does not match the per-call usage events"
    if summary["route_fallbacks"] or len(summary["routed_models"]) > 1:
        return "the model route changed during the turn"
    return None


def drain(stream: BinaryIO, destination: Path, limit: int, result: dict[str, Any]) -> None:
    """Copy a pipe to a file up to ``limit`` bytes, then keep the pipe drained."""

    written = 0
    with destination.open("wb") as sink:
        while chunk := stream.read(65536):
            keep = chunk[: max(limit - written, 0)]
            if keep:
                sink.write(keep)
                written += len(keep)
            if len(keep) < len(chunk):
                result["truncated"] = True
    result["bytes"] = written


def run_agent(binary: Path, workspace: Path, prompt: str, args: argparse.Namespace, output: Path) -> dict[str, Any]:
    """Run ``s-code exec`` in the workspace and return process facts for the record."""

    command = [
        str(binary), "exec", "--stream-json", "--ephemeral",
        "--permission-mode", args.permission_mode, "--timeout", str(args.timeout),
    ]
    if args.model:
        command += ["--model", args.model]
    command += ["--", prompt]
    environment = dict(os.environ)
    environment["S_CODE_WORKSPACE"] = workspace.as_uri()
    outputs = {name: {"truncated": False, "bytes": 0} for name in ("events", "stderr")}
    started = time.monotonic()
    started_at = datetime.now(timezone.utc)
    process = subprocess.Popen(
        command,
        cwd=workspace,
        env=environment,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    threads = [
        threading.Thread(target=drain, args=(process.stdout, output / ARTIFACTS["events"], MAX_EVENT_BYTES, outputs["events"]), daemon=True),
        threading.Thread(target=drain, args=(process.stderr, output / ARTIFACTS["stderr"], MAX_STDERR_BYTES, outputs["stderr"]), daemon=True),
    ]
    for thread in threads:
        thread.start()
    timed_out = False
    try:
        process.wait(timeout=args.timeout + args.grace_seconds)
    except subprocess.TimeoutExpired:
        # The CLI's own --timeout is the primary bound and releases the ephemeral
        # session. Only the client process is killed here: the launcher may have
        # autostarted a shared local service in the same process group.
        timed_out = True
        process.kill()
        process.wait()
    for thread in threads:
        thread.join(timeout=max(args.grace_seconds, 1))
    return {
        "started_at": started_at.isoformat(timespec="seconds").replace("+00:00", "Z"),
        "elapsed_seconds": round(time.monotonic() - started, 3),
        "exit_code": process.returncode,
        "timed_out": timed_out,
        "events_truncated": outputs["events"]["truncated"],
        "stderr_truncated": outputs["stderr"]["truncated"],
    }


def remove_bytecode_caches(workspace: Path) -> list[str]:
    """Benchmark hygiene: drop ``__pycache__`` directories that hold only ``.pyc`` files.

    Running the task's documented test command compiles protected test modules
    into ``__pycache__``, which the protected grader rejects as an undeclared
    path. Only directories named ``__pycache__`` whose entries are all regular
    ``.pyc`` files are removed, so no task source or other candidate file can
    be touched; everything else is left for the grader to judge, and the
    removed paths are recorded in the run record.
    """

    removed: list[str] = []
    for root, directories, _ in os.walk(workspace, topdown=True, followlinks=False):
        root_path = Path(root)
        if root_path == workspace:
            directories[:] = [directory for directory in directories if directory != ".git"]
        for name in list(directories):
            path = root_path / name
            if name != "__pycache__" or path.is_symlink():
                continue
            if all(entry.is_file() and not entry.is_symlink() and entry.suffix == ".pyc" for entry in path.iterdir()):
                shutil.rmtree(path)
                directories.remove(name)
                removed.append(path.relative_to(workspace).as_posix())
    return sorted(removed)


def benchmark_command(command: str, track: str, task_id: str, *arguments: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(BENCHMARK_SCRIPT), command, "--track", track, "--task", task_id, *arguments],
        check=False,
        text=True,
        capture_output=True,
    )


def prepare_workspace(args: argparse.Namespace, workspace: Path) -> None:
    arguments = ["--destination", str(workspace)]
    if args.polyglot_root:
        arguments += ["--polyglot-root", args.polyglot_root]
    completed = benchmark_command("prepare", args.track, args.task, *arguments)
    if completed.returncode != 0:
        detail = completed.stderr.strip() or completed.stdout.strip() or f"exit status {completed.returncode}"
        raise ValueError(f"benchmark preparation failed: {detail}")


def grade_workspace(args: argparse.Namespace, workspace: Path, output: Path) -> dict[str, Any]:
    arguments = ["--workspace", str(workspace), "--timeout", str(args.grader_timeout)]
    if args.playwright_browsers:
        arguments += ["--playwright-browsers", args.playwright_browsers]
    completed = benchmark_command("grade", args.track, args.task, *arguments)
    (output / ARTIFACTS["grade_log"]).write_text(
        f"exit status: {completed.returncode}\n--- stdout ---\n{completed.stdout}\n--- stderr ---\n{completed.stderr}",
        encoding="utf-8",
    )
    lines = [line for line in completed.stdout.splitlines() if line.strip()]
    try:
        result = json.loads(lines[-1]) if lines else None
    except json.JSONDecodeError:
        result = None
    if not isinstance(result, dict) or not isinstance(result.get("passed"), bool):
        result = {
            "task": args.task,
            "track": args.track,
            "passed": False,
            "reason": "the grader did not produce a structured result",
            "grader_exit_code": completed.returncode,
        }
    (output / ARTIFACTS["grade"]).write_text(json.dumps(result, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return result


def harness_identity(binary: Path) -> dict[str, Any]:
    """Identify the measured binary and the checkout it is expected to come from."""

    def command(*arguments: str, timeout: float) -> str | None:
        try:
            completed = subprocess.run(list(arguments), check=False, text=True, capture_output=True, timeout=timeout)
        except (OSError, subprocess.TimeoutExpired):
            return None
        return completed.stdout if completed.returncode == 0 else None

    version = command(str(binary), "--version", timeout=60)
    revision = command("git", "-C", str(REPOSITORY), "rev-parse", "HEAD", timeout=60)
    status = command("git", "-C", str(REPOSITORY), "status", "--porcelain", timeout=60)
    return {
        "name": "s-code",
        "version": version.strip().splitlines()[0].strip() if version and version.strip() else None,
        "source_revision": revision.strip() if revision else None,
        "source_dirty": bool(status.strip()) if status is not None else None,
    }


def build_record(
    *,
    harness: dict[str, Any],
    track: str,
    task: dict[str, Any],
    configuration: dict[str, Any],
    process: dict[str, Any],
    summary: dict[str, Any],
    grader: dict[str, Any],
    removed_caches: list[str],
) -> dict[str, Any]:
    usage = usage_record(summary)
    turn_elapsed = None
    if summary["first_timestamp"] is not None and summary["last_timestamp"] is not None:
        turn_elapsed = round(summary["last_timestamp"] - summary["first_timestamp"], 3)
    passed = bool(grader.get("passed"))
    reason = exclusion_reason(passed, process, summary, usage)
    return {
        "schema_version": SCHEMA_VERSION,
        "kind": RECORD_KIND,
        "harness": harness,
        "task": {"track": track, "id": task["id"], "protected_sha256": task["protected_sha256"]},
        "configuration": configuration,
        "started_at": process["started_at"],
        "elapsed_seconds": process["elapsed_seconds"],
        "process": {key: process[key] for key in ("exit_code", "timed_out", "events_truncated", "stderr_truncated")},
        "turn": {
            "session_id": summary["session_id"],
            "turn_id": summary["turn_id"],
            "status": summary["status"],
            "error_code": summary["error_code"],
            "model": summary["model"],
            "effective_model": effective_model(summary),
            "routed_models": summary["routed_models"],
            "route_fallbacks": summary["route_fallbacks"],
            "context_compactions": summary["context_compactions"],
            "approvals_requested": summary["approvals_requested"],
            "elapsed_seconds": turn_elapsed,
        },
        "usage": usage,
        "events": {
            "total": summary["total"],
            "malformed_lines": summary["malformed_lines"],
            "by_kind": summary["by_kind"],
        },
        "workspace_normalization": {"removed_bytecode_caches": removed_caches},
        "grader": grader,
        "passed": passed,
        "comparable": reason is None,
        "exclusion_reason": reason,
        "artifacts": dict(ARTIFACTS),
    }


def run(args: argparse.Namespace) -> int:
    task = find_task(load_manifest(), args.track, args.task)
    binary = resolve_binary(args.s_code)
    output = output_destination(args.output)
    prompt = compose_prompt(args.track, task)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.mkdir()
    workspace = output / ARTIFACTS["workspace"]
    (output / ARTIFACTS["prompt"]).write_text(prompt, encoding="utf-8")
    prepare_workspace(args, workspace)

    configuration = {
        "invocation": "s-code exec --stream-json --ephemeral",
        "model": args.model,
        "permission_mode": args.permission_mode,
        "timeout_seconds": args.timeout,
        "grace_seconds": args.grace_seconds,
        "grader_timeout_seconds": args.grader_timeout,
        "prompt_version": PROMPT_VERSION,
        "prompt_sha256": hashlib.sha256(prompt.encode("utf-8")).hexdigest(),
    }
    harness = harness_identity(binary)
    process = run_agent(binary, workspace, prompt, args, output)
    with (output / ARTIFACTS["events"]).open(encoding="utf-8", errors="replace") as events:
        summary = summarize_events(events)
    removed_caches = remove_bytecode_caches(workspace)
    grader = grade_workspace(args, workspace, output)
    record = build_record(
        harness=harness,
        track=args.track,
        task=task,
        configuration=configuration,
        process=process,
        summary=summary,
        grader=grader,
        removed_caches=removed_caches,
    )
    (output / RECORD_NAME).write_text(json.dumps(record, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(
        json.dumps(
            {
                "record": str(output / RECORD_NAME),
                "track": args.track,
                "task": args.task,
                "passed": record["passed"],
                "comparable": record["comparable"],
                "exclusion_reason": record["exclusion_reason"],
                "elapsed_seconds": record["elapsed_seconds"],
                "total_units": record["usage"]["total_units"],
            },
            sort_keys=True,
        )
    )
    return 0 if record["comparable"] else 2


def bounded_int(minimum: int, maximum: int):
    def parse(value: str) -> int:
        number = int(value)
        if number < minimum or number > maximum:
            raise argparse.ArgumentTypeError(f"must be between {minimum} and {maximum}")
        return number

    return parse


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    result.add_argument("--track", required=True, choices=("algorithm", "project", "frontend"))
    result.add_argument("--task", required=True)
    result.add_argument("--s-code", required=True, help="S-Code launcher or CLI binary to measure")
    result.add_argument("--output", required=True, help="new run directory beneath .work/")
    result.add_argument("--model", help="model identifier passed to s-code exec --model")
    result.add_argument("--permission-mode", default="workspace", choices=PERMISSION_MODES)
    result.add_argument("--timeout", type=bounded_int(1, 86_400), default=600, help="s-code exec --timeout seconds")
    result.add_argument(
        "--grace-seconds", type=bounded_int(0, 3_600), default=30,
        help="extra seconds allowed for the CLI to exit after its own timeout before it is killed",
    )
    result.add_argument("--grader-timeout", type=float, default=120.0)
    result.add_argument("--polyglot-root", help="frozen polyglot checkout for algorithm tasks")
    result.add_argument("--playwright-browsers", help="browser cache passed to the frontend grader")
    return result


def main() -> int:
    arguments = parser().parse_args()
    try:
        return run(arguments)
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        print(f"harness benchmark run error: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
