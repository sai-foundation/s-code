#!/usr/bin/env python3
"""Public pilot checks. Run outside the candidate; never import candidate code.

Usage: python3 grade.py --workspace /absolute/candidate --task report-train
The harness must separately verify protected-file hashes. This is a functional
grader, not a sandbox for deliberately hostile Python. No final holdouts here.
"""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import sqlite3
import sys
import tempfile
import unittest

# Load exactly one trusted helper without searching the candidate or its parent.
_helper_path = Path(__file__).resolve().parents[1] / "bounded_process.py"
_helper_spec = importlib.util.spec_from_file_location("grading_process", _helper_path)
_helper = importlib.util.module_from_spec(_helper_spec)
_helper_spec.loader.exec_module(_helper)
bounded_run = _helper.run

WORKSPACE = None
TASK = None


class PilotCase(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="s-code-pilot-grade-")
        self.addCleanup(self.temporary.cleanup)
        self.directory = Path(self.temporary.name)
        self.env = {key: os.environ[key] for key in ("PATH", "LANG", "SYSTEMROOT", "WINDIR") if key in os.environ}
        self.env.update(TMPDIR=str(self.directory), PYTHONUTF8="1", PYTHONDONTWRITEBYTECODE="1")

    def cli(self, module, *args, code=0):
        result = bounded_run([sys.executable, "-m", module, *map(str, args)], cwd=WORKSPACE, env=self.env, timeout=20)
        self.assertEqual(result.returncode, code, result.stderr)
        self.assertNotIn("Traceback", result.stderr)
        if code in (0, 1):
            self.assertEqual(result.stderr, "")
        else:
            self.assertEqual(result.stdout, "")
            self.assertTrue(result.stderr.strip())
        return result


class PublicBaseline(PilotCase):
    def test_existing_tests(self):
        result = bounded_run([sys.executable, "-m", "unittest", "discover", "-s", "tests", "-v"], cwd=WORKSPACE, env=self.env, timeout=90)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertNotIn("Ran 0 tests", result.stderr)


class ReportCase(PilotCase):
    def setUp(self):
        super().setUp()
        self.source, self.output = self.directory / "input.jsonl", self.directory / "output.json"

    def write(self, events):
        self.source.write_text("\n".join(json.dumps(e, ensure_ascii=False) for e in events) + "\n", encoding="utf-8")

    def report(self, *args, code=0):
        result = self.cli("incident_report", self.source, "--output", self.output, *args, code=code)
        self.assertEqual(result.stdout, "")
        if code == 0:
            data = self.output.read_text(encoding="utf-8")
            self.assertTrue(data.endswith("\n"))
            return json.loads(data)
        return result


class ReportTrain(ReportCase):
    def test_counts_zero_large_default_and_ties(self):
        messages = ["雪", "alpha", "z", "alpha", "雪", "b"]
        self.write([{"service": "api", "severity": "info", "message": m} for m in messages])
        original = self.report()
        self.assertEqual(self.report("--top", "3"), original)
        zero = self.report("--top", "0")
        self.assertEqual(zero["top_messages"], [])
        self.assertEqual({k: v for k, v in zero.items() if k != "top_messages"}, {k: v for k, v in original.items() if k != "top_messages"})
        self.assertEqual(self.report("--top", "40")["top_messages"], [{"message": "alpha", "count": 2}, {"message": "雪", "count": 2}, {"message": "b", "count": 1}, {"message": "z", "count": 1}])

    def test_invalid_top_preserves_output(self):
        self.write([])
        for value in ("-1", "1.5", "word"):
            with self.subTest(value=value):
                self.output.write_bytes(b"previous\n")
                self.report("--top", value, code=2)
                self.assertEqual(self.output.read_bytes(), b"previous\n")

    def test_late_input_error_preserves_output(self):
        self.source.write_text('{"service":"api","severity":"info","message":"ok"}\n\n{bad\n')
        self.output.write_bytes(b"previous\n")
        result = self.report("--top", "10", code=2)
        self.assertIn("line 3:", result.stderr)
        self.assertEqual(self.output.read_bytes(), b"previous\n")


class ReportDev(ReportCase):
    def test_byte_boundary_unicode_crlf(self):
        line = json.dumps({"service": "api", "severity": "info", "message": "雪雪"}, ensure_ascii=False)
        size = len(line.encode("utf-8"))
        for terminator in ("\n", "\r\n", ""):
            with self.subTest(terminator=repr(terminator)):
                self.source.write_bytes((line + terminator).encode("utf-8"))
                self.assertEqual(self.report("--max-line-bytes", str(size))["total"], 1)
                self.output.write_bytes(b"previous\n")
                result = self.report("--max-line-bytes", str(size - 1), code=2)
                self.assertIn("line 1:", result.stderr)
                self.assertEqual(self.output.read_bytes(), b"previous\n")
        self.assertEqual(self.report()["total"], 1)

    def test_late_oversize_and_invalid_options(self):
        short = json.dumps({"service": "a", "severity": "info", "message": "ok"})
        long = json.dumps({"service": "a", "severity": "info", "message": "x" * 200})
        self.source.write_text(short + "\n\n" + long + "\n")
        self.output.write_bytes(b"previous\n")
        result = self.report("--max-line-bytes", "100", code=2)
        self.assertIn("line 3:", result.stderr)
        self.assertEqual(self.output.read_bytes(), b"previous\n")
        for value in ("0", "-1", "1.5", "nan"):
            self.report("--max-line-bytes", value, code=2)
            self.assertEqual(self.output.read_bytes(), b"previous\n")

    def test_limit_includes_empty_physical_lines(self):
        self.source.write_text(" " * 12 + "\n")
        self.output.write_bytes(b"previous\n")
        result = self.report("--max-line-bytes", "10", code=2)
        self.assertIn("line 1:", result.stderr)
        self.assertEqual(self.output.read_bytes(), b"previous\n")


class QueueCase(PilotCase):
    def setUp(self):
        super().setUp()
        self.database = self.directory / "queue.sqlite"

    def queue(self, command, *args, code=0, database=None):
        result = self.cli("durable_queue", command, database or self.database, *args, code=code)
        if code == 0:
            self.assertTrue(result.stdout.endswith("\n"))
            self.assertEqual(len(result.stdout.splitlines()), 1)
            return json.loads(result.stdout)
        return result

    def rows(self):
        with sqlite3.connect(self.database) as connection:
            connection.row_factory = sqlite3.Row
            return {row["task_id"]: dict(row) for row in connection.execute("SELECT * FROM tasks")}

    def snapshot(self):
        with sqlite3.connect(self.database) as connection:
            return tuple(connection.iterdump())


class QueueTrain(QueueCase):
    def test_nonfinite_now_preserves_existing_rows(self):
        self.queue("enqueue", "waiting", '{"value":1}', "--now", "0")
        lease = self.queue("claim", "worker", "--lease-seconds", "10", "--now", "1")
        initial = self.snapshot()
        for value in ("nan", "inf", "-inf"):
            for command, args in [("stats", []), ("enqueue", ["new", "{}"]), ("claim", ["other", "--lease-seconds", "10"]), ("fail", ["waiting", "worker", lease["lease_token"], "--error", "retry"])]:
                with self.subTest(value=value, command=command):
                    self.queue(command, *args, "--now=" + value, code=2)
                    self.assertEqual(self.snapshot(), initial)

    def test_nonfinite_options_never_create_database(self):
        number = 0
        for value in ("nan", "inf", "-inf"):
            for command, args in [("stats", ["--now=" + value]), ("enqueue", ["id", "{}", "--now=" + value]), ("claim", ["w", "--lease-seconds", "1", "--now=" + value]), ("fail", ["id", "w", "token", "--error", "retry", "--now=" + value]), ("claim", ["w", "--lease-seconds=" + value, "--now", "1"])]:
                number += 1
                database = self.directory / f"missing-{number}.sqlite"
                with self.subTest(value=value, command=command, args=args):
                    self.queue(command, *args, code=2, database=database)
                    self.assertFalse(database.exists(), "invalid options initialized the database")
                    self.assertFalse(Path(str(database) + "-wal").exists())

    def test_invalid_duration_cannot_claim(self):
        self.queue("enqueue", "waiting", "[]", "--now", "0")
        initial = self.snapshot()
        for value in ("nan", "inf", "-inf", "0", "-1"):
            with self.subTest(value=value):
                self.queue("claim", "w", "--lease-seconds=" + value, "--now", "1", code=2)
                self.assertEqual(self.snapshot(), initial)

    def test_finite_fractional_and_negative_times_still_work(self):
        self.queue("enqueue", "id", "null", "--now=-3.5")
        first = self.queue("claim", "a", "--lease-seconds", "2.25", "--now=-2.5")
        self.assertEqual(first["lease_expires_at"], -0.25)
        self.assertEqual(first["attempt"], 1)
        self.assertIsNone(self.queue("claim", "b", "--lease-seconds", "1", "--now=-0.5"))
        second = self.queue("claim", "b", "--lease-seconds", "1.5", "--now=0")
        self.assertEqual(second["attempt"], 2)
        self.assertNotEqual(first["lease_token"], second["lease_token"])
        initial = self.snapshot()
        self.queue("ack", "id", "a", first["lease_token"], code=2)
        self.assertEqual(self.snapshot(), initial)
        self.queue("ack", "id", "b", second["lease_token"])


class QueueDev(QueueCase):
    def test_purge_only_completed_including_stored_expired_lease(self):
        self.queue("enqueue", "done", '"done"', "--now", "0")
        lease = self.queue("claim", "w", "--lease-seconds", "10", "--now", "0")
        self.queue("ack", "done", "w", lease["lease_token"])
        self.queue("enqueue", "dead", '{"dead":true}', "--max-attempts", "1", "--now", "0")
        lease = self.queue("claim", "w", "--lease-seconds", "10", "--now", "0")
        self.queue("fail", "dead", "w", lease["lease_token"], "--error", "permanent", "--now", "0")
        self.queue("enqueue", "queued", "[]", "--priority=-10", "--now", "0")
        self.queue("enqueue", "expired", '{"keep":"雪"}', "--priority", "50", "--now", "0")
        self.queue("claim", "old", "--lease-seconds", "1", "--now", "0")
        self.queue("enqueue", "active", "[1,2]", "--priority", "100", "--now", "0")
        self.queue("claim", "active-worker", "--lease-seconds", "100000000000", "--now", "0")
        original = self.rows()
        self.assertEqual({row["status"] for row in original.values()}, {"completed", "queued", "leased", "dead"})
        self.assertEqual(self.queue("purge-completed"), {"removed": 1})
        self.assertEqual(self.rows(), {key: row for key, row in original.items() if key != "done"})
        self.assertEqual(self.queue("purge-completed"), {"removed": 0})
        self.assertEqual(self.rows(), {key: row for key, row in original.items() if key != "done"})

    def test_empty_existing_database(self):
        self.queue("init")
        self.assertEqual(self.queue("purge-completed"), {"removed": 0})
        self.assertEqual(self.rows(), {})


class FlowCase(PilotCase):
    def setUp(self):
        super().setUp()
        self.plan, self.output = self.directory / "plan.json", self.directory / "report.json"

    def python(self, source):
        return [sys.executable, "-c", source]

    def task(self, task_id, **extra):
        return {"id": task_id, "command": self.python(f"print({task_id!r})"), **extra}

    def flow(self, tasks, *args, code=0):
        self.plan.write_text(json.dumps({"tasks": tasks}), encoding="utf-8")
        result = self.cli("flow_runner", self.plan, "--output", self.output, *args, code=code)
        self.assertEqual(result.stdout, "")
        if code in (0, 1):
            return json.loads(self.output.read_text(encoding="utf-8"))
        return result


class FlowTrain(FlowCase):
    def test_duplicates_rejected_before_any_execution(self):
        marker = self.directory / "started"
        tasks = [self.task("a", command=self.python(f"from pathlib import Path; Path({str(marker)!r}).touch()")), self.task("invalid", depends_on=["a", "a"])]
        self.output.write_bytes(b"previous\n")
        result = self.flow(tasks, "--jobs", "3", code=2)
        self.assertIn("invalid", result.stderr)
        self.assertFalse(marker.exists())
        self.assertEqual(self.output.read_bytes(), b"previous\n")

    def test_diamond_and_dependency_order_are_valid(self):
        tasks = [self.task("finish", depends_on=["right", "left"]), self.task("right", depends_on=["start"]), self.task("left", depends_on=["start"]), self.task("start")]
        result = self.flow(tasks, "--jobs", "3")
        self.assertEqual(result["status"], "succeeded")
        self.assertEqual([task["id"] for task in result["tasks"]], [task["id"] for task in tasks])
        self.assertTrue(all(task["attempts"] == 1 for task in result["tasks"]))


class FlowDev(FlowCase):
    def test_stops_after_retries_without_running_later_independent_work(self):
        counter, marker = self.directory / "counter", self.directory / "later"
        source = f"from pathlib import Path; p=Path({str(counter)!r}); p.write_text(str(int(p.read_text())+1 if p.exists() else 1)); raise SystemExit(7)"
        tasks = [self.task("z-later", command=self.python(f"from pathlib import Path; Path({str(marker)!r}).touch()")), self.task("a-fails", command=self.python(source), retries=2)]
        result = self.flow(tasks, "--jobs", "1", "--fail-fast", code=1)
        self.assertEqual(counter.read_text(), "3")
        self.assertFalse(marker.exists())
        self.assertEqual([task["id"] for task in result["tasks"]], ["z-later", "a-fails"])
        skipped, failed = result["tasks"]
        self.assertEqual((skipped["status"], skipped["attempts"], skipped["exit_code"], skipped["stdout"], skipped["stderr"]), ("skipped", 0, None, "", ""))
        self.assertIn("fail-fast", skipped.get("reason", "").lower())
        self.assertEqual((failed["status"], failed["attempts"], failed["exit_code"]), ("failed", 3, 7))
        self.flow(tasks, "--jobs", "1", code=1)
        self.assertTrue(marker.exists(), "ordinary mode must retain independent execution")

    def test_successful_retry_does_not_trigger_fail_fast(self):
        marker = self.directory / "once"
        source = f"from pathlib import Path; import sys; p=Path({str(marker)!r}); prior=p.exists(); p.touch(); sys.exit(0 if prior else 7)"
        result = self.flow([self.task("a", command=self.python(source), retries=1), self.task("b")], "--jobs", "1", "--fail-fast")
        self.assertEqual([task["status"] for task in result["tasks"]], ["succeeded", "succeeded"])
        self.assertEqual([task["attempts"] for task in result["tasks"]], [2, 1])

    def test_already_running_task_may_finish(self):
        started, finished, later = [self.directory / name for name in ("started", "finished", "later")]
        worker = self.directory / "worker.py"
        worker.write_text("""from pathlib import Path
import sys, time
root = Path(sys.argv[1])
if sys.argv[2] == 'running':
    (root / 'started').touch()
    (root / 'finished').touch()
else:
    deadline = time.monotonic() + 5
    while not (root / 'started').exists():
        if time.monotonic() >= deadline:
            raise SystemExit(8)
        time.sleep(.01)
    raise SystemExit(7)
""", encoding="utf-8")
        tasks = [self.task("a-running", command=[sys.executable, str(worker), str(self.directory), "running"]), self.task("b-fails", command=[sys.executable, str(worker), str(self.directory), "fails"]), self.task("z-later", depends_on=["b-fails"], command=self.python(f"from pathlib import Path; Path({str(later)!r}).touch()"))]
        result = self.flow(tasks, "--jobs", "2", "--fail-fast", code=1)
        self.assertTrue(started.exists())
        self.assertTrue(finished.exists())
        self.assertFalse(later.exists())
        self.assertEqual([task["status"] for task in result["tasks"]], ["succeeded", "failed", "skipped"])


TASKS = {
    "report-train": [ReportTrain], "report-dev": [ReportTrain, ReportDev],
    "queue-train": [QueueTrain], "queue-dev": [QueueTrain, QueueDev],
    "flow-train": [FlowTrain], "flow-dev": [FlowTrain, FlowDev],
}


def main():
    global WORKSPACE, TASK
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workspace", required=True, type=Path)
    parser.add_argument("--task", required=True, choices=sorted(TASKS))
    args = parser.parse_args()
    WORKSPACE, TASK = args.workspace.resolve(), args.task
    if not WORKSPACE.is_dir() or Path(__file__).resolve().is_relative_to(WORKSPACE):
        parser.error("grader must be outside an existing candidate workspace")
    suite = unittest.TestSuite(unittest.defaultTestLoader.loadTestsFromTestCase(case) for case in [PublicBaseline, *TASKS[TASK]])
    result = unittest.TextTestRunner(verbosity=2, stream=sys.stderr).run(suite)
    print(json.dumps({"task": TASK, "passed": result.wasSuccessful(), "checks": result.testsRun, "failures": len(result.failures), "errors": len(result.errors)}, sort_keys=True))
    return 0 if result.wasSuccessful() else 1


if __name__ == "__main__":
    raise SystemExit(main())
