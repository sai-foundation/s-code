import { describe, expect, it, vi } from "vitest";
import { PrivacyProtections, isProtectedPath, type ProtectionPolicy, type ProtectionRule, type ProtectionChange } from "./privacy-protections";
const rule: ProtectionRule = { id: "rule", path: "/repo/private", canonical_path: "/real/private", kind: "file", created_at: "now", device: null, inode: null };
const policy = (revision: number, rules: ProtectionRule[] = []): ProtectionPolicy => ({ revision, changed_at: revision ? "now" : null, rules });
describe("file protection state", () => {
  it("matches exact and descendant path boundaries including canonical names without guessing aliases", () => {
    expect(isProtectedPath("private", "/repo", [rule])).toBe(true);
    expect(isProtectedPath("private/key", "/repo", [rule])).toBe(true);
    expect(isProtectedPath("private-copy", "/repo", [rule])).toBe(false);
    expect(isProtectedPath("/real/private/key", null, [rule])).toBe(true);
    expect(isProtectedPath("private", null, [rule])).toBe(false);
    expect(isProtectedPath("/repo/private", null, [{ ...rule, canonical_path: null }])).toBe(true);
  });
  it("uses confirmed revisions and never marks a path protected before server confirmation", async () => {
    let resolve!: (value: ProtectionPolicy) => void;
    const mutate = vi.fn((_change: ProtectionChange, _revision: number, _signal: AbortSignal) => new Promise<ProtectionPolicy>(done => resolve = done));
    const store = new PrivacyProtections(() => {}); store.reset(async () => policy(4), mutate);
    await store.refresh(); const adding = store.change({ path: "/repo/private" });
    expect(mutate.mock.calls[0][1]).toBe(4); expect(store.policy?.rules).toEqual([]); expect(store.saving).toBe(true);
    resolve(policy(5, [rule])); expect(await adding).toBe(true); expect(store.policy?.revision).toBe(5);
  });
  it("refreshes a conflicting mutation without silently retrying a stale overwrite", async () => {
    let fetches = 0;
    const mutate = vi.fn(async () => { throw new Error("409 Protection changed"); });
    const store = new PrivacyProtections(() => {}); store.reset(async () => ++fetches === 1 ? policy(1) : policy(2, [rule]), mutate);
    await store.refresh(); expect(await store.change({ path: "/another" })).toBe(false);
    expect(mutate).toHaveBeenCalledTimes(1); expect(store.policy?.revision).toBe(2); expect(store.error).toContain("retry");
  });
  it("discards in-flight account/session results, even when abort is ignored", async () => {
    let resolve!: (value: ProtectionPolicy) => void; let signal!: AbortSignal;
    const store = new PrivacyProtections(() => {});
    store.reset(async abort => { signal = abort; return new Promise(done => resolve = done); });
    const pending = store.refresh(); store.reset(async () => policy(8)); await store.refresh(); resolve(policy(99, [rule])); await pending;
    expect(signal.aborted).toBe(true); expect(store.policy?.revision).toBe(8); expect(store.policy?.rules).toEqual([]);
  });
  it("coalesces an account-wide update during a mutation and preserves a reset after removal", async () => {
    let resolve!: (value: ProtectionPolicy) => void; let reads = 0;
    const store = new PrivacyProtections(() => {});
    store.reset(async () => ++reads === 1 ? policy(1, [rule]) : policy(3), () => new Promise(done => resolve = done));
    await store.refresh(); const removal = store.change({ remove: rule.id }); await store.refresh(); await store.refresh();
    resolve(policy(2)); await removal; await Promise.resolve();
    expect(reads).toBe(2); expect(store.policy?.revision).toBe(3); expect(store.policy?.changed_at).toBe("now");
    store.reset(); expect(store.policy).toBeNull(); expect(store.loading).toBe(false);
  });
});
