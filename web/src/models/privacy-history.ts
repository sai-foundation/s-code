import type { PrivacyPage, PrivacyRequest } from "../../generated/protocol";

type FetchPage = (before: number | null, signal: AbortSignal) => Promise<PrivacyPage>;
/** Metadata only. Serial refreshes reread through the loaded boundary atomically. */
export class PrivacyHistory {
  requests: PrivacyRequest[] = [];
  nextBefore: number | null = null;
  loading = false;
  hasLoaded = false;
  error: string | null = null;
  private epoch = 0;
  private controller?: AbortController;
  private timer?: ReturnType<typeof setTimeout>;
  private dirty = false;
  private fetch?: FetchPage;
  constructor(private changed: () => void) {}
  reset(fetch?: FetchPage) {
    this.epoch++;
    this.controller?.abort();
    clearTimeout(this.timer);
    this.timer = undefined;
    this.fetch = fetch;
    this.dirty = this.loading = this.hasLoaded = false;
    this.requests = [];
    this.nextBefore = null;
    this.error = null;
    this.changed();
  }
  invalidate() {
    if (!this.fetch) return;
    this.dirty = true;
    this.schedule();
  }
  private schedule() {
    if (this.loading || this.timer || !this.dirty) return;
    this.timer = setTimeout(() => { this.timer = undefined; void this.load(); }, 500);
  }
  async load(older = false) {
    if (!this.fetch || (older && this.nextBefore === null)) return;
    if (this.loading) { if (!older) this.dirty = true; return; }
    clearTimeout(this.timer); this.timer = undefined;
    const epoch = this.epoch, fetch = this.fetch;
    const controller = this.controller = new AbortController();
    const oldest = older ? undefined : this.requests.at(-1)?.sequence;
    let before = older ? this.nextBefore : null;
    this.loading = true; this.error = null;
    if (!older) this.dirty = false;
    this.changed();
    try {
      const values: PrivacyRequest[] = [];
      let next: number | null = null;
      do {
        const page = await fetch(before, controller.signal);
        if (epoch !== this.epoch || controller.signal.aborted) return;
        if (!Array.isArray(page.requests) || new Set(page.requests.map(r => r.id)).size !== page.requests.length ||
          page.requests.some((r, i) => !r.id || !Number.isSafeInteger(r.sequence) || r.sequence <= 0 ||
            (before !== null && r.sequence >= before) || (i > 0 && r.sequence >= page.requests[i - 1].sequence)) ||
          (page.next_before !== null && (page.next_before !== page.requests.at(-1)?.sequence || (before !== null && page.next_before >= before)))) {
          throw new Error("Invalid privacy history pagination");
        }
        values.push(...page.requests); next = page.next_before;
        if (older || oldest === undefined || next === null || (page.requests.at(-1)?.sequence ?? 0) <= oldest) break;
        before = next;
      } while (true);
      const byID = new Map(this.requests.map(r => [r.id, r]));
      values.forEach(r => byID.set(r.id, r));
      this.requests = [...byID.values()].sort((a, b) => b.sequence - a.sequence);
      this.nextBefore = next; this.hasLoaded = true;
    } catch (error) {
      if (epoch === this.epoch && !controller.signal.aborted) this.error = `Could not load privacy history: ${(error as Error).message}`;
    } finally {
      if (epoch === this.epoch) { this.loading = false; this.changed(); this.schedule(); }
    }
  }
}
