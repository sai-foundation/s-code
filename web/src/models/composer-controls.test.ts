import { describe, expect, it } from "vitest";
import { composerControls, pendingInputLabel, sameComposerContext, nextTurnInputAttempt, pendingInputsOwned } from "./composer-controls";

describe("running composer controls", () => {
  it("makes Queue, Steer and Stop distinct and blocks unsupported input", () => {
    expect(composerControls("follow up", true, false, true, false)).toMatchObject({ label: "Queue", steer: true, stop: true, disabled: false });
    expect(composerControls("", true, false, true, false)).toMatchObject({ label: "Stop", steer: false, stop: false });
    expect(composerControls("next", true, false, false, false)).toMatchObject({ label: "Queue", steer: false, disabled: true });
    expect(composerControls("next", true, true, true, false).disabled).toBe(true);
    expect(composerControls("next", true, false, true, true).disabled).toBe(true);
  });
  it("keeps local protection usable while a task runs without queue support", () => {
    expect(composerControls("/protect /repo/private", true, false, false, false)).toMatchObject({ label: "Protect", disabled: false, steer: false });
    expect(composerControls("protect this topic", true, false, true, false).label).toBe("Queue");
  });
  it("does not claim that a pending instruction was already applied", () => {
    expect(pendingInputLabel("steer", 0)).toBe("Guidance pending");
    expect(pendingInputLabel("queue", 1)).toBe("Follow-up 2 · pending");
  });
  it("rejects stale queue results after account, connection, or session changes", () => {
    const origin = { generation: 1, sessionId: "one", account: "alice" };
    expect(sameComposerContext(origin, { ...origin })).toBe(true);
    for (const current of [{ ...origin, generation: 2 }, { ...origin, sessionId: "two" }, { ...origin, account: "bob" }]) expect(sameComposerContext(origin, current)).toBe(false);
  });
});


describe("queued input acknowledgement recovery", () => {
  const context = { account: "alice", sessionId: "session-a", turnId: "turn-a", mode: "queue", content: "follow up" };
  it("retains the idempotency key after an uncertain failure and creates a new key after confirmation", () => {
    const first = nextTurnInputAttempt(null, context, () => "first");
    expect(nextTurnInputAttempt(first, context, () => "duplicate")).toBe(first);
    expect(nextTurnInputAttempt(null, context, () => "confirmed-next").key).toBe("confirmed-next");
  });
  it("never reuses a key for changed account, task, target turn, content or mode", () => {
    const first = nextTurnInputAttempt(null, context, () => "first");
    for (const changed of [{ account: "bob" }, { sessionId: "session-b" }, { turnId: "turn-b" }, { content: "different" }, { mode: "steer" }]) {
      expect(nextTurnInputAttempt(first, { ...context, ...changed }, () => "new").key).toBe("new");
    }
  });
  it("accepts only complete lists from the requested session and account", () => {
    const scope = { organization_id: "org", team_id: "team", actor_id: "actor" };
    const input = { session_id: "session-a", scope };
    expect(pendingInputsOwned([input], "session-a", scope)).toBe(true);
    expect(pendingInputsOwned([], "session-a", scope)).toBe(true);
    expect(pendingInputsOwned([input, { ...input, session_id: "session-b" }], "session-a", scope)).toBe(false);
    expect(pendingInputsOwned([input], "session-a", { ...scope, actor_id: "other" })).toBe(false);
    expect(pendingInputsOwned([{}], "session-a", scope)).toBe(false);
    expect(pendingInputsOwned(null, "session-a", scope)).toBe(false);
  });
});
