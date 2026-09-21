import { describe, expect, it } from "vitest";
import { privacyStatus, privacyEventPage, privacySourcePage } from "./privacy";

describe("privacy delivery labels", () => {
  it("never treats a pending or failed dispatch as proof of delivery or non-delivery", () => {
    expect(privacyStatus("accepted")).toBe("Accepted by endpoint");
    expect(privacyStatus("rejected")).toContain("may have been received");
    expect(privacyStatus("connection_error")).toContain("unknown");
    expect(privacyStatus("attempted")).toContain("not confirmed");
    expect(privacyStatus("future-status")).toContain("not confirmed");
  });
});

import { privacyFileIndex } from "./privacy";
import type { PrivacyRequest } from "../../generated/protocol";
it("lists complete source names and counts once per request without implying successful delivery", () => {
  const path = `${"very-long/".repeat(50)}<script>.rs`;
  const source = { source: path, kind: "file excerpt", content_bytes: 10, partial: true };
  const request: PrivacyRequest = { id: "first", sequence: 1, turn_id: "turn", started_at: "2026-09-20T00:00:00Z", destination: "https://example.test", model: "fixture", purpose: "agent", status: "attempted", request_bytes: 10, sources: [source, source], unattributed: [] };
  const files = privacyFileIndex([request, { ...request, id: "second", status: "rejected" }]);
  expect(files).toHaveLength(1);
  expect(files[0].source).toBe(path);
  expect(files[0].requests.size).toBe(2);
  expect([...files[0].statuses]).toEqual(["Request started · delivery not confirmed", "Rejected by endpoint · data may have been received"]);
});

it("keeps every loaded older event reachable through bounded pages", () => {
  const requests = Array.from({ length: 451 }, (_, i) => ({ id: String(451 - i), sequence: 451 - i } as PrivacyRequest));
  const first = privacyEventPage(requests, 0);
  const middle = privacyEventPage(requests, 1);
  const last = privacyEventPage(requests, 2);
  expect([first.requests.length, middle.requests.length, last.requests.length]).toEqual([200, 200, 51]);
  expect([...first.requests, ...middle.requests, ...last.requests]).toEqual(requests);
  expect(last.requests.at(-1)?.sequence).toBe(1);
  expect(privacyEventPage(requests.slice(0, 1), 2).page).toBe(0);
  expect(privacyEventPage([], 4).requests).toEqual([]);
});

it("keeps sources and unattributed context after item 100 reachable without unbounded lists", () => {
  const sources = Array.from({ length: 251 }, (_, index) => ({ source: `file-${index + 1}.ts`, kind: "file", partial: false, content_bytes: 1 }));
  const pages = [0, 1, 2].map(index => privacySourcePage(sources, index));
  expect(pages.map(page => page.sources.length)).toEqual([100, 100, 51]);
  expect(pages.flatMap(page => page.sources)).toEqual(sources);
  expect(pages[2].sources.at(-1)?.source).toBe("file-251.ts");
  const context = sources.map(source => source.source);
  expect(privacySourcePage(context, 2).sources.at(-1)).toBe("file-251.ts");
  expect(privacySourcePage(sources.slice(0, 4), 2).page).toBe(0);
});
