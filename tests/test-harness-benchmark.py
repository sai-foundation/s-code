#!/usr/bin/env python3
"""Prepare and grade frozen coding-harness benchmark tasks."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time
from typing import Any


REPOSITORY = Path(__file__).resolve().parents[1]
MANIFEST_PATH = REPOSITORY / "tests/benchmarks/harness/manifest.json"
WORK_ROOT = (REPOSITORY / ".work").resolve()


def load_manifest() -> dict[str, Any]:
    with MANIFEST_PATH.open(encoding="utf-8") as source:
        return json.load(source)


def tasks(manifest: dict[str, Any]):
    for track, entries in manifest["tracks"].items():
        for task in entries:
            yield track, task


def validate_manifest(manifest: dict[str, Any]) -> None:
    if manifest.get("schema_version") != 1:
        raise ValueError("unsupported harness benchmark manifest schema")
    commit = manifest["algorithm_source"]["commit"]
    if len(commit) != 40 or any(character not in "0123456789abcdef" for character in commit):
        raise ValueError("algorithm source must use a full lowercase Git commit")

    seen: set[str] = set()
    for track, task in tasks(manifest):
        task_id = task.get("id")
        if not task_id or task_id in seen:
            raise ValueError(f"missing or duplicate task id: {task_id!r}")
        seen.add(task_id)
        grader = task.get("grader")
        if not isinstance(grader, list) or not grader or not all(isinstance(item, str) for item in grader):
            raise ValueError(f"{task_id}: grader must be a non-empty argv array")
        protected = task.get("protected_paths")
        if not isinstance(protected, list) or not protected:
            raise ValueError(f"{task_id}: protected_paths must not be empty")
        if any(Path(path).is_absolute() or ".." in Path(path).parts for path in protected):
            raise ValueError(f"{task_id}: protected path escapes the task workspace")
        fixture = task.get("fixture")
        if track != "algorithm" and (not fixture or not (REPOSITORY / fixture).is_dir()):
            raise ValueError(f"{task_id}: fixture directory does not exist")


def find_task(manifest: dict[str, Any], track: str, task_id: str) -> dict[str, Any]:
    for candidate in manifest["tracks"].get(track, []):
        if candidate["id"] == task_id:
            return candidate
    raise ValueError(f"unknown {track} task: {task_id}")


def work_destination(value: str) -> Path:
    destination = Path(value).resolve()
    if destination == WORK_ROOT or WORK_ROOT not in destination.parents:
        raise ValueError("benchmark destinations must be beneath .work/")
    return destination


def git(workspace: Path, *arguments: str, check: bool = True) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["git", "-C", str(workspace), *arguments],
        check=check,
        text=True,
        capture_output=True,
    )


def prepare(args: argparse.Namespace, manifest: dict[str, Any]) -> None:
    task = find_task(manifest, args.track, args.task)
    destination = work_destination(args.destination)
    if destination.exists():
        raise ValueError(f"destination already exists: {destination}")
    destination.parent.mkdir(parents=True, exist_ok=True)

    if args.track == "algorithm":
        if not args.polyglot_root:
            raise ValueError("algorithm preparation requires --polyglot-root")
        root = Path(args.polyglot_root).resolve()
        candidates = list(root.rglob(task["implementation"]))
        tests = list(root.rglob(task["test_file"]))
        if len(candidates) != 1 or len(tests) != 1 or candidates[0].parent != tests[0].parent:
            raise ValueError(f"could not uniquely resolve source files for {args.task}")
        destination.mkdir()
        shutil.copy2(candidates[0], destination / candidates[0].name)
        shutil.copy2(tests[0], destination / tests[0].name)
    else:
        shutil.copytree(REPOSITORY / task["fixture"], destination)

    git(destination, "init", "-q")
    git(destination, "add", ".")
    git(
        destination,
        "-c",
        "user.name=Opencoding benchmark",
        "-c",
        "user.email=benchmark@example.invalid",
        "commit",
        "-qm",
        "frozen benchmark fixture",
    )
    print(json.dumps({"prepared": str(destination), "track": args.track, "task": args.task}))


def grade(args: argparse.Namespace, manifest: dict[str, Any]) -> int:
    task = find_task(manifest, args.track, args.task)
    workspace = work_destination(args.workspace)
    if not (workspace / ".git").is_dir():
        raise ValueError("workspace is not a prepared benchmark Git repository")
    protected = task["protected_paths"]
    changed = git(workspace, "status", "--short", "--", *protected).stdout.strip()
    if changed:
        result = {
            "task": args.task,
            "track": args.track,
            "passed": False,
            "reason": "protected benchmark files changed",
            "changed": changed.splitlines(),
        }
        print(json.dumps(result, sort_keys=True))
        return 2

    environment = os.environ.copy()
    if args.playwright_browsers:
        environment["PLAYWRIGHT_BROWSERS_PATH"] = str(Path(args.playwright_browsers).resolve())
    started = time.monotonic()
    completed = subprocess.run(
        task["grader"],
        cwd=workspace,
        env=environment,
        timeout=args.timeout,
        text=True,
    )
    elapsed = round(time.monotonic() - started, 3)
    result = {
        "task": args.task,
        "track": args.track,
        "passed": completed.returncode == 0,
        "grader_exit_code": completed.returncode,
        "elapsed_seconds": elapsed,
    }
    print(json.dumps(result, sort_keys=True))
    return completed.returncode


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    commands = result.add_subparsers(dest="command")
    commands.add_parser("validate", help="validate the frozen benchmark catalog")

    prepare_parser = commands.add_parser("prepare", help="copy a frozen task beneath .work/")
    prepare_parser.add_argument("--track", required=True, choices=("algorithm", "project", "frontend"))
    prepare_parser.add_argument("--task", required=True)
    prepare_parser.add_argument("--destination", required=True)
    prepare_parser.add_argument("--polyglot-root")

    grade_parser = commands.add_parser("grade", help="run the frozen grader")
    grade_parser.add_argument("--track", required=True, choices=("algorithm", "project", "frontend"))
    grade_parser.add_argument("--task", required=True)
    grade_parser.add_argument("--workspace", required=True)
    grade_parser.add_argument("--timeout", type=float, default=120.0)
    grade_parser.add_argument("--playwright-browsers")
    return result


def main() -> int:
    arguments = parser().parse_args()
    manifest = load_manifest()
    try:
        validate_manifest(manifest)
        if arguments.command in (None, "validate"):
            task_count = sum(1 for _ in tasks(manifest))
            print(f"validated {task_count} frozen harness benchmark tasks")
            return 0
        if arguments.command == "prepare":
            prepare(arguments, manifest)
            return 0
        if arguments.command == "grade":
            return grade(arguments, manifest)
        raise AssertionError(f"unhandled command: {arguments.command}")
    except (OSError, ValueError, subprocess.CalledProcessError, subprocess.TimeoutExpired) as error:
        print(f"harness benchmark error: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
