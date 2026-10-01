"use strict";

const assert = require("node:assert/strict");
const test = require("node:test");
const { ApprovalQueue, approvalSummary } = require("../approvals");

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

test("public web and PDF reads are approved only with the complete target that was shown", async () => {
  const target = `https://www.example.com.${"padding.".repeat(40)}evil.test/docs`;
  for (const tool of ["web_open", "pdf_read"]) {
    const queue = new ApprovalQueue();
    queue.add({ id: "read", tool, summary: "Read public content", target });
    const posted = [];
    const api = { approval: async (id, approved) => posted.push({ id, approved }) };

    await assert.rejects(queue.decide(api, "read", true), /complete target/);
    await assert.rejects(queue.decide(api, "read", true, "https://www.example.com/"), /complete target/);
    assert.deepEqual(posted, []);
    assert.equal(queue.first().id, "read");

    assert.equal((await queue.decide(api, "read", true, target)).id, "read");
    assert.deepEqual(posted, [{ id: "read", approved: true }]);
  }
});

test("a public web or PDF read that names no target can only be rejected", async () => {
  for (const tool of ["web_open", "pdf_read"]) {
    for (const target of [null, undefined, "", "  "]) {
      const queue = new ApprovalQueue();
      queue.add({ id: "read", tool, summary: "Read public content", target });
      const posted = [];
      const api = { approval: async (id, approved) => posted.push({ id, approved }) };

      await assert.rejects(queue.decide(api, "read", true, target), /complete target/);
      assert.deepEqual(posted, []);

      assert.equal((await queue.decide(api, "read", false)).id, "read");
      assert.deepEqual(posted, [{ id: "read", approved: false }]);
    }
  }
});

test("other approvals keep their summary and need no target", async () => {
  assert.equal(approvalSummary("run_command", "Run command", "cargo test"), "Run command · cargo test");
  assert.equal(approvalSummary("run_command", "Run command · cargo test", "cargo test"), "Run command · cargo test");
  assert.equal(approvalSummary("run_command", "Run command", null), "Run command");
  assert.equal(approvalSummary("web_open", "Open public web page", "https://example.com/"), "Open public web page");
  assert.equal(approvalSummary("pdf_read", "Read public PDF", "https://example.com/paper.pdf"), "Read public PDF");

  const queue = new ApprovalQueue();
  queue.add({ id: "command", tool: "run_command", summary: "Run command", target: null });
  const posted = [];
  await queue.decide({ approval: async (id, approved) => posted.push({ id, approved }) }, "command", true);
  assert.deepEqual(posted, [{ id: "command", approved: true }]);
});
