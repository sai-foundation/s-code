export interface ProtectionRule { id: string; path: string; canonical_path: string | null; kind: "file" | "directory"; created_at: string; device: number | null; inode: number | null }
export interface ProtectionPolicy { revision: number; changed_at: string | null; rules: ProtectionRule[] }
export type ProtectionChange = { path: string } | { remove: string };
type Fetch = (signal: AbortSignal) => Promise<ProtectionPolicy>;
type Mutate = (change: ProtectionChange, revision: number, signal: AbortSignal) => Promise<ProtectionPolicy>;
export function isProtectedPath(path: string, root: string | null, rules: ProtectionRule[]): boolean {
  const absolute = path.startsWith("/") ? path : root ? `${root.replace(/\/$/, "")}/${path}` : null;
  if (!absolute) return false;
  return rules.some(rule => [rule.path, rule.canonical_path].some(value => value && (absolute === value || absolute.startsWith(`${value.replace(/\/$/, "")}/`))));
}
/** Scoped metadata store: never optimistically marks a path protected. */
export class PrivacyProtections {
  policy: ProtectionPolicy | null = null;
  loading = false;
  saving = false;
  error: string | null = null;
  private epoch = 0;
  private controller?: AbortController;
  private dirty = false;
  private fetch?: Fetch;
  private mutate?: Mutate;
  constructor(private changed: () => void) {}
  reset(fetch?: Fetch, mutate?: Mutate) {
    this.epoch++; this.controller?.abort(); this.fetch = fetch; this.mutate = mutate;
    this.policy = null; this.loading = this.saving = this.dirty = false; this.error = null; this.changed();
  }
  async refresh() {
    if (!this.fetch) return;
    if (this.loading || this.saving) { this.dirty = true; return; }
    const epoch = this.epoch, controller = this.controller = new AbortController();
    this.loading = true; this.dirty = false; this.error = null; this.changed();
    try { const policy = await this.fetch(controller.signal); if (epoch === this.epoch && !controller.signal.aborted) this.policy = policy; }
    catch (error) { if (epoch === this.epoch && !controller.signal.aborted) this.error = `Could not load protections: ${(error as Error).message}`; }
    finally { if (epoch === this.epoch) { this.loading = false; this.changed(); if (this.dirty) void this.refresh(); } }
  }
  async change(change: ProtectionChange): Promise<boolean> {
    if (!this.mutate || !this.policy || this.loading || this.saving) return false;
    const epoch = this.epoch, controller = this.controller = new AbortController();
    const revision = this.policy.revision; this.saving = true; this.error = null; this.changed();
    let successful = false;
    try { const policy = await this.mutate(change, revision, controller.signal); if (epoch !== this.epoch || controller.signal.aborted) return false; this.policy = policy; successful = true; }
    catch (error) {
      if (epoch !== this.epoch || controller.signal.aborted) return false;
      this.error = `Protection change failed: ${(error as Error).message}. Review the current rules and retry.`;
      // A competing client or an ambiguous transport result may have changed policy.
      try { const policy = await this.fetch!(controller.signal); if (epoch === this.epoch && !controller.signal.aborted) this.policy = policy; } catch { /* Retain the failure and last confirmed policy. */ }
    } finally { if (epoch === this.epoch) { this.saving = false; this.changed(); if (this.dirty) void this.refresh(); } }
    return successful;
  }
}
