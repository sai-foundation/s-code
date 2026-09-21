import { describe, expect, it } from "vitest";
import { changesAvailability, composerScopes, presenceVisibility, pickerContextMatches } from "./primary-controls";

describe("primary Work controls", () => {
  it("keeps Changes out of Chat even if stale workspace state exists", () => {
    expect(changesAvailability("chat", true, true, false).visible).toBe(false);
  });
  it("keeps Changes discoverable in Work before a workspace is selected", () => {
    expect(changesAvailability("work", false, true, false)).toMatchObject({ visible: true, action: "project" });
  });
  it("offers a protection explanation without enabling Git", () => {
    const availability = changesAvailability("work", true, true, true);
    expect(availability.reason).toContain("file protection");
    expect(availability.action).toBe("protections");
    expect(changesAvailability("work", true, true, false)).toEqual({ visible: true, reason: "", action: null });
  });
  it("fails closed while protection metadata is loading, saving, or unavailable", () => {
    expect(changesAvailability("work", true, true, false, "loading")).toMatchObject({ reason: expect.stringContaining("Loading"), action: null });
    expect(changesAvailability("work", true, true, false, "error")).toMatchObject({ reason: expect.stringContaining("could not be verified"), action: "retry-protections" });
  });
  it("requires reconnection before loading changes", () => {
    expect(changesAvailability("work", true, false, false).action).toBe("connect");
  });
});

describe("context visibility", () => {
  it("hides a lone client until explicitly inspected and hides disconnected clients", () => {
    expect(presenceVisibility(true, true, 1, false)).toBe(false);
    expect(presenceVisibility(true, true, 1, true)).toBe(true);
    expect(presenceVisibility(true, true, 2, false)).toBe(true);
    expect(presenceVisibility(false, true, 2, true)).toBe(false);
    expect(presenceVisibility(true, false, 2, true)).toBe(false);
  });
  it("distinguishes persisted session settings from new-conversation defaults", () => {
    expect(composerScopes(true)).toEqual({ model: "Model · this session", permission: "Permissions · this session" });
    expect(composerScopes(false)).toEqual({ model: "Model · default", permission: "Permissions · next Work" });
  });
});

describe("asynchronous context picker ownership", () => {
  const origin = { generation: 1, sessionId: "work-a", account: "account-a" };
  it("accepts only the same idle authenticated context", () => {
    expect(pickerContextMatches(origin, { ...origin }, true, false)).toBe(true);
    for (const current of [
      { ...origin, generation: 2 }, { ...origin, sessionId: "work-b" },
      { ...origin, sessionId: null }, { ...origin, account: "account-b" },
    ]) expect(pickerContextMatches(origin, current, true, false)).toBe(false);
    expect(pickerContextMatches(origin, origin, false, false)).toBe(false);
    expect(pickerContextMatches(origin, origin, true, true)).toBe(false);
  });
  it("does not apply a new-session default after opening an existing session", () => {
    expect(pickerContextMatches({ ...origin, sessionId: null }, origin, true, false)).toBe(false);
  });
});
