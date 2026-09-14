import { describe, expect, it } from "vitest";
import { applyWorkTransition, hasWorkspace, newSessionWorkspace, sessionMode } from "./session-mode";

describe("Chat and Work boundaries", () => {
  it("never passes an old default directory into a new Chat", () => {
    expect(newSessionWorkspace("chat", "file:///private/project")).toBe("");
    expect(newSessionWorkspace("work", "")).toBe("");
    expect(newSessionWorkspace("work", " file:///chosen/project ")).toBe("file:///chosen/project");
  });

  it("keeps legacy workspace sessions in Work and gives Chat no file controls", () => {
    expect(sessionMode({ workspace_uri: "file:///project" })).toBe("work");
    expect(hasWorkspace({ mode: "chat", workspace_uri: "file:///stale" })).toBe(false);
    expect(hasWorkspace({ mode: "work", workspace_uri: "" })).toBe(false);
    expect(hasWorkspace(null)).toBe(false);
  });

  it("promotes the same session without losing conversation fields", () => {
    const session = { id: "chat-a", mode: "chat" as const, workspace_uri: "", title: "Our plan" };
    const updated = applyWorkTransition(session, "chat-a", { mode: "work", workspace_uri: "file:///managed/a" });
    expect(updated).toEqual({ ...session, mode: "work", workspace_uri: "file:///managed/a" });
    expect(session.workspace_uri).toBe("");
    expect(hasWorkspace(updated)).toBe(true);
  });

  it("ignores events for other sessions, missing directories, and downgrades", () => {
    const session = { id: "work-a", mode: "work" as const, workspace_uri: "file:///managed/a" };
    expect(applyWorkTransition(session, "chat-b", { mode: "work", workspace_uri: "file:///b" })).toBe(session);
    expect(applyWorkTransition(session, "work-a", { mode: "work", workspace_uri: "" })).toBe(session);
    expect(applyWorkTransition(session, "work-a", { mode: "chat", workspace_uri: "" })).toBe(session);
  });
});
