// Optional browser regression: install Playwright, or set S_CODE_PLAYWRIGHT_MODULE
// to a Playwright module path. Uses the real checked-in application functions.
import fs from "node:fs";
import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import path from "node:path";
const root = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const { chromium } = await import(process.env.S_CODE_PLAYWRIGHT_MODULE || "playwright");
const browser = await chromium.launch({ headless: true, executablePath: process.env.S_CODE_BROWSER_EXECUTABLE });
try {
  const page = await browser.newPage();
  await page.setContent('<main id="messages"></main>');
  await page.addStyleTag({ content: fs.readFileSync(path.join(root, "web/app.css"), "utf8") });
  const source = fs.readFileSync(path.join(root, "web/app.js"), "utf8");
  const names = ["canBindToolProposal", "friendlyTool", "shouldRenderToolStep", "renderToolStep", "groupCodeModeTools", "toolStepDetail", "taskFeedback", "toolFailureCause"];
  const functions = names.map((name) => {
    const start = source.indexOf(`function ${name}(`);
    assert(start >= 0, `missing ${name}`);
    const end = source.indexOf("\nfunction ", start + 1);
    return source.slice(start, end < 0 ? undefined : end);
  }).join("\n");
  await page.addScriptTag({ content: `
    const state={toolSteps:new Map(),itemsById:new Map()};
    const $=(id)=>document.getElementById(id);
    function updateConversationState() {}
    function activityLabel(kind) {return kind;}
    function activityDetail() {return '';}
    function scopedToolLoader() { return async () => null; }
    function appendToolInspection(container, options) {
      const details = document.createElement('details'); details.className = options.className; container.append(details);
    }
    ${functions}
    globalThis.renderToolStep=renderToolStep;
  ` });
  const result = await page.evaluate(() => {
    const emit=(kind,id,tool,parent=null)=>renderToolStep(kind,{tool_call_id:id,tool,parent_tool_call_id:parent,display:tool},{turn_id:"turn"});
    emit("tool.running","program","execute");
    emit("tool.running","child","read_file","program");
    emit("tool.denied","child","read_file","program");
    emit("tool.completed","program","execute");
    const child=document.querySelector('[data-item-id="child"]');
    const nested=child.parentElement.className === "code-mode-children";
    emit("tool.proposed","model-direct","read_file");
    emit("approval.required","direct","read_file");
    const direct=document.querySelector('[data-item-id="direct"]');
    const retained=document.querySelector('[data-item-id="child"]')===child && child.classList.contains("decision");
    emit("tool.completed","late-child","search_text","late-parent");
    const paginatedLabel=document.querySelector('[data-item-id="late-child"]').textContent.includes("Code Mode");
    emit("tool.completed","late-parent","execute");
    const attachedLate=document.querySelector('[data-item-id="late-child"]').parentElement.parentElement.dataset.itemId==="late-parent";
    emit("tool.cancelled","cancelled-child","read_file","late-parent");
    emit("tool.completed","cancelled-child","read_file","late-parent");
    const cancellationFinal=document.querySelector('[data-item-id="cancelled-child"]').classList.contains("cancelled");
    emit("tool.failed", "failed", "read_file");
    const failed = document.querySelector('[data-item-id="failed"]');
    const failureExplained = failed.querySelector('.tool-step-cause').textContent.includes('Inspect details');
    const terminalNoAnimation = getComputedStyle(failed.querySelector('.tool-step-marker')).animationName === 'none';
    const compactSuccess = document.querySelector('[data-item-id="program"]').classList.contains('complete');
    return {failureExplained,terminalNoAnimation,compactSuccess,nested,retained,paginatedLabel,attachedLate,cancellationFinal,directIsTopLevel:direct.parentElement.id==="messages",childCount:document.querySelectorAll('[data-item-id="child"]').length};
  });
  assert.deepEqual(result, {failureExplained:true,terminalNoAnimation:true,compactSuccess:true,nested:true,retained:true,paginatedLabel:true,attachedLate:true,cancellationFinal:true,directIsTopLevel:true,childCount:1});
  const composerPage = await browser.newPage({ viewport: { width: 390, height: 740 }, reducedMotion: "reduce" });
  const html = fs.readFileSync(path.join(root, "web/index.html"), "utf8").replace(/<script\b[^>]*>[\s\S]*?<\/script>/g, "").replace(/<link[^>]+>/g, "");
  await composerPage.route("http://composer.test/**", route => route.fulfill({ contentType: "text/html", body: html }));
  await composerPage.goto("http://composer.test/");
  await composerPage.addStyleTag({ content: fs.readFileSync(path.join(root, "web/app.css"), "utf8") });
  const composerNames = ["composerControls", "sameComposerContext", "pendingInputLabel", "nextTurnInputAttempt", "pendingInputsOwned", "refreshPendingInputs", "reconcilePendingInputs", "isProtectionCommand", "composerRoute", "dispatchComposer", "updateSendAction", "renderPendingInputs", "submitTurnInput", "currentPickerContext", "accountKey", "ownsSession", "withComposerSubmission", "runTurn"];
  const composerFunctions = composerNames.map(name => {
    const start = source.indexOf(`function ${name}(`);
    assert(start >= 0, `missing ${name}`);
    const end = source.indexOf("\nfunction ", start + 1);
    const endAsync = source.indexOf("\nasync function ", start + 1);
    const stop = Math.min(...[end, endAsync].filter(value => value >= 0));
    return `${source.slice(Math.max(0, start - 6), start) === "async " ? "async " : ""}${source.slice(start, stop)}`;
  }).join("\n");
  const keyStart = source.indexOf('$("prompt").addEventListener("keydown",');
  const keyEnd = source.indexOf('$("connection-form").addEventListener', keyStart);
  await composerPage.addScriptTag({ content: `
    const $ = id => document.getElementById(id);
    const account = { organization_id: 'org', team_id: 'team', actor_id: 'actor' };
    const state = { session: { id: 'session-a', scope: account }, turn: 'turn-a', turnRunning: true, draftFiles: [], pendingInputs: [], capabilities: new Set(['turn.input_queue.v1']), generation: 1, authenticatedScope: account };
    let composerSubmissionPending = false, pendingTurnInputAttempt = null, pendingInputsReadVersion = 0;
    const scope = () => state.authenticatedScope;
    const resizePrompt = () => {};
    const closeMentionMenu = () => {};
    const toast = message => { globalThis.lastToast = message; };
    const addActivity = () => {};
    const composerTextDraftKey = () => 'fixture-draft';
    const submitProtectionPrompt = async content => { globalThis.localCommand = content; };
    const api = (path, options) => new Promise((resolve, reject) => { globalThis.pendingRequest = { path, body: options?.body ? JSON.parse(options.body) : null, resolve, reject }; });
    ${composerFunctions}
    ${source.slice(keyStart, keyEnd)}
    $('prompt').addEventListener('input', updateSendAction);
    $('prompt-form').addEventListener('submit', runTurn);
    $('steer-turn').addEventListener('click', () => withComposerSubmission(() => submitTurnInput($('prompt').value, 'steer')));
    globalThis.fixtureState = state;
    globalThis.fixtureAccount = account;
    updateSendAction();
  ` });
  await composerPage.locator('#prompt').fill('follow up');
  assert.equal(await composerPage.locator('#send-turn').textContent(), 'Queue');
  assert.equal(await composerPage.locator('#steer-turn').isVisible(), true);
  assert.equal(await composerPage.locator('#stop-turn').isVisible(), true);
  await composerPage.locator('#prompt').press('Enter');
  assert.equal(await composerPage.evaluate(() => pendingRequest.body.mode), 'queue');
  // A later draft survives the earlier request's confirmation.
  await composerPage.locator('#prompt').fill('new draft');
  await composerPage.evaluate(() => { globalThis.acknowledgedInput = { ...pendingRequest.body, status: 'pending', id: 'input-1', session_id: 'session-a', scope: fixtureAccount, mode: 'queue', content: 'follow up' }; pendingRequest.resolve(acknowledgedInput); });
  await composerPage.waitForFunction(() => pendingRequest.body === null);
  await composerPage.evaluate(() => pendingRequest.resolve([acknowledgedInput]));
  await composerPage.waitForFunction(() => !document.getElementById('send-turn').disabled);
  assert.equal(await composerPage.locator('#prompt').inputValue(), 'new draft');
  assert.match(await composerPage.locator('#pending-inputs').textContent(), /Follow-up 1 · pending/);
  await composerPage.locator('#prompt').press('Alt+Enter');
  assert.equal(await composerPage.evaluate(() => pendingRequest.body.mode), 'steer');
  // Switching sessions while confirmation is in flight must not clear or append there.
  await composerPage.evaluate(() => { fixtureState.session = { id: 'session-b', scope: fixtureAccount }; fixtureState.pendingInputs = []; });
  await composerPage.locator('#prompt').fill('session-b draft');
  await composerPage.evaluate(() => pendingRequest.resolve({ ...pendingRequest.body, status: 'pending', id: 'input-2', session_id: 'session-a', scope: fixtureAccount, mode: 'steer', content: 'new draft' }));
  await composerPage.waitForFunction(() => !document.getElementById('send-turn').disabled);
  assert.equal(await composerPage.locator('#prompt').inputValue(), 'session-b draft');
  assert.equal(await composerPage.evaluate(() => fixtureState.pendingInputs.length), 0);
  // Losing the server's acknowledgement must retry the same request key.
  await composerPage.locator('#prompt').fill('uncertain acknowledgement');
  await composerPage.locator('#prompt').press('Enter');
  const uncertainKey = await composerPage.evaluate(() => pendingRequest.body.idempotency_key);
  await composerPage.evaluate(() => pendingRequest.reject(new Error('Connection lost after acceptance')));
  await composerPage.waitForFunction(() => !document.getElementById('send-turn').disabled);
  await composerPage.locator('#prompt').press('Enter');
  assert.equal(await composerPage.evaluate(() => pendingRequest.body.idempotency_key), uncertainKey);
  await composerPage.evaluate(() => { globalThis.acknowledgedInput = { ...pendingRequest.body, id: 'input-retry', session_id: 'session-b', scope: fixtureAccount, status: 'pending' }; pendingRequest.resolve(acknowledgedInput); });
  await composerPage.waitForFunction(() => pendingRequest.body === null);
  await composerPage.evaluate(() => pendingRequest.resolve([acknowledgedInput]));
  await composerPage.waitForFunction(() => !document.getElementById('send-turn').disabled);
  assert.equal(await composerPage.evaluate(() => fixtureState.pendingInputs.filter(input => input.id === 'input-retry').length), 1);
  // Same text after a confirmed acknowledgement is a deliberate new request.
  await composerPage.locator('#prompt').fill('uncertain acknowledgement');
  await composerPage.locator('#prompt').press('Enter');
  assert.notEqual(await composerPage.evaluate(() => pendingRequest.body.idempotency_key), uncertainKey);
  await composerPage.evaluate(() => pendingRequest.resolve({ ...pendingRequest.body, id: 'input-next', session_id: 'session-b', scope: fixtureAccount, status: 'consumed' }));
  await composerPage.waitForFunction(() => pendingRequest.body === null);
  await composerPage.evaluate(() => pendingRequest.resolve([acknowledgedInput]));
  await composerPage.waitForFunction(() => !document.getElementById('send-turn').disabled);
  assert.equal(await composerPage.evaluate(() => fixtureState.pendingInputs.some(input => input.id === 'input-next')), false);
  // Late failed reads after account/generation changes remain silent and cannot replace current input.
  await composerPage.evaluate(() => { globalThis.readResult = refreshPendingInputs(); fixtureState.generation++; fixtureState.authenticatedScope = { ...fixtureAccount, actor_id: 'another' }; pendingRequest.reject(new Error('old account read failed')); });
  await composerPage.evaluate(() => readResult);
  assert.equal(await composerPage.evaluate(() => fixtureState.pendingInputs.length), 1);
  // A same-session response from another account is rejected in full.
  await composerPage.evaluate(() => { globalThis.readResult = refreshPendingInputs().then(() => 'accepted', () => 'rejected'); pendingRequest.resolve([{ session_id: 'session-b', scope: fixtureAccount }]); });
  assert.equal(await composerPage.evaluate(() => readResult), 'rejected');
  await composerPage.evaluate(() => { fixtureState.authenticatedScope = fixtureAccount; });
  // Delete confirmation removes its ID and reconciles the rest of the server queue.
  await composerPage.getByRole('button', { name: 'Remove queued follow-up 1' }).click();
  await composerPage.evaluate(() => pendingRequest.resolve({}));
  await composerPage.waitForFunction(() => pendingRequest.body === null);
  await composerPage.evaluate(() => pendingRequest.resolve([]));
  assert.equal(await composerPage.evaluate(() => fixtureState.pendingInputs.length), 0);
  // Exact race: POST starts, event GET starts, captured pending POST ACK arrives,
  // then event GET reports consumption. The ACK must never insert a ghost.
  await composerPage.evaluate(() => {
    globalThis.racePostResult = submitTurnInput('race follow-up', 'queue');
    globalThis.racePost = pendingRequest;
    globalThis.raceEventResult = refreshPendingInputs();
    globalThis.raceEventRead = pendingRequest;
    document.getElementById('prompt').value = 'newer draft during reconciliation';
    racePost.resolve({ ...racePost.body, id: 'race-input', session_id: 'session-b', scope: fixtureAccount, status: 'pending' });
  });
  await composerPage.evaluate(() => racePostResult);
  await composerPage.evaluate(() => { globalThis.postAckRead = pendingRequest; raceEventRead.resolve([]); });
  await composerPage.evaluate(() => raceEventResult);
  assert.equal(await composerPage.evaluate(() => fixtureState.pendingInputs.length), 0);
  assert.equal(await composerPage.locator('#prompt').inputValue(), 'newer draft during reconciliation');
  await composerPage.evaluate(() => postAckRead.resolve([]));
  assert.equal(await composerPage.evaluate(() => fixtureState.pendingInputs.length), 0);
  // Queue reconciliation failure is distinct from an already-confirmed submit.
  await composerPage.evaluate(() => {
    globalThis.confirmedResult = submitTurnInput('confirmed despite read failure', 'queue');
    globalThis.confirmedKey = pendingRequest.body.idempotency_key;
    pendingRequest.resolve({ ...pendingRequest.body, id: 'confirmed-input', session_id: 'session-b', scope: fixtureAccount, status: 'pending' });
  });
  await composerPage.evaluate(() => confirmedResult);
  await composerPage.evaluate(() => pendingRequest.reject(new Error('queue read unavailable')));
  assert.match(await composerPage.evaluate(() => lastToast), /Follow-up accepted.*Could not refresh pending inputs/);
  await composerPage.evaluate(() => {
    globalThis.nextResult = submitTurnInput('confirmed despite read failure', 'queue');
    globalThis.nextKey = pendingRequest.body.idempotency_key;
    pendingRequest.resolve({ ...pendingRequest.body, id: 'confirmed-next', session_id: 'session-b', scope: fixtureAccount, status: 'consumed' });
  });
  await composerPage.evaluate(() => nextResult);
  assert.equal(await composerPage.evaluate(() => confirmedKey === nextKey), false);
  await composerPage.evaluate(() => pendingRequest.resolve([]));
  await composerPage.locator('#prompt').fill('/protect /repo/private');
  await composerPage.locator('#prompt').press('Enter');
  assert.equal(await composerPage.evaluate(() => localCommand), '/protect /repo/private');
  await composerPage.locator('#prompt').fill('narrow layout');
  const narrow = await composerPage.evaluate(() => {
    const button = document.getElementById('send-turn').getBoundingClientRect();
    const marker = document.createElement('span'); marker.className = 'tool-step-marker'; document.body.append(marker);
    return { fits: button.left >= 0 && button.right <= innerWidth && button.bottom <= innerHeight, reduced: getComputedStyle(marker).animationName === 'none' };
  });
  assert.deepEqual(narrow, { fits: true, reduced: true });
  await composerPage.locator('#prompt').press('Tab');
  assert.notEqual(await composerPage.evaluate(() => document.activeElement?.id), 'prompt');
  await composerPage.close();
  console.log("Composer browser regression passed: keyboard queue/steer, protected command, stale session, preserved draft, narrow layout and reduced motion");
  console.log("Code Mode browser regression passed: nesting, caught denial, direct fallback, pagination and replay identity");
} finally { await browser.close(); }
