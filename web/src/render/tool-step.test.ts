import { describe, expect, it } from "vitest";
import { canBindToolProposal } from "./tool-step";

describe("tool lifecycle identity", () => {
  it("never reuses a nested denied call for a later direct call", () => {
    const child = { parentToolCallId: "program-1", proposed: "false", turnId: "turn-1", tool: "read_file" };
    expect(canBindToolProposal(child, "turn-1", "read_file")).toBe(false);
    expect(canBindToolProposal({ ...child, proposed: "true" }, "turn-1", "read_file")).toBe(false);
  });
  it("only binds a matching top-level pending proposal", () => {
    const proposed = { proposed: "true", turnId: "turn-1", tool: "read_file" };
    expect(canBindToolProposal(proposed, "turn-1", "read_file")).toBe(true);
    expect(canBindToolProposal(proposed, "turn-2", "read_file")).toBe(false);
    expect(canBindToolProposal(proposed, "turn-1", "search_text")).toBe(false);
    expect(canBindToolProposal({ ...proposed, proposed: "false" }, "turn-1", "read_file")).toBe(false);
  });
});
