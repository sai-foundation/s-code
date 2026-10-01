"use strict";

// Operations that may be approved only after their complete target was shown.
const EXACT_TARGET_TOOLS = new Set(["web_open", "pdf_read"]);
const MISSING_EXACT_TARGET = "The exact target is missing, so this request can only be rejected.";

function requiresExactTarget(tool) {
  return typeof tool === "string" && EXACT_TARGET_TOOLS.has(tool);
}

function exactTarget(target) {
  return typeof target === "string" && target.trim() !== "" ? target : null;
}

// Names an approval in a notification and in the status bar. A complete
// target is left to the dialog that shows all of it.
function approvalSummary(tool, summary, target) {
  if (requiresExactTarget(tool) || !target || summary.includes(target)) return summary;
  return `${summary} · ${target}`;
}

class ApprovalQueue {
  constructor() { this.pending = []; }

  add(approval) {
    if (!this.pending.some((candidate) => candidate.id === approval.id)) this.pending.push({ ...approval, deciding: false });
  }

  first() { return this.pending.find((approval) => !approval.deciding) || null; }

  replace(approvals) {
    this.pending = approvals.map((approval) => ({ ...approval, deciding: false }));
  }

  removeById(id) {
    if (!id) return;
    this.pending = this.pending.filter((approval) => approval.id !== id);
  }

  removeByToolCall(toolCallId) {
    if (!toolCallId) return;
    this.pending = this.pending.filter((approval) => approval.toolCallId !== toolCallId);
  }

  // `reviewedTarget` is the target the user was shown in full before approving.
  async decide(api, id, approved, reviewedTarget = null) {
    const approval = this.pending.find((candidate) => candidate.id === id);
    if (!approval || approval.deciding) return null;
    if (approved && requiresExactTarget(approval.tool)
      && (exactTarget(approval.target) === null || reviewedTarget !== approval.target)) {
      throw new Error("This request can be approved only after its complete target was shown.");
    }
    approval.deciding = true;
    try {
      await api.approval(approval.id, approved);
      this.pending = this.pending.filter((candidate) => candidate !== approval);
      return approval;
    } catch (error) {
      approval.deciding = false;
      throw error;
    }
  }
}

module.exports = { ApprovalQueue, MISSING_EXACT_TARGET, approvalSummary, exactTarget, requiresExactTarget };
