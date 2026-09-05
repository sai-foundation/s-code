"use strict";

const vscode = require("vscode");
const crypto = require("crypto");
const { range, ensureDirectoryUri, drainSse, reduceEventCursor, negotiateCapabilities } = require("./protocol");
const { parseFileHunks, rejectHunks } = require("./hunks");
const { ApprovalQueue } = require("./approvals");
const { validatedDaemonBase, browserBootstrapUrl } = require("./daemon-url");

const IDE_PROTOCOL_VERSION = "1.0";
const TOKEN_KEY = "s-code.daemonToken";
const SESSION_KEY = "s-code.sessionId";
const CLIENT_KEY = "s-code.clientInstanceId";
const EVENT_CURSORS_KEY = "s-code.eventCursors";

class DaemonApi {
  constructor(context) { this.context = context; }
  config() { return vscode.workspace.getConfiguration("s-code"); }
  base() { return validatedDaemonBase(this.config().get("daemonUrl")); }
  scope() { return { organization_id: this.config().get("organizationId"), team_id: this.config().get("teamId"), actor_id: this.config().get("actorId"), goal_id: null, task_id: null }; }
  async token(prompt = false) {
    let token = await this.context.secrets.get(TOKEN_KEY);
    if (!token && prompt) { token = await vscode.window.showInputBox({ title: "S-Code daemon token", password: true, ignoreFocusOut: true }); if (token) await this.context.secrets.store(TOKEN_KEY, token); }
    if (!token) throw new Error("No daemon token. Run “S-Code: Connect to Daemon”.");
    return token;
  }
  async request(path, init = {}) {
    const token = await this.token(false);
    const response = await fetch(`${this.base()}${path}`, { ...init, headers: { authorization: `Bearer ${token}`, "x-s-code-csrf": "1", ...(init.body ? { "content-type": "application/json" } : {}), ...(init.headers || {}) } });
    if (!response.ok) { const body = await response.text(); throw new Error(`Daemon ${response.status}: ${body}`); }
    return response.json();
  }
  sessions() { const s = this.scope(); const query = new URLSearchParams({ organization_id: s.organization_id, team_id: s.team_id, actor_id: s.actor_id }); return this.request(`/v1/sessions?${query}`); }
  snapshot(sessionId) { const s = this.scope(); const query = new URLSearchParams({ organization_id: s.organization_id, team_id: s.team_id, actor_id: s.actor_id, limit: "1" }); return this.request(`/v1/sessions/${encodeURIComponent(sessionId)}/snapshot?${query}`); }
  createSession(workspaceUri, title) { return this.request("/v1/sessions", { method: "POST", body: JSON.stringify({ scope: this.scope(), workspace_uri: workspaceUri, title, model: this.config().get("defaultModel") }) }); }
  updateContext(sessionId, editorContext) { return this.request(`/v1/sessions/${encodeURIComponent(sessionId)}/editor-context`, { method: "POST", body: JSON.stringify(editorContext) }); }
  startTurn(sessionId, content) { return this.request(`/v1/sessions/${encodeURIComponent(sessionId)}/turns`, { method: "POST", body: JSON.stringify({ scope: this.scope(), content }) }); }
  tool(sessionId, tool, args) { return this.request(`/v1/sessions/${encodeURIComponent(sessionId)}/tools`, { method: "POST", body: JSON.stringify({ scope: this.scope(), tool, arguments: args }) }); }
  approval(id, approved) { return this.request(`/v1/approvals/${encodeURIComponent(id)}`, { method: "POST", body: JSON.stringify({ scope: this.scope(), approved, approval_scope: "once" }) }); }
  async browserBootstrap() { const issued = await this.request("/v1/auth/browser-bootstrap", { method: "POST" }); return browserBootstrapUrl(this.base(), issued.token); }
}

class OriginalContentProvider {
  constructor() { this.documents = new Map(); }
  provideTextDocumentContent(uri) { return this.documents.get(uri.query) || ""; }
  put(content) { const key = crypto.randomUUID(); this.documents.set(key, content); return vscode.Uri.parse(`s-code-base:/original?${key}`); }
}

class ExtensionController {
  constructor(context) {
    this.context = context; this.api = new DaemonApi(context); this.approvals = new ApprovalQueue(); this.abort = null; this.timer = null; this.capabilities = new Set();
    this.provider = new OriginalContentProvider();
    this.status = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 50); this.status.command = "s-code.selectSession"; this.status.text = "$(hubot) S-Code: disconnected"; this.status.show();
    context.subscriptions.push(this.status, vscode.workspace.registerTextDocumentContentProvider("s-code-base", this.provider));
  }
  async connect() {
    const token = await this.api.token(true); if (!token) return;
    const manifest = await this.api.request("/v1/capabilities");
    this.capabilities = negotiateCapabilities(manifest, IDE_PROTOCOL_VERSION, ["scope.team", "session.persistence", "event.sse_replay", "ide.context.v1"]);
    const existing = await this.currentSession(false);
    if (existing) { this.status.text = "$(hubot) S-Code: connected"; await this.sendContext(); await this.refreshApprovals(existing); this.subscribe(); }
    else await this.selectSession();
  }
  async currentSession(required = true) {
    const id = this.context.globalState.get(SESSION_KEY); if (id) return id;
    if (required) await this.selectSession(); return this.context.globalState.get(SESSION_KEY);
  }
  async selectSession() {
    const sessions = await this.api.sessions();
    const picked = await vscode.window.showQuickPick(sessions.map((session) => ({ label: session.title, description: session.model, detail: session.workspace_uri, session })), { title: "Select Team session" });
    if (!picked) return; await this.context.globalState.update(SESSION_KEY, picked.session.id); this.status.text = `$(hubot) ${picked.session.title}`; await this.sendContext(); await this.refreshApprovals(picked.session.id); this.subscribe();
  }
  async createSession() {
    const folder = vscode.workspace.workspaceFolders?.[0]; if (!folder) throw new Error("Open a workspace folder first.");
    const title = await vscode.window.showInputBox({ title: "Team session title", value: `Work in ${folder.name}` }); if (!title) return;
    const session = await this.api.createSession(ensureDirectoryUri(folder.uri), title); await this.context.globalState.update(SESSION_KEY, session.id); this.status.text = `$(hubot) ${session.title}`; await this.refreshApprovals(session.id); this.subscribe(); await this.sendContext();
  }
  async sendContext() {
    const sessionId = await this.currentSession(false); const editor = vscode.window.activeTextEditor; const folder = editor ? vscode.workspace.getWorkspaceFolder(editor.document.uri) : vscode.workspace.workspaceFolders?.[0];
    if (!sessionId || !folder) return;
    let clientId = this.context.globalState.get(CLIENT_KEY); if (!clientId) { clientId = `ide_${crypto.randomUUID()}`; await this.context.globalState.update(CLIENT_KEY, clientId); }
    const document = editor?.document; const selection = editor?.selection; const maxBytes = this.api.config().get("maxDocumentBytes");
    let text = null;
    if (document && this.api.config().get("includeActiveDocument")) { const candidate = document.getText(); if (Buffer.byteLength(candidate, "utf8") <= maxBytes) text = candidate; }
    const diagnostics = document ? vscode.languages.getDiagnostics(document.uri).slice(0, 500).map((item) => ({ range: range(item.range), severity: ["error", "warning", "information", "hint"][item.severity] || "unknown", message: item.message.slice(0, 8192), source: item.source || null, code: typeof item.code === "object" ? String(item.code.value) : item.code == null ? null : String(item.code) })) : [];
    await this.api.updateContext(sessionId, { scope: this.api.scope(), protocol_version: IDE_PROTOCOL_VERSION, client_instance_id: clientId, workspace_uri: ensureDirectoryUri(folder.uri), active_document: document ? { uri: document.uri.toString(), language_id: document.languageId, version: document.version, text } : null, selection: editor && selection && !selection.isEmpty ? { range: range(selection), text: document.getText(selection).slice(0, 131072) } : null, diagnostics });
  }
  scheduleContext() { clearTimeout(this.timer); this.timer = setTimeout(() => this.sendContext().catch(showError), 350); }
  async ask() { const sessionId = await this.currentSession(); if (!sessionId) return; await this.sendContext(); const prompt = await vscode.window.showInputBox({ title: "Ask S-Code Agent", prompt: "Current selection and diagnostics will be attached with provenance", ignoreFocusOut: true }); if (!prompt) return; await this.api.startTurn(sessionId, prompt); this.status.text = "$(sync~spin) S-Code: running"; }
  async openWeb() { await vscode.env.openExternal(vscode.Uri.parse(await this.api.browserBootstrap())); }
  async decide(approved) { const pending = this.approvals.first(); if (!pending) { vscode.window.showInformationMessage("No pending S-Code approval."); return; } await this.decideApproval(pending.id, approved); }
  async decideApproval(id, approved) { const pending = await this.approvals.decide(this.api, id, approved); if (!pending) { vscode.window.showInformationMessage("That S-Code approval is no longer pending."); return; } vscode.window.showInformationMessage(`${approved ? "Approved" : "Rejected"} ${pending.summary}.`); }
  async refreshApprovals(sessionId) { const snapshot = await this.api.snapshot(sessionId); this.approvals.replace((snapshot.pending_requests || []).map((request) => ({ id: request.id, toolCallId: null, tool: request.tool, summary: request.target && !request.summary.includes(request.target) ? `${request.summary} · ${request.target}` : request.summary, target: request.target || null }))); const scope = this.api.scope(); const key = `${scope.organization_id}\u0000${scope.team_id}\u0000${scope.actor_id}`; const cursors = this.context.globalState.get(EVENT_CURSORS_KEY, {}); await this.context.globalState.update(EVENT_CURSORS_KEY, { ...cursors, [key]: snapshot.snapshot_revision }); const pending = this.approvals.first(); if (pending) this.status.text = `$(shield) Approval: ${pending.summary}`; }
  async reviewDiff() {
    const sessionId = await this.currentSession(); if (!sessionId) return;
    const outcome = await this.api.tool(sessionId, "git_diff", { paths: [], max_bytes: 1048576 }); const result = outcome.tool_call?.result; const unified = result?.unified_diff || "";
    if (!unified) { vscode.window.showInformationMessage("No working-tree diff."); return; }
    const files = [...unified.matchAll(/^diff --git a\/(.+?) b\/(.+)$/gm)].map((match) => match[2]);
    const selected = await vscode.window.showQuickPick(files, { title: "Review changed file inline" }); if (!selected) return;
    const folder = vscode.workspace.workspaceFolders?.[0]; if (!folder) return; const current = vscode.Uri.joinPath(folder.uri, selected);
    const gitExtension = vscode.extensions.getExtension("vscode.git"); if (!gitExtension) throw new Error("Built-in Git extension is unavailable."); if (!gitExtension.isActive) await gitExtension.activate();
    const repository = gitExtension.exports.getAPI(1).repositories.find((repo) => current.fsPath.startsWith(repo.rootUri.fsPath)); if (!repository) throw new Error("No Git repository owns the selected file.");
    const original = await repository.show("HEAD", selected); const base = this.provider.put(original);
    await vscode.commands.executeCommand("vscode.diff", base, current, `S-Code Review: ${selected}`, { preview: false });
  }
  async reviewHunks() {
    const sessionId = await this.currentSession(); if (!sessionId) return;
    const outcome = await this.api.tool(sessionId, "git_diff", { paths: [], max_bytes: 1048576 }); const result = outcome.tool_call?.result; const unified = result?.unified_diff || "";
    if (!unified || result?.truncated) { vscode.window.showWarningMessage(result?.truncated ? "Diff is truncated; hunk mutation is disabled." : "No working-tree diff."); return; }
    const files = [...unified.matchAll(/^diff --git a\/(.+?) b\/(.+)$/gm)].map((match) => match[2]);
    const selected = await vscode.window.showQuickPick(files, { title: "Select a changed file for hunk review" }); if (!selected) return;
    const hunks = parseFileHunks(unified, selected);
    const rejected = await vscode.window.showQuickPick(hunks.map((hunk) => ({ label: hunk.header, description: hunk.lines.filter((line) => line.startsWith("+") || line.startsWith("-")).slice(0, 3).join("  "), hunk })), { title: "Select hunks to reject (unselected hunks remain)", canPickMany: true, ignoreFocusOut: true });
    if (!rejected?.length) { vscode.window.showInformationMessage("All reviewed hunks remain in the working tree."); return; }
    const folder = vscode.workspace.workspaceFolders?.[0]; if (!folder) throw new Error("Open a workspace folder first.");
    const uri = vscode.Uri.joinPath(folder.uri, selected); const bytes = await vscode.workspace.fs.readFile(uri); const current = new TextDecoder("utf-8", { fatal: true }).decode(bytes); const replacement = rejectHunks(current, rejected.map((item) => item.hunk));
    const expected = crypto.createHash("sha256").update(bytes).digest("hex");
    const proposed = await this.api.tool(sessionId, "apply_patch", { path: selected, expected_sha256: expected, content: replacement });
    if (proposed.outcome === "awaiting_approval") vscode.window.showInformationMessage(`Approval required to reject ${rejected.length} hunk(s).`); else vscode.window.showInformationMessage(`Rejected ${rejected.length} hunk(s).`);
  }
  subscribe() {
    if (this.abort) this.abort.abort();
    this.abort = new AbortController();
    const signal = this.abort.signal;
    let after = 0;
    let cursorScope = "";
    const run = async () => {
      while (!signal.aborted) {
        try {
          const token = await this.api.token(false);
          const scope = this.api.scope();
          const nextScope = `${scope.organization_id}\u0000${scope.team_id}\u0000${scope.actor_id}`;
          if (nextScope !== cursorScope) {
            const cursors = this.context.globalState.get(EVENT_CURSORS_KEY, {});
            after = Number.isSafeInteger(cursors[nextScope]) ? cursors[nextScope] : 0;
            cursorScope = nextScope;
          }
          const query = new URLSearchParams({ organization_id: scope.organization_id, team_id: scope.team_id, actor_id: scope.actor_id, after: String(after) });
          const response = await fetch(`${this.api.base()}/v1/events?${query}`, { headers: { authorization: `Bearer ${token}` }, signal });
          if (!response.ok || !response.body) throw new Error(`events ${response.status}`);
          const reader = response.body.getReader();
          const decoder = new TextDecoder();
          let buffer = "";
          while (!signal.aborted) {
            const { value, done } = await reader.read();
            if (done) break;
            buffer += decoder.decode(value, { stream: true });
            const parsed = drainSse(buffer);
            buffer = parsed.remainder;
            for (const event of parsed.events) {
              const reduction = reduceEventCursor(after, event.id);
              if (reduction.gap) throw new Error(`event sequence gap: expected ${reduction.gap.expected}, received ${reduction.gap.received}`);
              if (!reduction.accepted) continue;
              this.handleEvent(event.kind, event.envelope);
              after = reduction.cursor;
              const cursors = this.context.globalState.get(EVENT_CURSORS_KEY, {});
              await this.context.globalState.update(EVENT_CURSORS_KEY, { ...cursors, [cursorScope]: after });
            }
          }
        } catch (error) {
          if (!signal.aborted) await new Promise((resolve) => setTimeout(resolve, 1000));
        }
      }
    };
    run();
  }
  handleEvent(kind, envelope) { const sessionId = this.context.globalState.get(SESSION_KEY); if (envelope.session_id && envelope.session_id !== sessionId) return; const payload = envelope.payload || {}; if (kind === "approval.required" && payload.approval_id) { const projection = payload.approval_request || {}; const summary = projection.summary || payload.display || payload.tool || "tool action"; const target = projection.target || null; const visible = target && !summary.includes(target) ? `${summary} · ${target}` : summary; this.approvals.add({ id: payload.approval_id, toolCallId: payload.tool_call_id, tool: payload.tool, summary: visible, target }); this.status.text = `$(shield) Approval: ${visible}`; vscode.window.showInformationMessage(`S-Code requests: ${visible}`, "Approve once", "Reject").then((answer) => { if (answer) this.decideApproval(payload.approval_id, answer === "Approve once").catch(showError); }); } else if (kind === "approval.resolved") { this.approvals.removeById(payload.approval_id); } else if (["tool.completed", "tool.failed", "tool.denied"].includes(kind)) { this.approvals.removeByToolCall(payload.tool_call_id); } else if (kind === "turn.completed") { this.status.text = "$(check) S-Code: completed"; } else if (kind === "turn.failed") { this.status.text = "$(error) S-Code: failed"; } }
  dispose() { if (this.abort) this.abort.abort(); clearTimeout(this.timer); }
}

function showError(error) { vscode.window.showErrorMessage(`S-Code: ${error.message || error}`); }

function activate(context) {
  const controller = new ExtensionController(context); context.subscriptions.push(controller);
  const command = (name, action) => context.subscriptions.push(vscode.commands.registerCommand(name, () => action.call(controller).catch(showError)));
  command("s-code.connect", controller.connect); command("s-code.createSession", controller.createSession); command("s-code.selectSession", controller.selectSession); command("s-code.ask", controller.ask); command("s-code.sendContext", controller.sendContext); command("s-code.reviewDiff", controller.reviewDiff); command("s-code.reviewHunks", controller.reviewHunks); command("s-code.approve", () => controller.decide(true)); command("s-code.reject", () => controller.decide(false));
  command("s-code.openWeb", controller.openWeb);
  context.subscriptions.push(vscode.window.onDidChangeActiveTextEditor(() => controller.scheduleContext()), vscode.window.onDidChangeTextEditorSelection(() => controller.scheduleContext()), vscode.languages.onDidChangeDiagnostics(() => controller.scheduleContext()), vscode.workspace.onDidSaveTextDocument(() => controller.scheduleContext()));
  return controller;
}

function deactivate() {}
module.exports = { activate, deactivate, DaemonApi, ExtensionController };
