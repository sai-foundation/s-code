import { describe, expect, it } from "vitest";
import { diffFiles, diffPage } from "./diff-view";

describe("bounded diff inspection", () => {
  it("assigns old and new line numbers without counting file headers as edits", () => {
    const [file] = diffFiles("diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -4,2 +4,2 @@\n same\n-old\n+new\n\\ No newline at end of file");
    expect([file.additions, file.deletions]).toEqual([1, 1]);
    expect(file.lines.slice(4).map(line => [line.oldLine, line.newLine])).toEqual([[4, 4], [5, null], [null, 5], [null, null]]);
  });
  it("treats header-looking text inside a hunk as actual changed lines", () => {
    const [file] = diffFiles("diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -10,2 +20,2 @@\n--- flag\n+++ flag\n context");
    expect([file.additions, file.deletions]).toEqual([1, 1]);
    expect(file.lines[4]).toMatchObject({ kind: "delete", oldLine: 10, newLine: null });
    expect(file.lines[5]).toMatchObject({ kind: "add", oldLine: null, newLine: 20 });
    expect(file.lines[6]).toMatchObject({ oldLine: 11, newLine: 21 });
  });
  it("keeps every line reachable with at most 200 rendered rows per page", () => {
    const [file] = diffFiles(`diff --git a/a b/a\n@@ -0,0 +1,500 @@\n${Array.from({ length: 500 }, (_, i) => `+line ${i}`).join("\n")}`);
    const pages = [0, 1, 2].map(page => diffPage(file, page));
    expect(pages.map(page => page.lines.length)).toEqual([200, 200, 102]);
    expect(pages.flatMap(page => page.lines)).toEqual(file.lines);
    expect(diffPage(file, 99).page).toBe(2);
  });
  it("separates files and preserves quoted names, binary notices and HTML-like content as text", () => {
    const files = diffFiles('diff --git "a/name space" "b/name space"\nBinary files differ\ndiff --git a/b b/b\n@@ -0,0 +1 @@\n+<script>alert(1)</script>');
    expect(files).toHaveLength(2);
    expect(files[0].label).toContain('"a/name space"');
    expect(files[1].lines.at(-1)?.text).toBe("+<script>alert(1)</script>");
    expect(diffFiles("")).toEqual([]);
  });
});
