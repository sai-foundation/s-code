from __future__ import annotations

import json
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]


class DurableQueueTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(dir=ROOT)
        self.database = Path(self.temporary.name) / "queue.sqlite"
        self.run_cli("init", str(self.database))

    def tearDown(self):
        self.temporary.cleanup()

    def command(self, *arguments: str):
        return [sys.executable, "-m", "durable_queue", *arguments]

    def run_cli(self, *arguments: str, code: int = 0):
        completed = subprocess.run(
            self.command(*arguments), cwd=ROOT, text=True, capture_output=True
        )
        self.assertEqual(completed.returncode, code, completed.stderr)
        if code == 0:
            self.assertEqual(completed.stderr, "")
            return json.loads(completed.stdout)
        self.assertEqual(completed.stdout, "")
        self.assertNotIn("Traceback", completed.stderr)
        return completed.stderr

    def enqueue(self, task_id: str, payload: object, *extra: str):
        return self.run_cli(
            "enqueue", str(self.database), task_id,
            json.dumps(payload, separators=(",", ":")), *extra
        )

    def claim(self, worker: str, now: str = "100"):
        return self.run_cli(
            "claim", str(self.database), worker,
            "--lease-seconds", "10", "--now", now
        )

    def test_schema_wal_and_idempotent_enqueue(self):
        mode = sqlite3.connect(self.database).execute("PRAGMA journal_mode").fetchone()[0]
        self.assertEqual(mode, "wal")
        first = self.enqueue("alpha", {"value": 1}, "--priority", "3", "--now", "10")
        second = self.enqueue("alpha", {"value": 1}, "--priority", "3", "--now", "20")
        self.assertEqual(first, {"created": True, "task_id": "alpha"})
        self.assertEqual(second, {"created": False, "task_id": "alpha"})
        error = self.run_cli(
            "enqueue", str(self.database), "alpha", '{"value":2}',
            "--priority", "3", "--now", "30", code=2
        )
        self.assertIn("alpha", error)
        self.assertEqual(self.claim("worker")["payload"], {"value": 1})

    def test_priority_fifo_ack_and_stats(self):
        self.enqueue("low", [1], "--priority", "1", "--now", "1")
        self.enqueue("high-old", [2], "--priority", "9", "--now", "2")
        self.enqueue("high-new", [3], "--priority", "9", "--now", "3")
        first = self.claim("worker-a")
        second = self.claim("worker-b")
        self.assertEqual([first["task_id"], second["task_id"]], ["high-old", "high-new"])
        self.assertEqual(first["attempt"], 1)
        self.assertEqual(first["lease_owner"], "worker-a")
        self.assertEqual(first["lease_expires_at"], 110)
        self.assertTrue(first["lease_token"])
        self.run_cli("ack", str(self.database), first["task_id"], "worker-a", first["lease_token"])
        counts = self.run_cli("stats", str(self.database), "--now", "100")
        self.assertEqual(counts, {"completed": 1, "dead": 0, "leased": 1, "queued": 1, "total": 3})

    def test_expired_lease_rejects_stale_token(self):
        self.enqueue("renew", "payload", "--now", "1")
        old = self.claim("old", "5")
        renewed = self.claim("new", "16")
        self.assertEqual(renewed["task_id"], "renew")
        self.assertEqual(renewed["attempt"], 2)
        self.assertNotEqual(old["lease_token"], renewed["lease_token"])
        error = self.run_cli(
            "ack", str(self.database), "renew", "old", old["lease_token"], code=2
        )
        self.assertIn("lease", error.lower())
        self.run_cli("ack", str(self.database), "renew", "new", renewed["lease_token"])

    def test_retry_then_dead_and_validation(self):
        self.enqueue("fragile", {"x": True}, "--max-attempts", "2", "--now", "1")
        first = self.claim("worker", "2")
        failed = self.run_cli(
            "fail", str(self.database), "fragile", "worker", first["lease_token"],
            "--error", "temporary", "--now", "3"
        )
        self.assertEqual(failed, {"status": "queued", "task_id": "fragile"})
        second = self.claim("worker", "4")
        dead = self.run_cli(
            "fail", str(self.database), "fragile", "worker", second["lease_token"],
            "--error", "permanent", "--now", "5"
        )
        self.assertEqual(dead, {"status": "dead", "task_id": "fragile"})
        self.assertIsNone(self.claim("other", "20"))
        self.assertEqual(self.run_cli("stats", str(self.database), "--now", "20")["dead"], 1)
        self.run_cli(
            "enqueue", str(self.database), "bad", "{}", "--max-attempts", "0", code=2
        )
        self.run_cli("claim", str(self.database), "w", "--lease-seconds", "nan", code=2)

    def test_concurrent_claims_are_unique(self):
        for number in range(8):
            self.enqueue(f"task-{number}", number, "--now", str(number))
        processes = [
            subprocess.Popen(
                self.command(
                    "claim", str(self.database), f"worker-{number}",
                    "--lease-seconds", "60", "--now", "100"
                ),
                cwd=ROOT, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            )
            for number in range(8)
        ]
        claimed = []
        for process in processes:
            stdout, stderr = process.communicate(timeout=10)
            self.assertEqual(process.returncode, 0, stderr)
            claimed.append(json.loads(stdout)["task_id"])
        self.assertEqual(len(set(claimed)), 8)


if __name__ == "__main__":
    unittest.main()
