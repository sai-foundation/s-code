import { describe, expect, it } from "vitest";
import { matchesToolInspection, taskFeedback, toolFailureCause, toolIdFromDetail } from "./task-feedback";

describe("task feedback", () => {
  it("distinguishes generation, running tools, decisions and terminal outcomes", () => {
    expect(taskFeedback("turn.status", "calling_model").label).toBe("Generating");
    expect(taskFeedback("turn.status", "running_tool").label).toBe("Running tool");
    expect(taskFeedback("tool.running").label).toBe("Running tool");
    expect(taskFeedback("approval.required")).toMatchObject({ label: "Waiting for approval", waiting: true, terminal: false });
    expect(taskFeedback("turn.awaiting_input").label).toBe("Waiting for input");
    for (const status of ["completed", "failed", "cancelled"]) {
      expect(taskFeedback(`turn.${status}`).terminal).toBe(true);
      expect(taskFeedback("turn.status", status).terminal).toBe(true);
    }
    expect(taskFeedback("tool.denied")).toMatchObject({ label: "Denied", failure: true, terminal: true });
  });
  it("labels only confirmed local activity locally without inventing model activity", () => {
    expect(taskFeedback("turn.created", "", true)).toMatchObject({ label: "Applying locally", waiting: true });
    expect(taskFeedback("turn.completed", "", true)).toMatchObject({ label: "Completed", terminal: true });
    expect(taskFeedback("turn.created").label).toBe("Generating");
  });
  it("uses reported failures and never mislabels an allow-policy reason as the failure cause", () => {
    expect(toolFailureCause({ error: "Exact failure\nline 2", policy: { reason: "Allowed" } })).toBe("Exact failure\nline 2");
    expect(toolFailureCause({ status: "failed", policy: { reason: "Allowed" } })).toBeNull();
    expect(toolFailureCause({ status: "denied", policy: { reason: "Protected path" } })).toBe("Protected path");
    expect(toolFailureCause({ policy_reason: "Approval rejected" }, true)).toBe("Approval rejected");
    expect(toolFailureCause({ error: { token: "unknown shape" } })).toBeNull();
  });
});

describe("exact tool inspection ownership", () => {
  const prefix = "/v1/sessions/session-a/tools/";
  it("accepts only an ID on the current session's guarded route", () => {
    expect(toolIdFromDetail(`${prefix}tool_01ABC-123`, "session-a")).toBe("tool_01ABC-123");
    for (const href of [undefined, `${prefix}../other`, `${prefix}%2Fetc`, `${prefix}%2E%2E`, `${prefix}%3F`, `${prefix}x/y`, `${prefix}x?scope=other`, `${prefix}x#fragment`, `${prefix}%ZZ`, `${prefix}secret.txt`, "/v1/sessions/session-b/tools/tool-a", "https://example.test/details"]) {
      expect(toolIdFromDetail(href, "session-a")).toBeNull();
    }
  });
  it("rejects fetched details belonging to a different tool, session or account", () => {
    const account = { organization_id: "org", team_id: "team", actor_id: "actor" };
    const call = { request: { id: "tool-a", session_id: "session-a", scope: account } };
    expect(matchesToolInspection(call, "tool-a", "session-a", account)).toBe(true);
    expect(matchesToolInspection(call, "tool-b", "session-a", account)).toBe(false);
    expect(matchesToolInspection(call, "tool-a", "session-b", account)).toBe(false);
    expect(matchesToolInspection(call, "tool-a", "session-a", { ...account, actor_id: "other" })).toBe(false);
    expect(matchesToolInspection({}, "tool-a", "session-a", account)).toBe(false);
  });
});
