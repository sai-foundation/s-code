"""Bounded, content-free correlation from the daemon's existing public events.

Provider call IDs and daemon tool item IDs are different namespaces. Never join
them by position, display strings or coincidental equality. These artifacts are
private metadata; public reports should expose counts/digests only.
"""
import json
import hashlib
import re
import time
from datetime import datetime
from pathlib import Path

MAX_EVENT_BYTES = 16 * 1024 * 1024
MAX_EVENT_LINE = 256 * 1024
MAX_EVENT_FRAMES = 20_000
EVENT_DEADLINE_SECONDS = 10
TOOL_EVENTS = {"tool.proposed", "tool.running", "tool.completed", "tool.failed", "tool.denied", "approval.required"}
STATUSES = {"pending", "started", "completed", "failed", "denied", "awaiting_approval"}
LINK_FIELDS = {"schema_version", "session_id", "turn_id", "after_sequence", "through_sequence", "complete", "reason", "events"}


def identifier(value, secrets=()):
    return (isinstance(value, str) and re.fullmatch(r"[A-Za-z0-9_.:-]{1,200}", value) is not None
            and not any(secret and secret in value for secret in secrets))


def tool_metadata(snapshot, turn_id):
    if not isinstance(snapshot, dict) or not isinstance(snapshot.get("session"), dict) or not isinstance(snapshot.get("items"), list):
        raise ValueError("Invalid snapshot metadata")
    metadata = {}
    for item in snapshot["items"]:
        if not isinstance(item, dict):
            raise ValueError("Invalid snapshot item")
        if item.get("turn_id") != turn_id:
            continue
        content = item.get("content")
        if not isinstance(content, dict):
            raise ValueError("Invalid snapshot content")
        if content.get("type") != "tool_call":
            continue
        key, tool = content.get("tool_call_id"), content.get("tool")
        if not identifier(key) or not identifier(tool) or key in metadata:
            raise ValueError("Invalid snapshot tool identity")
        metadata[key] = {"tool": tool, "status": item.get("status"),
            "created_at": item.get("created_at"), "completed_at": item.get("completed_at")}
    return metadata


def link_map(record, snapshot, turn_id, *, require_complete=True):
    """Validate ownership and one-to-one correlation; absent legacy data is unknown."""
    try:
        expected = tool_metadata(snapshot, turn_id)
    except ValueError:
        return {}, "invalid_snapshot_tool_metadata"
    if not isinstance(record, dict) or type(record.get("schema_version")) is not int or record["schema_version"] != 1:
        return {}, "missing_tool_call_links"
    if (set(record) != LINK_FIELDS or type(record.get("complete")) is not bool
            or type(record.get("after_sequence")) is not int
            or type(record.get("through_sequence")) is not int or record["through_sequence"] < 0
            or record["complete"] and record.get("reason") is not None):
        return {}, "invalid_tool_call_links"
    if (record.get("session_id") != snapshot.get("session", {}).get("id")
            or record.get("turn_id") != turn_id or record.get("after_sequence") != 0
            or record.get("through_sequence") != snapshot.get("cursor")):
        return {}, "tool_call_link_ownership"
    if require_complete and record.get("complete") is not True:
        return {}, "incomplete_tool_call_links"
    rows = record.get("events")
    if not isinstance(rows, list) or len(rows) > MAX_EVENT_FRAMES:
        return {}, "invalid_tool_call_links"
    links, inverse, last_status, previous = {}, {}, {}, 0
    for row in rows:
        if (not isinstance(row, dict) or set(row) != {"sequence", "tool_item_id", "model_call_id", "tool", "status"}
                or type(row["sequence"]) is not int or not previous < row["sequence"] <= record["through_sequence"]
                or not all(identifier(row[k]) for k in ("tool_item_id", "model_call_id", "tool"))
                or not isinstance(row["status"], str) or row["status"] not in STATUSES):
            return {}, "invalid_tool_call_links"
        previous = row["sequence"]
        internal, external = row["tool_item_id"], row["model_call_id"]
        if internal not in expected or expected[internal]["tool"] != row["tool"]:
            return {}, "tool_call_link_ownership"
        if internal in links and links[internal] != external or external in inverse and inverse[external] != internal:
            return {}, "conflicting_tool_call_links"
        links[internal], inverse[external], last_status[internal] = external, internal, row["status"]
    if set(links) != set(expected) or any(last_status[k] != expected[k]["status"] for k in links):
        return {}, "missing_tool_call_link"
    return links, None


def empty_links(snapshot, turn_id, reason):
    snapshot = snapshot if isinstance(snapshot, dict) else {}
    session = snapshot.get("session")
    return {"schema_version": 1, "session_id": session.get("id") if isinstance(session, dict) else None,
            "turn_id": turn_id, "after_sequence": 0, "through_sequence": snapshot.get("cursor"),
            "complete": False, "reason": reason, "events": []}


def read_tool_call_links(response, snapshot, turn_id, *, secrets=()):
    """Replay only through the terminal snapshot watermark, projecting tool events.

    Non-tool data (including model deltas and reasoning) is never JSON-decoded or
    retained. All event payloads, display text, errors and headers are discarded.
    The response is local daemon SSE, never a provider/model response stream.
    The current API has no cancelled-tool notification. Such a snapshot stays
    incomplete; a cancelled turn is not a substitute for a tool correlation.
    """
    record = empty_links(snapshot, turn_id, "event_stream_incomplete")
    if not isinstance(snapshot, dict) or not isinstance(snapshot.get("session"), dict):
        record["reason"] = "snapshot_incomplete"
        return record
    through = snapshot.get("cursor")
    if type(through) is not int or through < 0 or snapshot.get("next_cursor") is not None:
        record["reason"] = "snapshot_incomplete"
        return record
    if through == 0:
        reason = link_map(record, snapshot, turn_id, require_complete=False)[1]
        record.update(complete=reason is None, reason=reason)
        return record
    deadline = time.monotonic() + EVENT_DEADLINE_SECONDS
    consumed = frames = previous = 0
    event_type, event_id, data = None, None, []
    try:
        while time.monotonic() < deadline:
            line = response.readline(MAX_EVENT_LINE + 1)
            if not line:
                break
            consumed += len(line)
            if len(line) > MAX_EVENT_LINE or consumed > MAX_EVENT_BYTES:
                record["reason"] = "event_size_limit"
                return record
            line = line.rstrip(b"\r\n")
            if not line:
                if event_id is None:
                    event_type, data = None, []  # Keepalive/comment frame.
                    continue
                frames += 1
                if frames > MAX_EVENT_FRAMES:
                    record["reason"] = "event_count_limit"
                    return record
                if not previous < event_id <= through:
                    record["reason"] = "event_sequence_mismatch"
                    return record
                previous = event_id
                if event_type in TOOL_EVENTS:
                    event = json.loads(b"\n".join(data))
                    if (not isinstance(event, dict) or type(event.get("sequence")) is not int
                            or event["sequence"] != event_id or event.get("type") != event_type):
                        record["reason"] = "event_identity_mismatch"
                        return record
                    if event.get("session_id") == record["session_id"] and event.get("turn_id") == turn_id:
                        note = event.get("notification") or {}
                        payload = event.get("payload") or {}
                        if not isinstance(note, dict) or not isinstance(payload, dict):
                            record["reason"] = "event_identity_mismatch"
                            return record
                        if note.get("type") == "tool_call_changed":
                            internal, status = note.get("item_id"), note.get("status")
                        elif note.get("type") == "approval_requested":
                            internal, status = note.get("tool_item_id"), "awaiting_approval"
                        else:
                            internal, status = None, None
                        external, tool = note.get("model_call_id"), note.get("tool")
                        # ToolProposed has only a provider ID: its projected
                        # item_id is a placeholder, not a persisted tool ID.
                        if external is not None and payload.get("tool_call_id") is not None:
                            if payload["tool_call_id"] != internal or payload.get("model_call_id") != external:
                                record["reason"] = "event_identity_mismatch"
                                return record
                            if (not all(identifier(value, secrets) for value in (internal, external, tool))
                                    or not isinstance(status, str) or status not in STATUSES):
                                record["reason"] = "invalid_tool_call_link"
                                return record
                            record["events"].append({"sequence": event_id, "tool_item_id": internal,
                                "model_call_id": external, "tool": tool, "status": status})
                event_type, event_id, data = None, None, []
                if previous == through:
                    reason = link_map(record, snapshot, turn_id, require_complete=False)[1]
                    record.update(complete=reason is None, reason=reason)
                    return record
            elif line.startswith(b"id:"):
                value = line[3:].strip()
                if not re.fullmatch(rb"[0-9]{1,20}", value):
                    record["reason"] = "event_sequence_mismatch"
                    return record
                event_id = int(value)
            elif line.startswith(b"event:"):
                event_type = line[6:].strip().decode("ascii")
            elif line.startswith(b"data:") and event_type in TOOL_EVENTS:
                data.append(line[5:].lstrip(b" "))
        record["reason"] = "event_stream_incomplete"
    except (ValueError, TypeError, KeyError, OSError, RecursionError):
        record["reason"] = "event_stream_unavailable"
    return record


def join_tool_evidence(snapshot, turn_id, bodies, record, *, required_ids=None):
    """Join public data and retain every chronology row, including missing content.

    By default every snapshot tool must have unambiguous public content. A
    provenance consumer may explicitly require just its selected edit/verifier;
    an absent issue then certifies that subset, not all other chronology rows.
    Complete ID correlation is always required for the whole snapshot.
    """
    try:
        metadata = tool_metadata(snapshot, turn_id)
    except ValueError:
        return {}, "invalid_snapshot_tool_metadata"
    output = {key: {**value, "arguments": {}, "result": None} for key, value in metadata.items()}
    links, issue = link_map(record, snapshot, turn_id)
    if issue:
        return output, issue
    if required_ids is not None and (not isinstance(required_ids, (list, tuple, set)) or not all(identifier(key) for key in required_ids)):
        return output, "invalid_required_tool_evidence"
    required = set(metadata) if required_ids is None else set(required_ids)
    if not required <= metadata.keys():
        return output, "invalid_required_tool_evidence"
    calls, results, ambiguous = {}, {}, set()
    if not isinstance(bodies, (list, tuple)):
        return output, "invalid_public_tool_evidence"
    for body in bodies:
        if not isinstance(body, dict) or not isinstance(body.get("messages"), list):
            return output, "invalid_public_tool_evidence"
        for message in body["messages"]:
            if not isinstance(message, dict):
                return output, "invalid_public_tool_evidence"
            if message.get("role") == "assistant":
                tool_calls = message.get("tool_calls", [])
                if not isinstance(tool_calls, list):
                    return output, "invalid_public_tool_evidence"
                for call in tool_calls:
                    try:
                        if not isinstance(call, dict) or not isinstance(call.get("function"), dict) or not identifier(call.get("id")):
                            return output, "invalid_public_tool_evidence"
                        function = call["function"]
                        arguments = json.loads(function["arguments"])
                        if not isinstance(arguments, dict):
                            continue
                        value = {"tool": function["name"], "arguments": arguments}
                        key = call["id"]
                    except (ValueError, TypeError, KeyError, RecursionError):
                        continue
                    if key in calls and calls[key] != value:
                        ambiguous.add(key)
                    calls[key] = value
            elif message.get("role") == "tool":
                try:
                    if not identifier(message.get("tool_call_id")):
                        return output, "invalid_public_tool_evidence"
                    key, value = message["tool_call_id"], json.loads(message["content"])
                except (ValueError, TypeError, KeyError, RecursionError):
                    continue
                if key in results and results[key] != value:
                    ambiguous.add(key)
                results[key] = value
    for internal, external in links.items():
        if external in calls.keys() & results.keys() - ambiguous and calls[external]["tool"] == metadata[internal]["tool"]:
            output[internal] = {**metadata[internal], "arguments": calls[external]["arguments"], "result": results[external]}
    if any(links[key] in ambiguous for key in required):
        return output, "conflicting_tool_evidence"
    if any(output[key]["result"] is None for key in required):
        return output, "tool_evidence_unavailable"
    return output, None


def normalized_scope(value):
    """Only the real API's optional null goal/task fields may be omitted."""
    required = {"organization_id", "team_id", "actor_id"}
    if (not isinstance(value, dict) or not required <= value.keys()
            or set(value) - required - {"goal_id", "task_id"}
            or any(not isinstance(value[key], str) or not value[key] for key in required)
            or value.get("goal_id") is not None or value.get("task_id") is not None):
        raise ValueError("Invalid benchmark scope")
    return {key: value[key] for key in sorted(required)}


def instant(value):
    """RFC3339 ordering retaining actual daemon nanoseconds on Python 3.9."""
    match = re.fullmatch(r"(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2})(?:\.(\d{1,9}))?(Z|[+-]\d{2}:\d{2})", value) if isinstance(value, str) else None
    if match is None:
        raise ValueError("Invalid timestamp")
    offset = match[3]
    if offset != "Z" and (int(offset[1:3]) > 23 or int(offset[4:6]) > 59):
        raise ValueError("Invalid timezone offset")
    base = datetime.fromisoformat(match[1] + offset.replace("Z", "+00:00"))
    return int(base.timestamp()) * 1_000_000_000 + int((match[2] or "").ljust(9, "0"))


def literal_lines(text):
    pieces = text.split("\n")
    return [line + "\n" for line in pieces[:-1]] + ([pieces[-1]] if pieces[-1] else [])


def relative_source(path, *, canonical=True):
    if (not isinstance(path, str) or not path or Path(path).is_absolute()
            or ".." in Path(path).parts or any(ord(char) < 32 or ord(char) == 127 for char in path)):
        raise ValueError("Invalid relative source path")
    normalized = Path(path).as_posix()
    if canonical and normalized != path or normalized == ".":
        raise ValueError("Noncanonical source path")
    return normalized


def verifier(call):
    """Independently check the public completed runner result, not model prose."""
    if call.get("status") != "completed" or call.get("tool") != "run_command":
        return False
    args = call.get("arguments", {}).get("args", [])
    program = call.get("arguments", {}).get("program", "")
    if not isinstance(program, str) or not isinstance(args, list) or not all(isinstance(x, str) for x in args):
        return False
    program = Path(program).name
    if set(args) & {"-h", "--help", "--version", "--collect-only", "--collectonly", "--list", "--listTests"}:
        return False
    recognized = (program in {"pytest", "pytest3"}
        or program in {"python", "python3"} and len(args) >= 2 and args[0] == "-m" and args[1] in {"pytest", "unittest"}
        or program in {"cargo", "go"} and args[:1] == ["test"]
        or program in {"npm", "pnpm", "yarn", "bun", "make"} and any(x == "test" or x.startswith("test:") for x in args))
    result = call.get("result")
    if not isinstance(result, dict):
        return False
    output = "\n".join(result.get(key, "") for key in ("stdout", "stderr") if isinstance(result.get(key, ""), str))
    def positive(word):
        return word.isascii() and word.isdigit() and 0 < int(word) <= 2**64-1
    counted = False
    for line in output.lower().splitlines():
        words = re.findall(r"[^\W_]+", line)
        counted |= any(positive(a) and b == "passed" or a == "pass" and positive(b) for a, b in zip(words, words[1:]))
        counted |= any(a == "ran" and positive(b) and c in ("test", "tests") for a, b, c in zip(words, words[1:], words[2:]))
        counted |= program == "go" and line.lstrip().startswith("--- pass: ")
    return bool(recognized and type(result.get("exit_code")) is int and result["exit_code"] == 0 and counted)


def span_metadata(value):
    """Validate an execution summary without treating its excerpt as full source."""
    if not isinstance(value, dict) or value.get("redacted", False) is not False:
        raise ValueError("Unavailable change span")
    lines, size, excerpt, truncated = (value[key] for key in ("line_count", "bytes", "excerpt", "truncated"))
    if (type(lines) is not int or type(size) is not int or not 0 <= lines <= size <= 2**64-1
            or (lines == 0) != (size == 0) or not isinstance(excerpt, str) or type(truncated) is not bool):
        raise ValueError("Invalid change span")
    encoded = excerpt.encode()
    if (truncated and not 0 < len(encoded) < size
            or not truncated and (len(encoded) != size or len(literal_lines(excerpt)) != lines)):
        raise ValueError("Invalid change preview")
    return lines, size, excerpt, truncated


def validate_source_change(lesson, turn, source, snapshot, bodies, links):
    """Audit literal change provenance from actual API/tool artifacts.

    The producer is not imported or executed. This checks source provenance,
    not semantic usefulness, test coverage, accounting, or the external grade.
    No response stream, private profile, reasoning or candidate code is executed.
    Return only a fixed reason; never propagate source or exception text.
    """
    try:
        if not isinstance(turn, dict) or not isinstance(snapshot, dict) or not isinstance(snapshot.get("session"), dict):
            return "observation_source_turn"
        lesson_fields = {"id", "source_session_id", "source_turn_id", "applicability", "guidance",
                         "evidence_tool_call_ids", "files", "created_at", "expires_at", "source_observation"}
        if not isinstance(lesson, dict) or set(lesson) != lesson_fields:
            return "observation_record"
        observation = lesson["source_observation"]
        if not isinstance(observation, dict) or set(observation) != {"change", "path", "sha256", "start_line", "end_line", "fragments", "truncated"}:
            return "observation_shape"
        change = observation["change"]
        if not isinstance(change, dict) or set(change) != {"previous_sha256"}:
            return "observation_change_marker"
        path = relative_source(observation["path"])
        file = source/path
        if file.is_symlink() or any((source/parent).is_symlink() for parent in Path(path).parents) or not file.is_file() or file.stat().st_size > 512*1024:
            return "observation_file"
        raw = file.read_bytes()
        if len(raw) > 512*1024:
            return "observation_file"
        content, digest = raw.decode("utf8"), hashlib.sha256(raw).hexdigest()
        if observation["sha256"] != digest or lesson["files"] != [{"path": path, "sha256": digest}]:
            return "observation_hash"
        if (turn.get("status") != "completed" or lesson["source_turn_id"] != turn.get("id")
                or lesson["source_session_id"] != turn.get("session_id")
                or snapshot.get("session", {}).get("id") != turn.get("session_id")
                or normalized_scope(snapshot["session"]["scope"]) != normalized_scope(turn["scope"])):
            return "observation_source_turn"
        expected_id = "change_" + hashlib.sha256((turn["id"]+":"+path).encode()).hexdigest()
        if lesson["id"] != expected_id or len(json.dumps(lesson, ensure_ascii=False, separators=(",", ":")).encode()) > 3200:
            return "observation_record"
        if (lesson["applicability"] != "Previously verified source change: "+path
                or lesson["guidance"] != "Source from an edit before successful verification; its enclosing span may include unchanged lines. Not a procedure or a test-coverage claim."):
            return "observation_labels"
        start, end = observation["start_line"], observation["end_line"]
        lines = literal_lines(content)
        if type(start) is not int or type(end) is not int or not 1 <= start <= end <= min(len(lines), 2**32-1) or type(observation["truncated"]) is not bool:
            return "observation_range"
        evidence = lesson["evidence_tool_call_ids"]
        if not isinstance(evidence, list) or len(evidence) != 2 or not all(identifier(key) for key in evidence) or evidence[0] == evidence[1]:
            return "observation_evidence_ids"
        tools, issue = join_tool_evidence(snapshot, turn["id"], bodies, links, required_ids=evidence)
        if issue:
            return issue
        edited, verified = [tools[key] for key in evidence]
        if not verifier(verified):
            return "observation_final_verifier"
        if edited["status"] != "completed" or edited["tool"] != "apply_patch":
            return "observation_edit_status"
        verified_at = instant(verified["created_at"])
        if instant(verified["completed_at"]) < verified_at:
            return "observation_final_verifier"
        if instant(lesson["created_at"]) < instant(verified["completed_at"]):
            return "observation_timestamp"
        if instant(edited["created_at"]) >= verified_at or instant(edited["completed_at"]) > verified_at:
            return "observation_edit_after_verifier"
        if instant(edited["completed_at"]) < instant(edited["created_at"]):
            return "observation_edit_after_verifier"
        for key, call in tools.items():
            if key == evidence[1] or call["tool"] in {"read_file", "list_files", "search_text", "git_diff", "git_status"}:
                continue
            if (call["status"] not in {"completed", "failed", "denied", "cancelled"}
                    or (instant(call["created_at"]), key) > (verified_at, evidence[1])
                    or call["completed_at"] is None or instant(call["completed_at"]) > verified_at
                    or instant(call["completed_at"]) < instant(call["created_at"])):
                return "observation_post_verification_change"
        result = edited["result"]
        if (not isinstance(result, dict) or relative_source(edited["arguments"]["path"], canonical=False) != path
                or relative_source(result.get("path"), canonical=False) != path or result.get("sha256") != digest
                or type(result.get("bytes_written")) is not int or result["bytes_written"] != len(raw)):
            return "observation_edit_identity"
        previous = result["previous_sha256"]
        if change["previous_sha256"] != previous or previous is not None and (
                not isinstance(previous, str) or re.fullmatch(r"[0-9a-fA-F]{64}", previous) is None or previous.lower() == digest):
            return "observation_previous_hash"
        summary = result["change_summary"]
        if not isinstance(summary, dict) or summary.get("text_preview_available", True) is not True:
            return "observation_change_summary"
        before_count, _, _, _ = span_metadata(summary["before_span"])
        after_count, after_bytes, excerpt, preview_truncated = span_metadata(summary["after_span"])
        before_total, after_total, first = (summary[key] for key in ("before_total_lines", "after_total_lines", "first_changed_line"))
        if (type(first) is not int or first != start or after_count == 0 or end != start-1+after_count
                or type(before_total) is not int or not 0 <= before_total <= 2**64-1
                or type(after_total) is not int or after_total != len(lines)
                or before_total < start-1+before_count
                or before_total-(start-1+before_count) != after_total-end
                or previous is None and (before_total != 0 or start != 1)):
            return "observation_change_range"
        changed = "".join(lines[start-1:end])
        if len(changed.encode()) != after_bytes or (not changed.startswith(excerpt) if preview_truncated else changed != excerpt):
            return "observation_change_content"
        fragments = observation["fragments"]
        if not isinstance(fragments, list) or not 1 <= len(fragments) <= 2:
            return "observation_fragments"
        previous_end, retained = start-1, 0
        for fragment in fragments:
            if not isinstance(fragment, dict) or set(fragment) != {"start_line", "text"}:
                return "observation_fragment_shape"
            first, text = fragment["start_line"], fragment["text"]
            if type(first) is not int or not isinstance(text, str) or not text:
                return "observation_fragment_type"
            last = first + len(literal_lines(text))-1
            if first <= previous_end or first < start or last > end or text != "".join(lines[first-1:last]):
                return "observation_fragment_lines"
            retained += len(text.encode())
            previous_end = last
        if observation["truncated"] != (retained < len(changed.encode())):
            return "observation_truncation"
        if instant(lesson["expires_at"]) - instant(lesson["created_at"]) != 30*24*3600*1_000_000_000:
            return "observation_expiry"
    except (KeyError, TypeError, ValueError, OSError, OverflowError):
        return "observation_provenance_unavailable"
    return None
