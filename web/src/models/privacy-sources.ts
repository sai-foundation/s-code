import type { PrivacyRequest, PrivacySource } from "../../generated/protocol";

export type SourceCategory = "project" | "external" | "attachments" | "other";
export type SourceClassification = { category: SourceCategory; path?: string; reason: string };
const fileKinds = new Set(["file", "file text", "file excerpt", "project_instructions", "selection", "diagnostic"]);

/** Lexical attribution only: never resolves symlinks or touches a recorded path. */
export function classifyPrivacySource(source: PrivacySource, root: string | null): SourceClassification {
  const raw = source.source;
  if (source.kind.startsWith("attachment") || raw.startsWith("attachment://")) return { category: "attachments", reason: "Recorded attachment label; its name does not establish a local path or unique file identity." };
  const unresolved = (reason: string): SourceClassification => ({ category: "other", reason });
  if (!raw || raw.includes("\0") || raw.endsWith("… [label truncated]")) return unresolved("The recorded label does not identify a complete local path.");
  let path = raw;
  if (raw.startsWith("file:")) {
    try {
      // URL normalizes dot segments, so inspect decoded input first.
      if (decodeURIComponent(raw).split(/[\\/]/).includes("..")) return unresolved("Relative traversal cannot establish a file's location.");
      const url = new URL(raw);
      if (url.hostname && url.hostname !== "localhost") return unresolved("Remote file URI; no local path is established.");
      if (url.search || url.hash) return unresolved("File URI includes non-path metadata.");
      path = decodeURIComponent(url.pathname);
    } catch { return unresolved("The file URI could not be interpreted as a local path."); }
  } else if (/^[a-z][a-z\d+.-]*:/i.test(raw)) return unresolved("Remote URI or non-file context source.");
  const parts = path.split("/").filter(part => part && part !== ".");
  if (!parts.length || parts.includes("..") || path.includes("\0") || path.includes("\\")) return unresolved("The recorded path is unresolved; no local file identity is assumed.");
  if (path.startsWith("/")) {
    path = `/${parts.join("/")}`;
    const prefix = root === "/" ? "/" : root ? `${root.replace(/\/$/, "")}/` : null;
    if (!prefix || !path.startsWith(prefix)) return { category: "external", path, reason: root ? "Recorded path outside this project. Only ledger metadata is shown; this path is never browsed." : "Recorded absolute path. This conversation has no project root; only ledger metadata is shown." };
    if (source.content_bytes <= 0) return unresolved("Only source metadata is recorded; no content bytes are attributed.");
    return { category: "project", path: path.slice(prefix.length), reason: "Recorded project file path." };
  }
  if (!fileKinds.has(source.kind)) return unresolved("A context label is not assumed to be a project-relative file.");
  if (!root) return unresolved("Relative file path without a project root; its location is unknown.");
  if (source.content_bytes <= 0) return unresolved("Only source metadata is recorded; no content bytes are attributed.");
  return { category: "project", path: parts.join("/"), reason: "Recorded project-relative file path." };
}

export interface RecordedSource {
  key: string;
  source: string;
  kind: string;
  category: Exclude<SourceCategory, "project">;
  reason: string;
  path?: string;
  requests: Set<string>;
  destinations: Set<string>;
  statuses: Set<string>;
  state: "none" | "partial" | "entire";
  contentBytes: number;
  unknownDelivery: boolean;
  unattributed: boolean;
}
export function recordedSources(requests: PrivacyRequest[], root: string | null): RecordedSource[] {
  const records = new Map<string, RecordedSource>();
  for (const request of requests) {
    const sources = [...request.sources.map(source => ({ ...source, unattributed: false })), ...request.unattributed.map(source => ({ source, kind: "unattributed", partial: true, content_bytes: 0, unattributed: true }))];
    for (const source of sources) {
      const classification = source.unattributed ? { category: "other" as const, reason: "Included context without individually traceable file sources." } : classifyPrivacySource(source, root);
      if (classification.category === "project") continue;
      const key = JSON.stringify([source.unattributed, source.kind, source.source]);
      const record = records.get(key) ?? { key, source: source.source, kind: source.kind, category: classification.category, reason: classification.reason, path: classification.path, requests: new Set<string>(), destinations: new Set<string>(), statuses: new Set<string>(), state: "none", contentBytes: 0, unknownDelivery: false, unattributed: source.unattributed };
      record.requests.add(request.id); record.destinations.add(request.destination); record.statuses.add(request.status);
      record.contentBytes = Math.max(record.contentBytes, source.content_bytes);
      if (source.content_bytes > 0) {
        if (request.status === "accepted" && !source.partial) record.state = "entire";
        else if (record.state !== "entire") record.state = "partial";
        if (request.status !== "accepted") record.unknownDelivery = true;
      }
      records.set(key, record);
    }
  }
  return [...records.values()].sort((a, b) => a.source.localeCompare(b.source) || a.kind.localeCompare(b.kind));
}
export const SOURCE_OVERVIEW_PAGE_SIZE = 40;
export class SourceOverviewState {
  expanded = new Set<RecordedSource["category"]>();
  pages = new Map<RecordedSource["category"], number>();
  reset() { this.expanded.clear(); this.pages.clear(); }
  page(records: RecordedSource[], category: RecordedSource["category"], query: string) {
    const needle = query.trim().toLowerCase();
    const all = records.filter(record => record.category === category);
    const matching = all.filter(record => !needle || [record.source, record.path ?? "", record.kind, ...record.destinations, ...record.statuses].some(value => value.toLowerCase().includes(needle)));
    const pages = Math.max(1, Math.ceil(matching.length / SOURCE_OVERVIEW_PAGE_SIZE));
    const page = Math.max(0, Math.min(this.pages.get(category) ?? 0, pages - 1)); this.pages.set(category, page);
    return { all, matching, page, pages, rows: matching.slice(page * SOURCE_OVERVIEW_PAGE_SIZE, (page + 1) * SOURCE_OVERVIEW_PAGE_SIZE) };
  }
}
export function recordedSourceState(record: RecordedSource): string {
  if (record.unattributed) return "Context recorded · file attribution unavailable";
  if (record.contentBytes === 0) return record.category === "attachments" ? "Name only · no attachment content recorded" : "Metadata only · no content bytes attributed";
  if (record.state === "entire") return "Full captured content · endpoint accepted";
  return record.unknownDelivery ? "Partial or delivery unknown" : "Partial captured content · endpoint accepted";
}
