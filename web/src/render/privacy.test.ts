import { describe, expect, it } from "vitest";
import { privacyStatus } from "./privacy";

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
