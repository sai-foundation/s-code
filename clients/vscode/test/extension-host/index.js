"use strict";

const assert = require("node:assert/strict");
const http = require("node:http");
const vscode = require("vscode");

const TOKEN_KEY = "s-code.daemonToken";
const SESSION_KEY = "s-code.sessionId";

function json(response, status, value) {
  response.writeHead(status, { "content-type": "application/json" });
  response.end(JSON.stringify(value));
}

async function requestBody(request) {
  const chunks = [];
  for await (const chunk of request) chunks.push(chunk);
  return JSON.parse(Buffer.concat(chunks).toString("utf8"));
}

async function run() {
  let editorContext;
  const streams = new Set();
  const server = http.createServer(async (request, response) => {
    try {
      assert.equal(request.headers.authorization, "Bearer extension-host-secret");
      if (request.url === "/v1/capabilities") {
        return json(response, 200, {
          protocol_version: "1.0",
          server_version: "fixture",
          capabilities: ["scope.team", "session.persistence", "event.sse_replay", "ide.context.v1"].map((id) => ({ id, version: "1", maturity: "stable", enabled: true, attributes: {} })),
          contracts: [],
        });
      }
      if (request.url === "/v1/sessions/session-shared/editor-context" && request.method === "POST") {
        editorContext = await requestBody(request);
        return json(response, 200, { accepted: true });
      }
      if (request.url.startsWith("/v1/sessions/session-shared/snapshot?") && request.method === "GET") {
        return json(response, 200, { pending_requests: [], snapshot_revision: 0 });
      }
      if (request.url === "/v1/auth/browser-bootstrap" && request.method === "POST") {
        assert.equal(request.headers["x-s-code-csrf"], "1");
        return json(response, 200, { token: "single-use-browser-token" });
      }
      if (request.url.startsWith("/v1/events?")) {
        response.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-store" });
        response.write(": connected\n\n");
        streams.add(response);
        request.on("close", () => streams.delete(response));
        return;
      }
      json(response, 404, { error: "unexpected fixture request", path: request.url });
    } catch (error) {
      json(response, 500, { error: error.message });
    }
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));

  let controller;
  const diagnostics = vscode.languages.createDiagnosticCollection("s-code-e2e");
  try {
    const address = server.address();
    const configuration = vscode.workspace.getConfiguration("s-code");
    await configuration.update("daemonUrl", `http://127.0.0.1:${address.port}`, vscode.ConfigurationTarget.Global);
    await configuration.update("organizationId", "org-extension", vscode.ConfigurationTarget.Global);
    await configuration.update("teamId", "team-extension", vscode.ConfigurationTarget.Global);
    await configuration.update("actorId", "user-extension", vscode.ConfigurationTarget.Global);

    const folder = vscode.workspace.workspaceFolders[0];
    const uri = vscode.Uri.joinPath(folder.uri, "context.rs");
    const document = await vscode.workspace.openTextDocument(uri);
    const editor = await vscode.window.showTextDocument(document);
    editor.selection = new vscode.Selection(0, 0, 0, 7);
    diagnostics.set(uri, [new vscode.Diagnostic(new vscode.Range(0, 0, 0, 2), "fixture diagnostic", vscode.DiagnosticSeverity.Warning)]);

    const extension = vscode.extensions.getExtension("s-code.s-code");
    assert.ok(extension, "development extension was not discovered");
    controller = await extension.activate();
    const commands = new Set(await vscode.commands.getCommands(true));
    for (const command of ["s-code.connect", "s-code.selectSession", "s-code.sendContext", "s-code.reviewHunks", "s-code.approve", "s-code.reject"]) {
      assert.ok(commands.has(command), `extension did not register ${command}`);
    }
    await controller.context.secrets.store(TOKEN_KEY, "extension-host-secret");
    await controller.context.globalState.update(SESSION_KEY, "session-shared");
    await controller.connect();
    const browserUrl = await controller.api.browserBootstrap();

    assert.ok(editorContext, "extension did not submit editor context");
    assert.equal(
      browserUrl,
      `http://127.0.0.1:${address.port}/#s-code-bootstrap=single-use-browser-token`,
    );
    assert.ok(!browserUrl.includes("extension-host-secret"));
    assert.equal(editorContext.protocol_version, "1.0");
    assert.equal(editorContext.scope.organization_id, "org-extension");
    assert.equal(editorContext.scope.team_id, "team-extension");
    assert.equal(editorContext.scope.actor_id, "user-extension");
    assert.equal(editorContext.active_document.text, "fn main() {}\n");
    assert.equal(editorContext.selection.text, "fn main");
    assert.equal(editorContext.diagnostics.length, 1);
    assert.equal(editorContext.diagnostics[0].message, "fixture diagnostic");
    assert.equal(controller.context.globalState.get(SESSION_KEY), "session-shared");
    console.log("VS Code Extension Host session restore and editor-context test passed");
  } finally {
    diagnostics.dispose();
    controller?.dispose();
    for (const stream of streams) stream.end();
    await new Promise((resolve) => server.close(resolve));
  }
}

module.exports = { run };
