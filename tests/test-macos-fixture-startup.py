#!/usr/bin/env python3
"""Regression checks for the desktop fixture's bounded, DNS-free startup."""
import importlib.util
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(filename))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


desktop = load("desktop_checks", "test-macos-desktop.py")
fixture = load("model_fixture", "macos-model-fixture.py")


class FixtureStartupTests(unittest.TestCase):
    def process(self, source, *arguments):
        process = subprocess.Popen([sys.executable, "-c", source, *arguments],
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                   start_new_session=True)
        self.addCleanup(process.stderr.close)
        self.addCleanup(process.stdout.close)
        self.addCleanup(desktop.stop, process)
        return process

    def test_delayed_split_announcement(self):
        process = self.process(
            "import sys,time; time.sleep(.05); "
            "sys.stdout.write('123'); sys.stdout.flush(); time.sleep(.05); "
            "print('45', flush=True); time.sleep(120)")
        self.assertEqual(desktop.fixture_endpoint(process, timeout=3), "http://127.0.0.1:12345/v1")

    def test_partial_announcement_obeys_deadline(self):
        process = self.process("import sys,time; sys.stdout.write('123'); sys.stdout.flush(); time.sleep(120)")
        start = time.monotonic()
        with self.assertRaisesRegex(AssertionError, "startup deadline"):
            desktop.fixture_endpoint(process, timeout=.25)
        self.assertLess(time.monotonic() - start, 2)

    def test_silent_startup_obeys_deadline(self):
        process = self.process("import time; time.sleep(120)")
        with self.assertRaisesRegex(AssertionError, "startup deadline"):
            desktop.fixture_endpoint(process, timeout=.25)

    def test_early_exit_is_reported_without_waiting_for_deadline(self):
        process = self.process("raise SystemExit(7)")
        start = time.monotonic()
        with self.assertRaisesRegex(AssertionError, "closed stdout before readiness"):
            desktop.fixture_endpoint(process, timeout=10)
        self.assertLess(time.monotonic() - start, 3)
        self.assertEqual(process.wait(timeout=1), 7)

    def test_invalid_announcements(self):
        for port in ("zero", "0", "65536", "-1"):
            with self.subTest(port=port):
                process = self.process("import sys; print(sys.argv[1], flush=True)", port)
                with self.assertRaisesRegex(AssertionError, "Invalid model fixture port"):
                    desktop.fixture_endpoint(process, timeout=3)

    def test_overlong_announcement_is_rejected(self):
        process = self.process("import sys,time; sys.stdout.write('1'*128); sys.stdout.flush(); time.sleep(120)")
        with self.assertRaisesRegex(AssertionError, "announcement was too long"):
            desktop.fixture_endpoint(process, timeout=3)

    def test_cleanup_reaps_an_exited_fixture(self):
        process = self.process("raise SystemExit(7)")
        self.assertEqual(process.stdout.read(), b"")
        # Leave the exited child unreaped, as when startup fails before wait().
        time.sleep(.05)
        desktop.stop(process)
        self.assertEqual(process.returncode, 7)

    def test_timeout_cleanup_stops_child_processes(self):
        with tempfile.TemporaryDirectory() as directory:
            pid_file = Path(directory) / "child.pid"
            process = self.process(
                "import pathlib,subprocess,sys,time; "
                "child=subprocess.Popen([sys.executable,'-c','import time; time.sleep(120)']); "
                "pathlib.Path(sys.argv[1]).write_text(str(child.pid)); time.sleep(120)", str(pid_file))
            desktop.wait_until(pid_file.exists, "fixture child startup", timeout=3)
            child_pid = int(pid_file.read_text())
            try:
                with self.assertRaisesRegex(AssertionError, "startup deadline"):
                    desktop.fixture_endpoint(process, timeout=.25)
            finally:
                desktop.stop(process)
            desktop.wait_until(lambda: not desktop.running(child_pid), "fixture child cleanup", timeout=3)

    def test_loopback_server_startup_never_resolves_dns(self):
        with patch("socket.getfqdn", side_effect=AssertionError("Unexpected DNS lookup")):
            with fixture.LoopbackHTTPServer(("127.0.0.1", 0), fixture.Handler) as server:
                self.assertEqual(server.server_name, "127.0.0.1")
                self.assertGreater(server.server_port, 0)

    def test_real_fixture_announces_healthy_endpoint(self):
        process = subprocess.Popen([sys.executable, str(Path(fixture.__file__))],
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                   start_new_session=True)
        self.addCleanup(process.stderr.close)
        self.addCleanup(process.stdout.close)
        self.addCleanup(desktop.stop, process)
        endpoint = desktop.fixture_endpoint(process, timeout=3)
        self.assertEqual(desktop.request(endpoint, "/models")["data"][0]["id"], "desktop-fixture")


if __name__ == "__main__":
    unittest.main()
