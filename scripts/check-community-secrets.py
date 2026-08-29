#!/usr/bin/env python3
"""Reject high-confidence credential material from Community repository files."""

from __future__ import annotations

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
    "AWS access key": re.compile(rb"\b(?:AKIA|ASIA)[A-Z0-9]{16}\b"),
    "Slack token": re.compile(rb"\bxox[baprs]-[A-Za-z0-9-]{20,}\b"),
}


def candidates() -> list[Path]:
    if (ROOT / ".git").exists():
        names = subprocess.check_output(
            ["git", "ls-files"], cwd=ROOT, text=True
        ).splitlines()
        return [ROOT / name for name in names if name]
    return [path for path in ROOT.rglob("*") if path.is_file()]


def main() -> int:
    failures: list[str] = []
    for path in candidates():
        if path.name == "COMMUNITY-EXPORT.json" or not path.is_file():
            continue
        if path.stat().st_size > MAX_SCAN_BYTES:
            failures.append(f"{path.relative_to(ROOT)}: file exceeds secret scan limit")
            continue
        contents = path.read_bytes()
        for label, pattern in PATTERNS.items():
            if pattern.search(contents):
                failures.append(f"{path.relative_to(ROOT)}: possible {label}")
    if failures:
        print("\n".join(failures), file=sys.stderr)
        return 1
    print(f"Community secret scan passed ({len(candidates())} files)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
