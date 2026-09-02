import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


class IncidentReportTest(unittest.TestCase):
    def run_cli(self, lines, existing_output=None):
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            source = directory / "events.jsonl"
            output = directory / "report.json"
            source.write_text("\n".join(lines) + "\n", encoding="utf-8")
            if existing_output is not None:
                output.write_text(existing_output, encoding="utf-8")
            result = subprocess.run(
                [
                    sys.executable,
                    "-m",
                    "incident_report",
                    str(source),
                    "--output",
                    str(output),
                ],
                cwd=ROOT,
                capture_output=True,
                text=True,
                check=False,
            )
            content = output.read_text(encoding="utf-8") if output.exists() else None
            return result, content

    def test_builds_deterministic_report(self):
        result, content = self.run_cli(
            [
                '{"service":"web","severity":"error","message":"timeout"}',
                '{"service":"api","severity":"warning","message":"slow"}',
                '{"service":"web","severity":"error","message":"timeout"}',
                '{"service":"api","severity":"info","message":"ready"}',
                '{"service":"api","severity":"error","message":"slow"}',
            ]
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(content.endswith("\n"))
        self.assertEqual(
            json.loads(content),
            {
                "total": 5,
                "by_severity": {"info": 1, "warning": 1, "error": 3},
                "by_service": {"api": 3, "web": 2},
                "top_messages": [
                    {"message": "slow", "count": 2},
                    {"message": "timeout", "count": 2},
                    {"message": "ready", "count": 1},
                ],
            },
        )

    def test_ignores_empty_lines(self):
        result, content = self.run_cli(
            ["", '{"service":"worker","severity":"info","message":"ok"}', ""]
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(content)["total"], 1)

    def test_invalid_event_preserves_existing_output(self):
        result, content = self.run_cli(
            [
                '{"service":"web","severity":"info","message":"ready"}',
                '{"service":"web","severity":"fatal","message":"down"}',
            ],
            existing_output="previous\n",
        )
        self.assertEqual(result.returncode, 2)
        self.assertIn("line 2:", result.stderr)
        self.assertEqual(content, "previous\n")


if __name__ == "__main__":
    unittest.main()
