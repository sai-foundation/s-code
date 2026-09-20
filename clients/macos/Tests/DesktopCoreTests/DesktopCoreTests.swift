import Foundation
import DesktopCore

final class DesktopCoreTests {
    let scope = Scope(profileID: "profile-a")
    func snapshot(text: String = "Hi", revision: Int = 1, session: String = "session-a") -> JSON {
        .object(["protocol_version": .string("1.0"), "snapshot_revision": .number(Double(revision)), "cursor": .number(Double(revision)), "session": .object(["id": .string(session), "scope": scope.json]), "items": .array([.object(["id": .string("message-a"), "session_id": .string(session), "kind": .string("agent_message"), "content": .object(["type": .string("message"), "role": .string("assistant"), "content": .string(text)])])])])
    }
    func delta(_ text: String, offset: Int, sequence: Int, session: String = "session-a", item: String = "message-a", turn: String = "turn-a") -> JSON {
        .object(["sequence": .number(Double(sequence)), "session_id": .string(session), "turn_id": .string(turn), "timestamp": .string("2026-09-20T00:00:02Z"), "notification": .object(["type": .string("agent_message_delta"), "item_id": .string(item), "delta": .string(text), "byte_offset": .number(Double(offset))])])
    }
    func message(_ id: String, role: String, text: String, turn: String = "turn-a", date: String = "2026-09-20T00:00:01Z") -> JSON {
        .object(["id": .string(id), "session_id": .string("session-a"), "turn_id": .string(turn),
                 "kind": .string(role == "user" ? "user_message" : "agent_message"), "status": .string("completed"),
                 "created_at": .string(date), "content": .object(["type": .string("message"), "role": .string(role), "content": .string(text)])])
    }
    func activeSnapshot(revision: Int, items: [JSON]) -> JSON {
        snapshot(revision: revision).replacing("items", with: .array(items)).replacing("turns", with: .array([
            .object(["id": .string("turn-a"), "status": .string("running")])
        ]))
    }
    func testWireMessageKindsRenderAsMessages() throws {
        let user = TranscriptRow(message("user-a", role: "user", text: "Question"))
        let assistant = TranscriptRow(message("answer-a", role: "assistant", text: "Answer"))
        try expectEqual(user.kind, "user_message"); try expectTrue(user.isMessage)
        try expectEqual(user.role, "user"); try expectEqual(user.text, "Question")
        try expectEqual(assistant.kind, "agent_message"); try expectTrue(assistant.isMessage)
        try expectEqual(assistant.role, "assistant"); try expectEqual(assistant.text, "Answer")
    }
    func testFirstDeltaCreatesAnswerAndSnapshotsPreserveIt() throws {
        let user = message("user-a", role: "user", text: "Question")
        var state = TranscriptState()
        try state.load(activeSnapshot(revision: 1, items: [user]), scope: scope)
        try expectTrue(state.apply(delta("你", offset: 0, sequence: 2)))
        try expectEqual(state.rows.count, 2)
        try expectEqual(state.rows[1].kind, "agent_message")
        try expectEqual(state.rows[1].status, "streaming")
        try expectEqual(state.rows[1].role, "assistant")
        try expectEqual(state.rows[1].value["turn_id"].string, "turn-a")
        let tool: JSON = .object(["id": .string("tool-a"), "session_id": .string("session-a"), "kind": .string("tool_call"), "created_at": .string("2026-09-20T00:00:03Z")])
        try state.load(activeSnapshot(revision: 3, items: [user, tool]), scope: scope)
        try expectEqual(state.rows.map(\.id), ["user-a", "message-a", "tool-a"])
        try expectEqual(state.cursor, 2)
        try expectTrue(state.apply(delta("好", offset: 3, sequence: 4)))
        try expectEqual(state.rows[1].text, "你好")
        let final = message("message-a", role: "assistant", text: "你好!", date: "2026-09-20T00:00:02Z")
        try state.load(snapshot(revision: 5).replacing("items", with: .array([user, final, tool])).replacing("turns", with: .array([])), scope: scope)
        try expectEqual(state.rows.map(\.id), ["user-a", "message-a", "tool-a"])
        try expectEqual(state.rows[1].text, "你好!")
        try expectEqual(state.rows[1].status, "completed")
        try expectEqual(state.prepareForReconnect(), 5)
    }
    func testReconnectReplaysMissingUnicodePrefixWithoutDuplicatingText() throws {
        let user = message("user-a", role: "user", text: "Question")
        var state = TranscriptState()
        try state.load(activeSnapshot(revision: 1, items: [user]), scope: scope)
        try expectTrue(state.apply(delta("你", offset: 0, sequence: 2)))
        // The connection misses sequence 3. A fresh snapshot omits the entire
        // unfinished answer, even though its cursor already covers that delta.
        try state.load(activeSnapshot(revision: 4, items: [user]), scope: scope)
        try expectEqual(state.cursor, 2)
        try expectEqual(state.prepareForReconnect(), 1)
        try expectTrue(state.apply(delta("你", offset: 0, sequence: 2)))
        try expectTrue(state.apply(delta("好", offset: 3, sequence: 3)))
        try expectTrue(state.apply(delta("!", offset: 6, sequence: 4)))
        try expectEqual(state.rows.last?.text, "你好!")
        try expectEqual(state.cursor, 4)
        // A corrupt replay is not silently appended or accepted.
        try expectFalse(state.apply(delta("坏", offset: 0, sequence: 5)))
        try expectEqual(state.rows.last?.text, "你好!")
    }
    func testColdMidTurnSnapshotRecoversJournalAndSkipsPersistedAnswers() throws {
        let previous = message("previous-answer", role: "assistant", text: "Already complete", turn: "old-turn")
        let user = message("user-a", role: "user", text: "Question")
        var state = TranscriptState()
        try state.load(activeSnapshot(revision: 8, items: [previous, user]), scope: scope)
        try expectEqual(state.prepareForReconnect(), 0)
        try expectTrue(state.apply(delta("Already", offset: 0, sequence: 2, item: "previous-answer", turn: "old-turn")))
        try expectTrue(state.apply(delta("Older paginated answer", offset: 0, sequence: 3, item: "older-answer", turn: "older-turn")))
        try expectTrue(state.apply(delta("你", offset: 0, sequence: 6)))
        try expectTrue(state.apply(delta("好", offset: 3, sequence: 7)))
        try expectEqual(state.rows.map(\.id), ["previous-answer", "user-a", "message-a"])
        try expectEqual(state.rows.first?.text, "Already complete")
        try expectEqual(state.rows.last?.text, "你好")
    }
    func testMissingFirstDeltaRequestsReplayAndStructuralEventsBlockStaleSnapshot() throws {
        let user = message("user-a", role: "user", text: "Question")
        var state = TranscriptState()
        try state.load(activeSnapshot(revision: 1, items: [user]), scope: scope)
        try expectFalse(state.apply(delta("好", offset: 3, sequence: 3)))
        try expectEqual(state.rows.count, 1)
        try state.load(activeSnapshot(revision: 4, items: [user]), scope: scope)
        try expectEqual(state.prepareForReconnect(), 0)
        try expectTrue(state.apply(delta("你", offset: 0, sequence: 2)))
        try expectTrue(state.apply(delta("好", offset: 3, sequence: 3)))
        let structural: JSON = .object(["sequence": .number(10), "session_id": .string("session-a"),
            "notification": .object(["type": .string("turn_status_changed"), "status": .string("completed")])])
        try expectFalse(state.apply(structural))
        try expectEqual(state.revision, 10)
        try expectEqual(state.cursor, 3)
        try state.load(snapshot(text: "Stale completion", revision: 9), scope: scope)
        try expectEqual(state.rows.last?.text, "你好")
        try state.load(snapshot(text: "Final answer", revision: 10).replacing("turns", with: .array([])), scope: scope)
        try expectEqual(state.rows.last?.text, "Final answer")
        try expectEqual(state.cursor, 10)
    }
    func testProfileCredentialReuseRequiresIdentityEndpointAndProvider() throws {
        let saved = Profile(model: "model-a")
        var edited = saved
        edited.name = "Renamed"; edited.model = "model-b"
        try expectTrue(edited.canReuseCredential(from: saved))
        edited.id = UUID().uuidString.lowercased()
        try expectFalse(edited.canReuseCredential(from: saved))
        edited = saved; edited.endpoint = "https://other.example/v1"
        try expectFalse(edited.canReuseCredential(from: saved))
        edited = saved; edited.provider = "anthropic"
        try expectFalse(edited.canReuseCredential(from: saved))
    }
    func testEndpointRestrictions() throws {
        for value in ["https://api.sai.foundation/v1", "http://localhost:8123/v1", "http://127.0.0.1:1"] { try expectNoThrow(try EndpointPolicy.validate(value)) }
        for value in ["http://example.com/v1", "https://user:key@example.com", "https://example.com?key=secret", "file:///etc/passwd", "https://example.com/#key", "http://127.0.0.1.evil.test"] { try expectThrows(try EndpointPolicy.validate(value)) }
        try expectThrows(try EndpointPolicy.validate("https://example.com", loopbackOnly: true))
    }
    func testRequestsKeepCredentialsOutOfURLAndScopeEveryOperation() throws {
        let client = try APIClient(base: URL(string: "http://127.0.0.1:8000")!, token: "private-token", scope: scope)
        let request = try client.makeRequest("/v1/sessions")
        try expectEqual(request.value(forHTTPHeaderField: "Authorization"), "Bearer private-token")
        try expectEqual(request.value(forHTTPHeaderField: "X-S-Code-CSRF"), "1")
        try expectTrue(request.url!.absoluteString.contains("actor_id=desktop_profile-a"))
        try expectFalse(request.url!.absoluteString.contains("private-token"))
        try expectThrows(try client.makeRequest("https://evil.test"))
        try expectThrows(try client.makeRequest("/v1/../secret"))
    }
    func testSSEUnicodeAndCRLFFraming() throws {
        var parser = SSEParser(), result: [JSON] = []
        let raw = ": ping\r\nid: 2\r\nevent: model.delta\r\ndata: {\"text\":\"你好🌱\"}\r\n\r\ndata: {\n" + "data: \"number\": 2}\n\n"
        for b in raw.utf8 { if let event = try parser.append(b) { result.append(event) } }
        try expectEqual(result.count, 2); try expectEqual(result[0]["text"].string, "你好🌱"); try expectEqual(result[1]["number"].integer, 2)
    }
    func testSSERejectsUnboundedOrMalformedFrames() throws {
        var parser = SSEParser()
        try expectThrows(try Array(repeating: UInt8(65), count: 2 * 1024 * 1024 + 1).forEach { _ = try parser.append($0) })
        parser = SSEParser()
        try expectThrows(try "data: not-json\n\n".utf8.forEach { _ = try parser.append($0) })
    }
    func testUnicodeOffsetsDuplicatesAndCrossSessionEvents() throws {
        var state = TranscriptState(); try state.load(snapshot(text: "你好"), scope: scope)
        try expectFalse(state.apply(delta("!", offset: 2, sequence: 2)))
        try expectEqual(state.rows[0].text, "你好")
        try expectTrue(state.apply(delta("!", offset: 6, sequence: 5))) // Global sequence gaps are allowed.
        try expectTrue(state.apply(delta("!", offset: 6, sequence: 5))) // Replay does not duplicate text.
        try expectTrue(state.apply(delta("secret", offset: 0, sequence: 6, session: "other")))
        try expectEqual(state.rows[0].text, "你好!")
        try expectEqual(state.cursor, 6)
    }
    func testStaleSnapshotsAndForeignScopeCannotReplaceTranscript() throws {
        var state = TranscriptState(); try state.load(snapshot(revision: 3), scope: scope)
        try expectTrue(state.apply(delta("!", offset: 2, sequence: 4)))
        try state.load(snapshot(text: "stale", revision: 2), scope: scope)
        try expectEqual(state.rows[0].text, "Hi!")
        try expectThrows(try state.load(snapshot(), scope: Scope(profileID: "another-account")))
        try expectEqual(state.rows[0].text, "Hi!")
    }
    func testPaginationDeduplicatesAndRejectsOtherSession() throws {
        var state = TranscriptState(); try state.load(snapshot(), scope: scope)
        try state.load(snapshot(), scope: scope, older: true)
        try expectEqual(state.rows.count, 1)
        try expectThrows(try state.load(snapshot(session: "other"), scope: scope, older: true))
    }
    func testProfilePersistenceIsolationAndNoSecrets() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let repo = ProfileRepository(root: root)
        let a = Profile(name: "First", model: "model-a"), b = Profile(name: "Second", model: "model-b")
        try repo.save([a, b]); try expectEqual(try repo.load(), [a, b])
        try expectNotEqual(repo.directory(a), repo.directory(b))
        try expectFalse(try String(contentsOf: root.appendingPathComponent("profiles.json")).contains("api_key"))
        let permissions = try FileManager.default.attributesOfItem(atPath: root.appendingPathComponent("profiles.json").path)[.posixPermissions] as! NSNumber
        try expectEqual(permissions.intValue & 0o777, 0o600)
        try expectThrows(try Profile(id: "../../escape", model: "x").validate())
    }
    func testTenThousandDeltasPreserveEveryByte() throws {
        var state = TranscriptState(); try state.load(snapshot(text: ""), scope: scope)
        let start = Date()
        for index in 0..<10_000 { try expectTrue(state.apply(delta("🌱", offset: index * 4, sequence: index + 2))) }
        try expectEqual(state.rows[0].text.utf8.count, 40_000)
        print("Desktop reducer: 10,000 Unicode deltas in \(Date().timeIntervalSince(start)) seconds")
    }
}

struct CheckFailure: Error, CustomStringConvertible { let description: String }
func expectTrue(_ value: Bool) throws { if !value { throw CheckFailure(description: "expected true") } }
func expectFalse(_ value: Bool) throws { try expectTrue(!value) }
func expectEqual<T: Equatable>(_ a: T, _ b: T) throws { if a != b { throw CheckFailure(description: "values differ: \(a) != \(b)") } }
func expectNotEqual<T: Equatable>(_ a: T, _ b: T) throws { try expectTrue(a != b) }
func expectNoThrow<T>(_ action: @autoclosure () throws -> T) throws { _ = try action() }
func expectThrows<T>(_ action: @autoclosure () throws -> T) throws {
    do { _ = try action() } catch { return }
    throw CheckFailure(description: "expected an error")
}
@main struct DesktopChecks {
    static func main() async throws {
        let t = DesktopCoreTests()
        let checks: [(String, () throws -> Void)] = [
            ("wire message kinds", t.testWireMessageKindsRenderAsMessages),
            ("first delta and midstream snapshots", t.testFirstDeltaCreatesAnswerAndSnapshotsPreserveIt),
            ("reconnect Unicode replay", t.testReconnectReplaysMissingUnicodePrefixWithoutDuplicatingText),
            ("cold mid-turn journal recovery", t.testColdMidTurnSnapshotRecoversJournalAndSkipsPersistedAnswers),
            ("missing prefix and structural revision barrier", t.testMissingFirstDeltaRequestsReplayAndStructuralEventsBlockStaleSnapshot),
            ("credential reuse identity", t.testProfileCredentialReuseRequiresIdentityEndpointAndProvider),
            ("endpoint restrictions", t.testEndpointRestrictions),
            ("authentication and scope", t.testRequestsKeepCredentialsOutOfURLAndScopeEveryOperation),
            ("SSE Unicode and CRLF", t.testSSEUnicodeAndCRLFFraming),
            ("SSE bounds", t.testSSERejectsUnboundedOrMalformedFrames),
            ("Unicode offsets and replay", t.testUnicodeOffsetsDuplicatesAndCrossSessionEvents),
            ("stale snapshots and scope isolation", t.testStaleSnapshotsAndForeignScopeCannotReplaceTranscript),
            ("pagination", t.testPaginationDeduplicatesAndRejectsOtherSession),
            ("profile persistence", t.testProfilePersistenceIsolationAndNoSecrets),
            ("10,000 deltas", t.testTenThousandDeltasPreserveEveryByte)
        ]
        for (name, run) in checks { try run(); print("PASS: \(name)") }
        print("\(checks.count) desktop checks passed")
        try SessionCoordinationChecks.run()
        if CommandLine.arguments.count == 5 && CommandLine.arguments[1] == "--engine" {
            try await EngineIntegration.run(executable: CommandLine.arguments[2], endpoint: CommandLine.arguments[3], root: CommandLine.arguments[4])
        }
    }
}
