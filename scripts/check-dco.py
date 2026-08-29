#!/usr/bin/env python3
"""Require a Developer Certificate of Origin sign-off on pull-request commits."""

from __future__ import annotations

import argparse
import re
import subprocess
import sys


SIGN_OFF = re.compile(r"(?im)^Signed-off-by:\s+.+\s+<[^<>\s]+@[^<>\s]+>\s*$")


def git(*arguments: str) -> str:
    result = subprocess.run(
        ["git", *arguments], text=True, capture_output=True, check=False
    )
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip()
        raise ValueError(f"git {' '.join(arguments)} failed: {detail}")
    return result.stdout


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base", required=True, help="reviewed base commit")
    parser.add_argument("--head", required=True, help="pull-request head commit")
    return parser.parse_args()


def main() -> int:
    arguments = parse_args()
    try:
        commits = git("rev-list", "--reverse", "--no-merges", f"{arguments.base}..{arguments.head}").splitlines()
        if not commits:
            raise ValueError("pull request contains no non-merge commits")
        missing: list[str] = []
        for commit in commits:
            author = git("show", "-s", "--format=%an <%ae>", commit).strip()
            if "[bot]" in author:
                continue
            message = git("show", "-s", "--format=%B", commit)
            if SIGN_OFF.search(message) is None:
                missing.append(commit)
        if missing:
            raise ValueError(
                "commits missing a Signed-off-by trailer: "
                + ", ".join(commit[:12] for commit in missing)
            )
    except ValueError as error:
        print(f"DCO check failed: {error}", file=sys.stderr)
        return 1
    print(f"DCO check passed ({len(commits)} commits)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
