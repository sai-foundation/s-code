// Render the real Web shell with synthetic, non-sensitive conversation content.
// Run with S_CODE_PLAYWRIGHT_MODULE and S_CODE_BROWSER_EXECUTABLE when needed.
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
const root = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const { chromium } = await import(process.env.S_CODE_PLAYWRIGHT_MODULE || "playwright");
const browser = await chromium.launch({ headless: true, executablePath: process.env.S_CODE_BROWSER_EXECUTABLE });
try {
  const page = await browser.newPage({ viewport: { width: 960, height: 600 }, reducedMotion: "reduce" });
  await page.route("**/*", route => {
    const request = route.request();
    if (request.url() === "http://theme-preview.test/" && request.resourceType() === "document") return route.fulfill({ contentType: "text/html", body: fs.readFileSync(path.join(root, "web/index.html"), "utf8") });
    if (request.url() === "http://theme-preview.test/app.css" && request.resourceType() === "stylesheet") return route.fulfill({ contentType: "text/css", body: fs.readFileSync(path.join(root, "web/app.css"), "utf8") });
    if (request.url() === "http://theme-preview.test/app.js" && request.resourceType() === "script") return route.fulfill({ contentType: "application/javascript", body: "" });
    return route.abort();
  });
  await page.goto("http://theme-preview.test/");
  await page.evaluate(() => {
    const $ = id => document.getElementById(id);
    $("conversation-view").classList.remove("is-empty");
    $("session-title").textContent = "Build something great";
    $("session-mode-badge").textContent = "Work";
    $("session-mode-badge").dataset.mode = "work";
    $("new-session-mode").hidden = true;
    $("model-chip-value").textContent = "Your model";
    $("model-chip-scope").textContent = "Model";
    $("permission-chip-scope").textContent = "Permissions";
    $("permission-chip-value").textContent = "Manual approval";
    $("quick-diff").hidden = false;
    $("quick-diff").disabled = false;
    $("toggle-privacy").disabled = false;
    for (const [role, text] of [["user", "Make the workspace feel clear and focused."], ["assistant", "A clean interface, a fresh palette, and space for your next idea."]]) {
      const article = document.createElement("article");
      article.className = `message ${role}`;
      const label = document.createElement("div"); label.className = "message-label"; label.textContent = role === "user" ? "You" : "S-Code";
      const body = document.createElement("div"); body.className = "message-body";
      const p = document.createElement("p"); p.textContent = text; body.append(p);
      if (role === "assistant") {
        const pre = document.createElement("pre"); const code = document.createElement("code");
        code.textContent = "const idea = await build();\nreturn idea;"; pre.append(code); body.append(pre);
      }
      article.append(label, body); $("messages").append(article);
    }
    const sessions = document.querySelector(".sessions");
    sessions.replaceChildren(); sessions.classList.remove("empty");
    for (const [i, title] of ["Build something great", "Explore the project", "Review recent changes"].entries()) {
      const button = document.createElement("button"); button.className = `session${i === 0 ? " active" : ""}`;
      const label = document.createElement("span"); label.textContent = title; button.append(label); sessions.append(button);
    }
  });
  const output = path.join(root, "web/src/theme-previews");
  fs.mkdirSync(output, { recursive: true });
  for (const theme of ["light", "dark", "terminal", "midnight", "nord"]) {
    await page.evaluate(theme => { document.documentElement.dataset.theme = theme; document.documentElement.dataset.colorScheme = theme === "light" ? "light" : "dark"; }, theme);
    await page.screenshot({ path: path.join(output, `${theme}.png`) });
  }
} finally { await browser.close(); }
