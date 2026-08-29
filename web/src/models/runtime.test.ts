import { describe, expect, it } from "vitest";
import { parseClientEvent, parseTranscriptSnapshot } from "./runtime";

function snapshot() {
  return {
    protocol_version: "1.0.0",
    snapshot_revision: 7,
    session: { id: "session-a" },
    turns: [{ id: "turn-a", session_id: "session-a" }],
    items: [{
      id: "item-a",
      session_id: "session-a",
      turn_id: "turn-a",
      kind: "agent_message",
      status: "completed",
      revision: 1,
      content: { type: "message", role: "assistant", content: "done" }
    }],
    pending_requests: [],
    pending_questions: [],
    pending_inputs: [],
    attachments: [],
    artifacts: [],
    cursor: 7
  };
}

describe("protocol runtime validation", () => {
  it("accepts a valid owned snapshot", () => {
    expect(parseTranscriptSnapshot(snapshot()).items[0]?.id).toBe("item-a");
  });

  it("rejects a cross-session item", () => {
    const value = snapshot();
    value.items[0]!.session_id = "session-b";
    expect(() => parseTranscriptSnapshot(value)).toThrow("different session");
  });

  it("rejects duplicate item identities", () => {
    const value = snapshot();
    value.items.push({ ...value.items[0]! });
    expect(() => parseTranscriptSnapshot(value)).toThrow("duplicate item");
  });

  it("validates exact session usage while migrating older snapshots to zero", () => {
    expect(parseTranscriptSnapshot(snapshot()).usage.total_tokens).toBe(0);
    const value = snapshot();
    Object.assign(value, {
      usage: {
        input_tokens: 12,
        output_tokens: 3,
        total_tokens: 99,
        model_calls: 2,
        tool_calls: 1,
        turns: 1,
      },
    });
    expect(() => parseTranscriptSnapshot(value)).toThrow("total does not match");
  });

  it("rejects an unversioned client event", () => {
    expect(() => parseClientEvent({
      id: "event-a",
      sequence: 1,
      timestamp: "2026-07-27T00:00:00Z",
      session_id: "session-a",
      turn_id: "turn-a",
      type: "model.delta",
      payload: {}
    })).toThrow("payload_version");
  });

  it("validates restored and live MCP progress", () => {
    const value = snapshot();
    (value.items as Array<Record<string, unknown>>)[0] = {
      ...value.items[0]!,
      kind: "mcp_call",
      content: {
        type: "mcp_call",
        tool_call_id: "item-a",
        server: "fixture",
        tool: "echo",
        namespaced_tool: "mcp.fixture.echo",
        progress: { progress: 101, total: 100, message: "invalid" },
      },
    };
    expect(() => parseTranscriptSnapshot(value)).toThrow("cannot exceed total");
    expect(() => parseClientEvent({
      id: "event-progress",
      sequence: 8,
      timestamp: "2026-07-27T00:00:00Z",
      session_id: "session-a",
      turn_id: "turn-a",
      item_id: "item-a",
      type: "mcp.progress",
      payload_version: 1,
      notification: {
        type: "mcp_progress_changed",
        item_id: "item-a",
        server: "fixture",
        tool: "echo",
        progress: -1,
        total: 100,
        message: null,
      },
      payload: {},
    })).toThrow("finite non-negative");
  });
});
