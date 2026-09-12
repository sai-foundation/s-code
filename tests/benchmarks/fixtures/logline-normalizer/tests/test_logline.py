from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]
VALID = (
    "2026-09-12T10:03:07Z WARN scheduler: queue depth 12\n"
    "\n"
    "2026-09-12T10:03:08Z info api: request served \n"
    "   \n"
    "2026-09-12T10:03:09Z Error worker-2: retry budget exhausted\n"
)


class LoglineTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(dir=ROOT)
        self.directory = Path(self.temporary.name)
        self.input = self.directory / "service.log"
        self.output = self.directory / "events.jsonl"

    def tearDown(self):
        self.temporary.cleanup()

    def run_cli(self, *arguments, code=0):
        # The candidate package is resolved from the workspace explicitly so
        # the protected grader's safe-path environment does not hide it.
        environment = {**os.environ, "PYTHONPATH": str(ROOT)}
        completed = subprocess.run(
            [sys.executable, "-m", "logline", *arguments],
            cwd=ROOT, env=environment, text=True, capture_output=True, timeout=15,
        )
        self.assertEqual(completed.returncode, code, completed.stdout + completed.stderr)
        self.assertNotIn("Traceback", completed.stderr)
        return completed

    def test_normalizes_levels_and_writes_one_event_per_line(self):
        self.input.write_text(VALID, encoding="utf-8")
        completed = self.run_cli(str(self.input), "--output", str(self.output))
        self.assertEqual(json.loads(completed.stdout), {"events": 3})
        lines = self.output.read_text(encoding="utf-8").splitlines(keepends=True)
        self.assertEqual(len(lines), 3)
        self.assertTrue(all(line.endswith("\n") for line in lines))
        events = [json.loads(line) for line in lines]
        self.assertEqual(
            events[0],
            {"component": "scheduler", "level": "warn", "message": "queue depth 12", "timestamp": "2026-09-12T10:03:07Z"},
        )
        self.assertEqual(events[1]["level"], "info")
        self.assertEqual(events[1]["message"], "request served")
        self.assertEqual(
            events[2],
            {"component": "worker-2", "level": "error", "message": "retry budget exhausted", "timestamp": "2026-09-12T10:03:09Z"},
        )
        self.assertEqual(lines[0], json.dumps(events[0], sort_keys=True) + "\n")

    def test_blank_lines_are_ignored_and_the_count_is_reported(self):
        self.input.write_text("\n\n2026-01-01T00:00:00Z debug boot: ready\n\n", encoding="utf-8")
        completed = self.run_cli(str(self.input), "--output", str(self.output))
        self.assertEqual(json.loads(completed.stdout), {"events": 1})
        self.assertEqual(self.output.read_text(encoding="utf-8").count("\n"), 1)

    def test_malformed_line_reports_its_position_and_leaves_output_untouched(self):
        self.input.write_text(
            "2026-09-12T10:03:07Z WARN scheduler: fine\n\n2026-09-12 10:03:08Z info api: bad stamp\n",
            encoding="utf-8",
        )
        self.output.write_bytes(b"sentinel\n")
        completed = self.run_cli(str(self.input), "--output", str(self.output), code=2)
        self.assertIn("line 3:", completed.stderr)
        self.assertEqual(completed.stdout, "")
        self.assertEqual(self.output.read_bytes(), b"sentinel\n")

    def test_unknown_level_and_empty_message_are_rejected_before_writing(self):
        cases = [
            ("2026-09-12T10:03:07Z NOTICE api: hi\n", 1),
            ("2026-09-12T10:03:07Z info api: ok\n2026-09-12T10:03:08Z warn api:   \n", 2),
            ("2026-09-12T10:03:07Z info Api: ok\n", 1),
        ]
        for text, line in cases:
            with self.subTest(line=line, text=text):
                self.input.write_text(text, encoding="utf-8")
                completed = self.run_cli(str(self.input), "--output", str(self.output), code=2)
                self.assertIn(f"line {line}:", completed.stderr)
                self.assertFalse(self.output.exists())

    def test_unreadable_input_is_a_usage_error(self):
        missing = self.directory / "absent.log"
        completed = self.run_cli(str(missing), "--output", str(self.output), code=2)
        self.assertIn("cannot read input", completed.stderr)
        self.assertIn(str(missing), completed.stderr)
        self.assertFalse(self.output.exists())


if __name__ == "__main__":
    unittest.main()
