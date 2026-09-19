import { describe, expect, it } from "vitest";
import { accountDraftContext, accountKey, accountPermissionKey, accountPresenceClientId, guardAccountResponse, ownsSession } from "./account-state";

const alice = { organization_id: "org", team_id: "team", actor_id: "alice" };
const bob = { ...alice, actor_id: "bob" };

describe("account-scoped composer state", () => {
  it("isolates text, attachments and permission defaults, while preserving a returning account", () => {
    const storage = new Map<string, unknown>();
    const establishedAccount = accountKey(alice);
    const oldDraft = accountDraftContext(establishedAccount);
    storage.set(oldDraft, { text: "Alice private draft", files: ["private.txt"] });
    storage.set(accountPermissionKey(establishedAccount), "workspace");
    // Form fields now show Bob. Saving the outgoing draft still uses the
    // established Alice identity, not mutable form values.
    const nextAccount = accountKey(bob);
    storage.set(oldDraft, { text: "Alice edited draft", files: ["private.txt"] });
    expect(storage.get(accountDraftContext(nextAccount))).toBeUndefined();
    expect(storage.get(accountPermissionKey(nextAccount)) ?? "manual").toBe("manual");
    expect(storage.get(accountDraftContext(accountKey(alice)))).toEqual({ text: "Alice edited draft", files: ["private.txt"] });
    expect(accountDraftContext(establishedAccount, "session-a")).not.toBe(oldDraft);
  });

  it("includes organization and team without delimiter collisions", () => {
    expect(accountKey({ ...alice, team_id: "other" })).not.toBe(accountKey(alice));
    expect(accountKey({ ...alice, organization_id: "other" })).not.toBe(accountKey(alice));
    expect(accountKey({ organization_id: "a:b", team_id: "c", actor_id: "d" }))
      .not.toBe(accountKey({ organization_id: "a", team_id: "b:c", actor_id: "d" }));
  });

  it("rejects selecting another account's session even through a delayed action callback", () => {
    const session = { scope: alice };
    expect(ownsSession(session, alice)).toBe(true);
    expect(ownsSession(session, bob)).toBe(false);
    expect(ownsSession(session, null)).toBe(false);
  });

  it("rejects a delayed Alice response after switching to Bob", async () => {
    let generation = 1;
    let resolve!: (value: string[]) => void;
    const response = new Promise<string[]>((done) => { resolve = done; });
    const guarded = guardAccountResponse(response, generation, () => generation);
    const rejected = expect(guarded).rejects.toMatchObject({ name: "AbortError" });
    generation = 2;
    resolve(["Alice task"]);
    await rejected;
    await expect(guardAccountResponse(Promise.reject(new Error("Alice private error")), 1, () => generation)).rejects.toMatchObject({ name: "AbortError" });
    await expect(guardAccountResponse(Promise.resolve(["Bob task"]), generation, () => generation)).resolves.toEqual(["Bob task"]);
  });
});

describe("account-bound Web presence", () => {
  it("uses distinct IDs across accounts and restores the original on return or reload", () => {
    const values = new Map<string, string>([["oc.client-presence-id", "web:legacy-alice"]]);
    const storage = { getItem: (key: string) => values.get(key) ?? null, setItem: (key: string, value: string) => { values.set(key, value); } };
    let sequence = 0;
    const createId = () => `web:client-${++sequence}`;
    const aliceId = accountPresenceClientId(storage, accountKey(alice), createId);
    const bobId = accountPresenceClientId(storage, accountKey(bob), createId);
    expect(aliceId).not.toBe("web:legacy-alice");
    expect(bobId).not.toBe(aliceId);
    expect(accountPresenceClientId(storage, accountKey(alice), createId)).toBe(aliceId);
    expect(accountPresenceClientId(storage, accountKey(bob), createId)).toBe(bobId);
    expect(accountPresenceClientId(storage, accountKey({ ...alice, team_id: "other" }), createId)).not.toBe(aliceId);
    expect(accountPresenceClientId(storage, accountKey({ ...alice, organization_id: "other" }), createId)).not.toBe(aliceId);
    expect(sequence).toBe(4);
  });
});
