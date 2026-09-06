"""Offline bounds and failure semantics for public daemon-event correlation."""
import copy
import io
import json
import unittest
from unittest.mock import patch

import audit_evidence as audit


def snapshot(cursor=2):
    return {"session": {"id": "session_a", "scope": {"goal_id": None, "task_id": None}},
            "cursor": cursor, "next_cursor": None, "items": [
                {"turn_id": "turn_a", "created_at": "2026-09-01T00:00:00Z",
                 "completed_at": "2026-09-01T00:00:01Z", "status": "completed",
                 "content": {"type": "tool_call", "tool_call_id": "tool_a", "tool": "read_file"}}]}


def event(sequence, *, internal="tool_a", external="call_a", session="session_a",
          turn="turn_a", kind="tool.completed", status="completed"):
    return {"sequence": sequence, "type": kind, "session_id": session, "turn_id": turn,
            "payload": {"tool_call_id": internal, "model_call_id": external},
            "notification": {"type": "tool_call_changed", "item_id": internal,
                             "model_call_id": external, "tool": "read_file", "status": status}}


def frame(value):
    return (f'id: {value["sequence"]}\nevent: {value["type"]}\ndata: '
            + json.dumps(value) + '\n\n').encode()


def stream(*values):
    return io.BytesIO(b"".join(frame(value) for value in values))


class AuditEvidenceTests(unittest.TestCase):
    def collect(self, *values, cursor=None):
        return audit.read_tool_call_links(stream(*values), snapshot(cursor or len(values)), "turn_a")

    def test_projection_ignores_other_sessions_and_non_tool_content(self):
        foreign = event(1, session="session_other")
        foreign["notification"] = "foreign payload need not be interpreted"
        actual = event(2)
        actual["payload"]["display"] = "private source sentinel"
        actual["notification"]["error"] = "private error sentinel"
        # Deliberately invalid JSON proves non-tool data is never parsed.
        tail = b'id: 3\nevent: model.delta\ndata: not JSON or public tool data\n\n'
        result = audit.read_tool_call_links(io.BytesIO(frame(foreign)+frame(actual)+tail), snapshot(3), "turn_a")
        self.assertTrue(result["complete"])
        self.assertEqual(result["events"], [{"sequence": 2, "tool_item_id": "tool_a",
                          "model_call_id": "call_a", "tool": "read_file", "status": "completed"}])
        encoded = json.dumps(result)
        for forbidden in ("private source", "private error", "not JSON", "session_other"):
            self.assertNotIn(forbidden, encoded)

    def test_proposed_placeholder_never_becomes_daemon_identity(self):
        proposed = event(1, internal="call_a", kind="tool.proposed", status="pending")
        del proposed["payload"]["tool_call_id"]
        result = self.collect(proposed, event(2))
        self.assertTrue(result["complete"])
        self.assertEqual(len(result["events"]), 1)
        missing = self.collect(proposed)
        self.assertFalse(missing["complete"])
        self.assertEqual(missing["reason"], "missing_tool_call_link")

    def test_payload_notification_disagreement_and_known_secret_are_rejected(self):
        mismatch = event(1)
        mismatch["payload"]["tool_call_id"] = "tool_other"
        self.assertEqual(self.collect(mismatch)["reason"], "event_identity_mismatch")
        result = audit.read_tool_call_links(stream(event(1, external="call_private_token")), snapshot(1),
                                           "turn_a", secrets=("private_token",))
        self.assertEqual(result["reason"], "invalid_tool_call_link")
        self.assertNotIn("private_token", json.dumps(result))

    def test_missing_watermark_socket_failure_and_pagination_are_unknown(self):
        self.assertEqual(self.collect(event(1), cursor=2)["reason"], "event_stream_incomplete")
        class Broken:
            def readline(self, _):
                raise OSError("private transport text")
        result = audit.read_tool_call_links(Broken(), snapshot(), "turn_a")
        self.assertEqual(result["reason"], "event_stream_unavailable")
        self.assertNotIn("private transport", json.dumps(result))
        paged = snapshot()
        paged["next_cursor"] = "more"
        self.assertEqual(audit.read_tool_call_links(Broken(), paged, "turn_a")["reason"], "snapshot_incomplete")

    def test_resource_deadlines_lines_total_bytes_and_frame_counts(self):
        with patch.object(audit.time, "monotonic", side_effect=[0, 11]):
            self.assertEqual(self.collect(event(1))["reason"], "event_stream_incomplete")
        for constant, bound, values, reason in (
            ("MAX_EVENT_LINE", 20, [event(1)], "event_size_limit"),
            ("MAX_EVENT_BYTES", 20, [event(1)], "event_size_limit"),
            ("MAX_EVENT_FRAMES", 1, [event(1), event(2)], "event_count_limit"),
        ):
            with self.subTest(constant=constant), patch.object(audit, constant, bound):
                self.assertEqual(self.collect(*values)["reason"], reason)

    def test_event_sequences_and_final_status_are_checked(self):
        self.assertEqual(self.collect(event(2), event(1), cursor=3)["reason"], "event_sequence_mismatch")
        self.assertEqual(self.collect(event(1), event(1))["reason"], "event_sequence_mismatch")
        self.assertEqual(self.collect(event(1, status="started"))["reason"], "missing_tool_call_link")
        mismatched = event(1)
        encoded = frame(mismatched).replace(b'"sequence": 1', b'"sequence": true')
        self.assertEqual(audit.read_tool_call_links(io.BytesIO(encoded), snapshot(1), "turn_a")["reason"], "event_identity_mismatch")

    def test_link_artifacts_have_strict_schema_ownership_and_bijection(self):
        valid = self.collect(event(1))
        self.assertEqual(audit.link_map(valid, snapshot(1), "turn_a"), ({"tool_a": "call_a"}, None))
        for key, value in (("schema_version", True), ("through_sequence", True), ("complete", 1),
                           ("reason", "unexpected"), ("extra", "unbound text"),
                           ("session_id", "session_other"), ("turn_id", "turn_other")):
            changed = copy.deepcopy(valid)
            changed[key] = value
            with self.subTest(key=key):
                self.assertIsNotNone(audit.link_map(changed, snapshot(1), "turn_a")[1])
        duplicate = self.collect(event(1), event(2, external="call_other"))
        self.assertEqual(duplicate["reason"], "conflicting_tool_call_links")

    def test_join_requires_explicit_mapping_and_preserves_unavailable_chronology(self):
        state = snapshot(1)
        valid = self.collect(event(1))
        body = {"messages": [
            {"role": "assistant", "tool_calls": [{"id": "call_a", "function": {
                "name": "read_file", "arguments": '{"path":"widget.py"}'}}]},
            {"role": "tool", "tool_call_id": "call_a", "content": '{"content":"source"}'},
        ]}
        joined, issue = audit.join_tool_evidence(state, "turn_a", [body], valid)
        self.assertIsNone(issue)
        self.assertEqual(joined["tool_a"]["arguments"], {"path": "widget.py"})
        self.assertEqual(joined["tool_a"]["result"], {"content": "source"})
        unavailable, issue = audit.join_tool_evidence(state, "turn_a", [body], None)
        self.assertEqual(issue, "missing_tool_call_links")
        self.assertIsNone(unavailable["tool_a"]["result"])
        self.assertEqual(unavailable["tool_a"]["created_at"], state["items"][0]["created_at"])
        changed = copy.deepcopy(body)
        changed["messages"][1]["content"] = '{"content":"compressed result"}'
        ambiguous, issue = audit.join_tool_evidence(state, "turn_a", [body, changed], valid)
        self.assertEqual(issue, "conflicting_tool_evidence")
        self.assertIsNone(ambiguous["tool_a"]["result"])
        for missing in ({"messages": body["messages"][:1]}, {"messages": body["messages"][1:]},
                        {"messages": []}):
            self.assertEqual(audit.join_tool_evidence(state, "turn_a", [missing], valid)[1],
                             "tool_evidence_unavailable")
        wrong_tool = copy.deepcopy(body)
        wrong_tool["messages"][0]["tool_calls"][0]["function"]["name"] = "replace_file"
        self.assertEqual(audit.join_tool_evidence(state, "turn_a", [wrong_tool], valid)[1],
                         "tool_evidence_unavailable")
        # An explicit empty requested subset makes no public-content claim.
        self.assertIsNone(audit.join_tool_evidence(state, "turn_a", [], valid, required_ids=[])[1])
        self.assertEqual(audit.join_tool_evidence(state, "turn_a", [body], valid,
                         required_ids=["tool_nonexistent"])[1], "invalid_required_tool_evidence")
        # Metadata-only pending mutation remains visible despite absent content.
        state["items"].append({"turn_id": "turn_a", "created_at": "2026-09-01T00:00:02Z",
            "completed_at": None, "status": "pending", "content": {
                "type": "tool_call", "tool_call_id": "tool_mutator", "tool": "replace_file"}})
        joined, issue = audit.join_tool_evidence(state, "turn_a", [body], valid)
        self.assertEqual(issue, "missing_tool_call_link")
        self.assertIn("tool_mutator", joined)
        self.assertIsNone(joined["tool_mutator"]["completed_at"])

    def test_malformed_api_and_public_tool_structures_return_fixed_issues(self):
        valid = self.collect(event(1))
        for malformed in ({}, {"items": [None]}, {"items": [{"turn_id": "turn_a", "content": []}]}):
            self.assertEqual(audit.link_map(valid, malformed, "turn_a")[1], "invalid_snapshot_tool_metadata")
        duplicate = snapshot(1)
        duplicate["items"].append(copy.deepcopy(duplicate["items"][0]))
        duplicate["items"][1]["status"] = "failed"
        self.assertEqual(audit.link_map(valid, duplicate, "turn_a")[1], "invalid_snapshot_tool_metadata")
        wrong_session = snapshot(1)
        wrong_session["session"] = []
        self.assertEqual(audit.link_map(valid, wrong_session, "turn_a")[1], "invalid_snapshot_tool_metadata")
        self.assertEqual(audit.read_tool_call_links(stream(event(1)), wrong_session, "turn_a")["reason"], "snapshot_incomplete")
        wrong_status = copy.deepcopy(valid)
        wrong_status["events"][0]["status"] = []
        self.assertEqual(audit.link_map(wrong_status, snapshot(1), "turn_a")[1], "invalid_tool_call_links")
        self.assertEqual(self.collect(event(1, status=[]))["reason"], "invalid_tool_call_link")
        self.assertEqual(audit.validate_source_change({}, [], None, snapshot(1), [], valid), "observation_source_turn")
        for bodies in (None, [[]], [{"messages": [None]}], [{"messages": [{"role": "assistant", "tool_calls": {}}]}],
            [{"messages": [{"role": "assistant", "tool_calls": [None]}]}],
            [{"messages": [{"role": "assistant", "tool_calls": [{"id": [], "function": {}}]}]}],
            [{"messages": [{"role": "tool", "tool_call_id": [], "content": "{}"}]}]):
            self.assertEqual(audit.join_tool_evidence(snapshot(1), "turn_a", bodies, valid)[1], "invalid_public_tool_evidence")
        self.assertEqual(audit.join_tool_evidence(snapshot(1), "turn_a", [], valid, required_ids=[[]])[1], "invalid_required_tool_evidence")

    def test_cancelled_snapshot_cannot_be_inferred_from_other_terminal_events(self):
        state = snapshot(1)
        state["items"][0]["status"] = "cancelled"
        result = audit.read_tool_call_links(stream(event(1, kind="tool.failed", status="failed")), state, "turn_a")
        self.assertFalse(result["complete"])
        self.assertEqual(result["reason"], "missing_tool_call_link")

    def test_scope_normalizes_only_real_optional_null_fields(self):
        required = {"organization_id": "o", "team_id": "t", "actor_id": "a"}
        self.assertEqual(audit.normalized_scope({**required, "goal_id": None, "task_id": None}), required)
        for malformed in ({**required, "goal_id": "nonempty"}, {**required, "extra": None},
                          {"organization_id": "o", "team_id": "t"}, []):
            with self.assertRaises(ValueError):
                audit.normalized_scope(malformed)

    def test_span_validation_uses_utf8_bytes_and_preserves_crlf_and_eof(self):
        full = "工具\r\nlast line"
        value = {"line_count": 2, "bytes": len(full.encode()), "excerpt": full, "truncated": False}
        self.assertEqual(audit.span_metadata(value), (2, len(full.encode()), full, False))
        shortened = {**value, "excerpt": "工", "truncated": True}
        self.assertEqual(audit.span_metadata(shortened)[2:], ("工", True))
        for malformed in ({**value, "bytes": len(full)}, {**value, "redacted": True},
                          {**value, "line_count": True}, {**value, "truncated": True},
                          {**value, "bytes": 2**64}):
            with self.assertRaises(ValueError):
                audit.span_metadata(malformed)

    def test_rfc3339_keeps_nanosecond_order_and_rejects_overflow(self):
        self.assertEqual(audit.instant("2026-09-01T01:00:00+01:00"), audit.instant("2026-09-01T00:00:00Z"))
        self.assertEqual(audit.instant("2026-09-01T00:00:00.000000002Z") - audit.instant("2026-09-01T00:00:00.000000001Z"), 1)
        for malformed in ("2026-09-01T00:00:00+01:99", "2026-09-01T00:00:00-24:00", None):
            with self.assertRaises(ValueError):
                audit.instant(malformed)


if __name__ == "__main__":
    unittest.main()
