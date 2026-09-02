#!/usr/bin/env python3
"""Workspace-external unittest runner that emits one structured result."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import sys
from typing import Optional
import unittest


_TRUSTED_START_TEST = unittest.TestResult.startTest
_TRUSTED_ADD_ERROR = unittest.TestResult.addError
_TRUSTED_ADD_FAILURE = unittest.TestResult.addFailure
_TRUSTED_ADD_SUBTEST = unittest.TestResult.addSubTest
_TRUSTED_ADD_UNEXPECTED_SUCCESS = unittest.TestResult.addUnexpectedSuccess
_TRUSTED_SUITE_CALL = unittest.TestSuite.__call__
_TRUSTED_SUITE_RUN = unittest.TestSuite.run
_TRUSTED_CASE_CALL = unittest.TestCase.__call__
_TRUSTED_CASE_RUN = unittest.TestCase.run
_TRUSTED_STDOUT = sys.stdout


class IntegrityResult(unittest.TestResult):
    """Result whose pass/fail counters do not trust candidate-mutated unittest APIs."""

    def __init__(self) -> None:
        super().__init__()
        self.integrity_tests_run = 0
        self.integrity_errors = 0
        self.integrity_failures = 0

    def startTest(self, test: unittest.case.TestCase) -> None:  # noqa: N802
        self.integrity_tests_run += 1
        _TRUSTED_START_TEST(self, test)

    def addError(self, test: unittest.case.TestCase, err: tuple) -> None:  # noqa: N802
        self.integrity_errors += 1
        _TRUSTED_ADD_ERROR(self, test, err)

    def addFailure(self, test: unittest.case.TestCase, err: tuple) -> None:  # noqa: N802
        self.integrity_failures += 1
        _TRUSTED_ADD_FAILURE(self, test, err)

    def addSubTest(self, test: unittest.case.TestCase, subtest: unittest.case.TestCase, err: Optional[tuple]) -> None:  # noqa: N802
        if err is not None:
            if issubclass(err[0], AssertionError):
                self.integrity_failures += 1
            else:
                self.integrity_errors += 1
        _TRUSTED_ADD_SUBTEST(self, test, subtest, err)

    def addUnexpectedSuccess(self, test: unittest.case.TestCase) -> None:  # noqa: N802
        self.integrity_failures += 1
        _TRUSTED_ADD_UNEXPECTED_SUCCESS(self, test)


def restore_unittest_execution_methods() -> None:
    """Undo ordinary import-time monkeypatches before the protected suite runs."""

    unittest.TestSuite.__call__ = _TRUSTED_SUITE_CALL
    unittest.TestSuite.run = _TRUSTED_SUITE_RUN
    unittest.TestCase.__call__ = _TRUSTED_CASE_CALL
    unittest.TestCase.run = _TRUSTED_CASE_RUN


def blocked_exit(code: int = 0) -> None:
    raise RuntimeError(f"candidate attempted immediate process exit: {code}")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--workspace", required=True)
    parser.add_argument("--pattern", required=True)
    parser.add_argument("--discover", action="store_true")
    arguments = parser.parse_args()
    workspace = Path(arguments.workspace).resolve()
    os.chdir(workspace)
    sys.path.insert(0, str(workspace))
    os._exit = blocked_exit
    try:
        import posix

        posix._exit = blocked_exit
    except ImportError:
        pass
    loader = unittest.TestLoader()
    if arguments.discover:
        tests_root = workspace / "tests"
        suite = loader.discover(
            str(tests_root), pattern="test*.py", top_level_dir=str(tests_root)
        )
    else:
        suite = loader.discover(str(workspace), pattern=arguments.pattern, top_level_dir=str(workspace))
    restore_unittest_execution_methods()
    result = IntegrityResult()
    _TRUSTED_SUITE_RUN(suite, result)
    successful = result.integrity_errors == 0 and result.integrity_failures == 0
    payload = {
        "errors": result.integrity_errors,
        "failures": result.integrity_failures,
        "successful": successful,
        "tests_run": result.integrity_tests_run,
    }
    _TRUSTED_STDOUT.write(
        "OPENCODING_GRADER_RESULT="
        + json.dumps(payload, separators=(",", ":"), sort_keys=True)
        + "\n"
    )
    _TRUSTED_STDOUT.flush()
    return 0 if successful else 1


if __name__ == "__main__":
    raise SystemExit(main())
