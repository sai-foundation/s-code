"""Retain terminal learning evidence before a benchmark changes session mode."""
import contextlib
import io
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest

from run import Daemon


class LearningOutcomeTests(unittest.TestCase):
    def test_terminal_outcome_is_captured_without_changing_mode_or_accounting(self):
        for mode, outcome in [("learn", {"status": "empty", "reason": "no_reusable_proposal", "saved_count": 0}),
                              ("learn", {"status": "failed", "reason": "reflection_failed", "saved_count": 0}),
                              ("reuse", None), ("off", None)]:
            with self.subTest(mode=mode, outcome=outcome), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                workspace = root / "workspace"
                workspace.mkdir()
                (workspace / "source.py").write_text("pass\n")
                output = root / "attempt"
                settings = {"mode": mode, "generation": 2}
                if outcome is not None:
                    settings["last_outcome"] = outcome
                events = []
                daemon = Daemon.__new__(Daemon)
                daemon.session = lambda *_: "session"
                daemon.meter = SimpleNamespace(records=[], denials=[], settle=lambda: events.append("settled"))

                def api(method, path, value=None):
                    events.append((method, path))
                    if "/turns" in path:
                        return {"id": "turn", "status": "completed"}
                    if "/snapshot?" in path:
                        return {"usage": {}}
                    if "/lessons?" in path:
                        return []
                    if "/learning?" in path:
                        self.assertIn("settled", events)
                        return settings
                    self.fail(f"Unexpected API operation: {method} {path}")

                daemon.api = api
                with contextlib.redirect_stdout(io.StringIO()):
                    result = daemon.run(workspace, {"id": "fixture", "phase": "training", "prompt": "fixture"}, mode, output)
                self.assertEqual(json.loads((output / "learning.json").read_text()), settings)
                self.assertEqual(json.loads((output / "lessons.json").read_text()), [])
                self.assertEqual(sum(isinstance(event, tuple) and "/learning?" in event[1] for event in events), 1)
                self.assertFalse(any(isinstance(event, tuple) and event[0] == "PUT" for event in events))
                self.assertIsNone(result["provider_usage"])
                self.assertFalse(result["provider_usage_complete"])
                self.assertEqual(result["requests"], [])


if __name__ == "__main__":
    unittest.main()
