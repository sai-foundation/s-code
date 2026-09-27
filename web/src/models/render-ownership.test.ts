import { describe, expect, it } from "vitest";
import { ownsRenderedItem } from "./render-ownership";

describe("render ownership", () => {
  it("rejects a late response after a reopened prompt replaces its card", () => {
    const submittedCard = {};
    const reopenedCard = {};
    const itemsById = new Map<string, object>([["question-1", submittedCard]]);

    expect(ownsRenderedItem(itemsById, "question-1", submittedCard)).toBe(true);

    itemsById.set("question-1", reopenedCard);

    expect(ownsRenderedItem(itemsById, "question-1", submittedCard)).toBe(false);
    expect(ownsRenderedItem(itemsById, "question-1", reopenedCard)).toBe(true);
  });

  it("rejects a response after its rendered item is removed", () => {
    const submittedCard = {};
    const itemsById = new Map<string, object>([["question-1", submittedCard]]);

    itemsById.delete("question-1");

    expect(ownsRenderedItem(itemsById, "question-1", submittedCard)).toBe(false);
  });
});
