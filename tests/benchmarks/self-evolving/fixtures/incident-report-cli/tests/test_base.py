import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class ReportTests(unittest.TestCase):
    def run_report(self, text, prior=None):
        with tempfile.TemporaryDirectory() as directory:
            source, output = Path(directory) / "events.jsonl", Path(directory) / "report.json"
            source.write_text(text, encoding="utf-8")
            if prior is not None:
                output.write_text(prior, encoding="utf-8")
            result = subprocess.run([sys.executable, "-m", "incident_report", str(source), "--output", str(output)], cwd=ROOT, text=True, capture_output=True, timeout=10)
            return result, output.read_text(encoding="utf-8") if output.exists() else None

    def test_empty(self):
        result, text = self.run_report("\n \n")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, "")
        self.assertTrue(text.endswith("\n"))
        self.assertEqual(json.loads(text), {"total": 0, "by_severity": {"info": 0, "warning": 0, "error": 0}, "by_service": {}, "top_messages": []})

    def test_unicode_counts_and_order(self):
        events = [{"service": s, "severity": v, "message": m, "ignored": 42} for s, v, m in [("web", "error", "timeout"), ("api", "info", "就绪"), ("api", "warning", "slow"), ("web", "error", "timeout"), ("api", "warning", "slow")]]
        result, text = self.run_report("\n".join(map(lambda e: json.dumps(e, ensure_ascii=False), events)))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(text), {"total": 5, "by_severity": {"info": 1, "warning": 2, "error": 2}, "by_service": {"api": 3, "web": 2}, "top_messages": [{"message": "slow", "count": 2}, {"message": "timeout", "count": 2}, {"message": "就绪", "count": 1}]})

    def test_late_error_is_atomic_and_physical(self):
        good = '{"service":"api","severity":"info","message":"ok"}'
        for bad in ["{", "[]", '{"service":"api","severity":"fatal","message":"bad"}', '{"service":7,"severity":"info","message":"bad"}']:
            with self.subTest(bad=bad):
                result, text = self.run_report(good + "\n\n" + bad, "previous\n")
                self.assertEqual(result.returncode, 2)
                self.assertIn("line 3:", result.stderr)
                self.assertNotIn("Traceback", result.stderr)
                self.assertEqual(result.stdout, "")
                self.assertEqual(text, "previous\n")


if __name__ == "__main__":
    unittest.main()
