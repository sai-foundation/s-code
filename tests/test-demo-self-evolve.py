#!/usr/bin/env python3
"""The demo must be deterministic, in order, and safe to publish."""

from __future__ import annotations

import importlib.util
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
DEMO = ROOT / "tools/demo/self_evolve_demo.py"
SANITISER = ROOT / "tools/demo/sanitize_fixture.py"
FIXTURE = ROOT / "tools/demo/fixtures/self-evolve-demo.json"
SCRIPT = ROOT / "scripts/demo-self-evolve.sh"


def load(path: Path, name: str):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


demo = load(DEMO, "self_evolve_demo")
sanitiser = load(SANITISER, "sanitize_fixture")


class ReplayFixtureTests(unittest.TestCase):
    def setUp(self):
        self.document = json.loads(FIXTURE.read_text(encoding="utf-8"))

    def test_fixture_carries_the_whole_lifecycle_in_order(self):
        self.assertEqual([state["state"] for state in self.document["states"]], list(demo.ORDER))

    def test_fixture_digest_matches_its_contents(self):
        demo.verify_fixture(self.document)

    def test_edited_fixture_is_refused(self):
        tampered = json.loads(json.dumps(self.document))
        tampered["states"][0]["headline"] = "something else"
        with self.assertRaises(SystemExit):
            demo.verify_fixture(tampered)

    def test_fixture_carries_no_path_host_or_secret(self):
        text = FIXTURE.read_text(encoding="utf-8")
        for pattern in (r"/network/", r"/home/", r"/tmp/", r"127\.0\.0\.1", r"localhost",
                        r"openrouter", r"Bearer ", r"sk-[A-Za-z0-9]", r"guangyuan", r"mila"):
            self.assertIsNone(re.search(pattern, text, re.IGNORECASE), pattern)

    def test_fixture_carries_no_real_identifier(self):
        text = FIXTURE.read_text(encoding="utf-8")
        minted = re.findall(r"\b(?:exp|skill|ses|turn|principal|eval)_(?:demo\d+|[A-Za-z0-9]{8,})\b", text)
        self.assertTrue(minted, "the fixture should mention the renamed identifiers")
        for identifier in minted:
            self.assertRegex(identifier, r"_demo\d+$", identifier)

    def test_poisoned_candidate_is_never_approved_or_verified(self):
        refused = next(state for state in self.document["states"] if state["state"] == "refused")
        self.assertTrue(refused["evidence"]["candidate_remained_unapproved"])
        self.assertEqual(refused["evidence"]["verdict"], "clean")
        self.assertTrue(refused["evidence"]["harmful_rule_absent_from_requests"])
        for state in self.document["states"]:
            if state["state"] in ("evaluated", "published", "verified", "reused"):
                self.assertNotIn("poison", json.dumps(state).lower())


class ReplayRunTests(unittest.TestCase):
    def run_demo(self, *arguments):
        return subprocess.run([sys.executable, str(DEMO), *arguments],
                              capture_output=True, text=True, check=False)

    def test_replay_is_deterministic(self):
        first = self.run_demo("--mode", "replay")
        second = self.run_demo("--mode", "replay")
        self.assertEqual(first.returncode, 0, first.stderr)
        self.assertEqual(first.stdout, second.stdout)

    def test_replay_shows_every_state_in_order(self):
        result = self.run_demo("--mode", "replay")
        positions = [result.stdout.index(demo.TITLES[state]) for state in demo.ORDER]
        self.assertEqual(positions, sorted(positions))

    def test_script_entry_point_runs_replay_without_a_provider(self):
        result = subprocess.run([str(SCRIPT)], capture_output=True, text=True, check=False,
                                env={"PATH": f"{Path(sys.executable).parent}:/usr/bin:/bin"})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("replay", result.stdout)

    def test_live_mode_never_falls_back_to_the_fixture(self):
        with tempfile.TemporaryDirectory() as directory:
            result = subprocess.run(
                [sys.executable, str(DEMO), "--mode", "live"], capture_output=True, text=True,
                check=False, env={"PATH": "/usr/bin:/bin", "HOME": directory,
                                  "S_CODE_RUNTIME_DIR": directory})
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("live mode needs a running local daemon", result.stdout + result.stderr)
        self.assertNotIn("replay", result.stdout)


class SanitiserTests(unittest.TestCase):
    def test_identifiers_are_renamed_stably(self):
        instance = sanitiser.Sanitiser()
        first = instance.name_for("exp_01ABCDEF")
        self.assertEqual(first, instance.name_for("exp_01ABCDEF"))
        self.assertNotEqual(first, instance.name_for("exp_01GHIJKL"))
        self.assertTrue(first.startswith("exp_demo"))

    def test_text_that_could_leak_is_refused(self):
        instance = sanitiser.Sanitiser()
        for leaky in ("/network/scratch/g/someone/run", "http://127.0.0.1:8080/v1",
                      "Bearer abc123", "sk-livekey", "see /home/user/notes"):
            with self.assertRaises(ValueError, msg=leaky):
                sanitiser.scrub({"text": leaky}, instance)

    def test_ordinary_lesson_text_survives(self):
        instance = sanitiser.Sanitiser()
        lesson = "Validate the whole manifest before writing the lock file."
        self.assertEqual(sanitiser.scrub({"lesson": lesson}, instance), {"lesson": lesson})


if __name__ == "__main__":
    unittest.main()
