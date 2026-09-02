"use strict";

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

  async decide(api, id, approved) {
    const approval = this.pending.find((candidate) => candidate.id === id);
    if (!approval || approval.deciding) return null;
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

module.exports = { ApprovalQueue };
