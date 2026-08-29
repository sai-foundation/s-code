#!/usr/bin/env python3
"""Fail when a repository Markdown file contains a broken local link."""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path
from urllib.parse import unquote


ROOT = Path(__file__).resolve().parents[1]
LINK = re.compile(r"!?\[[^\]]*]\(([^)\s]+)(?:\s+[^)]*)?\)")
SCHEME = re.compile(r"^[a-zA-Z][a-zA-Z0-9+.-]*:")


def default_files() -> list[Path]:
    result = subprocess.run(
        [
            "git",
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "--",
            "*.md",
        ],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    return sorted(
        {
            ROOT / name
            for name in result.stdout.splitlines()
            if name and (ROOT / name).is_file()
        }
    )


def requested_files(arguments: list[str]) -> list[Path]:
    if not arguments:
        return default_files()

    files: list[Path] = []
    for argument in arguments:
        path = (ROOT / argument).resolve()
        if path.is_dir():
            files.extend(sorted(path.rglob("*.md")))
        else:
            files.append(path)
    return files


def main() -> int:
    failures: list[str] = []
    files = requested_files(sys.argv[1:])

    for source in files:
        if not source.is_file():
            failures.append(f"{source.relative_to(ROOT)}: file does not exist")
            continue
        for line_number, line in enumerate(
            source.read_text(encoding="utf-8").splitlines(), start=1
        ):
            for match in LINK.finditer(line):
                raw = match.group(1).strip("<>")
                destination = unquote(raw.split("#", 1)[0])
                if (
                    not destination
                    or destination.startswith("/")
                    or SCHEME.match(destination)
                ):
                    continue
                target = (source.parent / destination).resolve()
                if not target.exists():
                    failures.append(
                        f"{source.relative_to(ROOT)}:{line_number}: "
                        f"broken local link {raw!r}"
                    )

    if failures:
        print("\n".join(failures), file=sys.stderr)
        return 1

    print(f"validated local links in {len(files)} Markdown files")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
