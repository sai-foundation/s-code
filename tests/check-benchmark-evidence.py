#!/usr/bin/env python3
"""Validate public benchmark fixtures, samples and derived README claims."""

from __future__ import annotations

import json
from pathlib import Path
from statistics import median


ROOT = Path(__file__).resolve().parents[1]
EVIDENCE = ROOT / "tests/benchmarks/evidence/glm-5.3-2026-08-31.json"
MANIFEST = ROOT / "tests/benchmarks/harness/manifest.json"


def close(actual: float, expected: float) -> None:
    if round(actual, 1) != round(expected, 1):
        raise ValueError(f"derived value {actual:.1f} does not match {expected:.1f}")


def main() -> int:
    evidence = json.loads(EVIDENCE.read_text(encoding="utf-8"))
    manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
    if evidence.get("schema_version") != 1:
        raise ValueError("unsupported benchmark evidence schema")
    known = {
        task["id"]
        for tasks in manifest["tracks"].values()
        for task in tasks
    }
    for task in evidence["tasks"]:
        if task["id"] not in known:
            raise ValueError(f"unknown benchmark task {task['id']}")
        if not (ROOT / task["fixture"]).is_dir():
            raise ValueError(f"missing frozen fixture {task['fixture']}")
        runs = task["runs"]
        if any(not run["passed"] or run["harness_exit_code"] != 0 for run in runs):
            raise ValueError(f"{task['id']} includes a non-passing comparison sample")
        community = [run for run in runs if run["harness"] == "community"]
        opencode = [run for run in runs if run["harness"] == "opencode"]
        summary = task["summary"]
        community_seconds = median(run["elapsed_seconds"] for run in community)
        community_tokens = median(run["total_tokens"] for run in community)
        if community_seconds != summary["community_median_seconds"]:
            raise ValueError(f"{task['id']} Community latency median drifted")
        if community_tokens != summary["community_median_total_tokens"]:
            raise ValueError(f"{task['id']} Community token median drifted")
        if len(opencode) == 3:
            opencode_seconds = median(run["elapsed_seconds"] for run in opencode)
            opencode_tokens = median(run["total_tokens"] for run in opencode)
            if opencode_seconds != summary["opencode_median_seconds"]:
                raise ValueError(f"{task['id']} OpenCode latency median drifted")
            if opencode_tokens != summary["opencode_median_total_tokens"]:
                raise ValueError(f"{task['id']} OpenCode token median drifted")
        elif len(opencode) == 1:
            opencode_seconds = opencode[0]["elapsed_seconds"]
            opencode_tokens = opencode[0]["total_tokens"]
            if opencode_seconds != summary["opencode_completed_seconds"]:
                raise ValueError(f"{task['id']} OpenCode elapsed sample drifted")
            if opencode_tokens != summary["opencode_completed_total_tokens"]:
                raise ValueError(f"{task['id']} OpenCode token sample drifted")
        else:
            raise ValueError(f"{task['id']} has an unsupported OpenCode sample count")
        close(
            (opencode_seconds - community_seconds) / opencode_seconds * 100,
            summary["community_latency_advantage_percent"],
        )
        close(
            (opencode_tokens - community_tokens) / opencode_tokens * 100,
            summary["community_token_advantage_percent"],
        )
    print(f"validated {sum(len(task['runs']) for task in evidence['tasks'])} public benchmark samples")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
