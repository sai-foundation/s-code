#!/usr/bin/env python3
"""Verify the canonical standalone Community repository boundary."""

from __future__ import annotations

import json
import re
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
HEX_REVISION = re.compile(r"^[0-9a-f]{40}$")
PLACEHOLDERS = ("OWNER/REPOSITORY", "example/s-code", "security@s-code.example")


def fail(message: str) -> None:
    raise ValueError(message)


def load(path: Path) -> dict[str, object]:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        fail(f"{path.name} must contain an object")
    return value


def matches(relative: str, candidate: str) -> bool:
    return relative == candidate or relative.startswith(f"{candidate}/")


def git_output(*arguments: str) -> str:
    result = subprocess.run(
        ["git", *arguments], cwd=ROOT, text=True, capture_output=True, check=False
    )
    if result.returncode != 0:
        fail(result.stderr.strip() or result.stdout.strip() or "git command failed")
    return result.stdout.strip()


def main() -> int:
    try:
        contract = load(ROOT / "community-release.json")
        if contract.get("schema_version") != 3:
            fail("unsupported Community release contract")
        source = contract.get("source")
        if not isinstance(source, dict) or source.get("authority") != "community_repository":
            fail("Community repository is not declared as its own source authority")
        repository = contract["public_repository"]
        identity = contract["repository_identity"]
        if identity.get("url") != "https://github.com/sl-7qx/s-code":
            fail("canonical repository identity is invalid")
        if not isinstance(identity.get("publication_enabled"), bool):
            fail("publication_enabled must be an explicit boolean")

        for relative in repository["required_files"]:
            if not (ROOT / relative).is_file():
                fail(f"required file is missing: {relative}")
        for relative in repository["required_directories"]:
            if not (ROOT / relative).is_dir():
                fail(f"required directory is missing: {relative}")

        revision = None
        tree = None
        if (ROOT / ".git").exists():
            if git_output("status", "--porcelain", "--untracked-files=all"):
                fail("Community verification requires a clean checkout")
            revision = git_output("rev-parse", "HEAD")
            tree = git_output("rev-parse", "HEAD^{tree}")
            if HEX_REVISION.fullmatch(revision) is None or HEX_REVISION.fullmatch(tree) is None:
                fail("Community checkout has no full revision and tree identity")
            tracked = git_output("ls-files", "-z")
            candidates = [ROOT / relative for relative in tracked.split("\x00") if relative]
        else:
            candidates = sorted(ROOT.rglob("*"))

        forbidden = repository["forbidden_integration_paths"]
        forbidden_licenses = repository["forbidden_license_identifiers"]
        file_count = 0
        byte_count = 0
        for path in candidates:
            relative = path.relative_to(ROOT).as_posix()
            if relative == ".git" or relative.startswith(".git/"):
                continue
            if path.is_symlink():
                fail(f"symlink is forbidden: {relative}")
            if any(matches(relative, candidate) for candidate in forbidden):
                fail(f"integration-only path is present: {relative}")
            if not path.is_file():
                continue
            size = path.stat().st_size
            if size > repository["maximum_file_bytes"]:
                fail(f"file exceeds size policy: {relative}")
            file_count += 1
            byte_count += size
            try:
                text = path.read_text(encoding="utf-8")
            except UnicodeError:
                continue
            if relative != "community-release.json" and any(
                identifier in text for identifier in forbidden_licenses
            ):
                fail(f"Enterprise license identifier is present: {relative}")
            if path.suffix in {".md", ".sh", ".yml", ".yaml"} and any(
                placeholder in text for placeholder in PLACEHOLDERS
            ):
                fail(f"release placeholder is present: {relative}")

    except (
        OSError,
        KeyError,
        TypeError,
        ValueError,
        json.JSONDecodeError,
    ) as error:
        print(f"Community tree verification failed: {error}", file=sys.stderr)
        return 1
    print(
        json.dumps(
            {
                "passed": True,
                "files": file_count,
                "bytes": byte_count,
                "revision": revision,
                "tree": tree,
            },
            sort_keys=True,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
