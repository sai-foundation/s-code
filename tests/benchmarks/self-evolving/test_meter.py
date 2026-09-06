"""Accounting and control validity without credentials, networking or model calls."""
import json
from pathlib import Path
import tempfile
import threading
import unittest

from run import Meter, provider_totals, tree_hash, observe_event
from pilot import raw_retrieve, protection_changes


class AccountingTests(unittest.TestCase):
    def test_unknown_or_malformed_usage_never_becomes_zero_cost(self):
        known = {"usage":{"prompt_tokens":11,"completion_tokens":7}, "cost":0.1}
        self.assertEqual(provider_totals([known])["total_tokens"], 18)
        for invalid in ({}, {"usage":None,"cost":0}, {"usage":{"prompt_tokens":True,"completion_tokens":2},"cost":0}, {**known,"error":"HTTPError"}, {**known,"invalid_event":True}):
            self.assertIsNone(provider_totals([known,invalid]))
        self.assertIsNone(provider_totals([]))

    def test_missing_dollar_cost_preserves_known_tokens(self):
        for cost in (None,float("nan"),-1):
            total=provider_totals([{"usage":{"prompt_tokens":11,"completion_tokens":7}, "cost":cost}])
            self.assertEqual(total["total_tokens"],18)
            self.assertIsNone(total["cost_usd"])

    def test_stream_error_cannot_certify_partial_usage(self):
        for ending in ({"error":{"code":502,"message":"upstream failed"}}, {"choices":[{"delta":{},"finish_reason":"error"}]}):
            record={}
            observe_event(record,{"id":"synthetic-generation", "usage":{"prompt_tokens":11,"completion_tokens":7,"cost":0.1}})
            self.assertIsNotNone(provider_totals([record]))
            observe_event(record,ending)
            self.assertEqual(record["error"],"ProviderStreamError")
            self.assertEqual(record["usage"]["prompt_tokens"],11)
            self.assertIsNone(provider_totals([record]))

    def test_equal_phase_cap_and_unknown_request_reservations(self):
        with tempfile.TemporaryDirectory() as directory:
            meter = Meter.__new__(Meter)
            meter.output = Path(directory)
            meter.prices = {"prompt":0, "completion":1}
            meter.lock = threading.Lock()
            meter.records, meter.denials = [], []
            meter.context = {"arm":"off"}
            meter.active = 0
            meter.max_calls, meter.max_cost = 8, 20
            meter.phase_start, meter.phase_max_cost = 0, 10
            self.assertIsNotNone(meter.reserve({"max_tokens":6}))
            meter.records[-1]["cost"] = float("nan")
            self.assertIsNone(meter.reserve({"max_tokens":5}))
            # A new arm gets an identical cap; prior unknown usage stays reserved.
            meter.phase_start = len(meter.records)
            meter.context = {"arm":"learned"}
            self.assertIsNotNone(meter.reserve({"max_tokens":6}))
            self.assertIsNone(meter.reserve({"max_tokens":5}))
            self.assertEqual([d["arm"] for d in meter.denials],["off","learned"])
            self.assertEqual(len(json.loads((meter.output/"budget-denials.json").read_text())),2)

    def test_raw_experience_is_bounded_relevant_and_invalidated_by_change(self):
        import hashlib
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root/"clock.py").write_text("clock implementation")
            item = {"path":"clock.py","sha256":hashlib.sha256((root/"clock.py").read_bytes()).hexdigest(), "applicability":"queue finite time", "excerpt":"Observed queue clock helper"}
            value = raw_retrieve([item],root,"queue validation")
            self.assertEqual(json.loads(value)["observations"][0]["file"],"clock.py")
            self.assertIsNone(raw_retrieve([item],root,"unrelated reports"))
            self.assertIsNone(raw_retrieve([{**item,"excerpt":"queue "*2000}],root,"queue"))
            (root/"clock.py").write_text("updated implementation")
            self.assertIsNone(raw_retrieve([item],root,"queue"))

    def test_optional_provider_detail_null_does_not_lose_main_usage(self):
        from evaluate import normalized
        result={"provider_usage":{"input_tokens":11,"output_tokens":7}, "grade":{"passed":True,"grading_complete":True}, "status":"completed", "cost_usd":0.1,"elapsed_seconds":1.0,"budget_denied":False}
        row=normalized({"id":"task"},17,"off",0,result,[{"usage":{"prompt_tokens_details":None,"completion_tokens_details":"unavailable"}}])
        self.assertTrue(row["usage_complete"])
        self.assertEqual(row["input_tokens"],11)
        self.assertIsNone(row["cached_input_tokens"])
        self.assertIsNone(row["reasoning_output_tokens"])
        result["status"]="awaiting_input"
        row=normalized({"id":"task"},17,"off",0,result,[])
        self.assertFalse(row["verified_success"])
        self.assertTrue(row["raw_grader_pass"])

    def test_new_test_bootstrap_and_config_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            baseline, final = Path(directory)/"base", Path(directory)/"final"
            baseline.mkdir(); (final/"tests").mkdir(parents=True)
            for name in ("conftest.py", "__init__.py", "sitecustomize.py", "test_regression.py"):
                (final/"tests"/name).write_text("# new file")
            (final/"tmp-owned-test-scratch").mkdir()
            (final/"tmp-owned-test-scratch/plan.json").write_text("{}")
            (final/"utility.py").write_text("# legitimate source addition")
            changed, unexpected = protection_changes(baseline,final)
            self.assertEqual(changed,[])
            self.assertEqual(set(unexpected),{"tests/conftest.py", "tests/__init__.py", "tests/sitecustomize.py"})

    def test_snapshot_digest_covers_new_and_committed_files(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root/"a").write_text("source")
            before = tree_hash(root)
            (root/"new").write_text("new source")
            self.assertNotEqual(tree_hash(root),before)
            after = tree_hash(root)
            (root/".git").mkdir()
            (root/".git/HEAD").write_text("new commit")
            self.assertEqual(tree_hash(root),after)


if __name__ == "__main__":
    unittest.main()
