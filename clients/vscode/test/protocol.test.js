"use strict";

const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { range, ensureDirectoryUri, drainSse, reduceEventCursor, negotiateCapabilities } = require("../protocol");

test("editor ranges preserve zero-based positions", () => {
  assert.deepEqual(
    range({ start: { line: 2, character: 3 }, end: { line: 4, character: 5 } }),
    { start: { line: 2, character: 3 }, end: { line: 4, character: 5 } }
  );
});

test("workspace URI is normalized as a directory", () => {
  assert.equal(ensureDirectoryUri({ toString: () => "file:///repo" }), "file:///repo/");
  assert.equal(ensureDirectoryUri({ toString: () => "file:///repo/" }), "file:///repo/");
});

test("SSE parser preserves an incomplete event for reconnection", () => {
  const parsed = drainSse("id: 4\nevent: model.delta\ndata: {\"payload\":{\"text\":\"hi\"}}\n\nid: 5\n");
  assert.equal(parsed.events.length, 1);
  assert.equal(parsed.events[0].id, 4);
  assert.equal(parsed.events[0].envelope.payload.text, "hi");
  assert.equal(parsed.remainder, "id: 5\n");
});

test("IDE consumes the shared CLI/Web cursor conformance cases", () => {
  const fixture = JSON.parse(fs.readFileSync(path.join(__dirname, "../../../tests/cases/transcript-reducer-conformance.json"), "utf8"));
  assert.equal(fixture.schema_version, 1);
  for (const entry of fixture.cases) {
    const reduction = reduceEventCursor(entry.initial_cursor, entry.sequence);
    assert.equal(reduction.accepted, entry.accepted, entry.name);
    assert.equal(reduction.cursor, entry.expected_cursor, entry.name);
    assert.deepEqual(reduction.gap, entry.expected_gap, entry.name);
  }
});

test("capability negotiation rejects major mismatch and ignores unknown capabilities", () => {
  const manifest = {
    protocol_version: "1.9",
    capabilities: [
      { id: "ide.context.v1", version: "1", enabled: true },
      { id: "future.unknown", version: "99", enabled: true }
    ]
  };
  const enabled = negotiateCapabilities(manifest, "1.0", ["ide.context.v1"]);
  assert.deepEqual([...enabled], ["ide.context.v1"]);
  assert.throws(() => negotiateCapabilities(manifest, "2.0", []), /Incompatible/);
  assert.throws(() => negotiateCapabilities(manifest, "1.0", ["missing"]), /required capability/);
});
