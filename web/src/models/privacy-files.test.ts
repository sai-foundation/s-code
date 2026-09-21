import { describe, expect, it } from "vitest";
import type { PrivacyRequest, PrivacySource } from "../../generated/protocol";
import { fileEvidence, fileRows, relativeSource, workspaceRoot, PrivacyFiles, type DirectoryPage } from "./privacy-files";
const source = (path: string, partial = false): PrivacySource => ({ source: path, partial, kind: "file", content_bytes: 10 });
const request = (id: string, status: string, sources: PrivacySource[]): PrivacyRequest => ({ id, status, sources, sequence: Number(id), turn_id: "turn", started_at: "2026-09-20T00:00:00Z", destination: "https://example.test", model: "test", purpose: "agent", request_bytes: 1, unattributed: [] });
describe("file attribution", () => {
  it("maps local paths without conflating attachments, zero bytes, remote URIs or another workspace", () => {
    expect(workspaceRoot("file:///tmp/my%20project")).toBe("/tmp/my project");
    expect(workspaceRoot("file://remote/tmp/repo")).toBeNull();
    expect(relativeSource(source("file:///tmp/repo/src/a.ts"), "/tmp/repo")).toBe("src/a.ts");
    for (const path of ["/tmp/repository/a.ts", "../a.ts", "https://example.test/a.ts", "file://remote/tmp/repo/a.ts"]) expect(relativeSource(source(path), "/tmp/repo")).toBeNull();
    expect(relativeSource({ ...source("a.ts"), kind: "attachment_text" }, "/tmp/repo")).toBeNull();
    expect(relativeSource({ ...source("a.ts"), content_bytes: 0 }, "/tmp/repo")).toBeNull();
  });
  it("only full accepted captured text is red; unknown/rejected delivery remains orange", () => {
    const values = fileEvidence([request("1", "accepted", [source("full"), source("part", true)]), request("2", "rejected", [source("rejected"), source("full")]), request("3", "connection_error", [source("unknown")])], "/tmp/repo");
    expect(values.get("full")?.state).toBe("entire"); expect(values.get("full")?.requests.size).toBe(2);
    for (const name of ["part", "rejected", "unknown"]) expect(values.get(name)?.state).toBe("partial");
  });
  it("includes recorded missing paths, avoids following replacement links, and globally caps visible rows", () => {
    const pages = new Map<string, DirectoryPage>([["", { entries: [{ path: "link", kind: "other", size: null }, ...Array.from({ length: 1000 }, (_, i) => ({ path: `file${i}`, kind: "file" as const, size: 0 }))], truncated: false }]]);
    const evidence = fileEvidence([request("1", "accepted", [source("link/old.ts"), source("deleted/deep/file.ts")])], "/repo");
    const all = fileRows(pages, evidence, new Set(["link", "deleted", "deleted/deep"]), "");
    expect(all.rows.length).toBe(200); expect(all.total).toBe(1005);
    expect(all.rows.find(row => row.path === "link")?.historical).toBe(true);
    const search = fileRows(pages, evidence, new Set(), "file.ts");
    expect(search.rows.map(row => row.path)).toEqual(["deleted", "deleted/deep", "deleted/deep/file.ts"]);
  });
  it("discards old folder results on reset even when abort is ignored", async () => {
    let resolve!: (page: DirectoryPage) => void; let signal!: AbortSignal;
    const files = new PrivacyFiles(() => {});
    files.reset("/old", async (_, abort) => { signal = abort; return new Promise(done => resolve = done); });
    files.reset(); resolve({ entries: [{ path: "secret", kind: "file", size: 1 }], truncated: false });
    await Promise.resolve(); await Promise.resolve();
    expect(signal.aborted).toBe(true); expect(files.pages.size).toBe(0);
  });
  it("does not eagerly crawl directory children and rejects malformed listing paths", async () => {
    const paths: string[] = [];
    const files = new PrivacyFiles(() => {});
    files.reset("/repo", async path => { paths.push(path); return { entries: [{ path: "folder", kind: "directory", size: null }], truncated: false }; });
    await Promise.resolve(); await Promise.resolve();
    expect(paths).toEqual([""]);
    await files.scan("folder"); expect(files.error).toContain("Invalid folder listing");
  });
});
