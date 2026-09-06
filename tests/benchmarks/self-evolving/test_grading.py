"""Offline checks for grader output bounds, cleanup, and filesystem isolation."""
from __future__ import annotations

import importlib.util
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

from bounded_process import DEADLINE, OUTPUT_LIMIT, run
import sandbox


def alive(pid):
    result = subprocess.run(["/bin/ps", "-o", "stat=", "-p", str(pid)],
                            capture_output=True, text=True, timeout=2)
    return result.returncode == 0 and bool(result.stdout.strip()) and not result.stdout.lstrip().startswith("Z")


class BoundedProcesses(unittest.TestCase):
    def assert_stopped(self, pid):
        deadline = time.monotonic() + 2
        while alive(pid) and time.monotonic() < deadline:
            time.sleep(0.02)
        self.assertFalse(alive(pid), f"child {pid} survived grading cleanup")

    def test_success_preserves_streams_and_unicode(self):
        result = run([sys.executable, "-c", "import sys; print('雪'); print('detail', file=sys.stderr)"],
                     max_output_bytes=100)
        self.assertEqual((result.returncode, result.stdout, result.stderr), (0, "雪\n", "detail\n"))

    def test_stdout_and_stderr_share_a_byte_quota(self):
        result = run([sys.executable, "-c", "import os; os.write(1,b'a'*3000); os.write(2,b'b'*3000)"],
                     max_output_bytes=4096)
        self.assertEqual(result.returncode, OUTPUT_LIMIT)
        self.assertIn("candidate-output-limit", result.stderr)
        self.assertLessEqual(len(result.stdout.encode()) + len(result.stderr.encode()), 4200)

    def test_output_limit_terminates_child_group(self):
        program = """import os, subprocess, sys
child = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(30)'])
print(child.pid, flush=True)
while True: os.write(1, b'x' * 65536)
"""
        result = run([sys.executable, "-c", program], timeout=3, max_output_bytes=4096)
        self.assertEqual(result.returncode, OUTPUT_LIMIT)
        self.assert_stopped(int(result.stdout.splitlines()[0]))

    def test_deadline_terminates_nested_process_group(self):
        program = """import subprocess, sys, time
child = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(30)'], start_new_session=True)
print(child.pid, flush=True)
time.sleep(30)
"""
        result = run([sys.executable, "-c", program], timeout=0.3)
        self.assertEqual(result.returncode, DEADLINE)
        self.assert_stopped(int(result.stdout.strip()))

    def test_success_cleans_up_background_children(self):
        program = """import subprocess, sys
child = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(30)'], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
print(child.pid)
"""
        result = run([sys.executable, "-c", program])
        self.assertEqual(result.returncode, 0)
        self.assert_stopped(int(result.stdout.strip()))

    def test_limits_are_validated_before_launch(self):
        for limit in (0, -1, float("inf"), True):
            with self.subTest(limit=limit), self.assertRaises(ValueError):
                run(["must-not-launch"], max_output_bytes=limit)
        for timeout in (0, -1, float("inf"), float("nan")):
            with self.subTest(timeout=timeout), self.assertRaises(ValueError):
                run(["must-not-launch"], timeout=timeout)


class TrustedBaseline(unittest.TestCase):
    @staticmethod
    def command(workspace):
        path = Path(__file__).resolve().parent/"fixtures/grade.py"
        spec = importlib.util.spec_from_file_location("public_baseline_grader", path)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        return module.baseline_command(workspace)

    def test_candidate_unittest_shadow_cannot_skip_a_failing_existing_test(self):
        for package in (False, True):
            with self.subTest(package=package), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary).resolve()
                workspace = root/"candidate"
                (workspace/"tests").mkdir(parents=True)
                (workspace/"tests/test_sentinel.py").write_text(
                    "import unittest\nclass Sentinel(unittest.TestCase):\n"
                    "    def test_existing_failure(self): self.fail('existing test still ran')\n")
                if package:
                    shadow = workspace/"unittest"
                    shadow.mkdir()
                    (shadow/"__init__.py").write_text("")
                    (shadow/"__main__.py").write_text("print('pretend tests succeeded')\n")
                else:
                    (workspace/"unittest.py").write_text("print('pretend tests succeeded')\n")
                old = run([sys.executable, "-m", "unittest", "discover", "-s", "tests"], cwd=workspace)
                self.assertEqual(old.returncode, 0)
                trusted = run(self.command(workspace), cwd=root)
                self.assertEqual(trusted.returncode, 1, trusted.stderr)
                self.assertIn("existing test still ran", trusted.stderr)
                self.assertIn("Ran 1 test", trusted.stderr)
                self.assertNotIn("pretend tests succeeded", trusted.stdout)

    def test_standard_library_priority_keeps_project_helpers_and_temporary_files_working(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            workspace = root/"candidate"
            tests = workspace/"tests"
            tests.mkdir(parents=True)
            for directory in (workspace, tests):
                for name in ("unittest", "json", "argparse"):
                    (directory/f"{name}.py").write_text("raise RuntimeError('candidate shadow imported')\n")
            (workspace/"utility.py").write_text("VALUE = 42\n")
            (tests/"test_helper.py").write_text("VALUE = 7\n")
            (tests/"test_imports.py").write_text(
                "import json, tempfile, unittest\nfrom pathlib import Path\nimport utility, test_helper\n"
                "class Imports(unittest.TestCase):\n"
                "    def test_normal_project(self):\n"
                "        self.assertEqual(json.loads('42'), utility.VALUE)\n"
                "        self.assertEqual(test_helper.VALUE, 7)\n"
                "        with tempfile.TemporaryDirectory(dir=Path.cwd()) as directory:\n"
                "            (Path(directory)/'result.json').write_text('{}')\n")
            trusted = run(self.command(workspace), cwd=root)
            self.assertEqual(trusted.returncode, 0, trusted.stderr)
            self.assertIn("Ran 1 test", trusted.stderr)

    def test_empty_suite_is_a_failure(self):
        with tempfile.TemporaryDirectory() as temporary:
            workspace = Path(temporary).resolve()/"candidate"
            (workspace/"tests").mkdir(parents=True)
            trusted = run(self.command(workspace), cwd=workspace.parent)
            self.assertEqual(trusted.returncode, 1)
            self.assertIn("No baseline tests discovered", trusted.stderr)


class GradingSandbox(unittest.TestCase):
    def test_linux_preserves_existing_entries_as_read_only_mounts(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            workspace, scratch = root / "candidate", root / "scratch"
            workspace.mkdir()
            (workspace / "tests").mkdir()
            (workspace / "source.py").write_text("pass\n")
            with patch.object(sandbox.sys, "platform", "linux"), patch.object(sandbox.shutil, "which", return_value="/usr/bin/bwrap"):
                argv = sandbox.command([sys.executable, "-c", "pass"], [workspace], scratch, workspace)
            self.assertIn(["--tmpfs", str(workspace)], [argv[i:i+2] for i in range(len(argv))])
            mounts = [argv[i:i+3] for i in range(len(argv))]
            self.assertNotIn(["--bind", str(workspace), str(workspace)], mounts)
            for entry in workspace.iterdir():
                self.assertIn(["--ro-bind", str(entry), str(entry)], mounts)

    @unittest.skipUnless(sys.platform == "darwin" or (sys.platform.startswith("linux") and shutil.which("bwrap")), "native grading sandbox unavailable")
    def test_native_sandbox_blocks_source_writes_and_external_reads(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            workspace, scratch = root / "candidate", root / "scratch"
            workspace.mkdir()
            source = workspace / "source.py"
            source.write_text("original\n")
            outside = root / "not-readable.txt"
            outside.write_text("synthetic private fixture\n")
            program = """from pathlib import Path
import tempfile
source, outside, workspace = map(Path, __import__('sys').argv[1:])
assert source.read_text() == 'original\\n'
try: source.write_text('changed\\n')
except PermissionError: pass
except OSError as error:
    assert error.errno == 30, error
else: raise AssertionError('source was writable')
try: outside.read_text()
except (PermissionError, FileNotFoundError): pass
else: raise AssertionError('external content was readable')
with tempfile.TemporaryDirectory(dir=workspace) as directory:
    (Path(directory) / 'result').write_text('temporary')
print('isolated')
"""
            result = sandbox.run([sys.executable, "-c", program, str(source), str(outside), str(workspace)],
                                 [workspace], scratch, workspace=workspace, timeout=10)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(result.stdout, "isolated\n")
            self.assertEqual(source.read_text(), "original\n")

    @unittest.skipUnless(sys.platform == "darwin" or (sys.platform.startswith("linux") and shutil.which("bwrap")), "native grading sandbox unavailable")
    def test_grader_loads_only_its_explicit_helper_inside_sandbox(self):
        here = Path(__file__).resolve().parent
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            workspace = root / "candidate"
            shutil.copytree(here / "fixtures/incident-report-cli", workspace,
                            ignore=shutil.ignore_patterns("__pycache__", "*.pyc"))
            grader = here / "fixtures/grade.py"
            result = sandbox.run([sys.executable, str(grader), "--workspace", str(workspace),
                                  "--task", "report-train"], [workspace, grader],
                                 root / "scratch", workspace=workspace, timeout=15)
            self.assertEqual(result.returncode, 1, result.stderr)
            verdict = json.loads(result.stdout)
            self.assertEqual(verdict["errors"], 0, result.stderr)
            self.assertEqual(verdict["checks"], 4)
            self.assertEqual(verdict["failures"], 2)

    @unittest.skipUnless(sys.platform == "darwin" or (sys.platform.startswith("linux") and shutil.which("bwrap")), "native grading sandbox unavailable")
    def test_nested_output_quota_works_inside_sandbox(self):
        helper = Path(__file__).with_name("bounded_process.py").resolve()
        program = """import importlib.util, sys
spec = importlib.util.spec_from_file_location('bounded', sys.argv[1])
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
result = module.run([sys.executable, '-c', 'import os; os.write(1,b"x"*1000000)'], max_output_bytes=4096)
assert result.returncode == 125, result
assert 'candidate-output-limit' in result.stderr
print('bounded')
"""
        with tempfile.TemporaryDirectory() as temporary:
            result = sandbox.run([sys.executable, "-c", program, str(helper)], [],
                                 Path(temporary).resolve(), timeout=10)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(result.stdout, "bounded\n")


if __name__ == "__main__":
    unittest.main()
