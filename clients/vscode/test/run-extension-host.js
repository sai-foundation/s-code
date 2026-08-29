"use strict";

const path = require("path");
const { runTests } = require("@vscode/test-electron");

async function main() {
  delete process.env.ELECTRON_RUN_AS_NODE;
  for (const name of Object.keys(process.env)) {
    if (name.startsWith("VSCODE_")) delete process.env[name];
  }
  const root = path.resolve(__dirname, "..");
  await runTests({
    version: "1.96.4",
    extensionDevelopmentPath: root,
    extensionTestsPath: path.resolve(__dirname, "extension-host", "index.js"),
    launchArgs: [
      path.resolve(__dirname, "fixture-workspace"),
      "--disable-extensions",
      "--skip-welcome",
      "--skip-release-notes",
    ],
  });
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
