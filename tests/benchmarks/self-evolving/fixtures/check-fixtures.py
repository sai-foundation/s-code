#!/usr/bin/env python3
"""Check pilot graders against untouched seeds, reference edits and ablations.

All edits occur in disposable copies. This supervisor-side file contains only
public pilot reference edits; NEVER copy it into a coding-agent workspace.
It does not contain final held-out tasks and performs no model calls.
"""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent


def replace(root, file, before, after):
    path = root / file
    text = path.read_text()
    if text.count(before) != 1:
        raise AssertionError(f"reference edit no longer matches exactly once: {file}: {before!r}")
    path.write_text(text.replace(before, after))


def report_train(root):
    replace(root, "incident_report/reports.py", "def summarize(events):", "def summarize(events, top=3):")
    replace(root, "incident_report/reports.py", ")[:3]", ")[:top]")
    replace(root, "incident_report/cli.py", "def main(argv=None):", "def nonnegative(text):\n    value = int(text)\n    if value < 0:\n        raise argparse.ArgumentTypeError('top must be non-negative')\n    return value\n\n\ndef main(argv=None):")
    replace(root, "incident_report/cli.py", '    parser.add_argument("input")', '    parser.add_argument("input")\n    parser.add_argument("--top", type=nonnegative, default=3)')
    replace(root, "incident_report/cli.py", "summarize(read_events(args.input))", "summarize(read_events(args.input), args.top)")


def report_dev(root):
    replace(root, "incident_report/events.py", "def read_events(path):", "def read_events(path, max_line_bytes=None):")
    replace(root, "incident_report/events.py", 'open(path, encoding="utf-8")', 'open(path, encoding="utf-8", newline="")')
    replace(root, "incident_report/events.py", "            if not line.strip():", "            if max_line_bytes is not None and len(line.rstrip('\\r\\n').encode('utf-8')) > max_line_bytes:\n                raise ValueError(f'{path}: line {number}: input line exceeds byte limit')\n            if not line.strip():")
    replace(root, "incident_report/cli.py", "def main(argv=None):", "def positive(text):\n    value = int(text)\n    if value <= 0:\n        raise argparse.ArgumentTypeError('limit must be positive')\n    return value\n\n\ndef main(argv=None):")
    replace(root, "incident_report/cli.py", '    parser.add_argument("input")', '    parser.add_argument("input")\n    parser.add_argument("--max-line-bytes", type=positive)')
    replace(root, "incident_report/cli.py", "read_events(args.input)", "read_events(args.input, args.max_line_bytes)")


def queue_train(root):
    replace(root, "durable_queue/clock.py", "import argparse", "import argparse\nimport math")
    replace(root, "durable_queue/clock.py", "        return float(text)", "        value = float(text)\n        if not math.isfinite(value):\n            raise ValueError('time must be finite')\n        return value")


def queue_dev(root):
    replace(root, "durable_queue/cli.py", '("init", "enqueue", "claim", "ack", "fail", "stats")', '("init", "enqueue", "claim", "ack", "fail", "stats", "purge-completed")')
    replace(root, "durable_queue/cli.py", '            else:\n                result = store.stats(connection, now)', '            elif args.command == "purge-completed":\n                result = {"removed": connection.execute("DELETE FROM tasks WHERE status=\'completed\'").rowcount}\n            else:\n                result = store.stats(connection, now)')


def flow_train(root):
    replace(root, "flow_runner/plan.py", '        resource = item.get("resource")', '        if len(dependencies) != len(set(dependencies)):\n            raise ValueError(f"task {task_id!r}: duplicate dependency")\n        resource = item.get("resource")')


def flow_dev(root):
    replace(root, "flow_runner/cli.py", '    parser.add_argument("plan")', '    parser.add_argument("plan")\n    parser.add_argument("--fail-fast", action="store_true")')
    replace(root, "flow_runner/cli.py", "run(tasks, args.jobs)", "run(tasks, args.jobs, fail_fast=args.fail_fast)")
    replace(root, "flow_runner/scheduler.py", "def run(tasks, jobs):", "def run(tasks, jobs, fail_fast=False):")
    replace(root, "flow_runner/scheduler.py", "    finished, running, resources = {}, {}, set()", "    finished, running, resources = {}, {}, set()\n    stopped = False")
    replace(root, "flow_runner/scheduler.py", "        while pending or running:", "        while pending or running:\n            if fail_fast and stopped:\n                for task_id in list(pending):\n                    finished[task_id] = skipped(task_id, 'fail-fast')\n                    del pending[task_id]")
    replace(root, "flow_runner/scheduler.py", '                    finished[task["id"]] = future.result()', '                    finished[task["id"]] = future.result()\n                    if finished[task["id"]]["status"] == "failed":\n                        stopped = True')


def ablate(root, family):
    if family == "report":
        replace(root, "incident_report/events.py", "len(line.rstrip('\\r\\n').encode('utf-8'))", "len(line.rstrip('\\r\\n'))")
    elif family == "queue":
        replace(root, "durable_queue/cli.py", "DELETE FROM tasks WHERE status='completed'", "DELETE FROM tasks WHERE status IN ('completed','dead')")
    else:
        replace(root, "flow_runner/cli.py", "fail_fast=args.fail_fast", "fail_fast=False")


def protected(root):
    return {str(path.relative_to(root)): hashlib.sha256(path.read_bytes()).hexdigest() for path in root.rglob("*") if path.is_file() and ("tests" in path.relative_to(root).parts or path.suffix == ".md") and "__pycache__" not in path.parts}


def grade(root, task, expected, label):
    result = subprocess.run([sys.executable, str(HERE / "grade.py"), "--workspace", str(root), "--task", task], capture_output=True, text=True, timeout=120)
    try:
        summary = json.loads(result.stdout)
    except ValueError:
        raise AssertionError(result.stderr + result.stdout) from None
    if summary["passed"] != expected or result.returncode != (0 if expected else 1) or summary["errors"]:
        raise AssertionError(f"{label}: unexpected grader outcome\n{result.stderr}\n{result.stdout}")
    print(json.dumps({"phase": label, **summary}), flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--family", choices=("report", "queue", "flow", "all"), default="all")
    args = parser.parse_args()
    projects = [("report", "incident-report-cli", report_train, report_dev), ("queue", "durable-task-queue", queue_train, queue_dev), ("flow", "dependency-flow-runner", flow_train, flow_dev)]
    for family, source, train, dev in projects:
        if args.family not in ("all", family):
            continue
        with tempfile.TemporaryDirectory(prefix="s-code-pilot-oracle-") as directory:
            root = Path(directory) / "candidate"
            shutil.copytree(HERE / source, root, ignore=shutil.ignore_patterns("__pycache__", "*.pyc"))
            original = protected(root)
            grade(root, family + "-train", False, "seed_missing_training_change")
            train(root)
            grade(root, family + "-train", True, "reference_training")
            grade(root, family + "-dev", False, "trained_tree_missing_development_change")
            dev(root)
            grade(root, family + "-dev", True, "reference_development")
            ablate(root, family)
            grade(root, family + "-dev", False, "development_ablation")
            if protected(root) != original:
                raise AssertionError("reference edits or candidate runs changed protected fixture files")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
