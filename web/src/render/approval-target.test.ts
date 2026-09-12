import { describe, expect, it } from "vitest";
import { approvalFilePaths } from "./approval-target";

describe("complete approval targets", () => {
  it("keeps all 32 paths including long, similar names and HTML-like text", () => {
    const paths = Array.from({ length: 32 }, (_, index) => `${"nested/".repeat(30)}file-${index}<tag>.ts`);
    expect(approvalFilePaths(JSON.stringify(paths))).toEqual(paths);
  });
  it("preserves filename boundaries for quotes and newlines", () => {
    const paths = ['file"one.ts', "file\ntwo.ts", "第三个.ts", "four.ts"];
    expect(approvalFilePaths(JSON.stringify(paths))).toEqual(paths);
  });
  it("retains legacy and unrecognized targets instead of hiding them", () => {
    for (const target of ["legacy.ts", '["partial",', "[]", '["a", 2]']) {
      expect(approvalFilePaths(target)).toEqual([target]);
    }
  });
});
