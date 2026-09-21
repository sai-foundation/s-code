import type { PrivacyRequest, PrivacySource } from "../../generated/protocol";
import { classifyPrivacySource } from "./privacy-sources";

export type FileState = "none" | "partial" | "entire";
export interface DirectoryEntry { path: string; kind: "file" | "directory" | "other"; size: number | null }
export interface DirectoryPage { entries: DirectoryEntry[]; truncated: boolean }
export interface FileEvidence { state: FileState; requests: Set<string>; unknownDelivery: boolean }
export interface FileRow extends DirectoryEntry { depth: number; expanded: boolean; evidence?: FileEvidence; historical?: boolean }
export const PRIVACY_ROW_LIMIT = 200;

export function workspaceRoot(uri: string): string | null {
  if (!uri) return null;
  if (uri.startsWith("/")) return uri.replace(/\/$/, "") || "/";
  try { const url = new URL(uri); return url.protocol === "file:" && (!url.hostname || url.hostname === "localhost") ? decodeURIComponent(url.pathname).replace(/\/$/, "") || "/" : null; } catch { return null; }
}
export function relativeSource(source: PrivacySource, root: string): string | null {
  const classification = classifyPrivacySource(source, root);
  return classification.category === "project" ? classification.path ?? null : null;
}
export function fileEvidence(requests: PrivacyRequest[], root: string): Map<string, FileEvidence> {
  const result = new Map<string, FileEvidence>();
  for (const request of requests) for (const source of request.sources) {
    const path = relativeSource(source, root); if (!path) continue;
    const item = result.get(path) ?? { state: "none", requests: new Set<string>(), unknownDelivery: false };
    item.requests.add(request.id);
    if (request.status === "accepted") { if (!source.partial) item.state = "entire"; else if (item.state === "none") item.state = "partial"; }
    else { item.unknownDelivery = true; if (item.state === "none") item.state = "partial"; }
    result.set(path, item);
  }
  return result;
}
export function fileStatus(row: FileRow): string {
  if (row.kind === "directory") return row.historical ? "Recorded folder · not browsable" : "Folder";
  if (row.evidence?.state === "entire") return "Entire file · captured version";
  if (row.evidence?.state === "partial") return row.evidence.unknownDelivery ? "Partial or delivery unknown" : "Partial text recorded";
  return row.kind === "other" ? "No recorded transmission · link not followed" : "No recorded transmission";
}
export function fileRows(pages: Map<string, DirectoryPage>, evidence: Map<string, FileEvidence>, expanded: Set<string>, query: string): { rows: FileRow[]; total: number } {
  const entries = new Map<string, DirectoryEntry>();
  pages.forEach(page => page.entries.forEach(entry => entries.set(entry.path, entry)));
  const historical = new Set<string>();
  for (const path of evidence.keys()) {
    const parts = path.split("/");
    parts.forEach((_, i) => {
      const parent = parts.slice(0, i + 1).join("/");
      const existing = entries.get(parent);
      if (!existing) entries.set(parent, { path: parent, kind: i < parts.length - 1 ? "directory" : "file", size: 0 });
      else if (i < parts.length - 1 && existing.kind !== "directory") { historical.add(parent); entries.set(parent, { ...existing, kind: "directory" }); }
    });
  }
  const children = new Map<string, DirectoryEntry[]>();
  const matching = new Set<string>(), needle = query.trim().toLowerCase();
  for (const entry of entries.values()) {
    const parent = entry.path.split("/").slice(0, -1).join("/");
    const siblings = children.get(parent) ?? []; siblings.push(entry); children.set(parent, siblings);
    if (needle && entry.path.toLowerCase().includes(needle)) { const parts = entry.path.split("/"); parts.forEach((_, i) => matching.add(parts.slice(0, i + 1).join("/"))); }
  }
  children.forEach(list => list.sort((a, b) => Number(b.kind === "directory") - Number(a.kind === "directory") || a.path.localeCompare(b.path)));
  const stack = (children.get("") ?? []).toReversed().map(entry => ({ entry, depth: 0 }));
  const rows: FileRow[] = []; let total = 0;
  while (stack.length) {
    const { entry, depth } = stack.pop()!;
    if (needle && !matching.has(entry.path)) continue;
    const open = expanded.has(entry.path) || Boolean(needle);
    total++;
    if (rows.length < PRIVACY_ROW_LIMIT) rows.push({ ...entry, depth, expanded: open, evidence: evidence.get(entry.path), historical: historical.has(entry.path) });
    if (entry.kind === "directory" && open) stack.push(...(children.get(entry.path) ?? []).toReversed().map(entry => ({ entry, depth: depth + 1 })));
  }
  return { rows, total };
}

export class PrivacyFiles {
  root: string | null = null;
  pages = new Map<string, DirectoryPage>();
  expanded = new Set<string>();
  evidence = new Map<string, FileEvidence>();
  error: string | null = null;
  private epoch = 0;
  private tasks = new Map<string, AbortController>();
  private fetch?: (path: string, signal: AbortSignal) => Promise<DirectoryPage>;
  constructor(private changed: () => void) {}
  get loading() { return this.tasks.size > 0; }
  reset(root: string | null = null, fetch?: (path: string, signal: AbortSignal) => Promise<DirectoryPage>) {
    this.epoch++; this.tasks.forEach(task => task.abort()); this.tasks.clear();
    this.root = root; this.fetch = fetch; this.pages.clear(); this.expanded.clear(); this.evidence.clear(); this.error = null;
    this.changed(); if (root && fetch) void this.scan("");
  }
  update(requests: PrivacyRequest[]) { this.evidence = this.root ? fileEvidence(requests, this.root) : new Map(); this.changed(); }
  toggle(row: FileRow) {
    if (this.expanded.has(row.path)) this.expanded.delete(row.path);
    else { this.expanded.add(row.path); if (!row.historical && !this.pages.has(row.path)) void this.scan(row.path); }
    this.changed();
  }
  refresh() {
    const historical = new Set(fileRows(this.pages, this.evidence, this.expanded, "").rows.filter(row => row.historical).map(row => row.path));
    this.epoch++; this.tasks.forEach(task => task.abort()); this.tasks.clear(); this.pages.clear();
    void this.scan("");
    // Do not recursively traverse; reload only folders explicitly opened by the user.
    this.expanded.forEach(path => { if (!historical.has(path)) void this.scan(path); });
  }
  async scan(path: string) {
    if (!this.fetch || !this.root || this.tasks.has(path)) return;
    const epoch = this.epoch, controller = new AbortController(); this.tasks.set(path, controller); this.changed();
    try {
      const page = await this.fetch(path, controller.signal);
      if (epoch !== this.epoch || controller.signal.aborted) return;
      // Only direct children, never absolute/traversing paths from an endpoint.
      if (!Array.isArray(page.entries) || page.entries.length > 3000 || page.entries.some(entry => {
        const parts = entry.path.split("/"); return parts.some(part => !part || part === "." || part === ".." || part.includes("\0")) || parts.slice(0, -1).join("/") !== path || !["file", "directory", "other"].includes(entry.kind);
      })) throw new Error("Invalid folder listing");
      this.pages.set(path, page); this.error = null;
    } catch (error) { if (epoch === this.epoch && !controller.signal.aborted) this.error = `Could not list ${path || "workspace"}: ${(error as Error).message}`; }
    finally { if (epoch === this.epoch) { this.tasks.delete(path); this.changed(); } }
  }
}
