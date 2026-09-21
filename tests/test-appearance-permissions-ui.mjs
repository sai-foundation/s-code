import fs from "node:fs";
import path from "node:path";
import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
const root = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const { chromium } = await import(process.env.S_CODE_PLAYWRIGHT_MODULE || "playwright");
const browser = await chromium.launch({ headless: true, executablePath: process.env.S_CODE_BROWSER_EXECUTABLE });
const patches = [];
const persistedModes = new Map();
let delayPatch = false, releasePatch = null;
let releaseUpload = null, turnRequests = 0, goalUpdates = 0, deletedAttachments = 0;
const profiles = [
  ["manual", "Manual approval", "Ask before changes that need approval."],
  ["accept_edits", "Accept edits", "Approve allowed file edits automatically."],
  ["workspace", "Workspace autonomy", "Approve workspace edits and sandboxed commands."],
  ["plan", "Plan only", "Read and plan without changing files."],
  ["full", "Full permission", "Run host commands with network and outside-workspace access."],
].map(([mode, label, description]) => ({ mode, label, description, file_changes: "test metadata", commands: "test metadata", network: "test metadata", source: "built_in", locked_reason: null }));
try {
  const page = await browser.newPage({ viewport: { width: 1100, height: 850 }, reducedMotion: "reduce", colorScheme: "light" });
  const errors = []; page.on("pageerror", error => errors.push(error.message));
  const source = fs.readFileSync(path.join(root, "web/app.js"), "utf8");
  // The driver is injected in the fixture response only, never the product bundle.
  const driver = `
    globalThis.testUI = {
      settled: () => !state.connecting && !state.connected,
      ready(id = 'session-a', mode = 'manual') {
        state.connected = true; state.turnRunning = false;
        state.authenticatedScope = formScope();
        state.capabilities = new Set(['composer.permission_picker.v1']);
        state.session = {id, mode:'work', scope:formScope(), workspace_uri:'file:///tmp/fixture', model:'fixture', title:'Fixture', status:'active'};
        permissionSessionId = id; state.permissionMode = mode; updateContextChips();
      },
      switchPending() { void selectSession({...state.session,id:'session-b'}).catch(() => {}); },
      returnToA() { void selectSession({...state.session,id:'session-a'}).catch(() => {}); },
      mode: () => state.permissionMode,
      stale() {state.generation++;},
      goal() {void editSessionGoal();},
      resumeGoal() {state.goal.status='paused';return toggleSessionGoal();},
      upload() {globalThis.uploadDone=false;void executeContent('inspect attachment',[new File(['fixture'],'fixture.txt',{type:'text/plain'})]).finally(() => {globalThis.uploadDone=true;});},
      workDraft() {state.session=null;permissionSessionId=null;newConversationMode='work';state.permissionMode='full';updateContextChips();},
      chatDraft() {chooseNewConversationMode('chat');},
      empty(value) {updateConversationState(!value);},
      markdown(content) {
        const node=document.createElement('div');appendMarkdownBlocks(node,content);
        return {links:[...node.querySelectorAll('a')].map(a=>a.href),images:[...node.querySelectorAll('img')].map(img=>img.src),scripts:node.querySelectorAll('script,[onerror]').length,text:node.textContent};
      },
    };`;
  await page.route("**/*", async route => {
    const request = route.request(), url = new URL(request.url());
    if (url.origin !== "http://appearance.test") return route.abort();
    if (url.pathname === "/" && request.resourceType() === "document") return route.fulfill({ contentType: "text/html", body: fs.readFileSync(path.join(root, "web/index.html"), "utf8") });
    if (url.pathname === "/app.css") return route.fulfill({ contentType: "text/css", body: fs.readFileSync(path.join(root, "web/app.css"), "utf8") });
    if (url.pathname === "/app.js") return route.fulfill({ contentType: "application/javascript", body: source + driver });
    if (url.pathname === "/v1/permission-profiles") return route.fulfill({ json: profiles });
    if (url.pathname.endsWith('/turns') && request.method() === 'POST') turnRequests++;
    if (url.pathname.endsWith('/goal') && request.method() === 'PATCH') goalUpdates++;
    if (url.pathname.endsWith('/goal') && request.method() === 'PUT') return route.fulfill({json:{...request.postDataJSON(),id:'goal-fixture',session_id:'session-a',status:'active',input_tokens:0,output_tokens:0,continuation_count:0,revision:1}});
    if (url.pathname.endsWith('/attachments') && request.method() === 'POST') {
      await new Promise(resolve => {releaseUpload=resolve;});
      return route.fulfill({json:{id:'attachment-fixture',file_name:'fixture.txt',media_type:'text/plain',byte_length:7}});
    }
    if (url.pathname === '/v1/attachments/attachment-fixture' && request.method() === 'DELETE') {deletedAttachments++;return route.fulfill({json:{}});}
    if (url.pathname.endsWith("/preferences") && request.method() === "PATCH") {
      const data = request.postDataJSON(); patches.push(data);
      if (delayPatch) await new Promise(resolve => { releasePatch = resolve; });
      persistedModes.set(url.pathname.split('/')[3], data.permission_mode);
      return route.fulfill({ json: {session_id:url.pathname.split('/')[3],permission_mode:data.permission_mode,source:'session',assistant_alias:'S-Code',locked_reason:null} });
    }
    if (url.pathname.endsWith("/preferences") && request.method() === "GET") return route.fulfill({json:{session_id:url.pathname.split('/')[3],permission_mode:persistedModes.get(url.pathname.split('/')[3]) || 'manual',source:'session',assistant_alias:'S-Code',locked_reason:null}});
    return route.fulfill({status:503,json:{detail:'Fixture offline'}});
  });
  await page.goto("http://appearance.test/");
  await page.waitForFunction(() => globalThis.testUI?.settled());
  const markdown = await page.evaluate(() => testUI.markdown('[safe](https://example.com/path) [bad](javascript:alert) ![bad](data:text/html,evil) <https://example.com/auto> ![safe](https://example.com/image.png) <script>alert(1)</script>'));
  assert.deepEqual(markdown.links, ['https://example.com/path','https://example.com/auto']);
  assert.deepEqual(markdown.images, ['https://example.com/image.png']);
  assert.equal(markdown.scripts, 0);
  assert.match(markdown.text, /javascript:alert/);
  await page.locator('.user-menu summary').click();
  await page.locator('#theme-toggle').click();
  await page.locator('#theme-options button').first().waitFor();
  assert.equal(await page.locator('#theme-options button').count(), 6);
  for (const theme of ['light','dark','terminal','midnight','nord','system']) {
    await page.locator(`[data-theme-choice="${theme}"]`).click();
    assert.equal(await page.locator('html').getAttribute('data-theme'), theme);
    assert.equal(await page.locator(`[data-theme-choice="${theme}"]`).getAttribute('aria-pressed'), 'true');
    const preview = await page.locator(`[data-theme-choice="${theme}"] img`).evaluate(async img => { await img.decode(); return img.naturalWidth; });
    assert.equal(preview, 960);
    const contrasts = await page.evaluate(() => {
      const style = getComputedStyle(document.documentElement);
      const luminance = token => {
        let hex = style.getPropertyValue(token).trim().slice(1);
        if (hex.length === 3) hex = [...hex].map(c => c+c).join("");
        const values = hex.match(/../g).map(value => parseInt(value,16)/255).map(value => value <= 0.04045 ? value/12.92 : ((value+0.055)/1.055)**2.4);
        return values[0]*0.2126 + values[1]*0.7152 + values[2]*0.0722;
      };
      return [['--text','--canvas'],['--muted','--surface'],['--on-accent','--accent']].map(([a,b]) => {
        const x=luminance(a),y=luminance(b); return (Math.max(x,y)+0.05)/(Math.min(x,y)+0.05);
      });
    });
    assert(contrasts.every(value => value >= 4.5), `${theme} readable text contrast: ${contrasts}`);
  }
  if (process.env.S_CODE_UI_SCREENSHOTS) await page.screenshot({path:path.join(process.env.S_CODE_UI_SCREENSHOTS, "theme-settings.png")});
  await page.emulateMedia({colorScheme:'dark'});
  await page.waitForFunction(() => document.documentElement.dataset.colorScheme === 'dark');
  await page.locator('[data-theme-choice="light"]').click();
  assert.equal(await page.locator('html').getAttribute('data-color-scheme'), 'light');
  await page.emulateMedia({colorScheme:'light'});
  await page.waitForFunction(() => document.querySelector('[data-theme-choice="system"] img').src === document.querySelector('[data-theme-choice="light"] img').src);
  await page.locator('[data-theme-choice="terminal"]').click();
  await page.reload();
  await page.waitForFunction(() => globalThis.testUI?.settled());
  assert.equal(await page.locator('html').getAttribute('data-theme'), 'terminal');
  await page.evaluate(() => testUI.ready());
  await page.locator('#permission-chip').click();
  await page.locator('#context-picker-dialog').waitFor({state:'visible'});
  assert.equal(await page.locator('#context-picker-query').isVisible(), false);
  assert.equal(await page.locator('#context-picker-results .picker-meta').count(), 0);
  if (process.env.S_CODE_UI_SCREENSHOTS) await page.screenshot({path:path.join(process.env.S_CODE_UI_SCREENSHOTS, "permissions.png")});
  await page.keyboard.press('End');
  assert.match(await page.locator(':focus').innerText(), /Full permission/);
  await page.keyboard.press('Enter');
  await page.locator('#action-dialog').waitFor({state:'visible'});
  assert.match(await page.locator('#action-description').innerText(), /without the OS sandbox/);
  await page.locator('#cancel-action').click();
  assert.equal(patches.length, 0);
  await page.locator('#permission-chip').click();
  await page.locator('#context-picker-dialog').waitFor({state:'visible'});
  await page.getByRole('option', {name:/Full permission/}).click();
  await page.locator('#confirm-action').click();
  await page.waitForFunction(() => globalThis.testUI.mode() === 'full');
  assert.equal(patches.length, 1); assert.equal(patches[0].permission_mode, 'full');
  // Escaping a slow downgrade must not permit a turn under the old Full mode.
  delayPatch = true;
  await page.locator('#permission-chip').click();
  await page.getByRole('option', {name:/Manual approval/}).click();
  await page.waitForFunction(() => document.getElementById('permission-chip').disabled);
  await page.keyboard.press('Escape');
  await page.locator('#prompt').fill('do not start under the old permission');
  assert.equal(await page.locator('#send-turn').isDisabled(), true);
  await page.evaluate(() => testUI.goal());
  await page.locator('#action-objective').fill('Do not start a Goal under stale Full permission');
  await page.locator('#confirm-action').click();
  await page.waitForFunction(() => document.getElementById('task-status').dataset.state === 'failed');
  assert.equal(turnRequests, 0, 'Goal must use the same permission gate as the composer');
  await page.evaluate(() => testUI.resumeGoal());
  assert.equal(goalUpdates, 0, 'resuming a Goal must not start a turn under stale Full permission');
  await page.evaluate(() => {testUI.switchPending();testUI.returnToA();});
  assert.equal(await page.locator('#send-turn').isDisabled(), true, 'navigation must retain pending downgrade');
  assert(releasePatch); delayPatch = false; releasePatch();
  await page.waitForFunction(() => globalThis.testUI.mode() === 'manual' && !document.getElementById('permission-chip').disabled);
  // Permission can begin changing while a local attachment upload is pending.
  await page.evaluate(() => testUI.ready('session-a','full'));
  await page.evaluate(() => testUI.upload());
  for (let attempt=0; !releaseUpload && attempt<100; attempt++) await new Promise(resolve => setTimeout(resolve, 20));
  assert(releaseUpload, 'attachment upload reached the daemon');
  delayPatch = true; releasePatch = null;
  await page.locator('#permission-chip').click();
  await page.getByRole('option', {name:/Manual approval/}).click();
  await page.waitForFunction(() => document.getElementById('permission-chip').disabled);
  releaseUpload();
  await page.waitForFunction(() => globalThis.uploadDone);
  assert.equal(turnRequests, 0, 'recheck permission after asynchronous attachment upload');
  assert.equal(deletedAttachments, 1, 'delete unattached upload after blocked submission');
  assert(releasePatch); delayPatch = false; releasePatch();
  await page.waitForFunction(() => globalThis.testUI.mode() === 'manual' && !document.getElementById('permission-chip').disabled);
  await page.keyboard.press('Escape');
  await page.evaluate(() => testUI.ready('session-a','full'));
  // A session switch must not carry Full into the chooser while its preferences load.
  await page.evaluate(() => testUI.switchPending());
  assert.equal(await page.locator('#permission-chip').isDisabled(), true);
  assert.equal(await page.evaluate(() => testUI.mode()), 'manual');
  await page.evaluate(() => testUI.ready('session-b'));
  await page.locator('#permission-chip').click();
  await page.locator('#context-picker-dialog').waitFor({state:'visible'});
  await page.getByRole('option', {name:/Full permission/}).click();
  await page.evaluate(() => testUI.stale());
  await page.locator('#confirm-action').click();
  assert.equal(patches.length, 3, 'stale confirmation cannot patch a new scope');
  await page.evaluate(() => {testUI.workDraft();testUI.empty(true);});
  assert.equal(await page.locator('.starter-actions').isVisible(), true);
  await page.evaluate(() => testUI.empty(false));
  assert.equal(await page.locator('.starter-actions').isVisible(), false);
  await page.evaluate(() => testUI.chatDraft());
  assert.equal(await page.evaluate(() => testUI.mode()), 'manual');
  assert.equal(await page.locator('.starter-actions').isVisible(), false);
  await page.setViewportSize({width:390,height:800});
  await page.locator('#context-picker-dialog').evaluate(dialog => dialog.close());
  await page.locator('#open-settings').evaluate(button => button.click());
  await page.locator('[data-theme-choice="nord"]').click();
  assert(await page.locator('[data-theme-choice="nord"]').isVisible());
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
  assert.deepEqual(errors, []);
  console.log('Appearance and permission UI: six previews, persistence, OS switching, full confirmation/cancel/scope, loading state, empty suggestions and narrow layout passed');
} finally { await browser.close(); }
