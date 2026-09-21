import { describe, expect, it } from "vitest";
import type { PrivacyRequest, PrivacySource } from "../../generated/protocol";
import { classifyPrivacySource, recordedSources, recordedSourceState, SourceOverviewState } from "./privacy-sources";
import { fileEvidence } from "./privacy-files";
const source = (value: string, kind = "file", content_bytes = 10, partial = false): PrivacySource => ({ source: value, kind, content_bytes, partial });
const request = (sources: PrivacySource[], id = "one", status = "accepted", unattributed: string[] = []): PrivacyRequest => ({ id, sources, status, unattributed, sequence: 1, turn_id: "turn", started_at: "2026-09-20T00:00:00Z", destination: `https://${id}.example.test`, model: "test", purpose: "agent", request_bytes: 1 });

describe("recorded source classification", () => {
  it("separates explicit outside paths and file URIs at workspace component boundaries", () => {
    for (const path of ["/outside/notes.txt", "file:///outside/my%20notes.txt", "/project-sibling/a.txt"]) expect(classifyPrivacySource(source(path), "/project").category).toBe("external");
    expect(classifyPrivacySource(source("file://localhost/project/src/a.ts"), "/project")).toMatchObject({ category: "project", path: "src/a.ts" });
    expect(classifyPrivacySource(source("src/a.ts", "diagnostic"), "/project").category).toBe("project");
    expect(classifyPrivacySource(source("/project/a", "context"), "/project").category).toBe("project");
  });
  it("does not normalize traversing, remote, truncated or nonpath labels into project files", () => {
    for (const path of ["../outside.txt", "/project/../outside.txt", "file:///project/../outside.txt", "file:///project/%2e%2e/outside.txt", "file://remote/project/a", "https://example.test/a", "mcp://example/tool", "/project/path… [label truncated]"]) expect(classifyPrivacySource(source(path), "/project").category).toBe("other");
    expect(classifyPrivacySource(source("conversation summary", "compaction"), "/project").category).toBe("other");
    expect(classifyPrivacySource(source("README.md", "context"), "/project").category).toBe("other");
    expect(fileEvidence([request([source("summary", "compaction"), source("readme", "file")])], "/project").has("summary")).toBe(false);
  });
  it("preserves all source metadata in no-root conversations", () => {
    const records = recordedSources([request([source("/tmp/report"), source("relative.txt"), source("summary", "context"), source("photo.png", "attachment")], "one", "accepted", ["Conversation and pasted text"])], null);
    expect(records).toHaveLength(5);
    expect(records.find(record => record.source === "/tmp/report")?.category).toBe("external");
    expect(records.find(record => record.source === "relative.txt")?.category).toBe("other");
    expect(records.find(record => record.kind === "unattributed")?.unattributed).toBe(true);
  });
  it("keeps attachment labels separate from file identity and zero-byte name-only evidence neutral", () => {
    const records = recordedSources([request([source("/outside/report.txt"), source("report.txt", "attachment"), source("report.txt", "attachment name only", 0), source("attachment://id/report.txt", "file")])], "/project");
    expect(records).toHaveLength(4);
    expect(records.filter(record => record.category === "attachments")).toHaveLength(3);
    const name = records.find(record => record.kind === "attachment name only")!;
    expect(name.state).toBe("none"); expect(recordedSourceState(name)).toContain("Name only");
    expect(fileEvidence([request([source("report.txt", "attachment")])], "/project").size).toBe(0);
  });
  it("aggregates exact kind plus source labels, counting each request once and retaining destinations/statuses", () => {
    const records = recordedSources([request([source("/outside/report", "file excerpt", 20, true), source("/outside/report", "file excerpt", 20, true)]), request([source("/outside/report", "file excerpt", 30, true)], "two", "rejected"), request([source("/elsewhere/report", "file excerpt", 10, true)], "three")], "/project");
    expect(records).toHaveLength(2);
    const record = records.find(record => record.source === "/outside/report")!;
    expect(record.requests.size).toBe(2); expect(record.destinations.size).toBe(2); expect([...record.statuses]).toEqual(["accepted", "rejected"]); expect(record.state).toBe("partial"); expect(record.unknownDelivery).toBe(true);
  });
  it("pages every recorded label, searches beyond page caps, and resets disclosure state", () => {
    const records = recordedSources([request(Array.from({ length: 101 }, (_, i) => source(`/outside/file-${String(i).padStart(3, "0")}.txt`)))], "/project");
    const state = new SourceOverviewState();
    const first = state.page(records, "external", ""); state.pages.set("external", 1); const middle = state.page(records, "external", ""); state.pages.set("external", 2); const last = state.page(records, "external", "");
    expect([first.rows.length, middle.rows.length, last.rows.length]).toEqual([40, 40, 21]); expect([...first.rows, ...middle.rows, ...last.rows]).toEqual(records);
    expect(state.page(records, "external", "file-100").rows[0].source).toBe("/outside/file-100.txt");
    state.expanded.add("external"); state.reset(); expect(state.pages.size).toBe(0); expect(state.expanded.size).toBe(0);
    expect(recordedSources([], null)).toEqual([]);
  });
});
