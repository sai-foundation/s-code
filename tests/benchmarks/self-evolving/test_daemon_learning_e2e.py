"""Real daemon/API/tool serialization with a local, deterministic fake provider.

No key file, provider network, paid inference or private profile reader is used.
Build the daemon before running this module. S_CODE_BENCH_DAEMON overrides the
binary; otherwise CARGO_TARGET_DIR/debug or target/debug is used. Python-only
jobs may skip a missing binary; build jobs set S_CODE_REQUIRE_LEARNING_E2E=1.
"""
import contextlib
import copy
import hashlib
import http.server
import io
import json
import os
from pathlib import Path
import tempfile
import threading
import time
import unittest
from unittest.mock import patch

from audit_evidence import join_tool_evidence, link_map, normalized_scope, validate_source_change
from run import Daemon, ROOT, SCOPE


def daemon_binary():
    override = os.environ.get("S_CODE_BENCH_DAEMON")
    path = Path(override) if override else Path(os.environ.get("CARGO_TARGET_DIR", "target"))/"debug/s-code-daemon"
    return path if path.is_absolute() else ROOT/path


class LocalProvider:
    def __init__(self, binary):
        self.binary, self.model, self.proxy_token = binary, "local-audit-model", "local-fixture-only"
        self.records, self.denials, self.bodies, self.errors = [], [], [], []
        self.context, self.responses = {}, []
        self.lock = threading.Lock()
        provider = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def do_POST(self):
                try:
                    length = int(self.headers.get("Content-Length", "0"))
                    if not 0 < length <= 2 * 1024 * 1024:
                        raise ValueError("request limit")
                    body = json.loads(self.rfile.read(length))
                    with provider.lock:
                        index = len(provider.records)
                        provider.bodies.append(body)
                        response = provider.responses.pop(0) if provider.responses else None
                        record = {"index": index, **provider.context, "started_at": time.time(),
                                  "usage": {"prompt_tokens": 10, "completion_tokens": 2}, "cost": 0}
                        provider.records.append(record)
                    if callable(response):
                        response = response(body)
                    if response is None:
                        delta, finish = {"content": "Fixture task finished."}, "stop"
                    else:
                        tool, arguments = response
                        offered = {t["function"]["name"] for t in body["tools"]}
                        if tool not in offered:
                            raise ValueError("fixture requested unavailable tool")
                        delta = {"tool_calls": [{"index": 0, "id": f"call_offline_{index}", "type": "function",
                            "function": {"name": tool, "arguments": json.dumps(arguments)}}]}
                        finish = "tool_calls"
                    events = [{"choices": [{"index": 0, "delta": delta, "finish_reason": None}]},
                              {"choices": [{"index": 0, "delta": {}, "finish_reason": finish}]},
                              {"choices": [], "usage": record["usage"]}]
                    payload = b"".join(b"data: " + json.dumps(event).encode() + b"\n\n" for event in events) + b"data: [DONE]\n\n"
                    self.send_response(200)
                    self.send_header("Content-Type", "text/event-stream")
                    self.send_header("Content-Length", str(len(payload)))
                    self.end_headers()
                    self.wfile.write(payload)
                    self.wfile.flush()
                    record.update(finished_at=time.time(), elapsed_seconds=time.time()-record["started_at"])
                except Exception as error:
                    # Fixed class only: never copy a request/header into logs.
                    provider.errors.append(type(error).__name__)
                    self.send_error(500, "local fixture failure")

        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.server.daemon_threads = True
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.url = f"http://127.0.0.1:{self.server.server_port}/v1"

    def settle(self):
        deadline = time.monotonic() + 2
        while any("finished_at" not in record for record in self.records) and time.monotonic() < deadline:
            time.sleep(.01)
        if self.errors or any("finished_at" not in record for record in self.records):
            raise AssertionError("Local fixture provider failed")

    def close(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=2)


class DaemonLearningEndToEnd(unittest.TestCase):
    def setUp(self):
        binary = daemon_binary()
        if not binary.is_file():
            if os.environ.get("S_CODE_REQUIRE_LEARNING_E2E") == "1" or os.environ.get("S_CODE_BENCH_DAEMON"):
                self.fail("Required local integration daemon binary is missing")
            self.skipTest("Build the daemon to run the local provider integration test")
        self.temporary = tempfile.TemporaryDirectory(prefix="s-code-learning-e2e-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        self.workspace = self.root/"workspace"
        (self.workspace/"tests").mkdir(parents=True)
        (self.workspace/"widget.py").write_text("def widget_total():\n    return 1\n")
        (self.workspace/"tests/test_widget.py").write_text(
            "import unittest\nfrom widget import widget_total\n\n"
            "class WidgetTest(unittest.TestCase):\n"
            "    def test_widget_total(self):\n        self.assertEqual(widget_total(), 2)\n")
        self.provider = LocalProvider(binary)
        self.addCleanup(self.provider.close)
        self.daemon = Daemon(self.root/"profile", self.provider, self.provider.url)
        self.addCleanup(self.daemon.close)

    def train(self, value=2, *, late_edit=False, replacement=None):
        def edit(body):
            prior = next(json.loads(m["content"]) for m in reversed(body["messages"]) if m["role"] == "tool")
            return "apply_patch", {"path": "widget.py", "expected_revision": prior["revision"],
                                   "edits": [{"old_text": "return 1", "new_text": replacement if replacement is not None else f"return {value}"}]}
        self.provider.responses = [
            ("read_file", {"path": "widget.py"}), edit,
            ("run_command", {"program": "python3", "args": ["-m", "unittest", "discover", "-s", "tests"], "timeout_seconds": 20}),
        ]
        if late_edit:
            self.provider.responses.append(lambda _: ("apply_patch", {
                "path": "widget.py", "expected_revision": hashlib.sha256((self.workspace/"widget.py").read_bytes()).hexdigest(),
                "edits": [{"old_text": f"return {value}", "new_text": "return 3"}]}))
        self.provider.responses.append(None)
        output = self.root/"training"
        with contextlib.redirect_stdout(io.StringIO()):
            result = self.daemon.run(self.workspace, {"id": "local-train", "phase": "train",
                "prompt": "Correct widget_total in widget.py, then verify its tests."}, "learn", output, arm="training")
        self.assertEqual(result["status"], "completed")
        self.assertTrue(result["provider_usage_complete"])
        self.assertEqual(len(result["requests"]), 5 if late_edit else 4)
        self.assertTrue(result["tool_call_links_complete"], json.loads((output/"tool-call-links.json").read_text())["reason"])
        self.assertEqual(self.provider.errors, [])
        return output, result

    def query(self, name, mode="reuse", prompt="Explain widget_total in widget.py."):
        self.provider.responses = [None]
        first = len(self.provider.bodies)
        with contextlib.redirect_stdout(io.StringIO()):
            result = self.daemon.run(self.workspace, {"id": name, "phase": "dev",
                "prompt": prompt}, mode, self.root/name, arm=mode)
        self.assertEqual(result["status"], "completed")
        self.assertTrue(result["tool_call_links_complete"])
        return self.provider.bodies[first]

    @staticmethod
    def experiences(body):
        values = []
        for message in body["messages"]:
            if message["role"] != "user":
                continue
            try: value = json.loads(message["content"])
            except (ValueError, TypeError): continue
            if isinstance(value, dict) and value.get("type") == "untrusted_project_experience":
                values.append(value)
        return values

    def test_real_save_reuse_hash_invalidation_and_missing_link_rejection(self):
        output, result = self.train()
        snapshot = json.loads((output/"snapshot.json").read_text())
        turn = json.loads((output/"turn.json").read_text())
        links = json.loads((output/"tool-call-links.json").read_text())
        lessons = json.loads((output/"lessons.json").read_text())
        outcome = json.loads((output/"learning.json").read_text())
        self.assertEqual(snapshot["session"]["scope"], {**SCOPE, "goal_id": None, "task_id": None})
        self.assertEqual(normalized_scope(snapshot["session"]["scope"]), SCOPE)
        self.assertEqual(outcome["last_outcome"]["status"], "saved")
        self.assertGreater(len(lessons), 0)
        mapping, issue = link_map(links, snapshot, turn["id"])
        self.assertIsNone(issue)
        self.assertTrue(all(a.startswith("tool_") and b.startswith("call_") and a != b for a,b in mapping.items()))
        tools, issue = join_tool_evidence(snapshot, turn["id"], self.provider.bodies, links)
        self.assertIsNone(issue)
        lesson = next(item for item in lessons if item["source_observation"]["path"] == "widget.py")
        # Uses actual persisted provenance IDs, not a fabricated equal-ID fixture.
        edit_id, verifier_id = lesson["evidence_tool_call_ids"]
        observation = lesson["source_observation"]
        self.assertTrue(lesson["id"].startswith("change_"))
        self.assertEqual(tools[edit_id]["tool"], "apply_patch")
        self.assertEqual(observation["change"], {"previous_sha256": hashlib.sha256(
            (output/"initial/widget.py").read_bytes()).hexdigest()})
        self.assertEqual(tools[edit_id]["result"]["previous_sha256"], observation["change"]["previous_sha256"])
        self.assertEqual(tools[edit_id]["result"]["sha256"], observation["sha256"])
        self.assertEqual(observation["sha256"], hashlib.sha256((self.workspace/"widget.py").read_bytes()).hexdigest())
        self.assertEqual(tools[edit_id]["result"]["bytes_written"], len((self.workspace/"widget.py").read_bytes()))
        self.assertEqual((observation["start_line"], observation["end_line"]), (2, 2))
        self.assertEqual(observation["fragments"], [{"start_line": 2, "text": "    return 2\n"}])
        self.assertEqual(sum(call["tool"] == "read_file" for call in tools.values()), 1)
        self.assertNotIn("content", tools[edit_id]["result"])
        self.assertEqual(tools[verifier_id]["arguments"]["args"], ["-m", "unittest", "discover", "-s", "tests"])
        self.assertEqual(tools[verifier_id]["result"]["exit_code"], 0)
        self.assertIn("Ran 1 test", tools[verifier_id]["result"]["stderr"])
        self.assertIsNone(turn["checkpoint"])
        # The same pure audit entry is used by the prospective report analyzer.
        # Its inputs are the actual terminal API, persisted lesson and outbound
        # request structures from this process, not equal-ID synthetic objects.
        self.assertIsNone(validate_source_change(lesson, turn, output/"final", snapshot, self.provider.bodies, links))
        missing = copy.deepcopy(links)
        missing["events"] = [row for row in missing["events"] if row["tool_item_id"] != edit_id]
        incomplete, issue = join_tool_evidence(snapshot, turn["id"], self.provider.bodies, missing)
        self.assertEqual(issue, "missing_tool_call_link")
        self.assertIsNone(incomplete[edit_id]["result"])
        self.assertEqual(validate_source_change(lesson, turn, output/"final", snapshot, self.provider.bodies, missing),
                         "missing_tool_call_link")
        self.assertEqual(join_tool_evidence(snapshot, turn["id"], self.provider.bodies, None)[1], "missing_tool_call_links")
        altered = copy.deepcopy(links)
        row = next(row for row in altered["events"] if row["tool_item_id"] == edit_id)
        row["model_call_id"] = mapping[verifier_id]
        self.assertEqual(link_map(altered, snapshot, turn["id"])[1], "conflicting_tool_call_links")
        self.assertEqual(validate_source_change(lesson, turn, output/"final", snapshot, self.provider.bodies, altered),
                         "conflicting_tool_call_links")
        self.assert_rejected_mutations(lesson, turn, output/"final", snapshot, links, mapping)
        envelope = self.experiences(self.query("local-reuse"))
        self.assertEqual(len(envelope), 1)
        self.assertTrue(any(item["observation"] == lesson["source_observation"] for item in envelope[0]["source_observations"]))
        (self.workspace/"widget.py").write_text("def widget_total():\n    return 3\n")
        self.assertEqual(validate_source_change(lesson, turn, self.workspace, snapshot, self.provider.bodies, links),
                         "observation_hash")
        self.assertEqual(self.experiences(self.query("local-stale")), [])
        self.assertEqual(self.experiences(self.query("local-off", "off")), [])

    def test_selective_recall_abstains_then_removes_already_visible_source(self):
        self.train()
        self.assertEqual(self.experiences(self.query("local-unanchored", prompt="Explain widget_total.")), [])
        first = len(self.provider.bodies)
        self.provider.responses = [("read_file", {"path": "widget.py"}), None]
        with contextlib.redirect_stdout(io.StringIO()):
            result = self.daemon.run(self.workspace, {"id": "local-read", "phase": "dev",
                "prompt": "Explain widget.py."}, "reuse", self.root/"local-read", arm="reuse")
        self.assertEqual(result["status"], "completed")
        self.assertTrue(result["tool_call_links_complete"])
        self.assertEqual(len(result["requests"]), 2)
        bodies = self.provider.bodies[first:]
        self.assertEqual(len(self.experiences(bodies[0])), 1)
        self.assertEqual(self.experiences(bodies[1]), [])
        # A fresh task must not inherit the earlier task's file lookup.
        self.assertEqual(self.experiences(self.query("local-new-task", prompt="Explain widget_total.")), [])

    def test_search_suppresses_a_fully_visible_single_line_observation(self):
        self.train()
        first = len(self.provider.bodies)
        self.provider.responses = [("search_text", {"query": "return"}), None]
        with contextlib.redirect_stdout(io.StringIO()):
            result = self.daemon.run(self.workspace, {"id": "local-search", "phase": "dev",
                "prompt": "Explain the computed total."}, "reuse", self.root/"local-search", arm="reuse")
        self.assertEqual(result["status"], "completed")
        self.assertTrue(result["tool_call_links_complete"])
        self.assertEqual(len(result["requests"]), 2)
        bodies = self.provider.bodies[first:]
        self.assertEqual(self.experiences(bodies[0]), [])
        search_result = next(json.loads(message["content"]) for message in bodies[1]["messages"]
                             if message["role"] == "tool")
        self.assertIn("widget.py:2:    return 2", search_result["stdout"])
        self.assertEqual(self.experiences(bodies[1]), [])

    def test_search_recall_adds_unseen_verified_lines_then_full_read_suppresses(self):
        self.assert_lookup_adds_unseen_verified_lines(("search_text", {"query": "return"}))

    def test_partial_read_recall_adds_unseen_verified_lines_then_full_read_suppresses(self):
        self.assert_lookup_adds_unseen_verified_lines(("read_file", {"path": "widget.py", "start_line": 4, "end_line": 4}))

    def assert_lookup_adds_unseen_verified_lines(self, lookup):
        output, _ = self.train(replacement="subtotal = 1\n    adjustment = 1\n    return subtotal + adjustment")
        lessons = json.loads((output/"lessons.json").read_text())
        lesson = next(item for item in lessons if item["source_observation"]["path"] == "widget.py")
        turn = json.loads((output/"turn.json").read_text())
        snapshot = json.loads((output/"snapshot.json").read_text())
        links = json.loads((output/"tool-call-links.json").read_text())
        self.assertIsNone(validate_source_change(lesson, turn, output/"final", snapshot,
                                                self.provider.bodies, links))
        first = len(self.provider.bodies)
        self.provider.responses = [
            lookup, ("read_file", {"path": "widget.py"}), None,
        ]
        with contextlib.redirect_stdout(io.StringIO()):
            result = self.daemon.run(self.workspace, {"id": "local-new-information", "phase": "dev",
                "prompt": "Explain the computed total."}, "reuse", self.root/"local-new-information", arm="reuse")
        self.assertEqual(result["status"], "completed")
        self.assertTrue(result["tool_call_links_complete"])
        self.assertEqual(len(result["requests"]), 3)
        initial, after_search, after_read = self.provider.bodies[first:]
        self.assertEqual(self.experiences(initial), [])
        recalled = self.experiences(after_search)
        self.assertEqual(len(recalled), 1)
        delivered = recalled[0]["source_observations"][0]
        self.assertEqual(delivered["id"], lesson["id"])
        self.assertEqual(delivered["source_turn"], lesson["source_turn_id"])
        self.assertEqual(delivered["observation"], lesson["source_observation"])
        fragments = delivered["observation"]["fragments"]
        retained_lines = "".join(fragment["text"] for fragment in fragments).splitlines()
        search_results = [json.loads(message["content"]) for message in after_search["messages"]
                          if message["role"] == "tool"]
        self.assertEqual(len(search_results), 1)
        seen = search_results[0]["stdout" if lookup[0] == "search_text" else "content"]
        self.assertIn("return subtotal + adjustment", seen)
        if lookup[0] == "read_file":
            self.assertEqual((search_results[0]["start_line"], search_results[0]["end_line"]), (4, 4))
        non_experience = [message for message in after_search["messages"]
                          if not self.experiences({"messages": [message]})]
        for unseen_line in ("    subtotal = 1", "    adjustment = 1"):
            self.assertIn(unseen_line, retained_lines)
            for message in non_experience:
                self.assertNotIn(unseen_line, json.dumps(message, ensure_ascii=False))
        read_results = [json.loads(message["content"]) for message in after_read["messages"]
                        if message["role"] == "tool"]
        self.assertEqual(len(read_results), 2)
        read = read_results[-1]
        source = (self.workspace/"widget.py").read_bytes()
        self.assertEqual(read["path"], "widget.py")
        self.assertEqual(read["content"], source.decode())
        self.assertEqual(read["sha256"], hashlib.sha256(source).hexdigest())
        self.assertEqual((read["start_line"], read["end_line"]), (1, 4))
        self.assertFalse(read["truncated"])
        self.assertEqual(self.experiences(after_read), [])
        self.assertEqual(self.experiences(self.query("local-multiline-off", "off")), [])

    def assert_rejected_mutations(self, lesson, turn, source, snapshot, links, mapping):
        early = copy.deepcopy(lesson)
        early.update(created_at="2000-01-01T00:00:00Z", expires_at="2000-01-31T00:00:00Z")
        self.assertEqual(validate_source_change(early, turn, source, snapshot, self.provider.bodies, links),
                         "observation_timestamp")
        for field, value, reason in (
            ("change", None, "observation_change_marker"),
            ("change", {"previous_sha256": None}, "observation_previous_hash"),
            ("fragments", [{"start_line": 1, "text": "    return 2\n"}], "observation_fragment_lines"),
            ("truncated", True, "observation_truncation"),
        ):
            bad = copy.deepcopy(lesson)
            bad["source_observation"][field] = value
            with self.subTest(field=field, value=value):
                self.assertEqual(validate_source_change(bad, turn, source, snapshot, self.provider.bodies, links), reason)
        edit_id, verifier_id = lesson["evidence_tool_call_ids"]
        reversed_time = copy.deepcopy(snapshot)
        item = next(item for item in reversed_time["items"] if item["content"].get("tool_call_id") == edit_id)
        item["completed_at"] = "2000-01-01T00:00:00Z"
        self.assertEqual(validate_source_change(lesson, turn, source, reversed_time, self.provider.bodies, links),
                         "observation_edit_after_verifier")
        # Mutate only already-public recorded tool results, preserving real IDs.
        cases = [
            (verifier_id, lambda result: result.update(exit_code=1), "observation_final_verifier"),
            (edit_id, lambda result: result.update(bytes_written=1), "observation_edit_identity"),
            (edit_id, lambda result: result["change_summary"]["after_span"].update(redacted=True), "observation_provenance_unavailable"),
            (edit_id, lambda result: result["change_summary"].update(first_changed_line=1), "observation_change_range"),
            (edit_id, lambda result: result["change_summary"]["after_span"].update(excerpt="    ret", truncated=True), None),
        ]
        for identity, mutate, expected in cases:
            bodies = copy.deepcopy(self.provider.bodies)
            for body in bodies:
                for message in body["messages"]:
                    if message.get("role") == "tool" and message.get("tool_call_id") == mapping[identity]:
                        result = json.loads(message["content"])
                        mutate(result)
                        message["content"] = json.dumps(result)
            with self.subTest(expected=expected):
                self.assertEqual(validate_source_change(lesson, turn, source, snapshot, bodies, links), expected)

    def test_failed_real_verifier_does_not_save_learning(self):
        output, _ = self.train(value=3)
        lessons = json.loads((output/"lessons.json").read_text())
        snapshot = json.loads((output/"snapshot.json").read_text())
        turn = json.loads((output/"turn.json").read_text())
        links = json.loads((output/"tool-call-links.json").read_text())
        tools, issue = join_tool_evidence(snapshot, turn["id"], self.provider.bodies, links)
        self.assertIsNone(issue)
        verifier = next(call for call in tools.values() if call["tool"] == "run_command")
        self.assertNotEqual(verifier["result"]["exit_code"], 0)
        self.assertEqual(lessons, [])
        self.assertEqual(json.loads((output/"learning.json").read_text())["last_outcome"]["status"], "skipped")

    def test_real_write_after_successful_verifier_does_not_save_learning(self):
        output, _ = self.train(late_edit=True)
        self.assertEqual(json.loads((output/"lessons.json").read_text()), [])
        outcome = json.loads((output/"learning.json").read_text())["last_outcome"]
        self.assertEqual(outcome["status"], "skipped")
        self.assertEqual(outcome["reason"], "changes_after_verification")


class E2eConfiguration(unittest.TestCase):
    def test_binary_path_supports_default_cargo_target_and_explicit_override(self):
        for environment, expected in (({}, ROOT/"target/debug/s-code-daemon"),
            ({"CARGO_TARGET_DIR": ".work/ci-cache/target"}, ROOT/".work/ci-cache/target/debug/s-code-daemon"),
            ({"CARGO_TARGET_DIR": "/tmp/local-target"}, Path("/tmp/local-target/debug/s-code-daemon")),
            ({"CARGO_TARGET_DIR": "/tmp/ignored", "S_CODE_BENCH_DAEMON": "/tmp/explicit-daemon"}, Path("/tmp/explicit-daemon"))):
            with self.subTest(environment=environment), patch.dict(os.environ, environment, clear=True):
                self.assertEqual(daemon_binary(), expected)

    def test_required_missing_binary_fails_before_creating_runtime(self):
        case = DaemonLearningEndToEnd("test_failed_real_verifier_does_not_save_learning")
        for environment in ({"S_CODE_REQUIRE_LEARNING_E2E": "1"}, {"S_CODE_BENCH_DAEMON": "explicit"}):
            with patch.dict(os.environ, environment, clear=True), patch(
                    __name__+".daemon_binary", return_value=Path("/missing-fixture-binary")):
                with self.assertRaisesRegex(AssertionError, "Required local integration"):
                    case.setUp()

    def test_optional_missing_binary_skips_before_creating_runtime(self):
        case = DaemonLearningEndToEnd("test_failed_real_verifier_does_not_save_learning")
        with patch.dict(os.environ, {}, clear=True), patch(
                __name__+".daemon_binary", return_value=Path("/missing-fixture-binary")):
            with self.assertRaises(unittest.SkipTest):
                case.setUp()


if __name__ == "__main__":
    unittest.main()
