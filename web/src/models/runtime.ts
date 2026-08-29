import type {
  ClientEvent,
  TranscriptItem,
  TranscriptSnapshot
} from "./protocol";

type JsonRecord = Record<string, unknown>;

function record(value: unknown, label: string): JsonRecord {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error(`${label} must be an object`);
  }
  return value as JsonRecord;
}

function string(value: unknown, label: string): string {
  if (typeof value !== "string" || value.length === 0) {
    throw new Error(`${label} must be a non-empty string`);
  }
  return value;
}

function optionalString(value: unknown, label: string): string | null {
  if (value == null) return null;
  return string(value, label);
}

function integer(value: unknown, label: string): number {
  if (typeof value !== "number" || !Number.isSafeInteger(value) || value < 0) {
    throw new Error(`${label} must be a non-negative integer`);
  }
  return value;
}

function finiteNonNegativeNumber(value: unknown, label: string): number {
  if (typeof value !== "number" || !Number.isFinite(value) || value < 0) {
    throw new Error(`${label} must be a finite non-negative number`);
  }
  return value;
}

function validateMcpProgress(value: unknown, label: string): void {
  const progress = record(value, label);
  const completed = finiteNonNegativeNumber(progress.progress, `${label}.progress`);
  const total = progress.total == null
    ? null
    : finiteNonNegativeNumber(progress.total, `${label}.total`);
  if (total != null && completed > total) {
    throw new Error(`${label}.progress cannot exceed total`);
  }
  if (progress.message != null && typeof progress.message !== "string") {
    throw new Error(`${label}.message must be a string or null`);
  }
}

export function parseClientEvent(value: unknown): ClientEvent {
  const event = record(value, "client event");
  string(event.id, "client event.id");
  integer(event.sequence, "client event.sequence");
  string(event.timestamp, "client event.timestamp");
  string(event.type, "client event.type");
  integer(event.payload_version, "client event.payload_version");
  optionalString(event.session_id, "client event.session_id");
  optionalString(event.turn_id, "client event.turn_id");
  optionalString(event.item_id, "client event.item_id");
  optionalString(event.request_id, "client event.request_id");
  record(event.payload, "client event.payload");
  if (event.notification != null) {
    const notification = record(event.notification, "client event.notification");
    const type = string(notification.type, "client event.notification.type");
    if (type === "mcp_progress_changed") {
      validateMcpProgress(notification, "client event.notification");
      string(notification.item_id, "client event.notification.item_id");
      string(notification.server, "client event.notification.server");
      string(notification.tool, "client event.notification.tool");
    }
    if (type === "reasoning_summary_delta") {
      string(notification.item_id, "client event.notification.item_id");
      if (typeof notification.delta !== "string") {
        throw new Error("client event.notification.delta must be a string");
      }
    }
  }
  return value as ClientEvent;
}

function parseTranscriptItem(value: unknown, sessionId: string): TranscriptItem {
  const item = record(value, "transcript item");
  string(item.id, "transcript item.id");
  if (string(item.session_id, "transcript item.session_id") !== sessionId) {
    throw new Error("transcript item belongs to a different session");
  }
  string(item.turn_id, "transcript item.turn_id");
  string(item.kind, "transcript item.kind");
  string(item.status, "transcript item.status");
  integer(item.revision, "transcript item.revision");
  const content = record(item.content, "transcript item.content");
  if (content.type === "mcp_call" && content.progress != null) {
    validateMcpProgress(content.progress, "transcript item.content.progress");
  }
  return value as TranscriptItem;
}

export function parseTranscriptSnapshot(value: unknown): TranscriptSnapshot {
  const snapshot = record(value, "transcript snapshot");
  string(snapshot.protocol_version, "transcript snapshot.protocol_version");
  integer(snapshot.snapshot_revision, "transcript snapshot.snapshot_revision");
  integer(snapshot.cursor, "transcript snapshot.cursor");
  const session = record(snapshot.session, "transcript snapshot.session");
  const sessionId = string(session.id, "transcript snapshot.session.id");
  if (!Array.isArray(snapshot.turns)) {
    throw new Error("transcript snapshot.turns must be an array");
  }
  const turnIds = new Set(snapshot.turns.map((value) => {
    const turn = record(value, "transcript turn");
    if (string(turn.session_id, "transcript turn.session_id") !== sessionId) {
      throw new Error("transcript turn belongs to a different session");
    }
    return string(turn.id, "transcript turn.id");
  }));
  if (!Array.isArray(snapshot.items)) {
    throw new Error("transcript snapshot.items must be an array");
  }
  const itemIds = new Set<string>();
  for (const value of snapshot.items) {
    const item = parseTranscriptItem(value, sessionId);
    if (!turnIds.has(item.turn_id)) {
      throw new Error("transcript item references an unavailable turn");
    }
    if (itemIds.has(item.id)) {
      throw new Error("transcript snapshot contains a duplicate item");
    }
    itemIds.add(item.id);
  }
  if (
    !Array.isArray(snapshot.pending_requests)
    || !Array.isArray(snapshot.pending_questions)
    || !Array.isArray(snapshot.pending_inputs)
    || !Array.isArray(snapshot.attachments)
    || !Array.isArray(snapshot.artifacts)
  ) {
    throw new Error("transcript snapshot request, question, input, attachment, and artifact lists must be arrays");
  }
  const itemCount = snapshot.item_count == null
    ? snapshot.items.length
    : integer(snapshot.item_count, "transcript snapshot.item_count");
  const nextCursor = optionalString(snapshot.next_cursor, "transcript snapshot.next_cursor");
  const usage = snapshot.usage == null
    ? {
      input_tokens: 0,
      output_tokens: 0,
      total_tokens: 0,
      model_calls: 0,
      tool_calls: 0,
      turns: 0,
    }
    : record(snapshot.usage, "transcript snapshot.usage");
  for (const field of [
    "input_tokens",
    "output_tokens",
    "total_tokens",
    "model_calls",
    "tool_calls",
    "turns",
  ]) {
    integer(usage[field], `transcript snapshot.usage.${field}`);
  }
  if (usage.total_tokens !== Number(usage.input_tokens) + Number(usage.output_tokens)) {
    throw new Error("transcript snapshot.usage total does not match input and output tokens");
  }
  return {
    ...(value as TranscriptSnapshot),
    item_count: itemCount,
    next_cursor: nextCursor,
    usage: usage as TranscriptSnapshot["usage"],
  };
}
