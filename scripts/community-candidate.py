#!/usr/bin/env python3
"""Create and bind Community qualification evidence to an exact Git tree."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys


FULL_REVISION = re.compile(r"^[0-9a-f]{40}$")
SHA256 = re.compile(r"^[0-9a-f]{64}$")
REPOSITORY = re.compile(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$")
CHECK_NAME = re.compile(r"^[a-z0-9][a-z0-9-]{0,63}$")
QUALIFICATION_TYPE = "s-code.community_qualification"
CANDIDATE_TYPE = "s-code.community_candidate"


def fail(message: str) -> None:
    raise ValueError(message)


def git_output(root: Path, *arguments: str) -> str:
    result = subprocess.run(
        ["git", *arguments],
        cwd=root,
        text=True,
        capture_output=True,
        check=False,
    )
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip() or str(result.returncode)
        fail(f"git {' '.join(arguments)} failed: {detail}")
    return result.stdout.strip()


def checked_revision(value: str, label: str) -> str:
    if FULL_REVISION.fullmatch(value) is None:
        fail(f"{label} must be a full lowercase Git revision")
    return value


def checked_positive(value: int, label: str) -> int:
    if value < 1:
        fail(f"{label} must be positive")
    return value


def checked_repository(value: str) -> str:
    if REPOSITORY.fullmatch(value) is None:
        fail("repository must use the OWNER/REPOSITORY form")
    return value


def load_object(path: Path, label: str) -> dict[str, object]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        fail(f"cannot read {label}: {error}")
    if not isinstance(value, dict):
        fail(f"{label} must contain a JSON object")
    return value


def write_object(path: Path, value: dict[str, object]) -> None:
    path = path.expanduser().resolve()
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.tmp")
    temporary.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    temporary.replace(path)


def exact_fields(value: dict[str, object], expected: set[str], label: str) -> None:
    actual = set(value)
    if actual != expected:
        fail(
            f"{label} fields differ: missing={sorted(expected - actual)} "
            f"unknown={sorted(actual - expected)}"
        )


def qualify(args: argparse.Namespace) -> None:
    root = args.root.expanduser().resolve()
    repository = checked_repository(args.repository)
    tested_revision = checked_revision(args.tested_revision, "tested revision")
    pull_request_head = checked_revision(args.pull_request_head, "pull-request head")
    if git_output(root, "rev-parse", "HEAD") != tested_revision:
        fail("tested revision is not the checked-out revision")
    if git_output(root, "status", "--porcelain", "--untracked-files=all"):
        fail("qualification requires a clean checkout")
    checks = args.check
    if len(checks) != len(set(checks)) or not checks:
        fail("qualification checks must be non-empty and unique")
    if any(CHECK_NAME.fullmatch(check) is None for check in checks):
        fail("qualification contains an invalid check name")
    evidence = {
        "schema_version": 2,
        "evidence_type": QUALIFICATION_TYPE,
        "outcome": "passed",
        "repository": repository,
        "tested_revision": tested_revision,
        "tested_tree": checked_revision(
            git_output(root, "rev-parse", "HEAD^{tree}"), "tested tree"
        ),
        "pull_request": checked_positive(args.pull_request, "pull request"),
        "pull_request_head_revision": pull_request_head,
        "workflow_run_id": checked_positive(args.workflow_run_id, "workflow run ID"),
        "workflow_run_attempt": checked_positive(
            args.workflow_run_attempt, "workflow run attempt"
        ),
        "passed_checks": sorted(checks),
    }
    write_object(args.output, evidence)


def validate_qualification(value: dict[str, object]) -> None:
    exact_fields(
        value,
        {
            "schema_version",
            "evidence_type",
            "outcome",
            "repository",
            "tested_revision",
            "tested_tree",
            "pull_request",
            "pull_request_head_revision",
            "workflow_run_id",
            "workflow_run_attempt",
            "passed_checks",
        },
        "qualification evidence",
    )
    if (
        value["schema_version"] != 2
        or value["evidence_type"] != QUALIFICATION_TYPE
        or value["outcome"] != "passed"
    ):
        fail("qualification evidence has an unsupported identity or outcome")
    if not isinstance(value["repository"], str):
        fail("qualification repository must be a string")
    checked_repository(value["repository"])
    for field in (
        "tested_revision",
        "tested_tree",
        "pull_request_head_revision",
    ):
        if not isinstance(value[field], str):
            fail(f"{field} must be a string")
        checked_revision(value[field], field.replace("_", " "))
    for field in ("pull_request", "workflow_run_id", "workflow_run_attempt"):
        if not isinstance(value[field], int) or isinstance(value[field], bool):
            fail(f"{field} must be an integer")
        checked_positive(value[field], field.replace("_", " "))
    checks = value["passed_checks"]
    if (
        not isinstance(checks, list)
        or not checks
        or any(not isinstance(check, str) or CHECK_NAME.fullmatch(check) is None for check in checks)
        or checks != sorted(set(checks))
    ):
        fail("qualification passed checks are invalid")


def validate_candidate(value: dict[str, object]) -> None:
    exact_fields(
        value,
        {
            "schema_version",
            "evidence_type",
            "outcome",
            "repository",
            "candidate_revision",
            "candidate_tree",
            "pull_request",
            "pull_request_head_revision",
            "qualification_revision",
            "qualification_run_id",
            "qualification_run_attempt",
            "qualification_evidence_sha256",
            "passed_checks",
        },
        "candidate evidence",
    )
    if (
        value["schema_version"] != 2
        or value["evidence_type"] != CANDIDATE_TYPE
        or value["outcome"] != "release_ready"
    ):
        fail("candidate evidence has an unsupported identity or outcome")
    if not isinstance(value["repository"], str):
        fail("candidate repository must be a string")
    checked_repository(value["repository"])
    for field in (
        "candidate_revision",
        "candidate_tree",
        "pull_request_head_revision",
        "qualification_revision",
    ):
        if not isinstance(value[field], str):
            fail(f"{field} must be a string")
        checked_revision(value[field], field.replace("_", " "))
    evidence_digest = value["qualification_evidence_sha256"]
    if not isinstance(evidence_digest, str) or SHA256.fullmatch(evidence_digest) is None:
        fail("qualification evidence sha256 must be a lowercase SHA-256 digest")
    for field in ("pull_request", "qualification_run_id", "qualification_run_attempt"):
        if not isinstance(value[field], int) or isinstance(value[field], bool):
            fail(f"{field} must be an integer")
        checked_positive(value[field], field.replace("_", " "))
    checks = value["passed_checks"]
    if (
        not isinstance(checks, list)
        or not checks
        or any(not isinstance(check, str) or CHECK_NAME.fullmatch(check) is None for check in checks)
        or checks != sorted(set(checks))
    ):
        fail("candidate passed checks are invalid")


def verify(args: argparse.Namespace) -> None:
    root = args.root.expanduser().resolve()
    candidate = load_object(args.candidate.expanduser().resolve(), "candidate evidence")
    validate_candidate(candidate)
    repository = checked_repository(args.repository)
    expected_revision = checked_revision(args.expected_revision, "expected revision")
    if candidate["repository"] != repository:
        fail("candidate repository does not match the release repository")
    if candidate["candidate_revision"] != expected_revision:
        fail("candidate evidence is not bound to the release revision")
    if git_output(root, "rev-parse", "HEAD") != expected_revision:
        fail("release revision is not the checked-out revision")
    release_tree = checked_revision(
        git_output(root, "rev-parse", "HEAD^{tree}"), "release tree"
    )
    if candidate["candidate_tree"] != release_tree:
        fail("release tree differs from the qualified candidate tree")


def finalize(args: argparse.Namespace) -> None:
    root = args.root.expanduser().resolve()
    qualification_path = args.qualification.expanduser().resolve()
    qualification_bytes = qualification_path.read_bytes()
    qualification = load_object(qualification_path, "qualification evidence")
    validate_qualification(qualification)
    repository = checked_repository(args.repository)
    candidate_revision = checked_revision(args.candidate_revision, "candidate revision")
    expected_head = checked_revision(args.expected_head_revision, "expected PR head")
    pull_request = checked_positive(args.pull_request, "pull request")
    if qualification["repository"] != repository:
        fail("qualification repository does not match the candidate repository")
    if qualification["pull_request"] != pull_request:
        fail("qualification pull request does not match the merged pull request")
    if qualification["pull_request_head_revision"] != expected_head:
        fail("qualification is not bound to the merged pull-request head")
    if git_output(root, "rev-parse", "HEAD") != candidate_revision:
        fail("candidate revision is not the checked-out revision")
    if git_output(root, "status", "--porcelain", "--untracked-files=all"):
        fail("candidate binding requires a clean checkout")
    candidate_tree = checked_revision(
        git_output(root, "rev-parse", "HEAD^{tree}"), "candidate tree"
    )
    if qualification["tested_tree"] != candidate_tree:
        fail("candidate tree differs from the tree that passed qualification")
    evidence = {
        "schema_version": 2,
        "evidence_type": CANDIDATE_TYPE,
        "outcome": "release_ready",
        "repository": repository,
        "candidate_revision": candidate_revision,
        "candidate_tree": candidate_tree,
        "pull_request": pull_request,
        "pull_request_head_revision": expected_head,
        "qualification_revision": qualification["tested_revision"],
        "qualification_run_id": qualification["workflow_run_id"],
        "qualification_run_attempt": qualification["workflow_run_attempt"],
        "qualification_evidence_sha256": hashlib.sha256(qualification_bytes).hexdigest(),
        "passed_checks": qualification["passed_checks"],
    }
    write_object(args.output, evidence)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)

    qualify_parser = subparsers.add_parser("qualify")
    qualify_parser.add_argument("--root", type=Path, default=Path.cwd())
    qualify_parser.add_argument("--output", type=Path, required=True)
    qualify_parser.add_argument("--repository", required=True)
    qualify_parser.add_argument("--tested-revision", required=True)
    qualify_parser.add_argument("--pull-request", type=int, required=True)
    qualify_parser.add_argument("--pull-request-head", required=True)
    qualify_parser.add_argument("--workflow-run-id", type=int, required=True)
    qualify_parser.add_argument("--workflow-run-attempt", type=int, required=True)
    qualify_parser.add_argument("--check", action="append", default=[])

    finalize_parser = subparsers.add_parser("finalize")
    finalize_parser.add_argument("--root", type=Path, default=Path.cwd())
    finalize_parser.add_argument("--qualification", type=Path, required=True)
    finalize_parser.add_argument("--output", type=Path, required=True)
    finalize_parser.add_argument("--repository", required=True)
    finalize_parser.add_argument("--candidate-revision", required=True)
    finalize_parser.add_argument("--pull-request", type=int, required=True)
    finalize_parser.add_argument("--expected-head-revision", required=True)

    verify_parser = subparsers.add_parser("verify")
    verify_parser.add_argument("--root", type=Path, default=Path.cwd())
    verify_parser.add_argument("--candidate", type=Path, required=True)
    verify_parser.add_argument("--repository", required=True)
    verify_parser.add_argument("--expected-revision", required=True)
    return parser.parse_args()


def main() -> int:
    try:
        args = parse_args()
        if args.command == "qualify":
            qualify(args)
        elif args.command == "finalize":
            finalize(args)
        else:
            verify(args)
        return 0
    except (OSError, TypeError, ValueError, json.JSONDecodeError) as error:
        print(f"Community candidate evidence failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
