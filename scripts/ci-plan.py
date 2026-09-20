#!/usr/bin/env python3
"""Select affected CI areas and reject failed or unexpectedly skipped checks."""

from __future__ import annotations

import argparse
import json
from pathlib import Path, PurePosixPath
import re
import subprocess


AREAS = ("rust", "policy", "protocol", "runtime", "install", "privacy", "desktop", "web", "docs", "vscode", "jetbrains", "benchmarks", "workflow", "audit")
JOBS = ("linux", "macos", "windows", "web", "docs", "vscode", "jetbrains", "benchmarks", "codeql")
REVISION = re.compile(r"^[0-9a-f]{40}$")
# New crates default to runtime coverage; only standalone evaluation/reporting
# crates are known not to affect the application's first-run path.
NON_RUNTIME_CRATES = {"compliance", "evals"}


def plan(paths: list[str], *, full: bool = False, public: bool = False) -> dict[str, bool]:
    flags = dict.fromkeys(AREAS, full)
    for path in paths:
        parts = PurePosixPath(path).parts
        if not path or path.startswith("/") or ".." in parts:
            raise ValueError("changed path must be repository-relative")
        if path.endswith(("/package.json", "/package-lock.json", "/npm-shrinkwrap.json", "/.npmrc")):
            flags["audit"] = True
        # Inspect executable locations before extensions: a Markdown fixture in
        # a crate can affect tests, and a renamed executable must still run CI.
        if path.startswith("crates/"):
            flags["rust"] = True
            flags["runtime"] |= len(parts) > 1 and parts[1] not in NON_RUNTIME_CRATES
            if path.endswith("Cargo.toml"):
                flags.update(dict.fromkeys(AREAS, True))
            if path.startswith("crates/protocol/"):
                for area in ("protocol", "web", "vscode", "jetbrains"):
                    flags[area] = True
        elif path.startswith("web/"):
            flags["web"] = True
            flags["runtime"] = True  # The daemon serves the embedded Web client.
            if path.startswith("web/generated/"):
                flags["protocol"] = True
        elif path.startswith(("docs/", "docs-site/")):
            flags["docs"] = True
        elif path.startswith(("clients/macos/", "scripts/macos/")) or path in (
            "scripts/build-macos-app.sh", "tests/test-macos-desktop.py", "tests/macos-model-fixture.py",
            "tests/test-macos-fixture-startup.py"
        ):
            flags["desktop"] = True
        elif path.startswith("clients/vscode/"):
            if not path.endswith(".md"):
                flags["vscode"] = True
        elif path.startswith("clients/jetbrains/"):
            if not path.endswith(".md"):
                flags["jetbrains"] = True
        elif path.startswith("tests/benchmarks/") or path in ("tests/test-harness-benchmark.py", "tests/test-harness-grader-integrity.sh"):
            flags["benchmarks"] = True
        elif path in ("s-code", "scripts/s-code", "scripts/install-from-source.sh", "scripts/source-dependencies.sh", "scripts/configure-shell-path.py", "tests/test-shell-path.py", "tests/test-source-dependencies.py", "tests/test-source-install.sh", "tests/test-source-install-real.sh", "tests/test-first-run.sh", "tests/wedged-s-code-daemon.py"):
            flags["install"] = flags["runtime"] = flags["rust"] = True
        elif path in ("tests/test-cli-e2e.sh", "tests/cli_pty_driver.py", "tests/model_fixture.py", "tests/test-privacy-security-use-cases.sh", "tests/cases/privacy-security-use-cases.jsonl"):
            flags["runtime"] = flags["rust"] = True
            if path in ("tests/test-privacy-security-use-cases.sh", "tests/cases/privacy-security-use-cases.jsonl"):
                flags["privacy"] = True
        elif path.startswith(("tests/test-web", "tests/check-web")) or path == "tests/cases/web-style-baseline.json":
            flags["web"] = True
        elif path == "tests/test-community-docs-site.sh":
            flags["docs"] = True
        elif path in ("tests/test-protocol-bindings.sh",):
            flags["protocol"] = flags["web"] = flags["vscode"] = flags["jetbrains"] = True
        elif path in ("about.hbs", "about.toml", "deny.toml", "tests/test-supply-chain.sh"):
            flags["policy"] = True
        elif path.startswith(".github/ISSUE_TEMPLATE/") or path in (".github/PULL_REQUEST_TEMPLATE.md", ".github/CODEOWNERS", ".github/dependabot.yml"):
            pass
        elif len(parts) == 1 and (path.endswith(".md") or path in ("LICENSE", "NOTICE", "DCO")):
            pass
        elif path.startswith("assets/") and path.endswith((".svg", ".png", ".jpg", ".jpeg")):
            pass
        else:
            # Lockfiles/toolchains, CI/routing scripts, shared fixtures and new
            # unclassified areas fail conservatively to all supported checks.
            flags.update(dict.fromkeys(AREAS, True))
    flags["desktop"] |= flags["runtime"] or flags["protocol"]
    flags["linux"] = any(flags[area] for area in ("rust", "policy", "protocol", "runtime", "install"))
    flags["macos"] = flags["rust"] or flags["runtime"] or flags["install"] or flags["desktop"]
    flags["windows"] = full
    flags["codeql"] = public and (full or flags["web"] or flags["docs"] or flags["vscode"] or flags["benchmarks"])
    flags["full"] = full
    return flags


def changed_paths(base: str, head: str) -> list[str]:
    if not REVISION.fullmatch(base) or not REVISION.fullmatch(head):
        raise ValueError("base and head must be full Git revisions")
    # No API pagination/file limits. No rename detection: both old and new
    # paths are selected, including deleted files and paths with newlines.
    result = subprocess.run(["git", "diff", "--name-only", "--no-renames", "-z", f"{base}...{head}", "--"], check=True, capture_output=True)
    return [path.decode("utf-8", errors="surrogateescape") for path in result.stdout.split(b"\0") if path]


def validate_gate(selected: object, needs: object, *, public: bool, full: bool = False) -> None:
    keys = set(AREAS) | set(JOBS) | {"full"}
    if not isinstance(selected, dict) or set(selected) != keys or any(type(value) is not bool for value in selected.values()):
        raise ValueError("missing or malformed CI plan")
    if not isinstance(needs, dict) or set(needs) != set(JOBS) | {"checks"}:
        raise ValueError("missing or unexpected CI job results")
    if selected["full"] != full:
        raise ValueError("CI plan does not match the requested verification mode")
    if full and selected != plan([], full=True, public=public):
        raise ValueError("full verification must select every applicable check")
    if selected["linux"] != any(selected[area] for area in ("rust", "policy", "protocol", "runtime", "install")) or selected["macos"] != any(selected[area] for area in ("rust", "runtime", "install", "desktop")):
        raise ValueError("CI plan has inconsistent platform dependencies")
    if (selected["runtime"] or selected["protocol"]) and not selected["desktop"]:
        raise ValueError("CI plan must cover desktop runtime and protocol dependencies")
    if selected["codeql"] != (public and (full or any(selected[area] for area in ("web", "docs", "vscode", "benchmarks")))):
        raise ValueError("CI plan has an inconsistent security scan selection")
    for job, result in needs.items():
        status = result.get("result") if isinstance(result, dict) else None
        expected = job == "checks" or selected[job]
        if status not in (("success",) if expected else ("skipped",)):
            raise ValueError(f"{job}: expected {'success' if expected else 'skipped'}, got {status}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    choose = sub.add_parser("plan")
    choose.add_argument("--base")
    choose.add_argument("--head")
    choose.add_argument("--full", action="store_true")
    choose.add_argument("--public", action="store_true")
    choose.add_argument("--output", type=Path)
    gate = sub.add_parser("gate")
    gate.add_argument("--plan", required=True)
    gate.add_argument("--needs", required=True)
    gate.add_argument("--public", action="store_true")
    gate.add_argument("--full", action="store_true")
    args = parser.parse_args()
    if args.command == "gate":
        validate_gate(json.loads(args.plan), json.loads(args.needs), public=args.public, full=args.full)
        print("All selected CI checks passed; unselected checks were skipped")
        return
    selected = plan([] if args.full else changed_paths(args.base or "", args.head or ""), full=args.full, public=args.public)
    print(json.dumps(selected, sort_keys=True))
    if args.output:
        with args.output.open("a", encoding="utf-8") as output:
            output.write("plan=" + json.dumps(selected, sort_keys=True) + "\n")
            for name, value in selected.items():
                output.write(f"{name}={str(value).lower()}\n")


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        raise SystemExit(f"CI planning failed: {error}")
