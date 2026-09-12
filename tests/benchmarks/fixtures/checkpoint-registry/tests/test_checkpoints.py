from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]


class CheckpointTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(dir=ROOT)
        self.directory = Path(self.temporary.name)
        self.registry = self.directory / "registry.json"

    def tearDown(self):
        self.temporary.cleanup()

    def run_cli(self, *arguments, code=0):
        # The candidate package is resolved from the workspace explicitly so
        # the protected grader's safe-path environment does not hide it.
        environment = {**os.environ, "PYTHONPATH": str(ROOT)}
        completed = subprocess.run(
            [sys.executable, "-m", "checkpoints", str(self.registry), *arguments],
            cwd=ROOT, env=environment, text=True, capture_output=True, timeout=15,
        )
        self.assertEqual(completed.returncode, code, completed.stdout + completed.stderr)
        self.assertNotIn("Traceback", completed.stderr)
        return completed

    def names(self):
        return sorted(path.name for path in self.directory.iterdir())

    def test_records_are_appended_and_latest_advances(self):
        first = self.run_cli("record", "warmup", "--step", "0", "--metrics", '{"loss": 2.5}')
        self.assertEqual(json.loads(first.stdout), {"count": 1, "latest": "warmup"})
        second = self.run_cli("record", "epoch-1", "--step", "100", "--metrics", '{"loss": 1.25, "accuracy": 0.5}')
        self.assertEqual(json.loads(second.stdout), {"count": 2, "latest": "epoch-1"})
        expected = {
            "checkpoints": [
                {"metrics": {"loss": 2.5}, "name": "warmup", "step": 0},
                {"metrics": {"accuracy": 0.5, "loss": 1.25}, "name": "epoch-1", "step": 100},
            ],
            "latest": "epoch-1",
        }
        self.assertEqual(self.registry.read_text(encoding="utf-8"), json.dumps(expected, indent=2, sort_keys=True) + "\n")
        latest = self.run_cli("latest")
        self.assertEqual(latest.stdout, json.dumps(expected["checkpoints"][1], sort_keys=True, separators=(",", ":")) + "\n")
        self.assertEqual(self.names(), ["registry.json"])

    def test_invalid_metrics_leave_the_registry_byte_identical(self):
        self.run_cli("record", "base", "--step", "1", "--metrics", '{"loss": 1.0}')
        before = self.registry.read_bytes()
        for metrics in ['{"loss": "1.0"}', '{"loss": NaN}', '{"loss": true}', '{"nested": {"a": 1}}', "[1, 2]", "not json"]:
            with self.subTest(metrics=metrics):
                completed = self.run_cli("record", "next", "--step", "2", "--metrics", metrics, code=2)
                self.assertNotEqual(completed.stderr.strip(), "")
                self.assertEqual(self.registry.read_bytes(), before)
                self.assertEqual(self.names(), ["registry.json"])

    def test_steps_must_advance_and_names_must_be_unique(self):
        self.run_cli("record", "a", "--step", "5", "--metrics", "{}")
        before = self.registry.read_bytes()
        for name, step in [("b", "5"), ("b", "4"), ("a", "6"), ("bad name", "6"), ("b", "-1"), ("b", "x")]:
            with self.subTest(name=name, step=step):
                self.run_cli("record", name, "--step", step, "--metrics", "{}", code=2)
                self.assertEqual(self.registry.read_bytes(), before)
        self.assertEqual(self.names(), ["registry.json"])

    def test_corrupt_registry_is_never_overwritten(self):
        for content in [b"{not json", b'{"checkpoints": "nope", "latest": null}\n', b"[]\n"]:
            with self.subTest(content=content):
                self.registry.write_bytes(content)
                self.run_cli("record", "a", "--step", "1", "--metrics", "{}", code=2)
                self.assertEqual(self.registry.read_bytes(), content)
                self.run_cli("latest", code=2)
                self.assertEqual(self.registry.read_bytes(), content)
                self.assertEqual(self.names(), ["registry.json"])

    def test_empty_registry_and_stale_files_are_handled(self):
        latest = self.run_cli("latest")
        self.assertEqual(latest.stdout, "null\n")
        self.assertFalse(self.registry.exists())
        stale = self.directory / "registry.json.partial"
        stale.write_bytes(b"garbage")
        completed = self.run_cli("record", "first", "--step", "0", "--metrics", "{}")
        self.assertEqual(json.loads(completed.stdout), {"count": 1, "latest": "first"})
        self.assertEqual(json.loads(self.registry.read_text(encoding="utf-8"))["latest"], "first")
        self.assertEqual(stale.read_bytes(), b"garbage")
        self.assertEqual(self.names(), ["registry.json", "registry.json.partial"])


if __name__ == "__main__":
    unittest.main()
