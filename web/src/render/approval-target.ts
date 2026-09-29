import type { ApprovalRequest } from "../models/protocol";

// New batch projections encode a complete path array in the existing string
// field. Historical single-file projections remain readable as plain text.
export function approvalFilePaths(target: string): string[] {
  try {
    const value: unknown = JSON.parse(target);
    if (Array.isArray(value) && value.length > 0 && value.every((path) => typeof path === "string")) {
      return value;
    }
  } catch { /* Legacy target text. */ }
  return [target];
}

// Operations that may be allowed only when their complete target is shown
// without truncation. Without a target to show, they can only be rejected.
const EXACT_TARGET_TOOLS: ReadonlySet<string> = new Set(["web_open"]);

export const MISSING_EXACT_TARGET = "The exact target is missing, so this request can only be rejected.";

export interface ReviewedApproval {
  tool?: unknown;
  target?: unknown;
}

export function requiresExactTarget(tool: unknown): boolean {
  return typeof tool === "string" && EXACT_TARGET_TOOLS.has(tool);
}

export function exactTarget(target: unknown): string | null {
  return typeof target === "string" && target.trim() !== "" ? target : null;
}

export function approvalCanBeAllowed(request: ReviewedApproval | null | undefined): boolean {
  return !requiresExactTarget(request?.tool) || exactTarget(request?.target) !== null;
}

export function appendApprovalTarget(container: HTMLElement, request: Pick<ApprovalRequest, "tool" | "target">): void {
  if (requiresExactTarget(request.tool)) {
    const target = exactTarget(request.target);
    const line = document.createElement("span");
    line.className = "approval-exact-target";
    line.textContent = target === null ? MISSING_EXACT_TARGET : `Target: ${target}`;
    // The whole approval wraps, so that no part of the target is cut off.
    container.classList.add("exact-target-approval");
    container.append(line);
    return;
  }
  if (!request.target) return;
  if (request.tool !== "apply_patch") {
    const target = document.createElement("span");
    target.textContent = `Target: ${request.target}`;
    container.append(target);
    return;
  }
  const paths = approvalFilePaths(request.target);
  const details = document.createElement("details");
  details.className = "approval-targets";
  const summary = document.createElement("summary");
  summary.textContent = `View all ${paths.length} file${paths.length === 1 ? "" : "s"}`;
  const list = document.createElement("ul");
  for (const path of paths) {
    const item = document.createElement("li");
    const code = document.createElement("code");
    // Render unusual filename characters visibly, without allowing them to
    // create fake list entries or treating filenames as HTML.
    code.textContent = /[\u0000-\u001f\u007f]/u.test(path) ? JSON.stringify(path) : path;
    item.append(code);
    list.append(item);
  }
  details.append(summary, list);
  container.append(details);
}
