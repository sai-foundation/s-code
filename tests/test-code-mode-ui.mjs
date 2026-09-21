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
  const names = ["canBindToolProposal", "friendlyTool", "shouldRenderToolStep", "renderToolStep", "groupCodeModeTools", "toolStepDetail", "taskFeedback", "toolFailureCause", "appendToolInspection"];
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
    let recorded = null, requests = 0, deferred = false, release;
    function scopedToolLoader() { return async () => {
      requests++; const snapshot = structuredClone(recorded);
      if (deferred) await new Promise(resolve => release = resolve);
      return snapshot;
    }; }
    ${functions}
    globalThis.renderToolStep=renderToolStep;
    globalThis.inspection = {
      reset() { state.toolSteps.clear(); state.itemsById.clear(); $('messages').replaceChildren(); requests = 0; },
      set(value) { recorded = value; },
      defer() { deferred = true; },
      release() { deferred = false; release(); },
      count() { return requests; },
      emit(kind, id = 'live') { renderToolStep(kind, { tool_call_id: id, tool: 'read_file' }, { turn_id: 'turn' }); }
    };
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
  await page.evaluate(() => {
    inspection.reset(); inspection.set({ status: "running", result: null, error: null });
    inspection.emit("tool.running"); inspection.emit("tool.completed", "unopened");
  });
  assert.equal(await page.evaluate(() => inspection.count()), 0, "closed success cards avoid unnecessary detail requests");
  await page.locator('[data-item-id="live"] summary').click();
  await page.waitForFunction(() => document.querySelector('[data-item-id="live"] pre').textContent.includes('running'));
  await page.evaluate(() => { inspection.set({ status: "completed", result: "FINAL RESULT", error: null }); inspection.emit("tool.completed"); });
  await page.waitForFunction(() => document.querySelector('[data-item-id="live"] pre').textContent.includes('FINAL RESULT'));
  await page.evaluate(() => {
    inspection.set({ status: "running", result: null, error: null }); inspection.defer();
    document.querySelector('[data-item-id="live"] details').dispatchEvent(new Event('refresh-tool-details'));
    inspection.set({ status: "failed", result: null, error: "PRECISE FAILURE" }); inspection.emit("tool.failed");
    inspection.release();
  });
  await page.waitForFunction(() => document.querySelector('[data-item-id="live"] .tool-step-cause').textContent === 'PRECISE FAILURE');
  assert.match(await page.locator('[data-item-id="live"] pre').textContent(), /PRECISE FAILURE/);
  assert.equal(await page.evaluate(() => inspection.count()), 4, "terminal refresh is retained while the previous fetch is in flight");
  console.log("Code Mode browser regression passed: nesting, caught denial, direct fallback, pagination and replay identity");
  console.log("Tool inspection browser regression passed: completion refresh, in-flight failure invalidation, and lazy success details");
} finally { await browser.close(); }
