from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]
VALID = """[service]
name = billing-api
port = 8443
workers = 4

[limits]
request_timeout_seconds = 2.5
max_body_kib = 512

[flags]
maintenance = off
tracing = on
"""
EXPECTED = {
    "flags": {"maintenance": False, "tracing": True},
    "limits": {"max_body_kib": 512, "request_timeout_seconds": 2.5},
    "service": {"name": "billing-api", "port": 8443, "workers": 4},
}


class ConfcheckTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(dir=ROOT)
        self.directory = Path(self.temporary.name)
        self.config = self.directory / "service.ini"
        self.summary = self.directory / "summary.json"

    def tearDown(self):
        self.temporary.cleanup()

    def run_cli(self, *arguments, code=0):
        # The candidate package is resolved from the workspace explicitly so
        # the protected grader's safe-path environment does not hide it.
        environment = {**os.environ, "PYTHONPATH": str(ROOT)}
        completed = subprocess.run(
            [sys.executable, "-m", "confcheck", *arguments],
            cwd=ROOT, env=environment, text=True, capture_output=True, timeout=15,
        )
        self.assertEqual(completed.returncode, code, completed.stdout + completed.stderr)
        self.assertNotIn("Traceback", completed.stderr)
        return completed

    def check(self, code=0):
        return self.run_cli(str(self.config), "--summary", str(self.summary), code=code)

    def test_valid_config_writes_a_typed_summary(self):
        self.config.write_text(VALID, encoding="utf-8")
        completed = self.check()
        self.assertEqual(json.loads(completed.stdout), {"flags": 2})
        text = self.summary.read_text(encoding="utf-8")
        self.assertEqual(text, json.dumps(EXPECTED, indent=2, sort_keys=True) + "\n")
        loaded = json.loads(text)
        self.assertIsInstance(loaded["service"]["port"], int)
        self.assertIsInstance(loaded["limits"]["request_timeout_seconds"], float)
        self.assertIs(loaded["flags"]["tracing"], True)

    def test_out_of_range_values_are_reported_and_the_summary_untouched(self):
        cases = [
            ("port = 8443", "port = 80", "[service] port"),
            ("workers = 4", "workers = 0", "[service] workers"),
            ("request_timeout_seconds = 2.5", "request_timeout_seconds = 0", "[limits] request_timeout_seconds"),
            ("max_body_kib = 512", "max_body_kib = 70000", "[limits] max_body_kib"),
            ("name = billing-api", "name = Billing", "[service] name"),
        ]
        for original, replacement, key in cases:
            with self.subTest(key=key):
                self.config.write_text(VALID.replace(original, replacement), encoding="utf-8")
                self.summary.write_bytes(b"sentinel\n")
                completed = self.check(code=2)
                self.assertIn(key, completed.stderr)
                self.assertIn(str(self.config), completed.stderr)
                self.assertEqual(completed.stdout, "")
                self.assertEqual(self.summary.read_bytes(), b"sentinel\n")

    def test_missing_section_and_unknown_keys_are_rejected_before_writing(self):
        without_limits = VALID.replace("[limits]\nrequest_timeout_seconds = 2.5\nmax_body_kib = 512\n\n", "")
        cases = [
            (without_limits, "[limits]"),
            (VALID.replace("workers = 4\n", "workers = 4\ncolour = blue\n"), "[service] colour"),
            (VALID + "\n[extra]\nenabled = on\n", "[extra]"),
            (VALID.replace("port = 8443\n", ""), "[service] port"),
        ]
        for text, fragment in cases:
            with self.subTest(fragment=fragment):
                self.config.write_text(text, encoding="utf-8")
                completed = self.check(code=2)
                self.assertIn(fragment, completed.stderr)
                self.assertFalse(self.summary.exists())

    def test_flag_values_must_be_on_or_off(self):
        self.config.write_text(VALID.replace("maintenance = off", "maintenance = yes"), encoding="utf-8")
        completed = self.check(code=2)
        self.assertIn("[flags] maintenance", completed.stderr)
        self.assertFalse(self.summary.exists())
        self.config.write_text(VALID.replace("[flags]\nmaintenance = off\ntracing = on\n", "[flags]\n"), encoding="utf-8")
        completed = self.check()
        self.assertEqual(json.loads(completed.stdout), {"flags": 0})
        self.assertEqual(json.loads(self.summary.read_text(encoding="utf-8"))["flags"], {})

    def test_unreadable_or_invalid_ini_is_a_usage_error(self):
        missing = self.directory / "absent.ini"
        completed = self.run_cli(str(missing), "--summary", str(self.summary), code=2)
        self.assertIn(str(missing), completed.stderr)
        self.assertFalse(self.summary.exists())
        self.config.write_text("port = 8443\n", encoding="utf-8")
        completed = self.check(code=2)
        self.assertIn(str(self.config), completed.stderr)
        self.assertFalse(self.summary.exists())


if __name__ == "__main__":
    unittest.main()
