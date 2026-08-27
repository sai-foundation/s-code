"use strict";

function parseFileHunks(unifiedDiff, path) {
  const lines = unifiedDiff.split("\n");
  const marker = `diff --git a/${path} b/${path}`;
  const start = lines.findIndex((line) => line === marker);
  if (start < 0) throw new Error(`No text diff found for ${path}`);
  const end = lines.findIndex((line, index) => index > start && line.startsWith("diff --git "));
  const section = lines.slice(start, end < 0 ? lines.length : end);
  if (section.some((line) => line === "GIT binary patch" || line.startsWith("Binary files "))) throw new Error("Binary hunks cannot be edited inline.");
  const hunks = [];
  for (let index = 0; index < section.length; index += 1) {
    const match = section[index].match(/^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@(.*)$/);
    if (!match) continue;
    const lines = [];
    index += 1;
    while (index < section.length && !section[index].startsWith("@@ ")) {
      if (!section[index].startsWith("\\ No newline")) lines.push(section[index]);
      index += 1;
    }
    index -= 1;
    hunks.push({
      index: hunks.length,
      header: `@@ -${match[1]},${match[2] || 1} +${match[3]},${match[4] || 1} @@${match[5]}`,
      oldStart: Number(match[1]),
      newStart: Number(match[3]),
      lines,
    });
  }
  if (!hunks.length) throw new Error(`No reviewable hunks found for ${path}`);
  return hunks;
}

function rejectHunks(currentContent, hunks) {
  const finalNewline = currentContent.endsWith("\n");
  const currentLines = currentContent.split("\n");
  if (finalNewline) currentLines.pop();
  for (const hunk of [...hunks].sort((left, right) => right.newStart - left.newStart)) {
    const current = hunk.lines.filter((line) => line.startsWith(" ") || line.startsWith("+")).map((line) => line.slice(1));
    const original = hunk.lines.filter((line) => line.startsWith(" ") || line.startsWith("-")).map((line) => line.slice(1));
    const offset = Math.max(0, hunk.newStart - 1);
    const actual = currentLines.slice(offset, offset + current.length);
    if (actual.length !== current.length || actual.some((line, index) => line !== current[index])) throw new Error(`File changed after diff at ${hunk.header}`);
    currentLines.splice(offset, current.length, ...original);
  }
  return `${currentLines.join("\n")}${finalNewline ? "\n" : ""}`;
}

module.exports = { parseFileHunks, rejectHunks };
