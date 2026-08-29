import { describe, expect, it } from "vitest";
import { parseRoute, teamRoute } from "./router";

describe("Team routes", () => {
  it("maps every durable Team section to a restorable route", () => {
    const sections = [
      "overview",
      "work",
      "goals",
      "agents",
      "capacity",
      "ownership",
      "budgets",
      "approvals",
      "outcomes",
      "audit",
    ] as const;
    sections.forEach((section) => {
      expect(parseRoute(teamRoute(section))).toEqual({ type: "team", section });
    });
  });

  it("fails closed to Home for unknown Team sections", () => {
    expect(parseRoute("/team/internal-debug")).toEqual({ type: "home" });
  });
});
