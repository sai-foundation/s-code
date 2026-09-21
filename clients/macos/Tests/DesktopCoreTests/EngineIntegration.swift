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
            let chat = try await client.request("/v1/sessions", method: "POST", body: .object(["scope": scope, "mode": .string("chat"), "workspace_uri": .string(""), "title": .string("New conversation"), "model": .string(profile.model)]))
            let chatID = chat["id"].string
            try expectTrue(!chatID.isEmpty)
            let recorder = EventRecorder()
            let stream = Task { try await client.stream(after: 0) { events in await recorder.add(events) } }
            defer { stream.cancel() }
            _ = try await client.request("/v1/sessions/\(chatID)/turns", method: "POST", body: .object(["scope": scope, "content": .string("desktop hello")]))
            let completed = try await poll(client, session: chatID) { $0["items"].array.contains { $0["content"]["role"].string == "assistant" && $0["content"]["content"].string.contains("Hello from the desktop fixture") } }
            let fallback = try await poll(client, session: chatID) { $0["session"]["title"].string == "desktop hello" }
            try expectEqual(fallback["session"]["title"].string, "desktop hello")
            _ = try await client.request("/v1/sessions/\(chatID)/turns", method: "POST", body: .object(["scope": scope, "content": .string("desktop retry title")]))
            _ = try await poll(client, session: chatID) { $0["session"]["title"].string == "Desktop conversation overview" }
            try expectTrue(completed["items"].array.contains { $0["kind"].string == "agent_message" && TranscriptRow($0).isMessage })
            let privacy = try await client.privacy(sessionID: chatID)
            try expectTrue(privacy.requests.contains { $0.purpose == "agent" && $0.status == "accepted" })
            try expectTrue(privacy.requests.contains { $0.purpose == "session_title" })
            try expectTrue(privacy.requests.allSatisfy { $0.model == profile.model && $0.requestBytes > 0 })
            try expectTrue(privacy.requests.allSatisfy { URLComponents(string: $0.destination)?.path.isEmpty == true })
            try expectEqual(privacy.requests.map(\.sequence), privacy.requests.map(\.sequence).sorted(by: >))
            let firstRequest = privacy.requests.first!
            let olderPrivacy = try await client.privacy(sessionID: chatID, before: firstRequest.sequence)
            try expectTrue(!olderPrivacy.requests.isEmpty)
            try expectTrue(olderPrivacy.requests.allSatisfy { $0.sequence < firstRequest.sequence })
            try expectFalse(olderPrivacy.requests.contains { $0.id == firstRequest.id })
            // Reuse this engine's valid authentication while asking for another
            // account's scope; the endpoint must reject it independently of auth.
            let authorization = try client.makeRequest("/v1/sessions").value(forHTTPHeaderField: "Authorization")!
            let foreign = try APIClient(base: client.base, token: String(authorization.dropFirst("Bearer ".count)), scope: Scope(profileID: "foreign-account"))
            do { _ = try await foreign.privacy(sessionID: chatID); throw CheckFailure(description: "foreign privacy scope accepted") } catch DesktopError.http(403) { }
            let failedChat = try await client.request("/v1/sessions", method: "POST", body: .object(["scope": scope, "mode": .string("chat"), "workspace_uri": .string(""), "title": .string("New conversation"), "model": .string(profile.model)]))
            let failedID = failedChat["id"].string
            try expectTrue(try await client.privacy(sessionID: failedID).requests.isEmpty)
            _ = try await client.request("/v1/sessions/\(failedID)/turns", method: "POST", body: .object(["scope": scope, "content": .string("desktop failure")]))
            let failedFirst = try await poll(client, session: failedID) { $0["turns"].array.last?["status"].string == "failed" }
            try expectEqual(failedFirst["session"]["title"].string, "desktop failure")
            // Explicit custom titles survive later successful turns.
            _ = try await client.request("/v1/sessions/\(failedID)", method: "PATCH", body: .object(["scope": scope, "title": .string("My chosen name")]))
            _ = try await client.request("/v1/sessions/\(failedID)/turns", method: "POST", body: .object(["scope": scope, "content": .string("desktop hello")]))
            let custom = try await poll(client, session: failedID) { $0["turns"].array.last?["status"].string == "completed" }
            try expectEqual(custom["session"]["title"].string, "My chosen name")
            let workspace = URL(fileURLWithPath: root).appendingPathComponent("project")
            try FileManager.default.createDirectory(at: workspace, withIntermediateDirectories: true)
            try Data("before desktop\n".utf8).write(to: workspace.appendingPathComponent("desktop-demo.txt"))
            let work = try await client.request("/v1/sessions", method: "POST", body: .object(["scope": scope, "mode": .string("work"), "workspace_uri": .string(workspace.absoluteString + "/"), "title": .string("New conversation"), "model": .string(profile.model)]))
            let workID = work["id"].string
            _ = try await client.request("/v1/sessions/\(workID)/turns", method: "POST", body: .object(["scope": scope, "content": .string("desktop edit")]))
            let pending = try await poll(client, session: workID) { !$0["pending_requests"].array.isEmpty }
            try expectEqual(pending["session"]["title"].string, "desktop edit")
            try expectFalse(WorkspaceInspection.hasGitRepository(workspace))
            // Reproduce a legacy desktop Changes request while the coding turn is
            // waiting. Its independent manual turn must not override the approval.
            let manual = try await client.request("/v1/sessions/\(workID)/tools", method: "POST", body: .object(["scope": scope, "tool": .string("git_diff"), "arguments": .object(["paths": .array([])])]))
            try expectEqual(manual["outcome"].string, "failed")
            let mixedSnapshot = try await client.request("/v1/sessions/\(workID)/snapshot")
            let mixed = SessionActivity(turns: mixedSnapshot["turns"].array)
            try expectEqual(mixed.waitingLabel, "Waiting for your approval")
            try expectFalse(mixed.isWorking)
            try expectEqual(mixed.feedback, nil)
            try expectEqual(mixedSnapshot["pending_requests"].array.count, 1)
            try expectEqual(try String(contentsOf: workspace.appendingPathComponent("desktop-demo.txt")), "before desktop\n")
            let approval = pending["pending_requests"].array[0]
            let toolID = pending["items"].array.first { $0["content"]["tool_call_id"] != .null }!["content"]["tool_call_id"].string
            let proposed = try await client.request("/v1/sessions/\(workID)/tools/\(toolID)")
            try expectEqual(proposed["request"]["arguments"]["content"].string, "after desktop\n")
            _ = try await client.request("/v1/approvals/\(approval["id"].string)", method: "POST", body: .object(["scope": scope, "approved": .bool(true), "approval_scope": .string("once")]))
            let afterApproval = try await poll(client, session: workID) { $0["turns"].array.contains { $0["id"] == approval["turn_id"] && $0["status"].string == "completed" } }
            try expectEqual(SessionActivity(turns: afterApproval["turns"].array).feedback, nil)
            try expectEqual(try String(contentsOf: workspace.appendingPathComponent("desktop-demo.txt")), "after desktop\n")
            _ = try await poll(client, session: workID) { $0["session"]["title"].string == "Desktop conversation overview" }
            // An opted-out first turn must remain unnamed after approval resume.
            let optout = try await client.request("/v1/sessions", method: "POST", body: .object(["scope": scope, "mode": .string("work"), "workspace_uri": .string(workspace.absoluteString + "/"), "title": .string("New conversation"), "model": .string(profile.model)]))
            let optoutID = optout["id"].string
            try Data("before desktop\n".utf8).write(to: workspace.appendingPathComponent("desktop-demo.txt"))
            _ = try await client.request("/v1/sessions/\(optoutID)/turns", method: "POST", body: .object(["scope": scope, "content": .string("desktop optout edit"), "generate_title": .bool(false)]))
            let optoutWaiting = try await poll(client, session: optoutID) { !$0["pending_requests"].array.isEmpty }
            let optoutApproval = optoutWaiting["pending_requests"].array[0]
            _ = try await client.request("/v1/approvals/\(optoutApproval["id"].string)", method: "POST", body: .object(["scope": scope, "approved": .bool(true), "approval_scope": .string("once")]))
            _ = try await poll(client, session: optoutID) { $0["turns"].array.last?["status"].string == "completed" }
            try await Task.sleep(for: .milliseconds(300))
            let optoutAfter = try await client.request("/v1/sessions/\(optoutID)")
            try expectEqual(optoutAfter["title"].string, "New conversation")
            let catalog = try await client.request("/v1/permission-profiles")
            let initialPreferences = try await client.request("/v1/sessions/\(workID)/preferences")
            try expectEqual(try SessionPermissions(preferences: initialPreferences, catalog: catalog.array, sessionID: workID).mode, .manual)
            for mode in PermissionMode.allCases {
                let updated = try await client.request("/v1/sessions/\(workID)/preferences", method: "PATCH", body: .object(["scope": scope, "permission_mode": .string(mode.rawValue)]))
                try expectEqual(try SessionPermissions(preferences: updated, catalog: catalog.array, sessionID: workID).mode, mode)
            }
            // Plan must remove write/command tools from the model's tool list.
            _ = try await client.request("/v1/sessions/\(workID)/turns", method: "POST", body: .object(["scope": scope, "content": .string("desktop plan")]))
            _ = try await poll(client, session: workID) { $0["turns"].array.last?["status"].string == "completed" }
            try expectEqual(try String(contentsOf: workspace.appendingPathComponent("desktop-demo.txt")), "after desktop\n")
            _ = try await client.request("/v1/sessions/\(workID)/preferences", method: "PATCH", body: .object(["scope": scope, "permission_mode": .string("accept_edits")]))
            try Data("before desktop\n".utf8).write(to: workspace.appendingPathComponent("desktop-demo.txt"))
            _ = try await client.request("/v1/sessions/\(workID)/turns", method: "POST", body: .object(["scope": scope, "content": .string("desktop edit")]))
            let autoEdited = try await poll(client, session: workID) { $0["turns"].array.last?["status"].string == "completed" }
            try expectTrue(autoEdited["pending_requests"].array.isEmpty)
            try expectEqual(try String(contentsOf: workspace.appendingPathComponent("desktop-demo.txt")), "after desktop\n")
            let chatPreferences = try await client.request("/v1/sessions/\(chatID)/preferences")
            try expectEqual(chatPreferences["permission_mode"].string, "manual")
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
            let workPrivacy = try await client.request("/v1/sessions/\(workID)/privacy")
            try expectFalse(workPrivacy.pretty.contains("before desktop"))
            try expectFalse(workPrivacy.pretty.contains("after desktop"))
            try expectFalse(workPrivacy.pretty.contains("api_key"))
            let events = await recorder.values
            try expectTrue(events.contains { $0["type"].string == "privacy.request.started" && $0["session_id"].string == chatID })
            try expectTrue(events.contains { $0["type"].string == "privacy.request.finished" && $0["session_id"].string == chatID })
            try expectTrue(events.contains { $0["type"].string == "session.updated" && $0["payload"]["reason"].string == "first_message" })
            try expectTrue(events.contains { $0["type"].string == "session.updated" && $0["payload"]["reason"].string == "first_turn_summary" })
            try expectTrue(events.contains { $0["notification"]["type"].string == "agent_message_delta" })
            let modelCatalog = try await client.models(sessionID: chatID)
            try expectEqual(modelCatalog.current, profile.model)
            try expectTrue(modelCatalog.options.contains { $0.id == profile.model && $0.selectable })
            try await client.changeModel(sessionID: chatID, model: "desktop-fixture-alternate")
            try expectEqual(try await client.models(sessionID: chatID).current, "desktop-fixture-alternate")
            try expectEqual(try await client.models(sessionID: workID).current, profile.model)
            stream.cancel()
            await engine.stop()
            let restarted = try await engine.start(profile: profile, key: "", repository: repo, executable: URL(fileURLWithPath: executable))
            let sessions = try await restarted.request("/v1/sessions")
            try expectTrue(sessions.array.contains { $0["id"].string == workID && $0["title"].string == "Desktop conversation overview" })
            try expectTrue(sessions.array.contains { $0["id"].string == chatID && $0["title"].string == "Desktop conversation overview" })
            try expectEqual(try await restarted.models(sessionID: chatID).current, "desktop-fixture-alternate")
            let savedPrivacy = try await restarted.privacy(sessionID: chatID)
            try expectTrue(Set(privacy.requests.map(\.id)).isSubset(of: Set(savedPrivacy.requests.map(\.id))))
            let savedPermissions = try await restarted.request("/v1/sessions/\(workID)/preferences")
            try expectEqual(savedPermissions["permission_mode"].string, "accept_edits")
            let other = try APIClient(base: restarted.base, token: "invalid", scope: Scope(profileID: UUID().uuidString))
            do { _ = try await other.request("/v1/sessions"); throw CheckFailure(description: "invalid token accepted") } catch DesktopError.http(401) { }
            await engine.stop()
            print("PASS: real engine Chat, Work, streaming, approval, file edit, question, cancellation, provider failure, proposed patch details, restart/history and auth")
            print("PASS: immediate local naming, model failure fallback, later-turn retry, title events and restart persistence")
            print("PASS: scoped model catalog, session model change/isolation and restart persistence")
            print("PASS: permission catalog, mode persistence/isolation, Plan tools and automatic accepted edits")
            print("PASS: native privacy API decoding, destinations/statuses, title requests, scoped access, cursor filtering, empty history, metadata-only records, live events and restart persistence")
        } catch { await engine.stop(); throw error }
    }
    static func poll(_ client: APIClient, session: String, until: (JSON) -> Bool) async throws -> JSON {
        var last = JSON.null
        for _ in 0..<120 {
            last = try await client.request("/v1/sessions/\(session)/snapshot", query: [.init(name: "limit", value: "100")])
            if until(last) { return last }
            if let turn = last["turns"].array.last, turn["status"].string == "failed", !turn["error_code"].string.hasPrefix("manual tool") { throw CheckFailure(description: "turn failed: \(last.pretty)") }
            try await Task.sleep(for: .milliseconds(100))
        }
        throw CheckFailure(description: "fixture timeout: \(last.pretty)")
    }
}
private actor EventRecorder {
    var values: [JSON] = []
    func add(_ batch: [JSON]) { values.append(contentsOf: batch) }
}
