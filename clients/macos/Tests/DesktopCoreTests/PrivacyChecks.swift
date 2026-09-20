import Foundation
import DesktopCore

@MainActor enum PrivacyChecks {
    static func request(_ sequence: Int, status: String = "accepted") -> JSON {
        .object(["id": .string("request-\(sequence)"), "sequence": .number(Double(sequence)), "turn_id": .string("turn-a"),
                 "started_at": .string("2026-09-20T12:13:14.123Z"), "destination": .string("https://example.test"),
                 "model": .string("test-model"), "purpose": .string("agent"), "status": .string(status), "request_bytes": .number(1234),
                 "sources": .array([.object(["source": .string("src/private.swift"), "kind": .string("read_file"), "content_bytes": .number(42), "partial": .bool(true)])]),
                 "unattributed": .array([.string("conversation_history")])])
    }
    static func page(_ sequences: [Int], next: Int? = nil, status: String = "accepted") throws -> PrivacyPage {
        try PrivacyPage(.object(["requests": .array(sequences.map { request($0, status: status) }), "next_before": next.map { .number(Double($0)) } ?? .null]))
    }
    static func run() async throws {
        let parsed = try page([5]).requests[0]
        try expectEqual(parsed.requestBytes, 1234)
        try expectEqual(parsed.sources[0].contentBytes, 42)
        try expectTrue(parsed.sources[0].partial)
        try expectEqual(parsed.unattributed, ["conversation_history"])
        try expectEqual(try page([5], status: "rejected").requests[0].statusLabel, "Rejected by endpoint · data may have been received")
        try expectEqual(try page([5], status: "connection_error").requests[0].statusLabel, "Connection error · delivery unknown")
        try expectEqual(try page([5], status: "future_status").requests[0].statusLabel, "Request started · delivery not confirmed")
        try expectThrows(try PrivacyPage(.object(["requests": .string("bad")])) )
        try expectThrows(try page([0]))
        try expectThrows(try page([2, 2]))
        try expectThrows(try page([1, 2]))
        let api = try APIClient(base: URL(string: "http://127.0.0.1:3456")!, token: "secret", scope: Scope(profileID: "privacy"))
        let request = try api.makeRequest("/v1/sessions/test/privacy", query: [.init(name: "before", value: "42")])
        let query = URLComponents(url: request.url!, resolvingAgainstBaseURL: false)!.queryItems!
        try expectTrue(query.contains(.init(name: "actor_id", value: "desktop_privacy")))
        try expectTrue(query.contains(.init(name: "organization_id", value: "org_local")))
        try expectTrue(query.contains(.init(name: "team_id", value: "team_local")))
        try expectTrue(query.contains(.init(name: "before", value: "42")))
        try expectFalse(request.url!.absoluteString.contains("secret"))

        let transport = PrivacyTransport(), delay = PrivacyDelay()
        let history = PrivacyHistory(refreshDelay: { await delay.wait() })
        history.reset(sessionID: "session-a") { try await transport.fetch($0) }
        let initial = Task { await history.refresh() }
        try expectEqual(await transport.next(), nil)
        await transport.resolve(try page([5, 4], next: 4, status: "attempted")); await initial.value
        let older = Task { await history.older() }
        try expectEqual(await transport.next(), 4)
        await history.refresh() // A live/manual refresh cannot race pagination.
        await transport.resolve(try page([3, 2], next: 2)); await older.value
        try expectEqual(history.requests.map(\.sequence), [5, 4, 3, 2])
        try expectEqual(history.nextBefore, 2)
        let refresh = Task { await history.refresh() }
        try expectEqual(await transport.next(), nil)
        await transport.resolve(try page([8, 7], next: 7))
        try expectEqual(await transport.next(), 7)
        await transport.resolve(try page([6, 5], next: 5))
        try expectEqual(await transport.next(), 5)
        // Until the entire loaded range is refreshed, the previous view is intact.
        try expectEqual(history.requests.map(\.sequence), [5, 4, 3, 2])
        await transport.resolve(try page([4, 3], next: 3))
        try expectEqual(await transport.next(), 3)
        await transport.resolve(try page([2, 1])); await refresh.value
        try expectEqual(history.requests.map(\.sequence), [8, 7, 6, 5, 4, 3, 2, 1])
        try expectEqual(history.requests.first(where: { $0.sequence == 5 })?.status, "accepted")
        try expectEqual(history.nextBefore, nil)

        let pending = Task { await history.refresh() }
        _ = await transport.next()
        // Same session ID in another account still gets a fresh generation.
        let other = PrivacyTransport()
        history.reset(sessionID: "session-a") { try await other.fetch($0) }
        let current = Task { await history.refresh() }
        _ = await other.next()
        await other.resolve(try page([100], next: 100)); await current.value
        await transport.resolve(try page([9])); await pending.value
        try expectEqual(history.requests.map(\.sequence), [100])
        try expectEqual(history.nextBefore, 100)
        try expectFalse(history.loading)
        let failing = Task { await history.older() }
        try expectEqual(await other.next(), 100)
        await other.fail(); await failing.value
        try expectTrue(history.error != nil)
        try expectEqual(history.nextBefore, 100)
        let retry = Task { await history.retry() }
        try expectEqual(await other.next(), 100)
        await other.resolve(try page([99])); await retry.value
        try expectEqual(history.requests.map(\.sequence), [100, 99])
        try expectEqual(history.error, nil)

        let event: JSON = .object(["session_id": .string("session-a"), "sequence": .number(101), "type": .string("privacy.request.finished")])
        history.receive([event.replacing("session_id", with: .string("other-session")), event.replacing("type", with: .string("agent_message_delta"))])
        try expectEqual(await other.count, 3)
        history.receive(Array(repeating: event, count: 1000))
        await delay.started()
        try expectEqual(await other.count, 3)
        await delay.release()
        try expectEqual(await other.next(), nil)
        // New events while a request is pending schedule only one follow-up.
        history.receive([event.replacing("sequence", with: .number(102))])
        await other.resolve(try page([101, 100, 99]))
        await delay.started()
        history.reset() // Closing drops metadata and cancels the queued refresh.
        await delay.release()
        try expectTrue(history.requests.isEmpty)
        try expectEqual(history.nextBefore, nil)
        try expectEqual(history.error, nil)
        try expectFalse(history.loading)
        let failure = PrivacyTransport()
        history.reset(sessionID: "old-session") { try await failure.fetch($0) }
        let staleFailure = Task { await history.refresh() }
        _ = await failure.next()
        let emptyPage = try page([])
        history.reset(sessionID: "new-session") { _ in emptyPage }
        await history.refresh()
        await failure.fail(); await staleFailure.value
        try expectTrue(history.hasLoaded)
        try expectTrue(history.requests.isEmpty)
        try expectEqual(history.error, nil)
        print("PASS: privacy schema/status/scoped requests, atomic multi-page refresh, pagination retry, stale account response rejection, event coalescing and close cleanup")
    }
}

private actor PrivacyTransport {
    var count = 0
    private var pending: CheckedContinuation<PrivacyPage, Error>?
    private var requests: [Int?] = []
    private var waiter: CheckedContinuation<Int?, Never>?
    func fetch(_ before: Int?) async throws -> PrivacyPage {
        count += 1
        return try await withCheckedThrowingContinuation { continuation in
            pending = continuation
            if let waiter { self.waiter = nil; waiter.resume(returning: before) }
            else { requests.append(before) }
        }
    }
    func next() async -> Int? {
        if !requests.isEmpty { return requests.removeFirst() }
        return await withCheckedContinuation { waiter = $0 }
    }
    func resolve(_ page: PrivacyPage) { let value = pending; pending = nil; value?.resume(returning: page) }
    func fail() { let value = pending; pending = nil; value?.resume(throwing: DesktopError.http(503)) }
}
private actor PrivacyDelay {
    private var continuation: CheckedContinuation<Void, Never>?
    private var start: CheckedContinuation<Void, Never>?
    private var waiting = false
    func wait() async {
        await withCheckedContinuation { continuation in
            self.continuation = continuation; waiting = true; start?.resume(); start = nil
        }
    }
    func started() async {
        if waiting { return }
        await withCheckedContinuation { start = $0 }
    }
    func release() { waiting = false; let value = continuation; continuation = nil; value?.resume() }
}
