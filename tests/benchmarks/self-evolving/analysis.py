#!/usr/bin/env python3
"""Preregistered, standard-library analysis of a sealed 12-task experiment.

Reads normalized measurements only; never loads prompts, candidates, credentials
or model responses. See ANALYSIS.md for the input contract and inference limits.
"""
from __future__ import annotations

import argparse
from collections import Counter, defaultdict
import hashlib
import json
import math
from pathlib import Path
import random
import statistics

ARMS = ("off", "raw", "learned")
SEEDS = (17, 29, 43)
FAMILIES = ("flow", "queue", "report")
HORIZONS = (1, 4, 12, 24)
BOOTSTRAP_SAMPLES = 10000
BOOTSTRAP_SEED = 20260905


def number(value):
    return type(value) in (int, float) and math.isfinite(value) and value >= 0


def token_number(value):
    return type(value) is int and value >= 0


def token_complete(record):
    return record is not None and record["usage_complete"] and all(token_number(record.get(key)) for key in ("input_tokens", "output_tokens"))


def tokens(record):
    return record["input_tokens"] + record["output_tokens"] if token_complete(record) else None


def success(record):
    return bool(record and record["verified_success"] is True and not record["budget_denied"])


def schedule(data):
    """A balanced set of six orders, with each arm in each position per task."""
    result = []
    tasks = sorted(data["protocol"]["task_manifest"], key=lambda task: (task["family"], task["task_id"]))
    for task_index, task in enumerate(tasks):
        base = list(ARMS if task_index % 2 == 0 else ("off", "learned", "raw"))
        for seed_index, seed in enumerate(SEEDS):
            rotation = (task_index + seed_index) % 3
            order = base[rotation:] + base[:rotation]
            result.append({"task_id": task["task_id"], "seed": seed, "arms": order})
    return result


def validate_record(record, training=False):
    if type(record.get("usage_complete")) is not bool:
        raise ValueError("every measurement needs a boolean usage_complete")
    for key in ("input_tokens", "output_tokens"):
        if record.get(key) is not None and not token_number(record[key]):
            raise ValueError(f"{key} must be a nonnegative integer or null")
    if record["usage_complete"] and not token_complete(record):
        raise ValueError("usage_complete=true requires both token counts")
    for key in ("cost_usd", "elapsed_seconds", "cached_input_tokens", "reasoning_output_tokens"):
        if record.get(key) is not None and not number(record[key]):
            raise ValueError(f"{key} must be finite and nonnegative or null")
    if training:
        if type(record.get("budget_denied", False)) is not bool:
            raise ValueError("training budget_denied must be boolean")
    else:
        if record.get("verified_success") is not None and type(record["verified_success"]) is not bool:
            raise ValueError("verified_success must be boolean or null")
        if "verified_success" not in record or type(record.get("budget_denied")) is not bool:
            raise ValueError("attempt needs verified_success and budget_denied")
        if record.get("order_position") is not None and (type(record["order_position"]) is not int or record["order_position"] not in (0, 1, 2)):
            raise ValueError("order_position must be 0, 1, 2 or null")


def validate(data):
    if data.get("schema_version") != 1:
        raise ValueError("schema_version must be 1")
    protocol = data["protocol"]
    fixed = {"arms": list(ARMS), "seeds": list(SEEDS), "primary_horizon": 12, "bootstrap_samples": BOOTSTRAP_SAMPLES, "bootstrap_seed": BOOTSTRAP_SEED, "target_reduction": 0.20}
    for key, value in fixed.items():
        if protocol.get(key) != value:
            raise ValueError(f"protocol {key} must equal preregistered value {value!r}")
    tasks = protocol["task_manifest"]
    if len(tasks) != 12 or len({task["task_id"] for task in tasks}) != 12:
        raise ValueError("exactly 12 unique task identities are required")
    if Counter(task["family"] for task in tasks) != Counter({family: 4 for family in FAMILIES}):
        raise ValueError("each of flow, queue, report needs exactly four tasks")
    for task in tasks:
        if not isinstance(task["task_id"], str) or not task["task_id"] or type(task.get("negative_control")) is not bool:
            raise ValueError("task IDs must be nonempty strings and control flags boolean")
    if Counter(task["family"] for task in tasks if task["negative_control"]) != Counter({family: 1 for family in FAMILIES}):
        raise ValueError("each family needs exactly one negative-transfer control")
    task_ids = {task["task_id"] for task in tasks}
    attempts, training = {}, {}
    for record in data["attempts"]:
        validate_record(record)
        key = (record["task_id"], record["seed"], record["arm"])
        if record["task_id"] not in task_ids or type(record["seed"]) is not int or record["seed"] not in SEEDS or record["arm"] not in ARMS:
            raise ValueError("attempt is outside the frozen task/seed/arm schedule")
        if key in attempts:
            raise ValueError(f"duplicate attempt {key!r}; selection among reruns is prohibited")
        attempts[key] = record
    for record in data["training"]:
        validate_record(record, training=True)
        key = (record["family"], record["component"])
        if record["family"] not in FAMILIES or record["component"] not in ("common", *ARMS):
            raise ValueError("unknown training family/component")
        if key in training:
            raise ValueError(f"duplicate training component {key!r}")
        training[key] = record
    return attempts, training


def training_total(training, family, arm, field="tokens"):
    records = [training.get((family, component)) for component in ("common", arm)]
    if field == "tokens":
        values = [tokens(record) if record else None for record in records]
    else:
        values = [record.get(field) if record else None for record in records]
    return sum(values) if all(number(value) for value in values) else None


def quality(records):
    present = [record for record in records if record is not None]
    verified = sum(success(record) for record in records)
    return {
        "planned": len(records), "attempted": len(present), "missing_runs": len(records) - len(present),
        "graded": sum(record["verified_success"] is not None for record in present),
        "grader_passes": sum(record["verified_success"] is True for record in present),
        "verified_successes_within_budget": verified,
        "success_rate_over_planned": verified / len(records) if records else None,
        "success_rate_over_attempted": verified / len(present) if present else None,
        "budget_denials": sum(record["budget_denied"] for record in present),
        "unknown_token_usage": sum(not token_complete(record) for record in present),
    }


def paired_quality(tasks, attempts, reference):
    counts = dict.fromkeys(("both_succeeded", "learned_only_succeeded", "reference_only_succeeded", "both_unsuccessful", "unknown_or_missing"), 0)
    for task in tasks:
        for seed in SEEDS:
            baseline, learned = [attempts.get((task["task_id"], seed, arm)) for arm in (reference, "learned")]
            if baseline is None or learned is None or baseline["verified_success"] is None or learned["verified_success"] is None:
                counts["unknown_or_missing"] += 1
            elif success(baseline) and success(learned):
                counts["both_succeeded"] += 1
            elif success(learned):
                counts["learned_only_succeeded"] += 1
            elif success(baseline):
                counts["reference_only_succeeded"] += 1
            else:
                counts["both_unsuccessful"] += 1
    return {"planned_pairs": len(tasks) * len(SEEDS), **counts}


def total_if_complete(values):
    return sum(values) if values and all(number(value) for value in values) else None


def arm_summary(tasks, attempts, training, arm, horizon):
    rows = [(task, attempts.get((task["task_id"], seed, arm))) for task in tasks for seed in SEEDS]
    records = [record for _, record in rows]
    output = quality(records)
    run_tokens = [tokens(record) if record else None for record in records]
    lifecycle = []
    for task, record in rows:
        run, train = tokens(record) if record else None, training_total(training, task["family"], arm)
        lifecycle.append(run + train / horizon if run is not None and train is not None else None)
    total, adjusted = total_if_complete(run_tokens), total_if_complete(lifecycle)
    successes = output["verified_successes_within_budget"]
    output.update({
        "run_input_tokens_recorded": sum(record.get("input_tokens") or 0 for record in records if record),
        "run_output_tokens_recorded": sum(record.get("output_tokens") or 0 for record in records if record),
        "all_attempt_run_tokens": total,
        "all_attempt_lifecycle_tokens": adjusted,
        "run_tokens_per_verified_success": total / successes if total is not None and successes else None,
        "lifecycle_tokens_per_verified_success": adjusted / successes if adjusted is not None and successes else None,
    })
    for field in ("cost_usd", "elapsed_seconds"):
        values = [record.get(field) if record else None for record in records]
        known = [value for value in values if number(value)]
        adjusted_values = []
        for (task, _), value in zip(rows, values):
            train = training_total(training, task["family"], arm, field)
            adjusted_values.append(value + train / horizon if value is not None and train is not None else None)
        output[field] = {"known_records": len(known), "recorded_sum": sum(known), "all_attempt_total": total_if_complete(values), "all_attempt_lifecycle_total": total_if_complete(adjusted_values), "observed_median": statistics.median(known) if known else None}
    for field in ("cached_input_tokens", "reasoning_output_tokens"):
        known = [record[field] for record in records if record and record.get(field) is not None]
        output[field] = {"known_records": len(known), "recorded_sum": sum(known)}
    return output


def cluster_totals(tasks, attempts, training, horizon):
    """Exclude an incomplete token block as a whole; retain known budget failures."""
    clusters, excluded = [], []
    for task in tasks:
        cluster = {"task_id": task["task_id"], "family": task["family"], "negative_control": task["negative_control"], "blocks": 0, "arms": {arm: {"tokens": 0.0, "successes": 0, "n": 0} for arm in ARMS}}
        for seed in SEEDS:
            reasons = []
            for arm in ARMS:
                record = attempts.get((task["task_id"], seed, arm))
                if record is None:
                    reasons.append(f"{arm}:missing_run")
                elif not token_complete(record) or record["verified_success"] is None:
                    reasons.append(f"{arm}:unknown_tokens_or_grade")
                if training_total(training, task["family"], arm) is None:
                    reasons.append(f"{arm}:unknown_training_tokens")
            if reasons:
                excluded.append({"task_id": task["task_id"], "seed": seed, "reasons": reasons})
                continue
            cluster["blocks"] += 1
            for arm in ARMS:
                record = attempts[(task["task_id"], seed, arm)]
                aggregate = cluster["arms"][arm]
                aggregate["tokens"] += tokens(record) + training_total(training, task["family"], arm) / horizon
                aggregate["successes"] += success(record)
                aggregate["n"] += 1
        if cluster["blocks"]:
            clusters.append(cluster)
    return clusters, excluded


def comparison(clusters, reference):
    totals = {arm: {field: sum(cluster["arms"][arm][field] for cluster in clusters) for field in ("tokens", "successes", "n")} for arm in (reference, "learned")}
    for arm in totals:
        value = totals[arm]
        value["tokens_per_verified_success"] = value["tokens"] / value["successes"] if value["successes"] else None
    baseline, learned = totals[reference], totals["learned"]
    base_ratio, learned_ratio = baseline["tokens_per_verified_success"], learned["tokens_per_verified_success"]
    reduction = 1 - learned_ratio / base_ratio if base_ratio and learned_ratio is not None else None
    delta = (learned["successes"] - baseline["successes"]) / baseline["n"] if baseline["n"] else None
    return {"reference": reference, "arms": totals, "token_reduction_fraction": reduction, "success_rate_difference": delta}


def quantile(values, probability):
    ordered = sorted(values)
    position = (len(ordered) - 1) * probability
    lower = int(position)
    upper = min(lower + 1, len(ordered) - 1)
    return ordered[lower] + (ordered[upper] - ordered[lower]) * (position - lower)


def bootstrap(clusters, samples=BOOTSTRAP_SAMPLES):
    strata = {family: [cluster for cluster in clusters if cluster["family"] == family] for family in FAMILIES}
    output = {arm: {"token_reductions": [], "success_differences": [], "undefined_token_draws": 0} for arm in ("off", "raw")}
    if any(not values for values in strata.values()):
        return {arm: {"draws": 0, "undefined_token_draws": 0, "token_reduction_ci95": None, "success_difference_ci95": None, "reason": "at least one family has no complete task cluster"} for arm in output}
    rng = random.Random(BOOTSTRAP_SEED)
    for _ in range(samples):
        sampled = [rng.choice(values) for values in strata.values() for _ in values]
        for arm, result in output.items():
            effect = comparison(sampled, arm)
            result["success_differences"].append(effect["success_rate_difference"])
            if effect["token_reduction_fraction"] is None:
                result["undefined_token_draws"] += 1
            else:
                result["token_reductions"].append(effect["token_reduction_fraction"])
    result = {}
    for arm, values in output.items():
        # Never delete undefined ratio draws and report an artificially narrowed CI.
        token_ci = None if values["undefined_token_draws"] else [quantile(values["token_reductions"], p) for p in (0.025, 0.975)]
        result[arm] = {"draws": samples, "undefined_token_draws": values["undefined_token_draws"], "token_reduction_ci95": token_ci, "success_difference_ci95": [quantile(values["success_differences"], p) for p in (0.025, 0.975)]}
    return result


def analyze(data, *, bootstrap_samples=None):
    """bootstrap_samples override is for synthetic tests; CLI always uses 10,000."""
    attempts, training = validate(data)
    tasks = sorted(data["protocol"]["task_manifest"], key=lambda task: (task["family"], task["task_id"]))
    issues = []
    for block in schedule(data):
        for position, arm in enumerate(block["arms"]):
            key = (block["task_id"], block["seed"], arm)
            record = attempts.get(key)
            if record is None:
                issues.append({"kind": "missing_run", "key": list(key)})
                continue
            for condition, kind in [(not token_complete(record), "unknown_token_usage"), (record["verified_success"] is None, "unknown_grade"), (record["budget_denied"], "budget_denial"), (record.get("order_position") != position, "order_mismatch_or_missing")]:
                if condition:
                    issues.append({"kind": kind, "key": list(key)})
    for family in FAMILIES:
        for component in ("common", *ARMS):
            record = training.get((family, component))
            if not token_complete(record):
                issues.append({"kind": "unknown_training_usage", "key": [family, component]})
            if record and record.get("budget_denied"):
                issues.append({"kind": "training_budget_denial", "key": [family, component]})
    arms = {arm: arm_summary(tasks, attempts, training, arm, 12) for arm in ARMS}
    clusters, excluded = cluster_totals(tasks, attempts, training, 12)
    intervals = bootstrap(clusters, samples=bootstrap_samples or BOOTSTRAP_SAMPLES)
    paired = {arm: {**comparison(clusters, arm), "bootstrap": intervals[arm]} for arm in ("off", "raw")}
    point = paired["off"]["token_reduction_fraction"]
    ci = intervals["off"]["token_reduction_ci95"]
    quality_guard = arms["learned"]["verified_successes_within_budget"] >= arms["off"]["verified_successes_within_budget"]
    meets_point = point is not None and point >= 0.20 - 1e-12
    excludes_zero = ci is not None and ci[0] > 0
    if issues:
        status = "inconclusive"
    elif not quality_guard:
        status = "not_met"
    elif point is None or ci is None:
        status = "inconclusive"
    else:
        status = "met_on_fixed_benchmark" if meets_point and excludes_zero else "not_met"
    sensitivity = {}
    for horizon in HORIZONS:
        candidates, _ = cluster_totals(tasks, attempts, training, horizon)
        sensitivity[str(horizon)] = {"arms": {arm: arm_summary(tasks, attempts, training, arm, horizon) for arm in ARMS}, "complete_pair_comparisons": {reference: comparison(candidates, reference) for reference in ("off", "raw")}}
    groups = {}
    for name, subset in [(family, [task for task in tasks if task["family"] == family]) for family in FAMILIES] + [("negative_controls", [task for task in tasks if task["negative_control"]]), ("transfer_tasks", [task for task in tasks if not task["negative_control"]])]:
        groups[name] = {arm: arm_summary(subset, attempts, training, arm, 12) for arm in ARMS}
    family_leaveout = {}
    for family in FAMILIES:
        selected = [cluster for cluster in clusters if cluster["family"] != family]
        family_leaveout[family] = {reference: comparison(selected, reference) for reference in ("off", "raw")}
    canonical = json.dumps(data, sort_keys=True, separators=(",", ":"), allow_nan=False).encode()
    return {
        "schema_version": 1, "input_sha256": hashlib.sha256(canonical).hexdigest(), "analysis_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        "scope": "Conditional evidence for these 12 tasks, three fixed seed projects and frozen training artifacts; not population noninferiority or general coding superiority.",
        "primary": {"comparison": "learned_vs_off", "metric": "all-attempt lifecycle tokens per verified success", "horizon_tasks_per_family": 12, "status": status, "observed_zero_success_regression_guardrail": quality_guard, "point_estimate_at_least_20_percent": meets_point, "ci95_excludes_zero_improvement": excludes_zero, "token_reduction_fraction": point, "token_reduction_ci95": ci, "data_integrity_issues": issues},
        "arms": arms, "paired_quality_all_planned": {reference: paired_quality(tasks, attempts, reference) for reference in ("off", "raw")}, "complete_pair_analysis": {"label": "Descriptive complete-pair results whenever any planned run is missing or unmetered", "blocks_included": sum(cluster["blocks"] for cluster in clusters), "blocks_excluded": excluded, "task_clusters": len(clusters), "comparisons": paired},
        "secondary_raw_control": {"comparison": "learned_vs_raw", "observed_zero_success_regression_guardrail": arms["learned"]["verified_successes_within_budget"] >= arms["raw"]["verified_successes_within_budget"], "statistical_noninferiority_proven": False, "complete_pair_result": paired["raw"]},
        "groups": groups, "training_horizon_sensitivity": sensitivity, "leave_one_family_out": family_leaveout,
        "training_components": data["training"],
        "bootstrap_method": {"draws": bootstrap_samples or BOOTSTRAP_SAMPLES, "seed": BOOTSTRAP_SEED, "method": "paired task-cluster percentile bootstrap, stratified by fixed family; retain all repetitions/arms together", "inference_warning": "Only four task identities per fixed family. Repetitions do not increase the number of independent task clusters. Degenerate intervals do not prove zero population quality loss."},
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("input", type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--schedule", action="store_true", help="emit the frozen arm order without analyzing measurements")
    args = parser.parse_args()
    data = json.loads(args.input.read_text())
    try:
        validate(data)
        result = schedule(data) if args.schedule else analyze(data)
        text = json.dumps(result, indent=2, sort_keys=True, allow_nan=False) + "\n"
    except (KeyError, TypeError, ValueError) as error:
        parser.error(str(error))
    if args.output:
        args.output.write_text(text)
    else:
        print(text, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
