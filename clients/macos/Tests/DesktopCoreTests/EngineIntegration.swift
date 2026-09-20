import Foundation
import DesktopCore

@MainActor enum EngineIntegration {
    static func run(executable: String, endpoint: String, root: String) async throws {
        let engine = Engine(), repo = ProfileRepository(root: URL(fileURLWithPath: root).appendingPathComponent("Profiles"))
        let profile = Profile(name: "Fixture", endpoint: endpoint, model: "desktop-fixture")
        let client = try await engine.start(profile: profile, key: "", repository: repo, executable: URL(fileURLWithPath: executable))
        print("Engine ready in \(engine.startupMilliseconds) ms")
        do {
            let scope = client.scope.json
            let chat = try await client.request("/v1/sessions", method: "POST", body: .object(["scope": scope, "mode": .string("chat"), "workspace_uri": .string(""), "title": .string("Desktop smoke"), "model": .string(profile.model)]))
            let chatID = chat["id"].string
            try expectTrue(!chatID.isEmpty)
            let recorder = EventRecorder()
            let stream = Task { try await client.stream(after: 0) { events in await recorder.add(events) } }
            defer { stream.cancel() }
            _ = try await client.request("/v1/sessions/\(chatID)/turns", method: "POST", body: .object(["scope": scope, "content": .string("desktop hello")]))
            let completed = try await poll(client, session: chatID) { $0["items"].array.contains { $0["content"]["role"].string == "assistant" && $0["content"]["content"].string.contains("Hello from the desktop fixture") } }
            try expectTrue(completed["items"].array.contains { $0["kind"].string == "agent_message" && TranscriptRow($0).isMessage })
            let workspace = URL(fileURLWithPath: root).appendingPathComponent("project")
            try FileManager.default.createDirectory(at: workspace, withIntermediateDirectories: true)
            try Data("before desktop\n".utf8).write(to: workspace.appendingPathComponent("desktop-demo.txt"))
            let work = try await client.request("/v1/sessions", method: "POST", body: .object(["scope": scope, "mode": .string("work"), "workspace_uri": .string(workspace.absoluteString + "/"), "title": .string("Desktop Work"), "model": .string(profile.model)]))
            let workID = work["id"].string
            _ = try await client.request("/v1/sessions/\(workID)/turns", method: "POST", body: .object(["scope": scope, "content": .string("desktop edit")]))
            let pending = try await poll(client, session: workID) { !$0["pending_requests"].array.isEmpty }
            try expectEqual(try String(contentsOf: workspace.appendingPathComponent("desktop-demo.txt")), "before desktop\n")
            let approval = pending["pending_requests"].array[0]
            let toolID = pending["items"].array.first { $0["content"]["tool_call_id"] != .null }!["content"]["tool_call_id"].string
            let proposed = try await client.request("/v1/sessions/\(workID)/tools/\(toolID)")
            try expectEqual(proposed["request"]["arguments"]["content"].string, "after desktop\n")
            _ = try await client.request("/v1/approvals/\(approval["id"].string)", method: "POST", body: .object(["scope": scope, "approved": .bool(true), "approval_scope": .string("once")]))
            _ = try await poll(client, session: workID) { $0["turns"].array.last?["status"].string == "completed" }
            try expectEqual(try String(contentsOf: workspace.appendingPathComponent("desktop-demo.txt")), "after desktop\n")
            _ = try await client.request("/v1/sessions/\(workID)/turns", method: "POST", body: .object(["scope": scope, "content": .string("desktop question")]))
            let questionSnapshot = try await poll(client, session: workID) { !$0["pending_questions"].array.isEmpty }
            let question = questionSnapshot["pending_questions"].array[0]
            _ = try await client.request("/v1/questions/\(question["id"].string)", method: "POST", body: .object(["scope": scope, "answers": .array([.object(["question_id": .string("choice"), "answer": .string("Option A")])])]))
            _ = try await poll(client, session: workID) { $0["turns"].array.last?["status"].string == "completed" }
            _ = try await client.request("/v1/sessions/\(chatID)/turns", method: "POST", body: .object(["scope": scope, "content": .string("desktop slow")]))
            try await Task.sleep(for: .milliseconds(250))
            let active = try await client.request("/v1/sessions/\(chatID)/snapshot")
            let activeTurn = active["turns"].array.last!["id"].string
            _ = try await client.request("/v1/turns/\(activeTurn)/cancel", method: "POST", body: .object(["scope": scope]))
            _ = try await poll(client, session: chatID) { $0["turns"].array.last?["status"].string == "cancelled" }
            let stoppedSession = try await client.request("/v1/sessions/\(chatID)")
            try expectEqual(stoppedSession["status"].string, "active")
            _ = try await client.request("/v1/sessions/\(chatID)/turns", method: "POST", body: .object(["scope": scope, "content": .string("desktop failure")]))
            let failed = try await poll(client, session: chatID) { $0["turns"].array.last?["status"].string == "failed" }
            try expectTrue(TurnFeedback.message(failed["turns"].array.last!) != nil)
            let events = await recorder.values
            try expectTrue(events.contains { $0["notification"]["type"].string == "agent_message_delta" })
            stream.cancel()
            await engine.stop()
            let restarted = try await engine.start(profile: profile, key: "", repository: repo, executable: URL(fileURLWithPath: executable))
            let sessions = try await restarted.request("/v1/sessions")
            try expectTrue(sessions.array.contains { $0["id"].string == workID })
            let other = try APIClient(base: restarted.base, token: "invalid", scope: Scope(profileID: UUID().uuidString))
            do { _ = try await other.request("/v1/sessions"); throw CheckFailure(description: "invalid token accepted") } catch DesktopError.http(401) { }
            await engine.stop()
            print("PASS: real engine Chat, Work, streaming, approval, file edit, question, cancellation, provider failure, proposed patch details, restart/history and auth")
        } catch { await engine.stop(); throw error }
    }
    static func poll(_ client: APIClient, session: String, until: (JSON) -> Bool) async throws -> JSON {
        var last = JSON.null
        for _ in 0..<120 {
            last = try await client.request("/v1/sessions/\(session)/snapshot", query: [.init(name: "limit", value: "100")])
            if until(last) { return last }
            if last["turns"].array.last?["status"].string == "failed" { throw CheckFailure(description: "turn failed: \(last.pretty)") }
            try await Task.sleep(for: .milliseconds(100))
        }
        throw CheckFailure(description: "fixture timeout: \(last.pretty)")
    }
}
private actor EventRecorder {
    var values: [JSON] = []
    func add(_ batch: [JSON]) { values.append(contentsOf: batch) }
}
