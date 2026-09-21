import { afterEach, describe, expect, it, vi } from "vitest";
import type { PrivacyPage, PrivacyRequest } from "../../generated/protocol";
import { PrivacyHistory } from "./privacy-history";
const request = (sequence: number, status = "accepted"): PrivacyRequest => ({ id: String(sequence), sequence, turn_id: "turn", started_at: "2026-09-20T00:00:00Z", destination: "https://example.test", model: "test", purpose: "agent", status, request_bytes: 1, sources: [], unattributed: [] });
const page = (sequences: number[], next: number | null): PrivacyPage => ({ requests: sequences.map(n => request(n)), next_before: next });
afterEach(() => vi.useRealTimers());
describe("privacy history", () => {
  it("rereads through the old boundary after a burst larger than a page, preserving older pages and updating statuses", async () => {
    let latest = 4;
    const cursors: Array<number | null> = [];
    const history = new PrivacyHistory(() => {});
    history.reset(async before => {
      cursors.push(before); const start = before === null ? latest : before - 1;
      return page([start, start - 1].filter(n => n > 0), start > 2 ? start - 1 : null);
    });
    await history.load(); await history.load(true);
    latest = 10; cursors.length = 0;
    await history.load();
    expect(cursors).toEqual([null, 9, 7, 5, 3]);
    expect(history.requests.map(r => r.sequence)).toEqual([10, 9, 8, 7, 6, 5, 4, 3, 2, 1]);
    expect(history.nextBefore).toBeNull();
  });
  it("coalesces invalidations during older loading and refreshes the newly loaded boundary", async () => {
    vi.useFakeTimers(); let resolve!: (value: PrivacyPage) => void;
    const fetch = vi.fn().mockResolvedValueOnce(page([4, 3], 3)).mockImplementationOnce(() => new Promise<PrivacyPage>(done => resolve = done)).mockResolvedValueOnce(page([5, 4], 4)).mockResolvedValueOnce(page([3, 2], 2)).mockResolvedValueOnce(page([1], null));
    const history = new PrivacyHistory(() => {}); history.reset(fetch);
    await history.load(); const older = history.load(true);
    history.invalidate(); history.invalidate(); await history.load();
    expect(fetch).toHaveBeenCalledTimes(2);
    resolve(page([2, 1], null)); await older;
    await vi.advanceTimersByTimeAsync(500);
    expect(fetch).toHaveBeenCalledTimes(5);
    expect(history.requests.map(r => r.sequence)).toEqual([5, 4, 3, 2, 1]);
  });
  it("rejects stale results even if transport ignores abort, and cancels queued refreshes", async () => {
    vi.useFakeTimers(); let resolve!: (value: PrivacyPage) => void; let signal!: AbortSignal;
    const history = new PrivacyHistory(() => {});
    history.reset(async (_, abort) => { signal = abort; return new Promise(done => resolve = done); });
    const loading = history.load(); history.invalidate(); history.reset();
    resolve(page([1], null)); await loading; await vi.advanceTimersByTimeAsync(1000);
    expect(signal.aborted).toBe(true); expect(history.requests).toEqual([]); expect(history.loading).toBe(false);
  });
  it("retains loaded history atomically on a failed refresh or nonprogressing cursor", async () => {
    const history = new PrivacyHistory(() => {});
    let calls = 0;
    history.reset(async () => ++calls === 1 ? page([2, 1], null) : page([4, 3], 4));
    await history.load(); await history.load();
    expect(history.requests.map(r => r.sequence)).toEqual([2, 1]); expect(history.error).toContain("pagination");
  });
});
