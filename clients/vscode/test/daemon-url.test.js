"use strict";

const assert = require("node:assert/strict");
const test = require("node:test");
const { validatedDaemonBase, browserBootstrapUrl } = require("../daemon-url");

test("accepts only explicit loopback HTTP origins", () => {
  assert.equal(validatedDaemonBase("http://127.0.0.1:4096/"), "http://127.0.0.1:4096");
  assert.equal(validatedDaemonBase("http://localhost:4096"), "http://localhost:4096");
  assert.equal(validatedDaemonBase("http://[::1]:4096"), "http://[::1]:4096");
  for (const value of [
    "https://127.0.0.1:4096",
    "http://example.com:4096",
    "http://127.0.0.1",
    "http://user@127.0.0.1:4096",
    "http://127.0.0.1:4096/v1",
    "http://127.0.0.1:4096?forward=example.com",
  ]) {
    assert.throws(() => validatedDaemonBase(value), /loopback/);
  }
});

test("puts only the single-use bootstrap in the Web fragment", () => {
  const url = browserBootstrapUrl("http://127.0.0.1:4096", "once + only");
  assert.equal(url, "http://127.0.0.1:4096/#s-code-bootstrap=once+%2B+only");
  assert.ok(!url.includes("daemon-secret"));
});
