import Foundation
import DesktopCore

@MainActor enum ModelChecks {
    static let scope = Scope(profileID: "model-checks")
    static func catalog() -> JSON { .array([
        .object(["id": .string("first"), "display_name": .string("First model"), "provider": .string("configured"), "available": .bool(true)]),
        .object(["id": .string("second"), "provider": .string("example"), "available": .bool(true)]),
        .object(["id": .string("locked"), "available": .bool(true), "locked_reason": .string("Team policy")]),
        .object(["id": .string("unavailable"), "available": .bool(false)])
    ]) }
    static func session(_ model: String = "first", id: String = "a") -> JSON {
        .object(["id": .string(id), "scope": scope.json, "model": .string(model)])
    }
    static func state(_ model: String = "first", id: String = "a") throws -> SessionModelState {
        try SessionModelState(session: session(model, id: id), catalog: catalog(), sessionID: id, scope: scope)
    }
    static func run() async throws {
        let unavailable = SessionModels()
        unavailable.reset(fetch: { throw DesktopError.http(503) })
        await unavailable.refresh()
        try expectTrue(unavailable.error != nil)
        try expectFalse(unavailable.allowsSubmission(localProtectionCommand: false))
        try expectTrue(unavailable.allowsSubmission(localProtectionCommand: true))
        let first = try state(), second = try state("second"), other = try state(id: "b")
        try expectTrue(first.options[0].matches("FIRST", configuredProvider: "SAI"))
        try expectTrue(first.options[0].matches("sai", configuredProvider: "SAI"))
        try expectTrue(first.options[1].matches("example", configuredProvider: "SAI"))
        try expectFalse(first.options[2].selectable)
        try expectFalse(first.options[3].selectable)
        try expectThrows(try SessionModelState(session: session(), catalog: catalog(), sessionID: "b", scope: scope))
        try expectThrows(try SessionModelState(session: session(), catalog: catalog(), sessionID: "a", scope: Scope(profileID: "other")))
        try expectThrows(try SessionModelState(session: session(), catalog: .array([catalog().array[0], catalog().array[0]]), sessionID: "a", scope: scope))
        try expectThrows(try SessionModelState(session: session(), catalog: .null, sessionID: "a", scope: scope))
        let models = SessionModels(), transport = ModelTransport()
        models.reset(actor: "account-1", sessionID: "a", fetch: { first }, mutate: { try await transport.mutate($0) })
        await models.refresh()
        await models.change(to: "locked"); await models.change(to: "unavailable"); await models.change(to: "unknown")
        try expectTrue(models.ready)
        try expectEqual(await transport.count, 0)
        let write = Task { await models.change(to: "second") }
        try expectEqual(await transport.next(), "second")
        try expectTrue(models.saving); try expectFalse(models.ready)
        models.reset(actor: "account-1", sessionID: "b", fetch: { other })
        await models.refresh()
        try expectTrue(models.ready); try expectFalse(models.saving)
        models.reset(actor: "account-2", sessionID: "a", fetch: { first })
        await models.refresh()
        try expectTrue(models.ready); try expectFalse(models.saving)
        // Returning while a write is outstanding cannot enable Send from stale data.
        models.reset(actor: "account-1", sessionID: "a", fetch: { second })
        await models.refresh()
        try expectTrue(models.saving); try expectFalse(models.ready)
        await transport.resolve(); await write.value
        try expectEqual(models.state?.current, "second"); try expectTrue(models.ready)
        // Failed PATCH is reconciled against server state; no optimistic model leaks.
        models.reset(actor: "account-1", sessionID: "a", fetch: { first }, mutate: { _ in throw DesktopError.http(503) })
        await models.refresh(); await models.change(to: "second")
        try expectEqual(models.state?.current, "first"); try expectTrue(models.ready); try expectTrue(models.error != nil)
        let delayed = ModelFetch()
        models.reset(actor: "account-1", sessionID: "a", fetch: { try await delayed.fetch() })
        let stale = Task { await models.refresh() }; await delayed.started()
        models.reset(actor: "account-2", sessionID: "a", fetch: { second })
        await models.refresh(); await delayed.resolve(first); await stale.value
        try expectEqual(models.state?.current, "second"); try expectTrue(models.ready)
        models.reset(actor: "account-2", sessionID: "a", fetch: { throw DesktopError.http(503) })
        await models.refresh(); try expectFalse(models.ready); try expectTrue(models.error != nil)
        print("PASS: model catalog search/policy, scoped identity, pending writes across selection/account changes, stale fetch rejection and failed-write reconciliation")
    }
}
private actor ModelTransport {
    var count = 0
    private var continuation: CheckedContinuation<Void, Error>?
    private var waiter: CheckedContinuation<String, Never>?
    private var request: String?
    func mutate(_ model: String) async throws {
        count += 1
        try await withCheckedThrowingContinuation { continuation in
            self.continuation = continuation
            if let waiter { self.waiter = nil; waiter.resume(returning: model) } else { request = model }
        }
    }
    func next() async -> String {
        if let request { self.request = nil; return request }
        return await withCheckedContinuation { waiter = $0 }
    }
    func resolve() { let value = continuation; continuation = nil; value?.resume() }
}
private actor ModelFetch {
    private var continuation: CheckedContinuation<SessionModelState, Error>?
    private var waiter: CheckedContinuation<Void, Never>?
    func fetch() async throws -> SessionModelState {
        try await withCheckedThrowingContinuation { continuation in
            self.continuation = continuation; waiter?.resume(); waiter = nil
        }
    }
    func started() async {
        if continuation != nil { return }
        await withCheckedContinuation { waiter = $0 }
    }
    func resolve(_ state: SessionModelState) { let value = continuation; continuation = nil; value?.resume(returning: state) }
}
