import type { ClientEvent, TranscriptSnapshot } from "../models/protocol";

export interface TranscriptProjection {
  activeSessionId: string | null;
  cursor: number;
  snapshotRevision: number;
  itemRevisions: ReadonlyMap<string, number>;
  itemByteLengths: ReadonlyMap<string, number>;
}

export interface EventReduction {
  state: TranscriptProjection;
  accepted: boolean;
  visible: boolean;
  gap: { expected: number; received: number } | null;
  appendGap: { itemId: string; expected: number; received: number } | null;
}

export function emptyTranscriptProjection(): TranscriptProjection {
  return {
    activeSessionId: null,
    cursor: 0,
    snapshotRevision: 0,
    itemRevisions: new Map(),
    itemByteLengths: new Map()
  };
}

export function selectTranscriptSession(
  state: TranscriptProjection,
  sessionId: string | null
): TranscriptProjection {
  if (state.activeSessionId === sessionId) return state;
  return {
    activeSessionId: sessionId,
    cursor: state.cursor,
    snapshotRevision: state.snapshotRevision,
    itemRevisions: new Map(),
    itemByteLengths: new Map()
  };
}

export function isTranscriptSnapshotStale(
  state: TranscriptProjection,
  snapshot: TranscriptSnapshot
): boolean {
  return snapshot.snapshot_revision < state.snapshotRevision;
}

export function applyTranscriptSnapshot(
  state: TranscriptProjection,
  snapshot: TranscriptSnapshot
): TranscriptProjection {
  if (isTranscriptSnapshotStale(state, snapshot)) return state;
  return {
    activeSessionId: snapshot.session.id,
    cursor: Math.max(state.cursor, snapshot.cursor),
    snapshotRevision: Math.max(state.snapshotRevision, snapshot.snapshot_revision),
    itemRevisions: new Map(
      snapshot.items.map((item) => [item.id, item.revision] as const)
    ),
    itemByteLengths: new Map(snapshot.items.flatMap((item) => {
      if (
        !item.content
        || item.content.type !== "message"
        || item.content.role !== "assistant"
        || typeof item.content.content !== "string"
      ) return [];
      return [[item.id, new TextEncoder().encode(item.content.content).byteLength] as const];
    }))
  };
}

export function reduceClientEvent(
  state: TranscriptProjection,
  event: ClientEvent
): EventReduction {
  if (event.sequence <= state.cursor) {
    return { state, accepted: false, visible: false, gap: null, appendGap: null };
  }
  if (state.cursor > 0 && event.sequence !== state.cursor + 1) {
    return {
      state,
      accepted: false,
      visible: false,
      gap: { expected: state.cursor + 1, received: event.sequence },
      appendGap: null
    };
  }
  const visible = event.session_id == null
    || event.session_id === state.activeSessionId;
  const byteLengths = new Map(state.itemByteLengths);
  if (visible && event.notification?.type === "agent_message_delta") {
    const itemId = event.notification.item_id;
    const expected = byteLengths.get(itemId) || 0;
    const received = event.notification.byte_offset;
    if (received != null && received !== expected) {
      return {
        state,
        accepted: false,
        visible: false,
        gap: null,
        appendGap: { itemId, expected, received }
      };
    }
    byteLengths.set(
      itemId,
      expected + new TextEncoder().encode(event.notification.delta).byteLength
    );
  }
  const revisions = new Map(state.itemRevisions);
  if (visible && event.item_id) {
    revisions.set(event.item_id, (revisions.get(event.item_id) || 0) + 1);
  }
  return {
    state: {
      activeSessionId: state.activeSessionId,
      cursor: event.sequence,
      snapshotRevision: Math.max(state.snapshotRevision, event.sequence),
      itemRevisions: revisions,
      itemByteLengths: byteLengths
    },
    accepted: true,
    visible,
    gap: null,
    appendGap: null
  };
}
