from __future__ import annotations

import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]


class FlowRunnerTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(dir=ROOT)
        self.directory = Path(self.temporary.name)
        self.plan = self.directory / "plan.json"
        self.report = self.directory / "report.json"

    def tearDown(self):
        self.temporary.cleanup()

    def write_plan(self, tasks):
        self.plan.write_text(json.dumps({"tasks": tasks}), encoding="utf-8")

    def python(self, source: str):
        return [sys.executable, "-c", source]

    def run_cli(self, code=0, jobs="3"):
        completed = subprocess.run(
            [sys.executable, "-m", "flow_runner", str(self.plan),
             "--jobs", jobs, "--output", str(self.report)],
            cwd=ROOT, text=True, capture_output=True, timeout=15,
        )
        self.assertEqual(completed.returncode, code, completed.stderr)
        if code in (0, 1):
            self.assertTrue(self.report.is_file())
            return json.loads(self.report.read_text(encoding="utf-8"))
        self.assertNotIn("Traceback", completed.stderr)
        return completed

    def test_dependencies_outputs_and_original_report_order(self):
        self.write_plan([
            {"id": "z-last", "command": self.python("print('z')"), "depends_on": ["a-first"]},
            {"id": "a-first", "command": self.python("import sys; print('a'); print('warn', file=sys.stderr)")},
        ])
        report = self.run_cli()
        self.assertEqual(report["status"], "succeeded")
        self.assertEqual([task["id"] for task in report["tasks"]], ["z-last", "a-first"])
        by_id = {task["id"]: task for task in report["tasks"]}
        self.assertEqual(by_id["a-first"]["stdout"], "a\n")
        self.assertEqual(by_id["a-first"]["stderr"], "warn\n")
        self.assertEqual(by_id["z-last"]["attempts"], 1)
        self.assertEqual(by_id["z-last"]["exit_code"], 0)

    def test_parallel_jobs_and_resource_exclusion(self):
        worker = self.directory / "worker.py"
        worker.write_text("""from pathlib import Path
import os
import sys
import time
root, name = Path(sys.argv[1]), sys.argv[2]
lock = root / "resource.lock"
if name in ("a", "b"):
    descriptor = os.open(lock, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
(root / (name + ".started")).touch()
if name in ("a", "c"):
    peer = root / (("c" if name == "a" else "a") + ".started")
    deadline = time.monotonic() + 5
    while not peer.exists():
        if time.monotonic() >= deadline:
            raise SystemExit("independent resource did not run concurrently")
        time.sleep(.01)
if name in ("a", "b"):
    os.close(descriptor)
    lock.unlink()
print(name)
""", encoding="utf-8")
        self.write_plan([
            {"id": "a", "command": [sys.executable, str(worker), str(self.directory), "a"], "resource": "database"},
            {"id": "b", "command": [sys.executable, str(worker), str(self.directory), "b"], "resource": "database"},
            {"id": "c", "command": [sys.executable, str(worker), str(self.directory), "c"], "resource": "network"},
        ])
        report = self.run_cli(jobs="3")
        self.assertEqual(report["status"], "succeeded")
        self.assertEqual([task["stdout"] for task in report["tasks"]], ["a\n", "b\n", "c\n"])

    def test_retry_failure_skip_and_independent_branch(self):
        counter = self.directory / "counter"
        retry_source = (
            "from pathlib import Path; import sys; "
            f"p=Path({str(counter)!r}); n=int(p.read_text())+1 if p.exists() else 1; "
            "p.write_text(str(n)); print(n); sys.exit(0 if n == 2 else 7)"
        )
        self.write_plan([
            {"id": "retry", "command": self.python(retry_source), "retries": 1},
            {"id": "fails", "command": self.python("import sys; sys.exit(9)")},
            {"id": "skipped", "command": self.python("raise SystemExit('must not run')"), "depends_on": ["fails"]},
            {"id": "independent", "command": self.python("print('ok')")},
        ])
        report = self.run_cli(code=1)
        by_id = {task["id"]: task for task in report["tasks"]}
        self.assertEqual(by_id["retry"]["status"], "succeeded")
        self.assertEqual(by_id["retry"]["attempts"], 2)
        self.assertEqual(by_id["fails"]["status"], "failed")
        self.assertEqual(by_id["fails"]["exit_code"], 9)
        self.assertEqual(by_id["skipped"]["status"], "skipped")
        self.assertEqual(by_id["skipped"]["attempts"], 0)
        self.assertIn("fails", by_id["skipped"].get("reason", ""))
        self.assertEqual(by_id["independent"]["stdout"], "ok\n")

    def test_timeout_is_retried_and_invalid_plan_preserves_report(self):
        self.report.write_text('{"preserve":true}\n', encoding="utf-8")
        self.write_plan([{
            "id": "slow", "command": self.python("import time; time.sleep(1)"),
            "timeout_seconds": .1, "retries": 1,
        }])
        report = self.run_cli(code=1)
        task = report["tasks"][0]
        self.assertEqual(task["status"], "failed")
        self.assertEqual(task["attempts"], 2)
        self.assertIsNotNone(task["exit_code"])
        self.assertIn("timeout", task["stderr"].lower())

        self.report.write_text('{"preserve":true}\n', encoding="utf-8")
        self.write_plan([
            {"id": "a", "command": self.python("print(1)"), "depends_on": ["b"]},
            {"id": "b", "command": self.python("print(2)"), "depends_on": ["a"]},
        ])
        self.run_cli(code=2)
        self.assertEqual(json.loads(self.report.read_text()), {"preserve": True})


if __name__ == "__main__":
    unittest.main()
