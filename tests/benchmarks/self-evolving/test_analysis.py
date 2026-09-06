"""Synthetic measurements only; no task text, model calls, or sealed data."""
import copy
from collections import Counter
import importlib.util
import json
from pathlib import Path
import unittest

SPEC = importlib.util.spec_from_file_location("transfer_analysis", Path(__file__).with_name("analysis.py"))
analysis = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(analysis)


def dataset():
    tasks = [{"task_id": f"synthetic-{family}-{index}", "family": family, "negative_control": index == 3} for family in analysis.FAMILIES for index in range(4)]
    data = {
        "schema_version": 1,
        "protocol": {"task_manifest": tasks, "seeds": list(analysis.SEEDS), "arms": list(analysis.ARMS), "primary_horizon": 12, "bootstrap_samples": 10000, "bootstrap_seed": 20260905, "target_reduction": 0.20},
        "training": [], "attempts": [],
    }
    for family in analysis.FAMILIES:
        for component in ("common", *analysis.ARMS):
            count = 1200 if component in ("common", "learned") else 0
            data["training"].append({"family": family, "component": component, "usage_complete": True, "input_tokens": count, "output_tokens": 0, "cost_usd": count / 100000, "elapsed_seconds": count / 100, "budget_denied": False})
    for block in analysis.schedule(data):
        for position, arm in enumerate(block["arms"]):
            count = {"off": 1000, "raw": 800, "learned": 500}[arm]
            data["attempts"].append({"task_id": block["task_id"], "seed": block["seed"], "arm": arm, "order_position": position, "verified_success": True, "usage_complete": True, "input_tokens": count - 100, "output_tokens": 100, "cost_usd": count / 100000, "elapsed_seconds": count / 50, "budget_denied": False})
    return data


class AnalysisTests(unittest.TestCase):
    def analyze(self, data):
        return analysis.analyze(data, bootstrap_samples=200)

    def test_lifecycle_accounting_and_primary_gate(self):
        result = self.analyze(dataset())
        self.assertEqual(result["primary"]["status"], "met_on_fixed_benchmark")
        self.assertEqual(result["arms"]["off"]["all_attempt_run_tokens"], 36000)
        self.assertEqual(result["arms"]["off"]["all_attempt_lifecycle_tokens"], 39600)
        self.assertEqual(result["arms"]["learned"]["all_attempt_lifecycle_tokens"], 25200)
        self.assertAlmostEqual(result["primary"]["token_reduction_fraction"], 1 - 700 / 1100)
        self.assertEqual(result["complete_pair_analysis"]["task_clusters"], 12)
        self.assertEqual(result["complete_pair_analysis"]["blocks_included"], 36)
        self.assertFalse(result["secondary_raw_control"]["statistical_noninferiority_proven"])

    def test_favorable_exposed_measurements_preserve_development_label_and_numerical_results(self):
        data = dataset()
        baseline = self.analyze(data)
        self.assertNotIn("evidence_class", baseline)
        self.assertNotIn("confirmatory_claim", baseline)
        data.update(evidence_class="exposed-development", confirmatory_claim=False)
        result = self.analyze(data)
        self.assertEqual(result["evidence_class"], "exposed-development")
        self.assertIs(result["confirmatory_claim"], False)
        self.assertIn("Previously exposed development", result["scope"])
        self.assertEqual(result["primary"]["status"], "met_on_fixed_benchmark")
        changed_metadata = {"input_sha256", "scope", "evidence_class", "confirmatory_claim"}
        self.assertEqual({k:v for k,v in result.items() if k not in changed_metadata},
                         {k:v for k,v in baseline.items() if k not in changed_metadata})

    def test_unknown_exposure_labels_and_false_confirmation_metadata_are_rejected(self):
        for label in (None, "", "exposed-developmnt", "confirmed", True, [], {}):
            data = dataset()
            data.update(evidence_class=label, confirmatory_claim=False)
            with self.subTest(label=label), self.assertRaisesRegex(ValueError, "evidence_class"):
                self.analyze(data)
        for claim in (None, True, 0, "false"):
            data = dataset()
            data.update(evidence_class="exposed-development", confirmatory_claim=claim)
            with self.subTest(claim=claim), self.assertRaisesRegex(ValueError, "confirmatory_claim"):
                self.analyze(data)

    def test_training_sensitivity_cannot_be_selected_afterward(self):
        result = self.analyze(dataset())
        sensitivity = result["training_horizon_sensitivity"]
        self.assertLess(sensitivity["1"]["complete_pair_comparisons"]["off"]["token_reduction_fraction"], 0)
        self.assertLess(sensitivity["4"]["complete_pair_comparisons"]["off"]["token_reduction_fraction"], .20)
        self.assertEqual(result["primary"]["horizon_tasks_per_family"], 12)
        self.assertAlmostEqual(sensitivity["24"]["arms"]["learned"]["lifecycle_tokens_per_verified_success"], 600)

    def test_missing_run_keeps_quality_denominator_and_invalidates_primary(self):
        data = dataset()
        missing = next(record for record in data["attempts"] if record["arm"] == "learned")
        data["attempts"].remove(missing)
        result = self.analyze(data)
        self.assertEqual(result["primary"]["status"], "inconclusive")
        self.assertEqual(result["arms"]["learned"]["planned"], 36)
        self.assertEqual(result["arms"]["learned"]["attempted"], 35)
        self.assertEqual(result["arms"]["learned"]["success_rate_over_planned"], 35 / 36)
        self.assertEqual(result["complete_pair_analysis"]["blocks_included"], 35)
        self.assertIsNone(result["arms"]["learned"]["all_attempt_run_tokens"])

    def test_unknown_usage_excludes_all_three_arms_only_from_token_description(self):
        data = dataset()
        row = next(record for record in data["attempts"] if record["arm"] == "raw")
        row.update(usage_complete=False, input_tokens=None, output_tokens=None)
        result = self.analyze(data)
        self.assertEqual(result["primary"]["status"], "inconclusive")
        paired = result["complete_pair_analysis"]
        self.assertEqual(paired["blocks_included"], 35)
        self.assertEqual(paired["comparisons"]["off"]["arms"]["off"]["n"], 35)
        self.assertEqual(result["arms"]["raw"]["verified_successes_within_budget"], 36)
        self.assertEqual(len(paired["blocks_excluded"]), 1)

    def test_budget_denial_keeps_spent_tokens_and_is_a_quality_failure(self):
        data = dataset()
        row = next(record for record in data["attempts"] if record["arm"] == "learned")
        row["budget_denied"] = True
        result = self.analyze(data)
        self.assertEqual(result["primary"]["status"], "inconclusive")
        self.assertEqual(result["complete_pair_analysis"]["blocks_included"], 36)
        self.assertEqual(result["arms"]["learned"]["all_attempt_run_tokens"], 18000)
        self.assertEqual(result["arms"]["learned"]["grader_passes"], 36)
        self.assertEqual(result["arms"]["learned"]["verified_successes_within_budget"], 35)
        self.assertEqual(result["paired_quality_all_planned"]["off"]["reference_only_succeeded"], 1)

    def test_ordinary_failure_is_included_in_cost_per_success_and_guardrail(self):
        data = dataset()
        row = next(record for record in data["attempts"] if record["arm"] == "learned")
        row.update(verified_success=False, input_tokens=9900, output_tokens=100)
        result = self.analyze(data)
        self.assertEqual(result["primary"]["status"], "not_met")
        self.assertAlmostEqual(result["arms"]["learned"]["lifecycle_tokens_per_verified_success"], (25200 + 9500) / 35)

    def test_raw_grader_pass_without_autonomous_completion_is_not_success(self):
        for status in ("failed", "awaiting_input"):
            data = dataset()
            row = next(record for record in data["attempts"] if record["arm"] == "learned")
            row.update(raw_grader_pass=True, verified_success=False, status=status)
            with self.subTest(status=status):
                result = self.analyze(data)
                learned = result["arms"]["learned"]
                self.assertEqual(learned["grader_passes"], 36)
                self.assertEqual(learned["verified_successes_within_budget"], 35)
                self.assertEqual(learned["success_rate_over_planned"], 35 / 36)
                self.assertEqual(result["primary"]["status"], "not_met")

    def test_optional_raw_grader_pass_requires_boolean_or_null(self):
        for value in (0, 1, "true", [], {}):
            data = dataset()
            data["attempts"][0]["raw_grader_pass"] = value
            with self.subTest(value=value), self.assertRaisesRegex(ValueError, "raw_grader_pass"):
                self.analyze(data)
        data = dataset()
        row = data["attempts"][0]
        row.update(raw_grader_pass=None, verified_success=None)
        result = self.analyze(data)
        self.assertEqual(result["arms"][row["arm"]]["grader_passes"], 35)
        self.assertEqual(result["primary"]["status"], "inconclusive")

    def test_cost_unknown_does_not_become_zero_or_invalidate_known_tokens(self):
        data = dataset()
        data["attempts"][0]["cost_usd"] = None
        data["attempts"][0]["elapsed_seconds"] = None
        result = self.analyze(data)
        arm = data["attempts"][0]["arm"]
        self.assertEqual(result["primary"]["status"], "met_on_fixed_benchmark")
        self.assertIsNone(result["arms"][arm]["cost_usd"]["all_attempt_total"])
        self.assertEqual(result["arms"][arm]["cost_usd"]["known_records"], 35)
        self.assertIsNone(result["arms"][arm]["elapsed_seconds"]["all_attempt_total"])

    def test_training_usage_missing_invalidates_its_whole_family(self):
        data = dataset()
        data["training"] = [row for row in data["training"] if (row["family"], row["component"]) != ("flow", "learned")]
        result = self.analyze(data)
        self.assertEqual(result["primary"]["status"], "inconclusive")
        self.assertEqual(result["complete_pair_analysis"]["blocks_included"], 24)
        self.assertIsNone(result["primary"]["token_reduction_ci95"])

    def test_training_budget_state_must_be_explicit_for_every_component(self):
        for component in ("common", *analysis.ARMS):
            for value in ("missing", None, 0, 1, "false"):
                data = dataset()
                row = next(record for record in data["training"] if record["component"] == component)
                if value == "missing":
                    del row["budget_denied"]
                else:
                    row["budget_denied"] = value
                with self.subTest(component=component, value=value), self.assertRaisesRegex(ValueError, "explicit boolean"):
                    self.analyze(data)
        data = dataset()
        data["training"][0]["budget_denied"] = True
        result = self.analyze(data)
        self.assertEqual(result["primary"]["status"], "inconclusive")
        self.assertTrue(any(issue["kind"] == "training_budget_denial" for issue in result["primary"]["data_integrity_issues"]))

    def test_zero_successes_undefined_bootstrap_draws_are_not_discarded(self):
        data = dataset()
        for row in data["attempts"]:
            row["verified_success"] = False
        result = self.analyze(data)
        self.assertEqual(result["primary"]["status"], "inconclusive")
        self.assertIsNone(result["arms"]["off"]["lifecycle_tokens_per_verified_success"])
        self.assertEqual(result["complete_pair_analysis"]["comparisons"]["off"]["bootstrap"]["undefined_token_draws"], 200)
        json.dumps(result, allow_nan=False)

    def test_duplicate_attempts_and_unregistered_tasks_rejected(self):
        data = dataset()
        data["attempts"].append(copy.deepcopy(data["attempts"][0]))
        with self.assertRaisesRegex(ValueError, "duplicate attempt"):
            self.analyze(data)
        data = dataset()
        data["attempts"][0]["task_id"] = "unregistered"
        with self.assertRaisesRegex(ValueError, "outside the frozen"):
            self.analyze(data)

    def test_bad_numeric_metadata_is_rejected(self):
        for key, value in [("input_tokens", True), ("output_tokens", -1), ("input_tokens", 3.5), ("cost_usd", float("nan")), ("elapsed_seconds", float("inf"))]:
            data = dataset()
            data["attempts"][0][key] = value
            with self.subTest(key=key, value=value), self.assertRaises(ValueError):
                self.analyze(data)
        data = dataset()
        data["protocol"]["primary_horizon"] = 24
        with self.assertRaisesRegex(ValueError, "preregistered"):
            self.analyze(data)

    def test_order_is_balanced_and_actual_order_is_checked(self):
        data = dataset()
        blocks = analysis.schedule(data)
        self.assertEqual(set(Counter(tuple(block["arms"]) for block in blocks).values()), {6})
        for task in data["protocol"]["task_manifest"]:
            selected = [block for block in blocks if block["task_id"] == task["task_id"]]
            for arm in analysis.ARMS:
                self.assertEqual(sorted(block["arms"].index(arm) for block in selected), [0, 1, 2])
        del data["attempts"][0]["order_position"]
        self.assertEqual(self.analyze(data)["primary"]["status"], "inconclusive")

    def test_reasoning_and_cached_tokens_are_not_double_counted(self):
        data = dataset()
        for row in data["attempts"]:
            row.update(reasoning_output_tokens=80, cached_input_tokens=200)
        result = self.analyze(data)
        self.assertEqual(result["arms"]["learned"]["all_attempt_run_tokens"], 18000)
        self.assertEqual(result["arms"]["learned"]["reasoning_output_tokens"]["recorded_sum"], 2880)

    def test_cluster_bootstrap_keeps_repetitions_and_is_reproducible(self):
        data = dataset()
        for row in data["attempts"]:
            if row["arm"] == "learned":
                row["input_tokens"] += int(row["task_id"].rsplit("-", 1)[1]) * 100
        first, second = self.analyze(data), self.analyze(data)
        self.assertEqual(first, second)
        self.assertEqual(first["complete_pair_analysis"]["task_clusters"], 12)
        self.assertNotEqual(*first["primary"]["token_reduction_ci95"])
        self.assertEqual(first["groups"]["negative_controls"]["off"]["planned"], 9)
        self.assertEqual(len(first["leave_one_family_out"]), 3)

    def test_opposite_seed_effects_cancel_within_each_task_cluster(self):
        data = dataset()
        for row in data["attempts"]:
            if row["arm"] == "learned":
                row["input_tokens"] = {17: 0, 29: 400, 43: 800}[row["seed"]]
        result = self.analyze(data)
        lower, upper = result["primary"]["token_reduction_ci95"]
        # An incorrect independent run-level bootstrap would create variation.
        self.assertAlmostEqual(lower, 1 - 700 / 1100)
        self.assertEqual(lower, upper)

    def test_partly_undefined_bootstrap_does_not_keep_only_favorable_draws(self):
        data = dataset()
        for row in data["attempts"]:
            if row["arm"] in ("off", "learned"):
                row["verified_success"] = row["task_id"].endswith("-0")
        result = self.analyze(data)
        self.assertIsNotNone(result["primary"]["token_reduction_fraction"])
        interval = result["complete_pair_analysis"]["comparisons"]["off"]["bootstrap"]
        self.assertGreater(interval["undefined_token_draws"], 0)
        self.assertLess(interval["undefined_token_draws"], 200)
        self.assertIsNone(interval["token_reduction_ci95"])
        self.assertEqual(result["primary"]["status"], "inconclusive")

    def test_positive_interval_cannot_replace_twenty_percent_point_target(self):
        data = dataset()
        for row in data["attempts"]:
            if row["arm"] == "learned":
                row["input_tokens"] = 700
        result = self.analyze(data)
        self.assertTrue(result["primary"]["ci95_excludes_zero_improvement"])
        self.assertFalse(result["primary"]["point_estimate_at_least_20_percent"])
        self.assertEqual(result["primary"]["status"], "not_met")


if __name__ == "__main__":
    unittest.main()
