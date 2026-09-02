"use strict";

const assert = require("node:assert/strict");
const test = require("node:test");
const { ApprovalQueue } = require("../approvals");

test("out-of-order approval clicks post the exact card id", async () => {
  const queue = new ApprovalQueue();
  queue.add({ id: "approval-a", toolCallId: "call-a", summary: "Edit a.txt" });
  queue.add({ id: "approval-b", toolCallId: "call-b", summary: "Run tests" });
  const posted = [];
  const api = { approval: async (id, approved) => posted.push({ id, approved }) };

  const second = await queue.decide(api, "approval-b", true);
  const first = await queue.decide(api, "approval-a", false);

  assert.equal(second.id, "approval-b");
  assert.equal(first.id, "approval-a");
  assert.deepEqual(posted, [
    { id: "approval-b", approved: true },
    { id: "approval-a", approved: false },
  ]);
  assert.equal(queue.first(), null);
});

test("resolved tool outcomes remove replayed approval requests", () => {
  const queue = new ApprovalQueue();
  queue.add({ id: "approval-a", toolCallId: "call-a", summary: "Edit a.txt" });
  queue.removeByToolCall("call-a");
  assert.equal(queue.first(), null);
});

test("snapshot rebuild restores a pending approval after extension reconnect", async () => {
  const queue = new ApprovalQueue();
  queue.add({ id: "stale", summary: "stale request" });
  queue.replace([{ id: "approval-from-snapshot", summary: "Run tests", tool: "run_command" }]);
  const posted = [];

  await queue.decide({ approval: async (id) => posted.push(id) }, "approval-from-snapshot", true);

  assert.deepEqual(posted, ["approval-from-snapshot"]);
  assert.equal(queue.first(), null);
});
