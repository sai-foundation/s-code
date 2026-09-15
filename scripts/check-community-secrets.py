#!/usr/bin/env python3
"""Reject high-confidence credential material from Community repository files."""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
MAX_SCAN_BYTES = 10 * 1024 * 1024
PATTERNS = {
    "private key": re.compile(rb"-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----"),
    "GitHub token": re.compile(rb"\b(?:gh[pousr]_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{40,})\b"),
    "OpenRouter key": re.compile(rb"\bsk-or-v1-[A-Za-z0-9_-]{20,}\b"),
    "OpenAI project or service key": re.compile(rb"\bsk-(?:proj|svcacct)-[A-Za-z0-9_-]{20,}\b"),
    "OpenAI legacy key": re.compile(rb"\bsk-[A-Za-z0-9]{32,}\b"),
    "Anthropic key": re.compile(rb"\bsk-ant-(?:api[0-9]{2}-)?[A-Za-z0-9_-]{20,}\b"),
    "Google API key": re.compile(rb"\bAIza[0-9A-Za-z_-]{35}\b"),
    "AWS access key": re.compile(rb"\b(?:AKIA|ASIA)[A-Z0-9]{16}\b"),
    "Slack token": re.compile(rb"\bxox[baprs]-[A-Za-z0-9-]{20,}\b"),
    "GitLab token": re.compile(rb"\bglpat-[A-Za-z0-9_-]{20,}\b"),
    "Hugging Face token": re.compile(rb"\bhf_[A-Za-z0-9]{30,}\b"),
}
KNOWN_NON_SECRETS = {
    b"sk-" + b"0123456789abcdef" * 2,
    # Historical skills.rs publication rejection test: repeating example digits.
    b"sk-" + b"1234567890abcdef" * 2 + b"1234",
}


def candidates() -> list[Path]:
    if (ROOT / ".git").exists():
        names = subprocess.check_output(
            ["git", "ls-files"], cwd=ROOT, text=True
        ).splitlines()
        return [ROOT / name for name in names if name]
    return [path for path in ROOT.rglob("*") if path.is_file()]


def assert_pattern_canaries() -> None:
    canaries = {
        "OpenAI project or service key": b"sk-proj-" + b"A" * 32,
        "OpenAI legacy key": b"sk-" + b"B" * 48,
        "Anthropic key": b"sk-ant-api03-" + b"C" * 32,
        "Google API key": b"AIza" + b"D" * 35,
        "GitLab token": b"glpat-" + b"E" * 24,
        "Hugging Face token": b"hf_" + b"F" * 32,
    }
    for label, sample in canaries.items():
        if not PATTERNS[label].search(sample):
            raise RuntimeError(f"secret pattern canary failed: {label}")


def contains_unallowlisted_match(pattern: re.Pattern[bytes], contents: bytes) -> bool:
    return any(match.group() not in KNOWN_NON_SECRETS for match in pattern.finditer(contents))


def historical_blobs():
    records = subprocess.check_output(
        ["git", "rev-list", "--objects", "--all"], cwd=ROOT, text=True
    ).splitlines()
    names: dict[str, str] = {}
    for record in records:
        object_id, _, name = record.partition(" ")
        names.setdefault(object_id, name)
    process = subprocess.Popen(
        ["git", "cat-file", "--batch"],
        cwd=ROOT,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
    )
    assert process.stdin is not None and process.stdout is not None
    try:
        for object_id, name in names.items():
            process.stdin.write(object_id.encode("ascii") + b"\n")
            process.stdin.flush()
            header = process.stdout.readline().decode("ascii", "replace").strip().split()
            if len(header) != 3 or header[1] == "missing":
                raise RuntimeError(f"could not inspect Git object {object_id[:12]}")
            object_type, size = header[1], int(header[2])
            contents = process.stdout.read(size)
            if process.stdout.read(1) != b"\n":
                raise RuntimeError("malformed git cat-file response")
            if object_type == "blob":
                yield object_id, name, contents
    finally:
        process.stdin.close()
        process.stdout.close()
        process.wait()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--history", action="store_true", help="also scan every blob reachable from local Git refs"
    )
    arguments = parser.parse_args()
    assert_pattern_canaries()
    failures: list[str] = []
    current = candidates()
    for path in current:
        if path.name == "COMMUNITY-EXPORT.json" or not path.is_file():
            continue
        if path.stat().st_size > MAX_SCAN_BYTES:
            failures.append(f"{path.relative_to(ROOT)}: file exceeds secret scan limit")
            continue
        contents = path.read_bytes()
        for label, pattern in PATTERNS.items():
            if contains_unallowlisted_match(pattern, contents):
                failures.append(f"{path.relative_to(ROOT)}: possible {label}")
    historical_count = 0
    if arguments.history and (ROOT / ".git").exists():
        for object_id, name, contents in historical_blobs():
            historical_count += 1
            display = name or "unnamed blob"
            if len(contents) > MAX_SCAN_BYTES:
                failures.append(
                    f"history {object_id[:12]} ({display}): file exceeds secret scan limit"
                )
                continue
            for label, pattern in PATTERNS.items():
                if contains_unallowlisted_match(pattern, contents):
                    failures.append(
                        f"history {object_id[:12]} ({display}): possible {label}"
                    )
    if failures:
        print("\n".join(failures), file=sys.stderr)
        return 1
    suffix = f", {historical_count} historical blobs" if arguments.history else ""
    print(f"Community secret scan passed ({len(current)} files{suffix})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
