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
  const names = ["canBindToolProposal", "friendlyTool", "shouldRenderToolStep", "renderToolStep", "groupCodeModeTools", "toolStepDetail"];
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
    return {nested,retained,paginatedLabel,attachedLate,cancellationFinal,directIsTopLevel:direct.parentElement.id==="messages",childCount:document.querySelectorAll('[data-item-id="child"]').length};
  });
  assert.deepEqual(result, {nested:true,retained:true,paginatedLabel:true,attachedLate:true,cancellationFinal:true,directIsTopLevel:true,childCount:1});
  console.log("Code Mode browser regression passed: nesting, caught denial, direct fallback, pagination and replay identity");
} finally { await browser.close(); }
