const path = require("path");

process.env.PLAYWRIGHT_BROWSERS_PATH ||= path.join(__dirname, "playwright-browsers");

const { defineConfig } = require("@playwright/test");

module.exports = defineConfig({
  testDir: "./tests",
  timeout: 20_000,
  workers: 1,
  use: { headless: true },
});
