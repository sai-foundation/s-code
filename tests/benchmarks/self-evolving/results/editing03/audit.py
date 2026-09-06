#!/usr/bin/env python3
"""Private, offline development audit. Never runs a daemon, grader or model.

Run only after the parent confirms all families ended. Request messages/responses,
profiles and sealed tasks are not exported.
"""
from __future__ import annotations

import argparse
import collections
import hashlib
import importlib
import json
import math
from pathlib import Path
import re
import sys

sys.dont_write_bytecode = True
ARMS = ("off", "raw", "learned")
FAMILIES = ("flow", "queue", "report")
SEEDS = (17, 29, 43)
HORIZONS = (1, 4, 12, 24)
TERMINAL = {"completed", "failed", "cancelled", "awaiting_approval", "awaiting_input"}


def read(path):
    return json.loads(path.read_text())


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def tree_hash(root):
    if not root.is_dir():
        return None
    digest = hashlib.sha256()
    for path in sorted(root.rglob("*")):
        if any(p in {".git", "__pycache__"} for p in path.relative_to(root).parts):
            continue
        if path.is_symlink():
            raise ValueError("An audited tree contains a symlink")
        if path.is_file():
            digest.update(str(path.relative_to(root)).encode() + b"\0" + path.read_bytes() + b"\0")
    return digest.hexdigest()


def count(value):
    return type(value) is int and value >= 0


def number(value):
    return type(value) in (int, float) and math.isfinite(value) and value >= 0


def safe_sum(values):
    values = list(values)
    return sum(values) if values and all(number(v) for v in values) else None


def observed_sum(values):
    known = [value for value in values if number(value)]
    return sum(known) if known else None


def accounting(records, expected_count):
    """Reported subtotals stay visible; no missing quantity is silently zero."""
    def known(record):
        usage = record.get("usage")
        return (isinstance(usage, dict)
                and all(count(usage.get(k)) for k in ("prompt_tokens", "completion_tokens"))
                and not record.get("error") and not record.get("invalid_event")
                and number(record.get("finished_at")))
    complete = expected_count > 0 and len(records) == expected_count and all(map(known, records))
    observed_input = sum(r.get("usage", {}).get("prompt_tokens", 0)
                         for r in records if isinstance(r.get("usage"), dict)
                         and count(r["usage"].get("prompt_tokens")))
    observed_output = sum(r.get("usage", {}).get("completion_tokens", 0)
                          for r in records if isinstance(r.get("usage"), dict)
                          and count(r["usage"].get("completion_tokens")))
    costs = [r.get("cost") if not r.get("error") and not r.get("invalid_event") else None for r in records]
    return {
        "usage_complete": complete,
        "input_tokens": observed_input if complete else None,
        "output_tokens": observed_output if complete else None,
        "total_tokens": observed_input + observed_output if complete else None,
        "observed_input_tokens_subtotal": observed_input if records else None,
        "observed_output_tokens_subtotal": observed_output if records else None,
        "cost_usd": safe_sum(costs) if len(records) == expected_count else None,
        "provider_seconds": safe_sum(r.get("elapsed_seconds") for r in records) if len(records) == expected_count else None,
        "provider_calls": expected_count,
        "unknown_usage_calls": expected_count - sum(map(known, records)) if expected_count else None,
        "request_errors": sum(bool(r.get("error") or r.get("invalid_event")) for r in records),
    }


def config_issues(body, freeze, seed, phase):
    """Only inspect configuration and whether tools exist; never emit messages."""
    coding = bool(body.get("tools"))
    checks = {
        "model": body.get("model") == freeze["model"],
        "provider": body.get("provider") == {"only": [freeze["provider"]], "order": [freeze["provider"]], "allow_fallbacks": False},
        "reasoning": body.get("reasoning") == {"effort": freeze["reasoning_effort"]},
        "temperature": body.get("temperature") == freeze["temperature"],
        "seed": body.get("seed") == seed,
        "usage_requested": body.get("stream_options", {}).get("include_usage") is True,
        "output_limit": body.get("max_tokens") in (8192, 16384, 32768) if coding else body.get("max_tokens") == 1024,
        "reflection_training_only": coding or phase == "training",
    }
    return [name for name, valid in checks.items() if not valid]


def planned_slots(freeze):
    slots = []
    for family in FAMILIES:
        slots.append({"slot_id": f"{family}/training/17", "family": family, "phase": "training", "seed": 17, "arm": "training", "order_position": None})
        for attempt in freeze["schedule"][family]:
            slots.append({"slot_id": f"{family}/development/{attempt['seed']}/{attempt['arm']}", "family": family, "phase": "development", **attempt})
    if len(slots) != 30 or len({s["slot_id"] for s in slots}) != 30:
        raise ValueError("Expected exactly three training and 27 development slots")
    return slots


def empty_slot(plan):
    return {**plan, "execution_state": "not_run", "daemon_status": None,
            "grading_complete": None, "raw_grader_pass": None, "verified_success": None,
            "qualified_success": None, "budget_denied": None, "elapsed_seconds": None,
            "initial_source_sha256": None, "initial_source_consistent": None,
            "final_source_sha256": None, "request_configuration_valid": None,
            "artifact_sha256": {}, "integrity_issues": [], **accounting([], 0)}


def summarize(rows):
    """All planned slots remain in the quality denominator, even when not run."""
    successes = sum(r["qualified_success"] is True for r in rows)
    total = safe_sum(r["total_tokens"] for r in rows)
    known = [r for r in rows if r["usage_complete"]]
    return {
        "planned": len(rows), "ended": sum(r["execution_state"] == "ended" for r in rows),
        "not_run": sum(r["execution_state"] == "not_run" for r in rows),
        "incomplete": sum(r["execution_state"] == "incomplete" for r in rows),
        "graded": sum(r["grading_complete"] is True for r in rows),
        "raw_grader_passes": sum(r["raw_grader_pass"] is True for r in rows),
        "verified_successes": sum(r["verified_success"] is True for r in rows),
        "qualified_successes": successes, "success_rate_all_planned": successes / len(rows) if rows else None,
        "budget_denials": sum(r["budget_denied"] is True for r in rows),
        "unknown_budget_slots": sum(r["budget_denied"] is None for r in rows),
        "complete_token_slots": len(known), "unknown_token_slots": len(rows) - len(known),
        "total_tokens": total, "known_complete_slot_tokens_subtotal": sum(r["total_tokens"] for r in known),
        "tokens_per_qualified_success": total / successes if total is not None and successes else None,
        "cost_usd": safe_sum(r["cost_usd"] for r in rows),
        "agent_seconds": safe_sum(r["elapsed_seconds"] for r in rows),
        "daemon_status_counts": dict(collections.Counter(r["daemon_status"] or r["execution_state"] for r in rows)),
    }


def lifecycle(rows, training, horizon):
    indexed = {(r["family"], r["component"]): r for r in training}
    output = {}
    for arm in ARMS:
        subset = [r for r in rows if r["arm"] == arm]
        tokens, costs = [], []
        for row in subset:
            common = indexed.get((row["family"], "common"), {})
            extra = indexed.get((row["family"], arm), {})
            base = row["total_tokens"]
            common_tokens = safe_sum([common.get("input_tokens"), common.get("output_tokens")])
            extra_tokens = safe_sum([extra.get("input_tokens"), extra.get("output_tokens")])
            tokens.append(base + (common_tokens + extra_tokens) / horizon
                          if base is not None and common_tokens is not None and extra_tokens is not None
                          and common.get("usage_complete") is True and extra.get("usage_complete") is True else None)
            costs.append(row["cost_usd"] + (common["cost_usd"] + extra["cost_usd"]) / horizon
                         if all(number(x) for x in (row["cost_usd"], common.get("cost_usd"), extra.get("cost_usd"))) else None)
        qualified = sum(r["qualified_success"] is True for r in subset)
        total, cost = safe_sum(tokens), safe_sum(costs)
        output[arm] = {"planned": len(subset), "qualified_successes": qualified,
                       "lifecycle_equivalent_tokens": total,
                       "tokens_per_qualified_success": total / qualified if total is not None and qualified else None,
                       "lifecycle_equivalent_cost_usd": cost,
                       "cost_per_qualified_success_usd": cost / qualified if cost is not None and qualified else None}
    learned = output["learned"]["tokens_per_qualified_success"]
    for control in ("off", "raw"):
        other = output[control]["tokens_per_qualified_success"]
        output[f"learned_reduction_vs_{control}"] = 1 - learned / other if learned is not None and other not in (None, 0) else None
    return output


def source_integrity(root, freeze):
    checks = {}
    manifest_path = root / "manifest.json"
    if not manifest_path.exists():
        return {"manifest_available": False}
    manifest = read(manifest_path)
    for key in ("source_sha256", "daemon_sha256", "model", "provider", "reasoning_effort", "seeds"):
        checks[key] = manifest.get(key) == freeze.get(key)
    checks["schedule"] = manifest.get("development_schedule") == freeze["schedule"][manifest.get("project")]
    checks["per_task_cost_cap"] = manifest.get("per_task_cost_cap") == freeze["task_cost_cap_usd"]
    checks["family_cost_cap"] = manifest.get("max_cost") == freeze["family_cost_cap_usd"]
    checks["copied_binary"] = (root / "s-code-daemon").is_file() and sha(root / "s-code-daemon") == freeze["daemon_sha256"]
    inventory = read(root / "source.json")
    checks["source_manifest"] = hashlib.sha256(json.dumps(inventory, sort_keys=True).encode()).hexdigest() == freeze["source_sha256"]
    checks["source_files"] = all((root / "source" / name).is_file() and not (root / "source" / name).is_symlink()
                                and sha(root / "source" / name) == digest for name, digest in inventory.items())
    return checks


def load_family(base, freeze, plans, components_function):
    family = plans[0]["family"]
    root = base / ".work" / f"pilot-editing-{family}-03"
    meters, bodies, file_hashes = {}, {}, {}
    requests = root / "requests"
    if requests.is_dir():
        for directory in sorted(requests.iterdir()):
            if not directory.is_dir() or not re.fullmatch(r"\d+", directory.name):
                continue
            index = int(directory.name)
            # Read only final metadata/request configuration, never response.sse.
            meters[index] = read(directory / "meter.json") if (directory / "meter.json").exists() else {}
            bodies[index] = read(directory / "request.json") if (directory / "request.json").exists() else {}
            file_hashes[index] = {name: sha(directory / name) for name in ("meter.json", "request.json") if (directory / name).exists()}
    claimed = collections.Counter()
    rows = []
    source_checks = source_integrity(root, freeze)
    frozen = read(root / "frozen.json") if (root / "frozen.json").exists() else {}
    trained_hash = tree_hash(root / "trained-source")
    for plan in plans:
        row = empty_slot(plan)
        attempt = root / "training" if plan["phase"] == "training" else root / f"seed-{plan['seed']}" / plan["arm"]
        result = read(attempt / "result.json") if (attempt / "result.json").exists() else None
        matched = [index for index, meter in meters.items() if meter.get("arm") == plan["arm"] and meter.get("seed") == plan["seed"]]
        indices = result.get("requests", []) if result else matched
        if attempt.exists() or indices:
            row["execution_state"] = "incomplete"
        if result is not None:
            status = result.get("status")
            row["daemon_status"] = status if status in TERMINAL else "unknown"
            if status in TERMINAL and (attempt / "final").is_dir():
                row["execution_state"] = "ended"
            row["elapsed_seconds"] = result.get("elapsed_seconds") if number(result.get("elapsed_seconds")) else None
            row["budget_denied"] = result.get("budget_denied") if type(result.get("budget_denied")) is bool else None
            grade = result.get("grade", {})
            if (attempt / "grade.json").exists() and read(attempt / "grade.json") != grade:
                row["integrity_issues"].append("result_grade_artifact_mismatch")
            row["grading_complete"] = grade.get("grading_complete") if type(grade.get("grading_complete")) is bool else None
            if row["grading_complete"] is True and type(grade.get("passed")) is bool:
                row["raw_grader_pass"] = grade["passed"]
                row["verified_success"] = grade["passed"] and status == "completed"
                row["qualified_success"] = row["verified_success"] and row["budget_denied"] is False
            for field in ("seed", "arm", "order_position"):
                if field in result and result[field] != plan[field]:
                    row["integrity_issues"].append(f"result_{field}_mismatch")
        if len(set(indices)) != len(indices) or any(type(i) is not int or i < 0 for i in indices):
            raise ValueError("Invalid or duplicate request indices")
        claimed.update(indices)
        record_list = [meters[i] for i in indices if i in meters]
        row.update(accounting(record_list, len(indices)))
        row["request_configuration_valid"] = None if not indices else True
        for index in indices:
            issues = config_issues(bodies.get(index, {}), freeze, plan["seed"], plan["phase"])
            if issues:
                row["request_configuration_valid"] = False
                row["integrity_issues"].extend(f"request_configuration_{issue}" for issue in issues)
            meter = meters.get(index, {})
            if meter.get("seed") != plan["seed"] or meter.get("arm") != plan["arm"]:
                row["integrity_issues"].append("request_assignment_mismatch")
        row["initial_source_sha256"] = tree_hash(attempt / "initial")
        row["final_source_sha256"] = tree_hash(attempt / "final")
        if plan["phase"] == "development" and row["initial_source_sha256"] is not None:
            row["initial_source_consistent"] = row["initial_source_sha256"] == trained_hash == frozen.get("source_hash")
            if not row["initial_source_consistent"]:
                row["integrity_issues"].append("initial_source_mismatch")
        if result and row["usage_complete"]:
            totals = result.get("provider_usage")
            if not isinstance(totals, dict) or any(totals.get(k) != row[k] for k in ("input_tokens", "output_tokens", "total_tokens")):
                row["integrity_issues"].append("result_provider_totals_mismatch")
        if result and result.get("provider_usage_complete") is not row["usage_complete"]:
            row["integrity_issues"].append("result_usage_completeness_mismatch")
        row["artifact_sha256"] = {name: sha(attempt / name) for name in ("result.json", "grade.json", "patch.diff") if (attempt / name).is_file()}
        row["request_metadata_sha256"] = hashlib.sha256(json.dumps([file_hashes.get(i) for i in indices], sort_keys=True).encode()).hexdigest() if indices else None
        row["integrity_issues"] = sorted(set(row["integrity_issues"]))
        rows.append(row)
    unclaimed = [i for i in meters if claimed[i] == 0]
    duplicated = [i for i, frequency in claimed.items() if frequency > 1]
    components = []
    component_error = None
    if (root / "training/result.json").exists():
        try:
            components = components_function(family, root)
        except (KeyError, TypeError, ValueError, OSError):
            component_error = "training_component_evidence_unavailable"
    if not components:
        components = [{"family": family, "component": name, "usage_complete": False,
                       "input_tokens": None, "output_tokens": None, "cost_usd": None,
                       "elapsed_seconds": None, "provider_seconds": None, "training_agent_seconds": None,
                       "budget_denied": None} for name in ("common", "learned", "off", "raw")]
    # Explicitly use the frozen evaluate.training_components logic, then verify
    # that unknown budget/meter state does not acquire an invented zero.
    training = rows[0]
    if training["execution_state"] != "ended" or training["budget_denied"] is None:
        component_error = component_error or "training_attempt_incomplete_or_budget_unknown"
    family_audit = {"family": family, "source_checks": source_checks,
                    "all_source_checks_pass": bool(source_checks) and all(source_checks.values()),
                    "request_records": len(meters), "claimed_once": sum(claimed[i] == 1 for i in meters),
                    "unclaimed_request_records": len(unclaimed), "multiply_claimed_request_records": len(duplicated),
                    "all_request_records_settled": all(number(r.get("finished_at")) for r in meters.values()) if meters else None,
                    "observed_request_accounting": accounting(list(meters.values()), len(meters)),
                    "training_component_issue": component_error,
                    "aggregate_sha256": sha(root / "results.json") if (root / "results.json").exists() else None}
    return rows, components, family_audit


def build_report(slots, training, family_audits):
    development = [r for r in slots if r["phase"] == "development"]
    family_checks = {a["family"]: a for a in family_audits}
    paired = []
    for family in FAMILIES:
        for seed in SEEDS:
            rows = [r for r in development if r["family"] == family and r["seed"] == seed]
            family_check = family_checks[family]
            comparable = (family_check["all_source_checks_pass"]
                          and not family_check["unclaimed_request_records"]
                          and not family_check["multiply_claimed_request_records"]
                          and all(r["execution_state"] == "ended" and r["grading_complete"] is True
                             and r["usage_complete"] and r["budget_denied"] is False
                             and r["initial_source_consistent"] is True and r["request_configuration_valid"] is True
                             and not r["integrity_issues"] for r in rows))
            paired.append({"family": family, "seed": seed, "comparable": comparable,
                           "arms": {r["arm"]: {key: r[key] for key in ("qualified_success", "raw_grader_pass", "daemon_status", "total_tokens", "cost_usd", "elapsed_seconds")} for r in rows}})
    arms = {arm: summarize([r for r in development if r["arm"] == arm]) for arm in ARMS}
    complete = (all(p["comparable"] for p in paired)
                and all(a["all_source_checks_pass"] and not a["unclaimed_request_records"] and not a["multiply_claimed_request_records"]
                        and a["all_request_records_settled"] is True and not a["training_component_issue"] for a in family_audits)
                and all(r["usage_complete"] and r["budget_denied"] is False for r in training)
                and all(r["qualified_success"] is True for r in slots if r["phase"] == "training"))
    return {"planned_slot_count": len(slots), "counts": summarize(slots), "training_components": training,
            "slots": slots, "family_audits": family_audits, "arms": arms,
            "families": {family: {arm: summarize([r for r in development if r["family"] == family and r["arm"] == arm]) for arm in ARMS} for family in FAMILIES},
            "paired_blocks": paired, "lifecycle": {str(h): lifecycle(development, training, h) for h in HORIZONS},
            "physical_experiment_accounting": {
                "provider_calls": sum(a["request_records"] for a in family_audits),
                "total_tokens": safe_sum(a["observed_request_accounting"]["total_tokens"] for a in family_audits),
                "cost_usd": safe_sum(a["observed_request_accounting"]["cost_usd"] for a in family_audits),
                "observed_input_tokens_subtotal": observed_sum(a["observed_request_accounting"]["observed_input_tokens_subtotal"] for a in family_audits),
                "observed_output_tokens_subtotal": observed_sum(a["observed_request_accounting"]["observed_output_tokens_subtotal"] for a in family_audits),
            },
            "development_readiness": {"complete_and_auditable": complete,
                "zero_observed_aggregate_success_regression_vs_off": arms["learned"]["qualified_successes"] >= arms["off"]["qualified_successes"],
                "decision": "independent_human_review_required", "confirmatory_claim": False},
            "interpretation": ["Development-only: three distinct transfer tasks, each repeated across three seeds and three arms.",
                "No formal significance estimate; repeated seeds are not independent task identities.",
                "Every planned slot remains in quality denominators; missing or unaccounted totals are null.",
                "Lifecycle totals allocate common training equally to all arms, with reflection charged only to learned, at every listed horizon.",
                "Lifecycle equivalents are not literal experiment spending. Agent time and provider time are separate; reflection wall time is not inferred.",
                "Historical and adaptively stopped rounds remain separately available and are not pooled as treatment effects."]}


def printable(value, digits=0):
    return "unknown" if value is None else f"{value:,.{digits}f}"


def markdown(report):
    lines = ["# Editing03 independent development audit", "", report["interpretation"][0], "",
             "This is descriptive development evidence. It does not establish a confirmatory advantage or general coding superiority.", "",
             "| Arm | Verified within budget / planned | Raw grader passes | Tokens | Cost USD | Agent seconds |",
             "| --- | ---: | ---: | ---: | ---: | ---: |"]
    for arm, row in report["arms"].items():
        lines.append(f"| {arm} | {row['qualified_successes']}/{row['planned']} | {row['raw_grader_passes']} | {printable(row['total_tokens'])} | {printable(row['cost_usd'], 6)} | {printable(row['agent_seconds'], 1)} |")
    lines += ["", "Training allocation sensitivity (tokens per verified success):", "",
              "| Horizon | Off | Raw | Learned | Learned reduction vs off |",
              "| ---: | ---: | ---: | ---: | ---: |"]
    for horizon, row in report["lifecycle"].items():
        reduction = row["learned_reduction_vs_off"]
        lines.append(f"| {horizon} | {printable(row['off']['tokens_per_qualified_success'], 1)} | {printable(row['raw']['tokens_per_qualified_success'], 1)} | {printable(row['learned']['tokens_per_qualified_success'], 1)} | {printable(None if reduction is None else reduction * 100, 2)}% |")
    lines += ["", "All planned slots:", "", "| Family | Phase | Seed | Arm | Execution | Task status | Grader | Qualified | Tokens |",
              "| --- | --- | ---: | --- | --- | --- | --- | --- | ---: |"]
    for r in report["slots"]:
        verdict = lambda v: "unknown" if v is None else ("pass" if v else "fail")
        lines.append(f"| {r['family']} | {r['phase']} | {r['seed']} | {r['arm']} | {r['execution_state']} | {r['daemon_status'] or 'unknown'} | {verdict(r['raw_grader_pass'])} | {verdict(r['qualified_success'])} | {printable(r['total_tokens'])} |")
    lines += ["", "Audit qualification:", ""] + [f"- {text}" for text in report["interpretation"][1:]]
    lines += ["", f"Complete and auditable development round: **{report['development_readiness']['complete_and_auditable']}**. Proceeding to a sealed holdout requires independent review; this helper does not authorize reveal.", "",
              "The JSON includes per-family and paired-seed details, unknown-accounting counts, configuration checks, artifact hashes, and separate common/reflection training components.", "",
              "Historical evidence is preserved in the companion historical JSON files; its hashes are recorded in this report's JSON.", ""]
    return "\n".join(lines)


def assert_public_safe(value):
    payload = json.dumps(value, ensure_ascii=False)
    if any(pattern in payload for pattern in ("/Users/", "/home/", "Bearer ", "sk-or-v1-", "BEGIN PRIVATE KEY")):
        raise ValueError("Public output contains a private-path or credential-shaped value")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path.cwd())
    parser.add_argument("--completion-record", type=Path, required=True)
    parser.add_argument("--all-families-ended", action="store_true", required=True,
                        help="Explicit parent confirmation; never use while any family is running")
    parser.add_argument("--output-dir", type=Path, required=True)
    args = parser.parse_args()
    base = args.root.resolve()
    confirmation = read(args.completion_record)
    if not isinstance(confirmation, list) or {r.get("family") for r in confirmation} != set(FAMILIES):
        parser.error("Completion record must retain final pipeline status for all three families")
    if args.output_dir.exists():
        parser.error("Output directory must be new; preserve prior reviews")
    freeze = read(base / ".work/editing03-development-freeze.json")
    source_manifest = read(base / ".work/editing03-freeze-artifacts/source.json")
    helpers = base / "tests/benchmarks/self-evolving"
    for name in ("analysis.py", "evaluate.py", "pilot.py", "run.py", "sandbox.py"):
        if sha(helpers / name) != source_manifest[f"tests/benchmarks/self-evolving/{name}"]:
            raise ValueError("Analysis dependency changed from the development freeze")
    sys.path.insert(0, str(helpers))
    evaluate = importlib.import_module("evaluate")
    plans = planned_slots(freeze)
    slots, training, audits = [], [], []
    for family in FAMILIES:
        rows, components, audit = load_family(base, freeze, [p for p in plans if p["family"] == family], evaluate.training_components)
        slots.extend(rows); training.extend(components); audits.append(audit)
    report = {"schema_version": 1, "round": "editing03", "phase": "development-only",
              "revision": freeze["revision"], "source_sha256": freeze["source_sha256"],
              "daemon_sha256": freeze["daemon_sha256"], "analysis_sha256": freeze["analysis_sha256"],
              "review_helper_sha256": sha(Path(__file__)), "freeze_sha256": sha(base / ".work/editing03-development-freeze.json"),
              "configuration": {k: freeze[k] for k in ("model", "provider", "reasoning_effort", "temperature", "seeds", "training_seed", "family_order", "task_cost_cap_usd", "family_cost_cap_usd")},
              "pipeline": [{"family": r["family"], "returncode": r.get("returncode")} for r in confirmation],
              **build_report(slots, training, audits)}
    histories = {}
    for name in ("self-evolving-development-evidence.json", "quality02-development-evidence.json"):
        value = read(base / ".work" / name)
        assert_public_safe(value)
        histories[name] = value
    report["historical_evidence"] = [{"file": name, "sha256": sha(base / ".work" / name)} for name in histories]
    assert_public_safe(report)
    args.output_dir.mkdir(parents=True, mode=0o700)
    for name, value in histories.items():
        # Preserve the exact previously sanitized artifact and its recorded hash.
        (args.output_dir / name).write_bytes((base / ".work" / name).read_bytes())
    (args.output_dir / "editing03.json").write_text(json.dumps(report, indent=2, allow_nan=False) + "\n")
    (args.output_dir / "editing03.md").write_text(markdown(report))
    print(json.dumps({"planned_slots": len(slots), "complete_and_auditable": report["development_readiness"]["complete_and_auditable"],
                      "report_sha256": sha(args.output_dir / "editing03.json")}))


if __name__ == "__main__":
    main()
