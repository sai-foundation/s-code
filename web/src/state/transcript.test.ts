// @ts-expect-error Vitest provides the Node built-in; the browser bundle intentionally omits Node types.
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import type { ClientEvent, TranscriptSnapshot } from "../models/protocol";
import {
  applyTranscriptSnapshot,
  emptyTranscriptProjection,
  isTranscriptSnapshotStale,
  reduceClientEvent,
  selectTranscriptSession
} from "./transcript";

function event(sequence: number, sessionId = "session-a"): ClientEvent {
  return {
    id: `event-${sequence}`,
    sequence,
    timestamp: "2026-07-27T00:00:00Z",
    session_id: sessionId,
    turn_id: "turn-a",
    item_id: "item-a",
    request_id: null,
    type: "model.delta",
    status: "streaming",
    payload_version: 1,
    notification: {
      type: "agent_message_delta",
      item_id: "item-a",
      delta: "hello",
      byte_offset: 0
    },
    payload: { text: "hello" }
  };
}

describe("transcript reducer", () => {
  it("satisfies the shared CLI/Web cursor conformance cases", () => {
    const fixture = JSON.parse(readFileSync(new URL(
      "../../../tests/cases/transcript-reducer-conformance.json",
      import.meta.url
    ), "utf8")) as {
      schema_version: number;
      cases: Array<{
        name: string;
        initial_cursor: number;
        sequence: number;
        session_id: string;
        accepted: boolean;
        visible: boolean;
        expected_cursor: number;
        expected_gap: { expected: number; received: number } | null;
      }>;
      append_cases: Array<{
        name: string;
        initial_cursor: number;
        sequence: number;
        initial_text: string;
        delta: string;
        byte_offset: number;
        accepted: boolean;
        expected_cursor: number;
        expected_text: string;
        expected_append_gap: { expected: number; received: number } | null;
      }>;
    };
    expect(fixture.schema_version).toBe(1);
    for (const testCase of fixture.cases) {
      const initial = {
        ...selectTranscriptSession(emptyTranscriptProjection(), "ses_1"),
        cursor: testCase.initial_cursor
      };
      const reduction = reduceClientEvent(
        initial,
        event(testCase.sequence, testCase.session_id)
      );
      expect(reduction.accepted, testCase.name).toBe(testCase.accepted);
      expect(reduction.visible, testCase.name).toBe(testCase.visible);
      expect(reduction.state.cursor, testCase.name).toBe(testCase.expected_cursor);
      expect(reduction.gap, testCase.name).toEqual(testCase.expected_gap);
    }
    for (const testCase of fixture.append_cases) {
      const initial = {
        ...selectTranscriptSession(emptyTranscriptProjection(), "session-a"),
        cursor: testCase.initial_cursor,
        itemByteLengths: new Map([
          ["item-a", new TextEncoder().encode(testCase.initial_text).byteLength]
        ])
      };
      const delta = event(testCase.sequence);
      if (delta.notification?.type === "agent_message_delta") {
        delta.notification.delta = testCase.delta;
        delta.notification.byte_offset = testCase.byte_offset;
      }
      const reduction = reduceClientEvent(initial, delta);
      expect(reduction.accepted, testCase.name).toBe(testCase.accepted);
      expect(reduction.state.cursor, testCase.name).toBe(testCase.expected_cursor);
      expect(reduction.appendGap && {
        expected: reduction.appendGap.expected,
        received: reduction.appendGap.received
      }, testCase.name).toEqual(testCase.expected_append_gap);
      const text = reduction.accepted
        ? testCase.initial_text + testCase.delta
        : testCase.initial_text;
      expect(text, testCase.name).toBe(testCase.expected_text);
    }
  });

  it("advances the team cursor but hides events from another session", () => {
    const initial = selectTranscriptSession(emptyTranscriptProjection(), "session-a");
    const foreign = reduceClientEvent(initial, event(1, "session-b"));
    expect(foreign.accepted).toBe(true);
    expect(foreign.visible).toBe(false);
    expect(foreign.state.cursor).toBe(1);
    expect(foreign.state.itemRevisions.size).toBe(0);
  });

  it("applies a visible item once and rejects replay", () => {
    const initial = selectTranscriptSession(emptyTranscriptProjection(), "session-a");
    const first = reduceClientEvent(initial, event(3));
    const replay = reduceClientEvent(first.state, event(3));
    expect(first.visible).toBe(true);
    expect(first.state.itemRevisions.get("item-a")).toBe(1);
    expect(replay.accepted).toBe(false);
    expect(replay.state).toBe(first.state);
  });

  it("accepts a global sequence jump in a Team-filtered stream", () => {
    const initial = selectTranscriptSession(emptyTranscriptProjection(), "session-a");
    const first = reduceClientEvent(initial, event(7));
    const jumped = event(9);
    if (jumped.notification?.type === "agent_message_delta") {
      jumped.notification.byte_offset = 5;
    }
    const gap = reduceClientEvent(first.state, jumped);

    expect(gap.accepted).toBe(true);
    expect(gap.gap).toBeNull();
    expect(gap.state.cursor).toBe(9);
  });

  it("rejects a text append offset gap without consuming the event", () => {
    const initial = selectTranscriptSession(emptyTranscriptProjection(), "session-a");
    const first = reduceClientEvent(initial, event(1));
    const next = event(2);
    if (next.notification?.type === "agent_message_delta") {
      next.notification.byte_offset = 4;
    }
    const gap = reduceClientEvent(first.state, next);

    expect(gap.accepted).toBe(false);
    expect(gap.appendGap).toEqual({ itemId: "item-a", expected: 5, received: 4 });
    expect(gap.state.cursor).toBe(1);
  });

  it("restores cursor and item revisions from a snapshot", () => {
    const snapshot = {
      session: { id: "session-a" },
      cursor: 9,
      snapshot_revision: 9,
      items: [{ id: "item-a", revision: 4 }]
    } as unknown as TranscriptSnapshot;
    const restored = applyTranscriptSnapshot(emptyTranscriptProjection(), snapshot);
    expect(restored.activeSessionId).toBe("session-a");
    expect(restored.cursor).toBe(9);
    expect(restored.snapshotRevision).toBe(9);
    expect(restored.itemRevisions.get("item-a")).toBe(4);
  });

  it("does not let a late stale snapshot replace newer event state", () => {
    const selected = selectTranscriptSession(emptyTranscriptProjection(), "session-a");
    const live = reduceClientEvent(selected, event(12)).state;
    const stale = {
      session: { id: "session-a" },
      cursor: 10,
      snapshot_revision: 10,
      items: [{ id: "old-item", revision: 1 }]
    } as unknown as TranscriptSnapshot;

    expect(isTranscriptSnapshotStale(live, stale)).toBe(true);
    expect(applyTranscriptSnapshot(live, stale)).toBe(live);
    expect(live.itemRevisions.has("old-item")).toBe(false);
  });
});
