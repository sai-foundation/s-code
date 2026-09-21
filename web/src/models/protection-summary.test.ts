import { describe, expect, it } from "vitest";
import { protectionGroups, protectionPath } from "./protection-summary";
import { fileEvidence } from "./privacy-files";
import type { ProtectionRule } from "./privacy-protections";
import type { PrivacyRequest } from "../../generated/protocol";

const rule = (path: string, canonical_path: string | null = null): ProtectionRule => ({ id: path, path, canonical_path, kind: "file", created_at: "now", device: null, inode: null });
describe("protected path summary", () => {
  it("groups only true project descendants and preserves complete names", () => {
    const rules = [rule("/repo/private/config.json"), rule("/repo-other/config.json"), rule("/external/secret.json")];
    expect(protectionGroups(rules, "/repo").map(group => [group.label, group.rules.length])).toEqual([["Project", 1], ["External", 2]]);
    expect(protectionPath(rules[0], "/repo")).toEqual({ group: "Project", name: "config.json", location: "private/config.json", fullPath: "/repo/private/config.json" });
    expect(protectionGroups(rules, null).map(group => group.label)).toEqual(["External"]);
  });
  it("does not present an external symlink target as project-local", () => {
    expect(protectionPath(rule("/repo/link", "/outside/private"), "/repo")).toEqual({ group: "External", name: "link", location: "/outside/private", fullPath: "/repo/link\nResolved: /outside/private" });
    expect(protectionPath(rule("/"), "/").location).toBe(".");
  });
  it("keeps every rule available for bounded rendering and full management", () => {
    const rules = Array.from({ length: 120 }, (_, index) => rule(`/repo/${index}/same-name.txt`));
    expect(protectionGroups(rules, "/repo")[0].rules).toEqual(rules);
    expect(protectionPath(rules[119], "/repo").fullPath).toBe("/repo/119/same-name.txt");
  });
  it("keeps historical transfer evidence independent from present protection", () => {
    const request = { id: "request", status: "accepted", sources: [{ source: "/repo/private.txt", kind: "file", content_bytes: 5, partial: false }] } as PrivacyRequest;
    const before = fileEvidence([request], "/repo");
    protectionGroups([rule("/repo/private.txt")], "/repo");
    expect(fileEvidence([request], "/repo")).toEqual(before);
    expect(before.get("private.txt")?.state).toBe("entire");
  });
});
