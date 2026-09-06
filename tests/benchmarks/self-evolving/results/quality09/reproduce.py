#!/usr/bin/env python3
"""Verify published hashes and arithmetic without private inputs or model calls."""
import hashlib
import importlib.util
import json
from pathlib import Path
import sys

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parent


def read(name):
    return json.loads((ROOT/name).read_text())


def main():
    entries = list(ROOT.iterdir())
    if any(path.is_symlink() or not path.is_file() for path in entries):
        raise ValueError("Package must contain only regular files")
    manifest = read("reproduction-files.json")
    actual = {path.name for path in entries}
    if actual != set(manifest["files"]) | {"reproduction-files.json"}:
        raise ValueError("Unexpected or missing package files")
    for name, digest in manifest["files"].items():
        path = ROOT/name
        if Path(name).name != name or path.is_symlink() or hashlib.sha256(path.read_bytes()).hexdigest() != digest:
            raise ValueError("Package hash mismatch")
    for name, row in read("quality09-independent-publication-files.json")["files"].items():
        if hashlib.sha256((ROOT/name).read_bytes()).hexdigest() != row["sha256"]:
            raise ValueError("Reviewed output changed")
    for name, digest in read("public-files.json")["files"].items():
        if hashlib.sha256((ROOT/name).read_bytes()).hexdigest() != digest:
            raise ValueError("Original frozen output changed")
    report = read("quality09.json")
    if hashlib.sha256((ROOT/"analysis.py").read_bytes()).hexdigest() != report["bindings"]["analysis_sha256"]:
        raise ValueError("Numerical implementation changed")
    spec = importlib.util.spec_from_file_location("quality09_public_analysis", ROOT/"analysis.py")
    analysis = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(analysis)
    if analysis.analyze(read("measurements.json")) != read("numerical-diagnostics.json"):
        raise ValueError("Numerical diagnostics differ")
    rows = [row for row in report["slots"] if row["phase"] == "exposed-development"]
    successes = {}
    for arm in ("off", "raw", "learned"):
        selected = [row for row in rows if row["arm"] == arm]
        successes[arm] = sum(row["verified_success"] for row in selected)
        if len(selected) != 36 or successes[arm] != report["arms"][arm]["verified_successes"]:
            raise ValueError("Success arithmetic differs")
    if report["development_readiness"]["prospective_screen_passed"] or report["confirmatory_claim"]:
        raise ValueError("Unexpected efficacy claim")
    if report["physical_experiment_accounting"]["total_tokens"] is not None or report["physical_experiment_accounting"]["cost_usd"] is not None:
        raise ValueError("Unknown totals were replaced")
    print(json.dumps({"passed": True, "scope": "Published hashes and numerical arithmetic only; private tool provenance is covered by the recorded independent audit.",
                      "successes": successes, "complete_tokens": None, "complete_cost_usd": None,
                      "development_screen_passed": False, "confirmatory_claim": False}))


if __name__ == "__main__":
    main()
