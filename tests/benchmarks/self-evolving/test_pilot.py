"""Pilot lifecycle and budget isolation checks without a daemon or provider."""
import contextlib
import io
import json
from pathlib import Path
import shutil
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import pilot


class PilotSchedule(unittest.TestCase):
    def test_only_complete_accounted_and_graded_schedules_are_comparable(self):
        schedule = pilot.development_schedule("report", [17, 29, 43])
        known = {"provider_usage_complete": True, "budget_denied": False,
                 "status": "failed", "grade": {"grading_complete": True, "passed": False}}
        # A genuine candidate failure is a measured outcome, not missing data.
        self.assertTrue(pilot.comparable_results(known, [known]*9, schedule))
        self.assertFalse(pilot.comparable_results(known, [known]*3, schedule))
        for incomplete in (
            {**known, "provider_usage_complete": False},
            {**known, "budget_denied": True},
            {**known, "grade": {"grading_complete": False, "infra_error": "grader_deadline"}},
        ):
            self.assertFalse(pilot.comparable_results(known, [known]*8+[incomplete], schedule))
            self.assertFalse(pilot.comparable_results(incomplete, [known]*9, schedule))

    def test_three_seeds_balance_each_arm_position_for_every_project(self):
        for project, first in (("report", "off"), ("queue", "raw"), ("flow", "learned")):
            schedule = pilot.development_schedule(project, [17, 29, 43])
            self.assertEqual(len({(item["seed"], item["arm"]) for item in schedule}), 9)
            self.assertEqual(schedule[0]["arm"], first)
            for arm in ("off", "raw", "learned"):
                self.assertEqual(sorted(item["order_position"] for item in schedule if item["arm"] == arm), [0, 1, 2])
            self.assertEqual(pilot.development_schedule(project, [17]), schedule[:3])

    def test_invalid_seed_and_budget_arguments_fail_before_creating_output(self):
        for seeds in ([], [17, 17], [True], ["17"]):
            with self.subTest(seeds=seeds), self.assertRaises(ValueError):
                pilot.development_schedule("report", seeds)
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)/"output"
            for extra in (["--seeds"], ["--seeds", "17", "17"], ["--training-only", "--seeds", "17"], ["--max-cost", "nan"], ["--max-cost", "inf"], ["--max-cost", "0"]):
                with self.subTest(arguments=extra), patch.object(pilot, "Meter") as meter:
                    with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit) as exit_status:
                        pilot.main(["--key-file", "/nonexistent/offline-key", "--project", "report", "--output", str(output), *extra])
                    self.assertEqual(exit_status.exception.code, 2)
                    meter.assert_not_called()
                    self.assertFalse(output.exists())


class PilotLifecycle(unittest.TestCase):
    def exercise(self, directory, *, seeds=None, training_status="completed", fail_attempt=None, training_only=False, training_unknown=False):
        root = Path(directory)
        here = root/"harness"
        fixture = here/"fixtures/seed"
        fixture.mkdir(parents=True)
        (fixture/"source.py").write_text("initial source\n")
        tasks = [{"id": f"report-{split}", "project": "report", "split": split, "prompt": f"{split}.md"} for split in ("train", "dev")]
        for task in tasks:
            (here/"fixtures"/task["prompt"]).write_text("Synthetic public task")
        (here/"fixtures/pilot.json").write_text(json.dumps({"projects": [{"id": "report", "source": "seed"}], "tasks": tasks}))
        (root/"target/debug").mkdir(parents=True)
        (root/"target/debug/s-code-daemon").write_bytes(b"offline fake executable, never launched")
        key = root/"dummy-key"
        key.write_text("offline dummy, never transmitted")
        output = root/"output"
        instances, runs, grades, meters, servers = [], [], [], [], []
        case = self

        def copy_source(source, destination):
            if destination.exists():
                shutil.rmtree(destination)
            shutil.copytree(source, destination)

        class FakeMeter:
            def __init__(self, key, output, model, max_cost, max_calls, provider):
                self.max_cost, self.max_calls, self.raw = max_cost, max_calls, None
                meters.append(self)

            def handler(self):
                return object

        class FakeServer:
            server_port = 12345

            def __init__(self, *args):
                self.closed = False
                servers.append(self)

            def serve_forever(self):
                raise AssertionError("Offline test must not start a proxy server")

            def shutdown(self):
                pass

            def server_close(self):
                self.closed = True

        class FakeDaemon:
            def __init__(self, profile, meter, proxy):
                case.assertTrue(all(instance.closed for instance in instances), "Previous daemon still alive")
                self.root, self.meter, self.closed = profile, meter, False
                learned = profile.name == "learned"
                case.assertEqual(profile.exists(), learned, "Profile reused or learned template missing")
                if learned:
                    case.assertEqual((profile/"state").read_text(), "durable training state written at close")
                    case.assertFalse((profile/"tool-cache").exists(), "Training cache leaked into transfer")
                profile.mkdir(parents=True, exist_ok=True)
                case.assertFalse((profile/"development-marker").exists(), "Earlier attempt contaminated this profile")
                instances.append(self)

            def run(self, workspace, task, mode, result_dir, *, seed=17, arm=None):
                case.assertFalse(self.closed)
                case.assertEqual(self.meter.phase_max_cost, 1.5)
                runs.append({"seed": seed, "arm": arm, "profile": self.root, "result_dir": result_dir})
                if (seed, arm) == fail_attempt:
                    raise RuntimeError("synthetic interrupted attempt")
                copy_source(workspace, result_dir/"initial")
                if arm == "training":
                    case.assertEqual(seed, 17)
                    (workspace/"source.py").write_text("same trained source\n")
                    (self.root/"tool-cache").mkdir()
                    (self.root/"tool-cache/training").write_text("must not be reused")
                else:
                    case.assertEqual((workspace/"source.py").read_text(), "same trained source\n", "Earlier candidate edits leaked")
                    case.assertEqual(self.meter.raw() if self.meter.raw else None, "raw evidence" if arm == "raw" else None)
                    case.assertEqual(mode, "reuse" if arm == "learned" else "off")
                    (self.root/"development-marker").write_text(f"{seed}/{arm}")
                    (workspace/"source.py").write_text(f"development edit {seed}/{arm}\n")
                shutil.copytree(workspace, result_dir/"final")
                result = {"task": task["id"], "seed": seed, "status": training_status if arm == "training" else "failed" if (seed, arm) == (29, "off") else "completed", "requests": [len(runs)], "budget_denied": (seed, arm) == (29, "raw"), "provider_usage_complete": not (training_unknown and arm == "training") and (seed, arm) != (29, "learned")}
                pilot.write_json(result_dir/"result.json", result)
                return result

            def session(self, *args):
                case.assertFalse(self.closed)
                return "new-frozen-metadata-session"

            def api(self, method, path):
                case.assertFalse(self.closed)
                return [{"id": "training-only-lesson"}] if "/lessons?" in path else {"mode": "reuse"}

            def close(self):
                case.assertFalse(self.closed, "Daemon closed more than once")
                if self.root.name == "training":
                    (self.root/"state").write_text("durable training state written at close")
                self.closed = True

        def fake_grade(result_dir, task, baseline):
            case.assertTrue(all(instance.closed for instance in instances), "Grading overlapped a daemon")
            case.assertTrue((result_dir/"final").is_dir())
            grades.append(result_dir)
            return {"task": task, "passed": True, "grading_complete": True}

        requested = seeds if seeds is not None else [17]
        arguments = ["--key-file", str(key), "--output", str(output), "--project", "report", "--max-cost", str(1.5 if training_only else 1.5*(1+3*len(requested)))]
        if training_only:
            arguments += ["--training-only"]
        if seeds is not None:
            arguments += ["--seeds", *map(str, seeds)]
        with contextlib.ExitStack() as stack:
            for name, replacement in (("ROOT", root), ("HERE", here), ("Daemon", FakeDaemon), ("Meter", FakeMeter), ("copy_tree", copy_source), ("grade", fake_grade)):
                stack.enter_context(patch.object(pilot, name, replacement))
            stack.enter_context(patch.object(pilot, "snapshot_source", return_value="offline-source-hash"))
            stack.enter_context(patch.object(pilot, "raw_corpus", return_value=[{"observation": "training only"}]))
            stack.enter_context(patch.object(pilot, "raw_retrieve", return_value="raw evidence"))
            stack.enter_context(patch.object(pilot.http.server, "ThreadingHTTPServer", FakeServer))
            stack.enter_context(patch.object(pilot.threading, "Thread", return_value=SimpleNamespace(start=lambda: None)))
            if fail_attempt is None:
                code = pilot.main(arguments)
            else:
                with self.assertRaisesRegex(RuntimeError, "synthetic interrupted attempt"):
                    pilot.main(arguments)
                code = None
        self.assertTrue(all(instance.closed for instance in instances))
        self.assertTrue(all(server.closed for server in servers))
        return SimpleNamespace(output=output, runs=runs, grades=grades, instances=instances, meter=meters[0], code=code)

    def test_three_seed_run_uses_fresh_profiles_and_retains_all_outcomes(self):
        with tempfile.TemporaryDirectory() as directory:
            run = self.exercise(directory, seeds=[17, 29, 43])
            self.assertEqual(run.code, 0)
            self.assertEqual(len(run.instances), 10)
            self.assertEqual(len(run.grades), 10)
            self.assertEqual(len({item["profile"] for item in run.runs}), 10)
            self.assertEqual(run.meter.max_calls, 2000)
            self.assertEqual(run.meter.max_cost, 15)
            result = json.loads((run.output/"results.json").read_text())
            self.assertEqual(len(result["development"]), 9)
            self.assertFalse(result["comparable"])
            self.assertEqual(result["training"]["seed"], 17)
            self.assertEqual(result["training"]["arm"], "training")
            for row in result["development"]:
                self.assertTrue((run.output/f"seed-{row['seed']}"/row["arm"]/"result.json").is_file())
            by_key = {(row["seed"], row["arm"]): row for row in result["development"]}
            self.assertEqual(by_key[29, "off"]["status"], "failed")
            self.assertTrue(by_key[29, "raw"]["budget_denied"])
            self.assertFalse(by_key[29, "learned"]["provider_usage_complete"])
            self.assertFalse((run.output/"training-profile/development-marker").exists())
            manifest = json.loads((run.output/"manifest.json").read_text())
            self.assertEqual(manifest["seeds"], [17, 29, 43])
            self.assertEqual(manifest["per_task_cost_cap"], 1.5)
            self.assertEqual(manifest["development_schedule"], [{key: row[key] for key in ("seed", "arm", "order_position")} for row in result["development"]])

    def test_single_seed_preserves_layout_and_training_stays_seed_seventeen(self):
        for seeds in (None, [29]):
            with self.subTest(seeds=seeds), tempfile.TemporaryDirectory() as directory:
                run = self.exercise(directory, seeds=seeds)
                self.assertEqual(run.runs[0]["seed"], 17)
                self.assertEqual(run.meter.max_calls, 800)
                self.assertEqual({item["seed"] for item in run.runs[1:]}, {17 if seeds is None else 29})
                for arm in ("off", "raw", "learned"):
                    self.assertTrue((run.output/arm/"result.json").is_file())
                self.assertFalse(any(run.output.glob("seed-*")))

    def test_failed_training_is_closed_and_graded_without_development(self):
        with tempfile.TemporaryDirectory() as directory:
            run = self.exercise(directory, seeds=[17, 29, 43], training_status="failed")
            self.assertEqual(run.code, 2)
            self.assertEqual(len(run.runs), 1)
            self.assertEqual(len(run.grades), 1)
            result = json.loads((run.output/"results.json").read_text())
            self.assertEqual(result["training"]["status"], "failed")
            self.assertNotIn("development", result)

    def test_interrupted_attempt_closes_daemon_and_keeps_prior_results_without_retry(self):
        with tempfile.TemporaryDirectory() as directory:
            run = self.exercise(directory, seeds=[17, 29, 43], fail_attempt=(29, "raw"))
            result = json.loads((run.output/"results.json").read_text())
            self.assertEqual(len(result["development"]), 3)
            self.assertFalse(result["comparable"])
            self.assertEqual([(row["seed"], row["arm"]) for row in run.runs].count((29, "raw")), 1)
            self.assertEqual(len(run.grades), 4)

    def test_training_only_freezes_closed_artifacts_and_spends_one_task_budget(self):
        with tempfile.TemporaryDirectory() as directory:
            run = self.exercise(directory, training_only=True)
            self.assertEqual(run.code, 0)
            self.assertEqual([(row["seed"], row["arm"]) for row in run.runs], [(17, "training")])
            self.assertEqual(len(run.grades), 1)
            self.assertEqual(run.meter.max_calls, 200)
            self.assertEqual(run.meter.max_cost, 1.5)
            self.assertEqual((run.output/"training-profile/state").read_text(), "durable training state written at close")
            self.assertEqual((run.output/"trained-source/source.py").read_text(), "same trained source\n")
            result = json.loads((run.output/"results.json").read_text())
            self.assertTrue(result["training_only"])
            self.assertFalse(result["comparable"])
            self.assertEqual(result["development"], [])
            self.assertEqual(result["frozen_artifact_sha256"], pilot.frozen_artifact_hashes(run.output))
            manifest = json.loads((run.output/"manifest.json").read_text())
            self.assertEqual(manifest["phase"], "training-only")
            self.assertEqual(manifest["seeds"], [])
            self.assertEqual(manifest["development_schedule"], [])
            self.assertFalse(any((run.output/arm).exists() for arm in ("off", "raw", "learned")))

    def test_training_only_retains_failure_and_unknown_accounting_without_retry(self):
        for status, unknown in (("failed", False), ("completed", True)):
            with self.subTest(status=status, unknown=unknown), tempfile.TemporaryDirectory() as directory:
                run = self.exercise(directory, training_only=True, training_status=status, training_unknown=unknown)
                result = json.loads((run.output/"results.json").read_text())
                self.assertEqual(len(run.runs), 1)
                self.assertEqual(len(run.grades), 1)
                self.assertEqual(result["training"]["status"], status)
                self.assertEqual(result["training"]["provider_usage_complete"], not unknown)
                if status == "failed":
                    self.assertEqual(run.code, 2)
                    self.assertFalse((run.output/"training-profile").exists())
                else:
                    self.assertEqual(run.code, 0)
                    self.assertFalse(result["comparable"])


class FrozenPilotArtifacts(unittest.TestCase):
    def test_changed_frozen_inputs_and_reused_profiles_stop_before_attempt(self):
        for mutation in ("trained-source/source.py", "training-profile/state", "frozen.json", "raw-corpus.json", "profile"):
            with self.subTest(mutation=mutation), tempfile.TemporaryDirectory() as directory:
                output = Path(directory)
                for name in ("trained-source", "training-profile", "workspace"):
                    (output/name).mkdir()
                (output/"trained-source/source.py").write_text("frozen")
                (output/"training-profile/state").write_text("closed")
                (output/"frozen.json").write_text("{}")
                (output/"raw-corpus.json").write_text("[]")
                (output/"workspace/previous").write_text("must survive rejected preparation")
                expected = pilot.frozen_artifact_hashes(output)
                profile = output/"profile"
                if mutation == "profile":
                    profile.mkdir()
                else:
                    (output/mutation).write_text("changed")
                with self.assertRaisesRegex(RuntimeError, "changed|must be new"):
                    pilot.prepare_development_attempt(output, profile, "learned", expected)
                self.assertEqual((output/"workspace/previous").read_text(), "must survive rejected preparation")


if __name__ == "__main__":
    unittest.main()
