"use strict";

const test = require("node:test");
const assert = require("node:assert/strict");
const { parseFileHunks, rejectHunks } = require("../hunks");

test("rejects selected hunks in descending order and preserves other changes", () => {
  const diff = [
    "diff --git a/a.txt b/a.txt",
    "--- a/a.txt",
    "+++ b/a.txt",
    "@@ -1,2 +1,2 @@",
    "-one",
    "+ONE",
    " two",
    "@@ -4,2 +4,2 @@",
    " four",
    "-five",
    "+FIVE",
    "",
  ].join("\n");
  const hunks = parseFileHunks(diff, "a.txt");
  assert.equal(hunks.length, 2);
  assert.equal(rejectHunks("ONE\ntwo\nthree\nfour\nFIVE\n", [hunks[1]]), "ONE\ntwo\nthree\nfour\nfive\n");
  assert.equal(rejectHunks("ONE\ntwo\nthree\nfour\nFIVE\n", hunks), "one\ntwo\nthree\nfour\nfive\n");
});

test("fails closed when content no longer matches the reviewed diff", () => {
  const diff = "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-old\n+new\n";
  const hunks = parseFileHunks(diff, "a.txt");
  assert.throws(() => rejectHunks("human edit\n", hunks), /changed after diff/);
});
