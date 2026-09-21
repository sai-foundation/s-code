import Foundation
import DesktopCore

@MainActor enum ProtectionChecks {
    static func policy(_ revision: Int, protected: Bool = false, canonical: JSON = .string("/canonical/private")) throws -> ProtectionPolicy {
        try ProtectionPolicy(.object(["revision": .number(Double(revision)), "changed_at": revision == 0 ? .null : .string("2026-09-20T00:00:00Z"), "rules": .array(protected ? [.object(["id": .string("rule-a"), "path": .string("/project/private"), "canonical_path": canonical, "kind": .string("file"), "created_at": .string("2026-09-20T00:00:00Z"), "device": .null, "inode": .null])] : [])]))
    }
    static func run() async throws {
        for command in ["/protect /tmp/a", "/protect", "protect file /tmp/a", "保护文件 /tmp/a", "保护目录/tmp/folder"] { try expectTrue(ProtectionCommand.recognizes(command)) }
        for prose in ["/", "/protection /tmp/a", "Please protect my file", "unprotect /tmp/a"] { try expectFalse(ProtectionCommand.recognizes(prose)) }
        let protected = try policy(1, protected: true)
        try expectTrue(protected.protects(path: "private", root: "/project"))
        try expectTrue(protected.protects(path: "private/child", root: "/project"))
        try expectFalse(protected.protects(path: "private-sibling", root: "/project"))
        try expectTrue(protected.protects(path: "/canonical/private/child", root: nil))
        try expectFalse(protected.protects(path: "private", root: nil))
        try expectTrue(try policy(1, protected: true, canonical: .null).protects(path: "private", root: "/project"))
        let api = try APIClient(base: URL(string: "http://127.0.0.1:3456")!, token: "secret", scope: Scope(profileID: "protection"))
        let delete = try api.makeRequest("/v1/sessions/session-a/privacy/protections/rule-a", method: "DELETE", query: [.init(name: "expected_revision", value: "7")])
        let query = URLComponents(url: delete.url!, resolvingAgainstBaseURL: false)!.queryItems!
        try expectTrue(query.contains(.init(name: "actor_id", value: "desktop_protection")))
        try expectTrue(query.contains(.init(name: "expected_revision", value: "7")))

        let transport = ProtectionTransport(), store = PrivacyProtections()
        store.reset(fetch: { try await transport.fetch("fetch") }, mutate: { change, revision in
            switch change { case .add: return try await transport.fetch("add:\(revision)"); case .remove: return try await transport.fetch("remove:\(revision)") }
        })
        let initial = Task { await store.refresh() }; try expectEqual(await transport.next(), "fetch")
        await transport.resolve(try policy(1)); await initial.value
        let adding = Task { await store.change(.add("/project/private")) }; try expectEqual(await transport.next(), "add:1")
        try expectTrue(store.policy!.rules.isEmpty); try expectTrue(store.saving)
        try expectFalse(await store.change(.add("/other")))
        await transport.resolve(protected); try expectTrue(await adding.value)
        try expectEqual(store.policy?.rules.count, 1)

        let conflict = Task { await store.change(.remove("rule-a")) }; try expectEqual(await transport.next(), "remove:1")
        await transport.fail(); try expectEqual(await transport.next(), "fetch")
        await transport.resolve(try policy(2, protected: true)); try expectFalse(await conflict.value)
        try expectEqual(store.policy?.revision, 2); try expectTrue(store.error?.contains("retry") ?? false)

        let removing = Task { await store.change(.remove("rule-a")) }; try expectEqual(await transport.next(), "remove:2")
        await store.refresh(); await store.refresh() // Account-wide event refreshes coalesce behind mutation.
        await transport.resolve(try policy(3)); try expectTrue(await removing.value)
        try expectEqual(await transport.next(), "fetch"); await transport.resolve(try policy(4))
        while store.loading { await Task.yield() }
        try expectEqual(store.policy?.revision, 4); try expectTrue(store.policy!.changedAt != nil)

        let stale = Task { await store.refresh() }; try expectEqual(await transport.next(), "fetch")
        let other = try policy(9)
        store.reset(fetch: { other }); await store.refresh()
        await transport.resolve(try policy(99, protected: true)); await stale.value
        try expectEqual(store.policy?.revision, 9); try expectTrue(store.policy!.rules.isEmpty)
        store.reset(); try expectTrue(store.policy == nil); try expectFalse(store.loading)
        print("PASS: protection scope/revisions, path boundaries, confirmed mutations, conflict reload, event coalescing and stale account rejection")
    }
}
private actor ProtectionTransport {
    private var pending: CheckedContinuation<ProtectionPolicy, Error>?
    private var requests: [String] = []
    private var waiter: CheckedContinuation<String, Never>?
    func fetch(_ name: String) async throws -> ProtectionPolicy {
        try await withCheckedThrowingContinuation { continuation in
            pending = continuation
            if let waiter { self.waiter = nil; waiter.resume(returning: name) } else { requests.append(name) }
        }
    }
    func next() async -> String {
        if !requests.isEmpty { return requests.removeFirst() }
        return await withCheckedContinuation { waiter = $0 }
    }
    func resolve(_ policy: ProtectionPolicy) { let continuation = pending; pending = nil; continuation?.resume(returning: policy) }
    func fail() { let continuation = pending; pending = nil; continuation?.resume(throwing: DesktopError.http(409)) }
}
