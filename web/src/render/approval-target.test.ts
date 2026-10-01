import { afterEach, describe, expect, it, vi } from "vitest";
import {
  MISSING_EXACT_TARGET,
  appendApprovalTarget,
  approvalCanBeAllowed,
  approvalFilePaths,
} from "./approval-target";

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

describe("exact approval targets", () => {
  afterEach(() => vi.unstubAllGlobals());

  function renderedLines(request: { tool: string; target: string | null }) {
    const lines: unknown[] = [];
    vi.stubGlobal("document", { createElement: () => ({ className: "", textContent: "" }) });
    const container = { append: (...nodes: unknown[]) => lines.push(...nodes), classList: { add: () => {} } };
    appendApprovalTarget(container as unknown as HTMLElement, request);
    return lines;
  }

  it("lets public web and PDF reads be allowed only with a target to review", () => {
    for (const tool of ["web_open", "pdf_read"]) {
      expect(approvalCanBeAllowed({ tool, target: "https://example.com/docs" })).toBe(true);
      for (const target of [null, undefined, "", "  ", 7]) {
        expect(approvalCanBeAllowed({ tool, target })).toBe(false);
      }
      expect(approvalCanBeAllowed({ tool })).toBe(false);
    }
  });
  it("renders the complete target of public web and PDF reads", () => {
    const target = `https://www.example.com.${"padding.".repeat(40)}evil.test/docs`;
    for (const tool of ["web_open", "pdf_read"]) {
      expect(renderedLines({ tool, target })).toEqual([
        { className: "approval-exact-target", textContent: `Target: ${target}` },
      ]);
    }
  });
  it("explains a public web or PDF read that names no target", () => {
    for (const tool of ["web_open", "pdf_read"]) {
      for (const target of [null, "", "  "]) {
        expect(renderedLines({ tool, target })).toEqual([
          { className: "approval-exact-target", textContent: MISSING_EXACT_TARGET },
        ]);
      }
    }
  });
  it("leaves every other approval as it was", () => {
    for (const tool of ["run_command", "apply_patch", "git_commit", ""]) {
      expect(approvalCanBeAllowed({ tool, target: null })).toBe(true);
    }
    expect(renderedLines({ tool: "run_command", target: "cargo test" })).toEqual([
      { className: "", textContent: "Target: cargo test" },
    ]);
    expect(renderedLines({ tool: "run_command", target: null })).toEqual([]);
  });
});
