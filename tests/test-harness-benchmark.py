#!/usr/bin/env python3
"""Prepare and grade frozen coding-harness benchmark tasks."""

from __future__ import annotations

import argparse
import ast
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import time
from typing import Any


REPOSITORY = Path(__file__).resolve().parents[1]
MANIFEST_PATH = REPOSITORY / "tests/benchmarks/harness/manifest.json"
WORK_ROOT = (REPOSITORY / ".work").resolve()
RUNNER_ROOT = REPOSITORY / "tests/benchmarks/runner"
MAX_PROTECTED_NODES = 20_000
MAX_PROTECTED_FILE_BYTES = 64 * 1024 * 1024
MAX_PROTECTED_TOTAL_BYTES = 512 * 1024 * 1024
MAX_GRADER_OUTPUT_BYTES = 2 * 1024 * 1024
PYTHON_RESULT_PREFIX = "OPENCODING_GRADER_RESULT="


def load_manifest() -> dict[str, Any]:
    with MANIFEST_PATH.open(encoding="utf-8") as source:
        return json.load(source)


def tasks(manifest: dict[str, Any]):
    for track, entries in manifest["tracks"].items():
        for task in entries:
            yield track, task


def validate_manifest(manifest: dict[str, Any]) -> None:
    if manifest.get("schema_version") != 3:
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
        editable = task.get("editable_paths")
        if not isinstance(editable, list) or not editable:
            raise ValueError(f"{task_id}: editable_paths must not be empty")
        if any(Path(path).is_absolute() or ".." in Path(path).parts for path in editable):
            raise ValueError(f"{task_id}: editable path escapes the task workspace")
        expected_tests = task.get("expected_tests")
        if not isinstance(expected_tests, int) or expected_tests < 1:
            raise ValueError(f"{task_id}: expected_tests must be a positive integer")
        digest = task.get("protected_sha256")
        if not isinstance(digest, str) or len(digest) != 64 or any(
            character not in "0123456789abcdef" for character in digest
        ):
            raise ValueError(f"{task_id}: protected_sha256 must be a lowercase SHA-256 digest")
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


def protected_digest(workspace: Path, protected_paths: list[str]) -> str:
    records: list[dict[str, Any]] = []
    nodes = 0
    total_bytes = 0
    for protected in sorted(protected_paths):
        root = workspace / protected
        if not root.exists() and not root.is_symlink():
            records.append({"path": protected, "type": "missing"})
            continue
        pending = [root]
        while pending:
            path = pending.pop()
            relative = path.relative_to(workspace).as_posix()
            metadata = path.lstat()
            nodes += 1
            if nodes > MAX_PROTECTED_NODES:
                raise ValueError("protected fixture contains too many filesystem nodes")
            if path.is_symlink():
                records.append(
                    {"path": relative, "type": "symlink", "target": os.readlink(path)}
                )
            elif path.is_dir():
                records.append({"path": relative, "type": "directory"})
                pending.extend(sorted(path.iterdir(), reverse=True))
            elif path.is_file():
                if metadata.st_size > MAX_PROTECTED_FILE_BYTES:
                    raise ValueError(f"protected file exceeds 64 MiB: {relative}")
                total_bytes += metadata.st_size
                if total_bytes > MAX_PROTECTED_TOTAL_BYTES:
                    raise ValueError("protected fixture exceeds 512 MiB")
                digest = hashlib.sha256()
                with path.open("rb") as source:
                    while chunk := source.read(1024 * 1024):
                        digest.update(chunk)
                records.append(
                    {
                        "path": relative,
                        "type": "file",
                        "executable": bool(metadata.st_mode & 0o111),
                        "sha256": digest.hexdigest(),
                    }
                )
            else:
                records.append({"path": relative, "type": "unsupported"})
    encoded = json.dumps(records, separators=(",", ":"), sort_keys=True).encode()
    return hashlib.sha256(encoded).hexdigest()


def under_declared_path(relative: str, declarations: list[str]) -> bool:
    return any(relative == item or relative.startswith(f"{item.rstrip('/')}/") for item in declarations)


def validate_candidate_tree(workspace: Path, task: dict[str, Any], track: str) -> None:
    declarations = task["protected_paths"] + task["editable_paths"]
    nodes = 0
    python_cache_directories: set[Path] = set()
    for root, directories, files in os.walk(workspace, topdown=True, followlinks=False):
        root_path = Path(root)
        if root_path == workspace:
            directories[:] = [directory for directory in directories if directory != ".git"]
        for name in [*directories, *files]:
            path = root_path / name
            relative = path.relative_to(workspace).as_posix()
            nodes += 1
            if nodes > MAX_PROTECTED_NODES:
                raise ValueError("candidate workspace contains too many filesystem nodes")
            if not under_declared_path(relative, declarations):
                raise ValueError(f"candidate created undeclared path: {relative}")
            if path.is_symlink():
                raise ValueError(f"candidate workspace contains a symbolic link: {relative}")
            if track != "frontend" and under_declared_path(relative, task["editable_paths"]):
                relative_path = Path(relative)
                cache_ancestor = next(
                    (workspace.joinpath(*relative_path.parts[: index + 1]) for index, part in enumerate(relative_path.parts) if part == "__pycache__"),
                    None,
                )
                if cache_ancestor is not None:
                    python_cache_directories.add(cache_ancestor)
                elif path.is_file() and path.suffix != ".py":
                    raise ValueError(
                        f"Python candidate contains a non-source file: {relative}"
                    )

    # Bytecode produced by an ordinary local test run must not be imported by
    # the trusted grader. It has already been checked for symlinks above.
    for cache in sorted(python_cache_directories, reverse=True):
        if cache.is_dir():
            shutil.rmtree(cache)

    changed = set(
        filter(None, git(workspace, "diff", "--name-only", "-z", "HEAD").stdout.split("\0"))
    )
    changed.update(
        filter(
            None,
            git(workspace, "ls-files", "--others", "--exclude-standard", "-z").stdout.split("\0"),
        )
    )
    forbidden = sorted(
        path for path in changed if not under_declared_path(path, task["editable_paths"])
    )
    if forbidden:
        raise ValueError(f"candidate changed non-editable path: {forbidden[0]}")

    for declaration in task["editable_paths"]:
        editable = workspace / declaration
        candidates = [editable] if editable.is_file() else list(editable.rglob("*.py")) if editable.is_dir() else []
        for candidate in candidates:
            if candidate.suffix != ".py":
                continue
            tree = ast.parse(candidate.read_text(encoding="utf-8"), filename=str(candidate))
            for node in ast.walk(tree):
                if isinstance(node, (ast.Import, ast.ImportFrom)):
                    modules = [alias.name.split(".", 1)[0] for alias in node.names] if isinstance(node, ast.Import) else [(node.module or "").split(".", 1)[0]]
                    forbidden_modules = {"builtins", "ctypes", "gc", "importlib", "inspect", "site", "sys", "unittest"}
                    if any(module in forbidden_modules for module in modules):
                        raise ValueError(
                            f"candidate imports a grader-introspection module: {candidate.relative_to(workspace)}"
                        )
                if isinstance(node, ast.Attribute) and node.attr in {"_exit", "abort", "kill", "killpg"}:
                    raise ValueError(f"candidate uses a forbidden process-control primitive: {candidate.relative_to(workspace)}")
                if isinstance(node, ast.Attribute) and node.attr in {
                    "addError",
                    "addFailure",
                    "addSubTest",
                    "stopTestRun",
                    "testsRun",
                    "wasSuccessful",
                }:
                    raise ValueError(f"candidate references a grader result primitive: {candidate.relative_to(workspace)}")
                if isinstance(node, ast.Call) and isinstance(node.func, ast.Name) and node.func.id in {"__import__", "compile", "eval", "exec"}:
                    raise ValueError(f"candidate uses dynamic Python execution: {candidate.relative_to(workspace)}")
                if isinstance(node, ast.Raise) and isinstance(node.exc, ast.Call) and isinstance(node.exc.func, ast.Name) and node.exc.func.id == "SystemExit":
                    raise ValueError(f"candidate raises SystemExit directly: {candidate.relative_to(workspace)}")
                if isinstance(node, ast.Constant) and isinstance(node.value, str) and PYTHON_RESULT_PREFIX in node.value:
                    raise ValueError(f"candidate contains the trusted grader marker: {candidate.relative_to(workspace)}")


def run_bounded(command: list[str], cwd: Path, environment: dict[str, str], timeout: float) -> subprocess.CompletedProcess[str]:
    WORK_ROOT.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryFile(dir=WORK_ROOT) as output:
        def child_limits() -> None:
            os.setsid()
            try:
                import resource

                resource.setrlimit(resource.RLIMIT_FSIZE, (MAX_GRADER_OUTPUT_BYTES, MAX_GRADER_OUTPUT_BYTES))
            except (ImportError, OSError, ValueError):
                pass

        process = subprocess.Popen(
            command,
            cwd=cwd,
            env=environment,
            stdout=output,
            stderr=subprocess.STDOUT,
            start_new_session=os.name != "posix",
            preexec_fn=child_limits if os.name == "posix" else None,
        )
        try:
            returncode = process.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            if os.name == "posix":
                os.killpg(process.pid, signal.SIGKILL)
            else:
                process.kill()
            process.wait()
            raise
        output.seek(0)
        encoded = output.read(MAX_GRADER_OUTPUT_BYTES + 1)
        if len(encoded) > MAX_GRADER_OUTPUT_BYTES:
            raise ValueError("grader output exceeds 2 MiB")
        return subprocess.CompletedProcess(command, returncode, stdout=encoded.decode("utf-8", "replace"), stderr="")


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
        commit = manifest["algorithm_source"]["commit"]
        if git(root, "rev-parse", "HEAD").stdout.strip() != commit:
            raise ValueError("polyglot checkout is not at the frozen manifest commit")
        tree = git(root, "ls-tree", "-r", "--name-only", commit).stdout.splitlines()
        if len(tree) > 100_000:
            raise ValueError("polyglot source tree is unexpectedly large")
        candidates = [path for path in tree if Path(path).name == task["implementation"]]
        tests = [path for path in tree if Path(path).name == task["test_file"]]
        if len(candidates) != 1 or len(tests) != 1 or Path(candidates[0]).parent != Path(tests[0]).parent:
            raise ValueError(f"could not uniquely resolve source files for {args.task}")
        destination.mkdir()
        for source in [candidates[0], tests[0]]:
            content = subprocess.run(
                ["git", "-C", str(root), "show", f"{commit}:{source}"],
                check=True,
                capture_output=True,
            ).stdout
            if len(content) > MAX_PROTECTED_FILE_BYTES:
                raise ValueError(f"polyglot source file exceeds 64 MiB: {source}")
            (destination / Path(source).name).write_bytes(content)
    else:
        shutil.copytree(REPOSITORY / task["fixture"], destination)

    digest = protected_digest(destination, task["protected_paths"])
    if digest != task["protected_sha256"]:
        raise ValueError(
            f"{args.task}: prepared protected fixture digest differs from the manifest"
        )

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
    try:
        validate_candidate_tree(workspace, task, args.track)
    except ValueError as error:
        print(json.dumps({"task": args.task, "track": args.track, "passed": False, "reason": str(error)}, sort_keys=True))
        return 2
    digest = protected_digest(workspace, protected)
    if digest != task["protected_sha256"]:
        result = {
            "task": args.task,
            "track": args.track,
            "passed": False,
            "reason": "protected benchmark files changed",
            "expected_sha256": task["protected_sha256"],
            "actual_sha256": digest,
        }
        print(json.dumps(result, sort_keys=True))
        return 2

    environment = {
        "HOME": str(WORK_ROOT / "harness-home"),
        "LANG": "C.UTF-8",
        "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
        "PYTHONNOUSERSITE": "1",
        "PYTHONSAFEPATH": "1",
    }
    Path(environment["HOME"]).mkdir(mode=0o700, parents=True, exist_ok=True)
    if args.playwright_browsers:
        environment["PLAYWRIGHT_BROWSERS_PATH"] = str(Path(args.playwright_browsers).resolve())
    started = time.monotonic()
    grader = list(task["grader"])
    if args.track == "frontend":
        playwright = RUNNER_ROOT / "node_modules/.bin/playwright"
        if not playwright.is_file():
            raise ValueError(
                "frontend grader dependencies are missing; run npm ci --prefix "
                "tests/benchmarks/runner --no-audit --no-fund"
            )
        environment["NODE_PATH"] = str(RUNNER_ROOT / "node_modules")
        grader = [str(playwright), "test", "--reporter=json", "--config", str(workspace / "playwright.config.js")]
    else:
        runner = RUNNER_ROOT / "python_unittest_runner.py"
        grader = [sys.executable, "-I", str(runner), "--workspace", str(workspace), "--pattern", task.get("test_file", "test*.py")]
        if args.track == "project":
            grader.append("--discover")
    completed = run_bounded(grader, workspace, environment, args.timeout)
    elapsed = round(time.monotonic() - started, 3)
    output = completed.stdout
    if args.track == "frontend":
        try:
            report = json.loads(output)
            observed_tests = int(report["stats"]["expected"])
            structured_success = int(report["stats"]["unexpected"]) == 0
        except (KeyError, TypeError, ValueError, json.JSONDecodeError):
            observed_tests = 0
            structured_success = False
    else:
        records = [line.removeprefix(PYTHON_RESULT_PREFIX) for line in output.splitlines() if line.startswith(PYTHON_RESULT_PREFIX)]
        try:
            report = json.loads(records[-1]) if len(records) == 1 else {}
            observed_tests = int(report["tests_run"])
            structured_success = bool(report["successful"]) and report["errors"] == 0 and report["failures"] == 0
        except (KeyError, TypeError, ValueError, json.JSONDecodeError):
            observed_tests = 0
            structured_success = False
    passed = completed.returncode == 0 and structured_success and observed_tests == task["expected_tests"]
    result = {
        "task": args.task,
        "track": args.track,
        "passed": passed,
        "trust_model": "trusted_workspace_only",
        "grader_exit_code": completed.returncode,
        "expected_tests": task["expected_tests"],
        "observed_tests": observed_tests,
        "elapsed_seconds": elapsed,
    }
    print(json.dumps(result, sort_keys=True))
    return 0 if passed else 2


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
