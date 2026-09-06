"""Synthetic grader/analysis handshake checks; no models or sealed task data."""
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import evaluate
from analysis import analyze
from evaluate import normalized
from pilot import grading_verdict
from test_analysis import dataset


class GradingHandshake(unittest.TestCase):
    def verdict(self, *, code=0, task="synthetic-task", passed=True, checks=2, failures=0, errors=0, stdout=None):
        if stdout is None:
            stdout = json.dumps(dict(task=task, passed=passed, checks=checks, failures=failures, errors=errors))
        return grading_verdict(subprocess.CompletedProcess([], code, stdout, ""), "synthetic-task")

    def test_complete_failures_and_candidate_exceptions_remain_known_failures(self):
        for counts in ({"failures": 1}, {"errors": 1}):
            verdict = self.verdict(code=1, passed=False, **counts)
            self.assertTrue(verdict["grading_complete"])
            self.assertIsNone(verdict["infra_error"])
            self.assertFalse(verdict["passed"])
        self.assertTrue(self.verdict()["grading_complete"])

    def test_outer_deadline_and_output_limit_are_unknown_even_after_a_json_line(self):
        for code, reason in ((124, "grader_deadline"), (125, "grader_output_limit"), (-9, "grader_process_failed")):
            verdict = self.verdict(code=code)
            self.assertFalse(verdict["grading_complete"])
            self.assertFalse(verdict["passed"])
            self.assertEqual(verdict["infra_error"], reason)

    def test_missing_or_malformed_grader_verdict_is_unknown(self):
        cases = [
            {"code": 1, "stdout": ""},
            {"stdout": "not JSON"}, {"stdout": "[]"}, {"stdout": "null"},
            {"stdout": '{"passed":true}'}, {"checks": 0}, {"checks": True},
            {"task": "wrong-task"}, {"failures": 1}, {"errors": -1},
            {"code": 1}, {"code": 0, "passed": False, "failures": 1},
        ]
        for arguments in cases:
            with self.subTest(arguments=arguments):
                verdict = self.verdict(**arguments)
                self.assertFalse(verdict["grading_complete"])
                self.assertIsNotNone(verdict["infra_error"])

    def test_off_grader_crash_makes_primary_inconclusive_even_when_learned_passes(self):
        data = dataset()
        row = next(record for record in data["attempts"] if record["arm"] == "off")
        result = {
            "provider_usage": {"input_tokens": row["input_tokens"], "output_tokens": row["output_tokens"]},
            "grade": self.verdict(code=1, stdout=""), "status": "completed",
            "cost_usd": row["cost_usd"], "elapsed_seconds": row["elapsed_seconds"], "budget_denied": False,
        }
        row.update(normalized({"id": row["task_id"]}, row["seed"], row["arm"], row["order_position"], result, []))
        self.assertIsNone(row["verified_success"])
        report = analyze(data, bootstrap_samples=200)
        self.assertEqual(report["primary"]["status"], "inconclusive")
        self.assertTrue(any(issue["kind"] == "unknown_grade" for issue in report["primary"]["data_integrity_issues"]))
        self.assertEqual(report["arms"]["off"]["planned"], 36)
        self.assertEqual(report["arms"]["off"]["graded"], 35)
        self.assertEqual(report["arms"]["learned"]["verified_successes_within_budget"], 36)


class FrozenArtifacts(unittest.TestCase):
    def family(self, root):
        for directory in ("trained-source", "training-profile", "training", "requests"):
            (root/directory).mkdir()
        (root/"trained-source/source.py").write_text("frozen source")
        (root/"training-profile/state").write_text("frozen state")
        (root/"requests/ledger.json").write_text("[]")
        (root/"raw-corpus.json").write_text("[]")
        (root/"training/result.json").write_text(json.dumps({"status": "completed", "grade": {"passed": True}}))
        source_hash = evaluate.tree_hash(root/"trained-source")
        (root/"frozen.json").write_text(json.dumps({"source_hash": source_hash, "lessons": []}))
        return {
            "pilot_root": str(root), "source_hash": source_hash,
            "frozen_file_hash": evaluate.digest(root/"frozen.json"),
            "raw_corpus_hash": evaluate.digest(root/"raw-corpus.json"),
            "requests_hash": evaluate.tree_hash(root/"requests"),
            "profile_hash": evaluate.tree_hash(root/"training-profile"),
            "training_result_hash": evaluate.digest(root/"training/result.json"),
        }

    def test_every_attempt_restores_source_and_rechecks_frozen_artifacts(self):
        for changed in ("trained-source/source.py", "training-profile/state", "raw-corpus.json", "requests/ledger.json"):
            with self.subTest(changed=changed), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                family = self.family(root)
                _, workspace, _ = evaluate.prepare_attempt(family, root/"profile-first", "learned")
                (workspace/"source.py").write_text("previous arm's output")
                evaluate.prepare_attempt(family, root/"profile-second", "off")
                self.assertEqual((workspace/"source.py").read_text(), "frozen source")
                (root/changed).write_text("changed between arms")
                with self.assertRaisesRegex(RuntimeError, "changed"):
                    evaluate.prepare_attempt(family, root/"profile-third", "raw")

    def test_corrupted_source_or_profile_copy_cannot_start_a_daemon(self):
        for corrupt_profile in (False, True):
            with self.subTest(profile=corrupt_profile), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                family = self.family(root)
                profile = root/"profile"
                if corrupt_profile:
                    original = shutil.copytree
                    def copy_bad(source, destination, *args, **kwargs):
                        result = original(source, destination, *args, **kwargs)
                        if Path(destination) == profile:
                            (profile/"state").write_text("corrupted copy")
                        return result
                    target = "evaluate.shutil.copytree"
                else:
                    original = evaluate.copy_tree
                    def copy_bad(source, destination):
                        original(source, destination)
                        (destination/"source.py").write_text("corrupted copy")
                    target = "evaluate.copy_tree"
                with patch(target, side_effect=copy_bad), self.assertRaisesRegex(RuntimeError, "Copied training"):
                    evaluate.prepare_attempt(family, profile, "learned")

    def test_grader_and_helpers_are_checked_before_and_after_grading(self):
        for during_grade in (False, True):
            with self.subTest(during_grade=during_grade), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                grader, helper = root/"grader.py", root/"helper.py"
                grader.write_text("frozen grader")
                helper.write_text("frozen helper")
                hashes = {path: evaluate.digest(path) for path in (grader, helper)}
                def fake_grade(*args, **kwargs):
                    helper.write_text("modified during grading")
                    return {"passed": True}
                if not during_grade:
                    grader.write_text("modified before grading")
                with patch("evaluate.grade", side_effect=fake_grade) as grade, self.assertRaisesRegex(RuntimeError, "Frozen grader or helper changed"):
                    evaluate.grade_frozen(root, "synthetic-task", root, grader=grader, trusted_helpers=(helper,), expected_hashes=hashes)
                self.assertEqual(grade.call_count, int(during_grade))


if __name__ == "__main__":
    unittest.main()
