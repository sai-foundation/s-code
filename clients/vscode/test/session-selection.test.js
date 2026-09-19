"use strict";

const assert = require("node:assert/strict");
const test = require("node:test");
const vm = require("node:vm");
const fs = require("node:fs");
const path = require("node:path");
const { createRequire } = require("node:module");

function fixture() {
  const scope = { organization_id: "org", team_id: "team", actor_id: "actor" };
  const folder = { name: "project", uri: { toString: () => "file:///project/" } };
  const state = new Map();
  const calls = { created: [], contexts: [], turns: [] };
  const vscode = {
    workspace: { workspaceFolders: [folder], getWorkspaceFolder: () => folder,
      registerTextDocumentContentProvider: () => ({ dispose() {} }) },
    window: { createStatusBarItem: () => ({ show() {} }), showInputBox: async () => "Explain this", showQuickPick: async (items) => items[0] },
    StatusBarAlignment: { Left: 1 }, languages: { getDiagnostics: () => [] },
  };
  const filename = path.resolve(__dirname, "../extension.js");
  const realRequire = createRequire(filename);
  const sandbox = { require: (name) => name === "vscode" ? vscode : realRequire(name), module: { exports: {} }, setTimeout, clearTimeout, Buffer, AbortController, URL };
  vm.runInNewContext(fs.readFileSync(filename, "utf8"), sandbox, { filename });
  const controller = new sandbox.module.exports.ExtensionController({ subscriptions: [], globalState: { get: (key, fallback) => state.get(key) ?? fallback, update: async (key, value) => state.set(key, value) } });
  const work = (id, extra = {}) => ({ id, title: id, model: "model", mode: "work", workspace_uri: "file:///project/", scope: { ...scope }, ...extra });
  let sessions = [work("work")];
  controller.api = {
    base: () => "http://127.0.0.1:4096", scope: () => ({ ...scope }),
    sessions: async () => sessions,
    config: () => ({ get: () => false }),
    createSession: async (uri, title) => { calls.created.push({ uri, title }); const s = work("new", { workspace_uri: uri }); sessions.push(s); return s; },
    updateContext: async (id, input) => {
      const s = sessions.find((s) => s.id === id);
      assert.equal(s.mode, "work"); assert.equal(s.workspace_uri, input.workspace_uri);
      calls.contexts.push(id);
    },
    startTurn: async (id) => calls.turns.push(id),
    token: async () => "token",
    request: async () => ({ protocol_version: "1.0", capabilities: ["scope.team", "session.persistence", "event.sse_replay", "ide.context.v1"].map((id) => ({ id, version: "1", enabled: true })) }),
  };
  controller.refreshApprovals = async () => {};
  controller.subscribe = () => {};
  return { controller, work, state, calls, vscode, scope, setSessions: (value) => { sessions = value; } };
}

test("newest Web Chat cannot hijack IDE connect or Ask, including cached Chat", async () => {
  const f = fixture();
  f.setSessions([f.work("chat", { mode: "chat", workspace_uri: "" }), f.work("work")]);
  f.state.set("s-code.sessionId", "chat");
  await f.controller.connect(); await f.controller.ask();
  assert.equal(f.state.get("s-code.sessionId"), "work");
  assert.deepEqual(f.calls.turns, ["work"]);
  assert.ok(f.calls.contexts.length >= 2);
  assert.ok(f.calls.contexts.every((id) => id === "work"));
  assert.equal(f.calls.created.length, 0);
});

test("Chat, promoted managed workspaces, deleted IDs and wrong accounts fall back to new project Work", async () => {
  for (const saved of ["chat", "managed", "deleted", "other-account"]) {
    const f = fixture();
    f.setSessions([f.work("chat", { mode: "chat", workspace_uri: "" }), f.work("managed", { workspace_uri: "file:///managed/task/" }), f.work("other-account", { scope: { ...f.scope, actor_id: "someone-else" } })]);
    f.state.set("s-code.sessionId", saved);
    await f.controller.ask();
    assert.deepEqual(f.calls.created, [{ uri: "file:///project/", title: "Work in project" }]);
    assert.deepEqual(f.calls.contexts, ["new"]);
    assert.deepEqual(f.calls.turns, ["new"]);
  }
});

test("stored matching session is retained; legacy sessions without mode remain compatible", async () => {
  const f = fixture();
  f.setSessions([f.work("newer"), f.work("saved", { mode: undefined })]);
  f.state.set("s-code.sessionId", "saved");
  assert.equal(await f.controller.currentSession(), "saved");
  assert.equal(f.calls.created.length, 0);
});

test("picker labels Work and excludes Chat and unrelated projects", async () => {
  const f = fixture();
  f.setSessions([f.work("chat", { mode: "chat" }), f.work("elsewhere", { workspace_uri: "file:///other/" }), f.work("work")]);
  let labels;
  f.vscode.window.showQuickPick = async (items) => { labels = items; return undefined; };
  await f.controller.selectSession();
  assert.deepEqual(Array.from(labels, (i) => i.label), ["work", "$(add) New Work session"]);
  assert.equal(labels[0].description, "Work · model");
  assert.equal(labels[0].detail, "file:///project/");
  assert.equal(f.calls.contexts.length, 0);
});

test("no folder does not create a workspace or attach editor context", async () => {
  const f = fixture(); f.vscode.workspace.workspaceFolders = [];
  assert.equal(await f.controller.currentSession(false), undefined);
  await assert.rejects(() => f.controller.ask(), /Open a workspace/);
  await f.controller.sendContext(); assert.equal(f.calls.contexts.length, 0);
});

test("active multi-root folder determines session and context together", async () => {
  const f = fixture();
  f.vscode.window.activeTextEditor = { document: { uri: { toString: () => "file:///second/code.rs" }, languageId: "rust", version: 1 } };
  f.vscode.workspace.getWorkspaceFolder = () => ({ name: "second", uri: { toString: () => "file:///second/" } });
  f.setSessions([f.work("first"), f.work("second", { workspace_uri: "file:///second/" })]);
  f.state.set("s-code.sessionId", "first");
  await f.controller.ask();
  assert.deepEqual(f.calls.contexts, ["second"]); assert.deepEqual(f.calls.turns, ["second"]);
});

test("account change while loading sessions discards the old result", async () => {
  const f = fixture();
  f.controller.api.sessions = async () => { f.scope.actor_id = "changed"; return [f.work("old")]; };
  assert.equal(await f.controller.currentSession(), undefined);
  assert.equal(f.calls.created.length, 0);
});

test("project change while prompt is open sends neither context nor turn", async () => {
  const f = fixture();
  f.vscode.window.showInputBox = async () => { f.vscode.workspace.workspaceFolders = []; return "late prompt"; };
  await f.controller.ask();
  assert.equal(f.calls.contexts.length, 0); assert.equal(f.calls.turns.length, 0);
});


test("equivalent directory URIs use the session's exact URI for backend context checks", async () => {
  const f = fixture();
  f.setSessions([f.work("work", { workspace_uri: "file:///project" })]);
  await f.controller.ask();
  assert.deepEqual(f.calls.contexts, ["work"]);
  assert.equal(f.calls.created.length, 0);
});

test("automatic session switching restores only new-session approvals and starts its stream", async () => {
  const f = fixture(); let subscriptions = 0;
  f.controller.refreshApprovals = Object.getPrototypeOf(f.controller).refreshApprovals;
  f.controller.subscribe = () => { subscriptions++; };
  f.controller.api.snapshot = async (id) => ({ pending_requests: [{ id: `approval-${id}`, tool: "apply_patch", summary: id }], snapshot_revision: 1 });
  f.controller.approvals.replace([{ id: "old-approval" }]);
  await f.controller.currentSession();
  assert.equal(f.controller.approvals.first().id, "approval-work");
  assert.equal(subscriptions, 1);
  f.setSessions([f.work("next")]);
  await f.controller.currentSession();
  assert.equal(f.controller.approvals.first().id, "approval-next");
  assert.equal(subscriptions, 2);
});

test("late approval snapshot cannot restore previous-project approvals", async () => {
  const f = fixture();
  f.controller.refreshApprovals = Object.getPrototypeOf(f.controller).refreshApprovals;
  f.controller.api.snapshot = async () => {
    f.vscode.workspace.workspaceFolders = [];
    return { pending_requests: [{ id: "late-approval", summary: "old project" }], snapshot_revision: 1 };
  };
  assert.equal(await f.controller.currentSession(), undefined);
  assert.equal(f.controller.approvals.first(), null);
});

test("changing projects during hunk selection never reads or edits the other project", async () => {
  const f = fixture(); let picks = 0; const tools = [];
  f.controller.api.tool = async (id, tool) => {
    tools.push(tool);
    return { tool_call: { result: { unified_diff: "diff --git a/code.rs b/code.rs\n--- a/code.rs\n+++ b/code.rs\n@@ -1 +1 @@\n-old\n+new\n" } } };
  };
  f.vscode.window.showQuickPick = async (items) => {
    if (++picks === 1) return items[0];
    f.vscode.workspace.workspaceFolders = [{ name: "other", uri: { toString: () => "file:///other/" } }];
    return [items[0]];
  };
  await f.controller.reviewHunks();
  assert.deepEqual(tools, ["git_diff"]);
});
