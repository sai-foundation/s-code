"use strict";

function range(value) {
  return {
    start: { line: value.start.line, character: value.start.character },
    end: { line: value.end.line, character: value.end.character }
  };
}

function ensureDirectoryUri(uri) {
  const value = uri.toString();
  return value.endsWith("/") ? value : `${value}/`;
}

function drainSse(buffer) {
  const events = [];
  let boundary;
  while ((boundary = buffer.indexOf("\n\n")) >= 0) {
    const block = buffer.slice(0, boundary);
    buffer = buffer.slice(boundary + 2);
    let id = 0;
    let kind = "message";
    let data = "";
    for (const line of block.split("\n")) {
      if (line.startsWith("id:")) id = Number(line.slice(3).trim());
      if (line.startsWith("event:")) kind = line.slice(6).trim();
      if (line.startsWith("data:")) data += line.slice(5).trim();
    }
    if (data) events.push({ id, kind, envelope: JSON.parse(data) });
  }
  return { events, remainder: buffer };
}

function reduceEventCursor(cursor, sequence) {
  if (!Number.isSafeInteger(cursor) || cursor < 0 || !Number.isSafeInteger(sequence) || sequence <= 0) {
    throw new Error("Event cursors must be non-negative safe integers and sequences must be positive.");
  }
  if (sequence <= cursor) return { accepted: false, cursor, gap: null };
  // Event IDs are global, but this stream is Team-filtered. Forward jumps are
  // valid when another Team owns the intervening event. The daemon terminates
  // a genuinely lagged stream so reconnect can replay from this global cursor.
  return { accepted: true, cursor: sequence, gap: null };
}

function major(version) {
  const value = Number.parseInt(String(version).split(".")[0], 10);
  return Number.isSafeInteger(value) && value >= 0 ? value : null;
}

function negotiateCapabilities(manifest, clientProtocol, required = []) {
  if (!manifest || major(manifest.protocol_version) !== major(clientProtocol)) {
    throw new Error(`Incompatible daemon protocol ${manifest?.protocol_version || "unknown"}; client supports ${clientProtocol}.`);
  }
  const enabled = new Set();
  for (const capability of Array.isArray(manifest.capabilities) ? manifest.capabilities : []) {
    if (capability?.enabled && major(capability.version) === 1 && typeof capability.id === "string") enabled.add(capability.id);
  }
  for (const id of required) if (!enabled.has(id)) throw new Error(`Daemon does not advertise required capability ${id} v1.`);
  return enabled;
}

module.exports = { range, ensureDirectoryUri, drainSse, reduceEventCursor, negotiateCapabilities };
