from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]


def snapshot_text(accounts, through_seq):
    return json.dumps({"accounts": accounts, "through_seq": through_seq}, indent=2, sort_keys=True) + "\n"


class LedgerTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(dir=ROOT)
        self.directory = Path(self.temporary.name)
        self.journal = self.directory / "journal.csv"
        self.snapshot = self.directory / "balances.json"

    def tearDown(self):
        self.temporary.cleanup()

    def compact(self, code=0):
        # The candidate package is resolved from the workspace explicitly so
        # the protected grader's safe-path environment does not hide it.
        environment = {**os.environ, "PYTHONPATH": str(ROOT)}
        completed = subprocess.run(
            [sys.executable, "-m", "ledger", "compact", str(self.journal), "--snapshot", str(self.snapshot)],
            cwd=ROOT, env=environment, text=True, capture_output=True, timeout=15,
        )
        self.assertEqual(completed.returncode, code, completed.stdout + completed.stderr)
        self.assertNotIn("Traceback", completed.stderr)
        return completed

    def names(self):
        return sorted(path.name for path in self.directory.iterdir())

    def test_compaction_folds_entries_and_empties_the_journal(self):
        self.journal.write_text("1,alice,40\n2,bob,15\n3,alice,-25\n4,carol,10\n5,carol,-10\n", encoding="utf-8")
        completed = self.compact()
        self.assertEqual(json.loads(completed.stdout), {"folded": 5, "through_seq": 5})
        self.assertEqual(self.snapshot.read_text(encoding="utf-8"), snapshot_text({"alice": 15, "bob": 15}, 5))
        self.assertEqual(self.journal.read_bytes(), b"")
        self.assertEqual(self.names(), ["balances.json", "journal.csv"])

    def test_later_compaction_continues_from_the_snapshot(self):
        self.journal.write_text("1,alice,40\n2,bob,15\n3,alice,-25\n", encoding="utf-8")
        self.compact()
        self.journal.write_text("6,bob,-15\n7,dave,5\n", encoding="utf-8")
        completed = self.compact()
        self.assertEqual(json.loads(completed.stdout), {"folded": 2, "through_seq": 7})
        self.assertEqual(self.snapshot.read_text(encoding="utf-8"), snapshot_text({"alice": 15, "dave": 5}, 7))
        before = self.snapshot.read_bytes()
        self.journal.write_text("7,alice,1\n", encoding="utf-8")
        completed = self.compact(code=2)
        self.assertIn("line 1:", completed.stderr)
        self.assertEqual(self.snapshot.read_bytes(), before)
        self.assertEqual(self.journal.read_bytes(), b"7,alice,1\n")
        self.journal.write_bytes(b"")
        completed = self.compact()
        self.assertEqual(json.loads(completed.stdout), {"folded": 0, "through_seq": 7})
        self.assertEqual(self.snapshot.read_bytes(), before)
        self.assertEqual(self.journal.read_bytes(), b"")

    def test_invalid_entries_leave_both_files_byte_identical(self):
        self.snapshot.write_text(snapshot_text({"alice": 3}, 2), encoding="utf-8")
        before_snapshot = self.snapshot.read_bytes()
        cases = [
            ("3,alice,1.5\n", 1),
            ("3,alice,1\n3,bob,2\n", 2),
            ("3,Alice,1\n", 1),
            ("3,alice,1\n\n4,bob,2\n", 2),
            ("3,alice\n", 1),
            ("2,alice,1\n", 1),
        ]
        for text, line in cases:
            with self.subTest(text=text):
                self.journal.write_text(text, encoding="utf-8")
                before_journal = self.journal.read_bytes()
                completed = self.compact(code=2)
                self.assertIn(f"line {line}:", completed.stderr)
                self.assertEqual(self.journal.read_bytes(), before_journal)
                self.assertEqual(self.snapshot.read_bytes(), before_snapshot)
                self.assertEqual(self.names(), ["balances.json", "journal.csv"])

    def test_corrupt_snapshot_is_never_overwritten(self):
        self.journal.write_text("1,alice,1\n", encoding="utf-8")
        for content in [b"{oops", b'{"accounts": [], "through_seq": 0}\n', b'{"accounts": {"alice": "1"}, "through_seq": 0}\n']:
            with self.subTest(content=content):
                self.snapshot.write_bytes(content)
                completed = self.compact(code=2)
                self.assertIn(str(self.snapshot), completed.stderr)
                self.assertEqual(self.snapshot.read_bytes(), content)
                self.assertEqual(self.journal.read_bytes(), b"1,alice,1\n")
                self.assertEqual(self.names(), ["balances.json", "journal.csv"])

    def test_no_temporary_files_survive_and_stale_files_are_left_alone(self):
        stale = self.directory / "balances.json.tmp"
        stale.write_bytes(b"stale")
        self.journal.write_text("1,alice,2\n", encoding="utf-8")
        completed = self.compact()
        self.assertEqual(json.loads(completed.stdout), {"folded": 1, "through_seq": 1})
        self.assertEqual(self.names(), ["balances.json", "balances.json.tmp", "journal.csv"])
        self.assertEqual(stale.read_bytes(), b"stale")
        self.assertEqual(json.loads(self.snapshot.read_text(encoding="utf-8")), {"accounts": {"alice": 2}, "through_seq": 1})


if __name__ == "__main__":
    unittest.main()
