import Foundation
import DesktopCore

@MainActor enum TaskPresentationChecks {
    static func run() async throws {
        let project = ProtectedPathLabel(path: "/project/secrets/.env", root: "/project/")
        try expectTrue(project.inProject); try expectEqual(project.name, ".env"); try expectEqual(project.parent, "secrets")
        try expectFalse(ProtectedPathLabel(path: "/project-backup/secrets", root: "/project").inProject)
        try expectFalse(ProtectedPathLabel(path: "/project/private", root: nil).inProject)
        try expectEqual(ProtectedPathLabel(path: "/project", root: "/project").parent, "Project root")
        try expectEqual(ProtectedPathLabel(path: "/file", root: "/").parent, "Project root")
        try expectFalse(ProtectedPathLabel(path: "/project/../private", root: "/project").inProject)
        func activity(_ status: String) -> SessionActivity { SessionActivity(turns: [.object(["id": .string("t"), "status": .string(status)])]) }
        try expectEqual(activity("calling_model").workingLabel, "Waiting for the model…")
        try expectEqual(activity("running_tool").workingLabel, "Running tools…")
        for status in ["failed", "cancelled", "completed", "awaiting_approval", "awaiting_input"] { try expectEqual(activity(status).workingLabel, nil) }
        let mixed = SessionActivity(turns: [.object(["id": .string("t"), "status": .string("awaiting_approval")]), .object(["id": .string("manual"), "status": .string("running_tool")])])
        try expectEqual(mixed.workingLabel, nil); try expectEqual(mixed.waitingLabel, "Waiting for your approval")
        let scope = Scope(profileID: "test")
        let detail: JSON = .object(["request": .object(["scope": scope.json, "session_id": .string("a"), "id": .string("tool-a"), "arguments": .object(["path": .string("private.txt")])]), "error": .string("Command exceeded its time limit")])
        try expectNoThrow(try ToolPresentation.validate(detail, toolID: "tool-a", sessionID: "a", scope: scope))
        try expectThrows(try ToolPresentation.validate(detail, toolID: "other", sessionID: "a", scope: scope))
        try expectThrows(try ToolPresentation.validate(detail, toolID: "tool-a", sessionID: "b", scope: scope))
        try expectThrows(try ToolPresentation.validate(detail, toolID: "tool-a", sessionID: "a", scope: Scope(profileID: "foreign")))
        let failed = TranscriptRow(.object(["status": .string("failed")]))
        try expectEqual(ToolPresentation.failure(row: failed, detail: detail), "Command exceeded its time limit")
        try expectEqual(ToolPresentation.failure(row: TranscriptRow(.object(["status": .string("completed")])), detail: detail), nil)
        let approval: JSON = .object(["session_id": .string("a"), "item_id": .string("approval-a")])
        let item: JSON = .object(["id": .string("approval-a"), "session_id": .string("a"), "kind": .string("approval"), "detail": .object(["href": .string("/v1/sessions/a/tools/tool-a")])])
        try expectEqual(ToolPresentation.approvalToolID(approval, rows: [TranscriptRow(item)], sessionID: "a"), "tool-a")
        for href in ["/v1/sessions/b/tools/tool-a", "/v1/sessions/a/tools/../private", "/v1/sessions/a/tools/tool-a?scope=other", "/v1/sessions/a/tools/"] {
            try expectEqual(ToolPresentation.approvalToolID(approval, rows: [TranscriptRow(item.replacing("detail", with: .object(["href": .string(href)])))], sessionID: "a"), nil)
        }
        try expectEqual(ToolPresentation.approvalToolID(approval, rows: [TranscriptRow(item)], sessionID: "b"), nil)
        // Pending requests render independently of the transcript page; their
        // direct association must remain inspectable when the row is unloaded.
        let pending = approval.replacing("tool_call_id", with: .string("tool-direct"))
        try expectEqual(ToolPresentation.approvalToolID(pending, rows: [], sessionID: "a"), "tool-direct")
        try expectEqual(ToolPresentation.approvalToolID(pending, rows: [TranscriptRow(item)], sessionID: "a"), "tool-direct")
        try expectEqual(ToolPresentation.approvalToolID(pending, rows: [], sessionID: "other"), nil)
        try expectEqual(ToolPresentation.approvalToolID(approval.replacing("tool_call_id", with: .null), rows: [TranscriptRow(item)], sessionID: "a"), "tool-a")
        for malformed in [JSON.string(""), .string("../escape"), .string("tool?actor=other"), .number(1)] {
            try expectEqual(ToolPresentation.approvalToolID(approval.replacing("tool_call_id", with: malformed), rows: [TranscriptRow(item)], sessionID: "a"), nil)
        }
        var provenance = TaskProvenance()
        let event: JSON = .object(["session_id": .string("a"), "type": .string("model.delta"), "payload": .object(["local": .bool(true), "item_id": .string("local-a")])])
        provenance.receive([event.replacing("session_id", with: .string("b")), event.replacing("payload", with: .object(["item_id": .string("normal"), "text": .string("Protected locally.")]))], sessionID: "a")
        try expectTrue(provenance.localMessages.isEmpty)
        provenance.receive([event], sessionID: "a"); try expectEqual(provenance.localMessages, ["local-a"])
        let protections = PrivacyProtections(), policy = try ProtectionChecks.policy(1)
        protections.reset(fetch: { policy }, mutate: { _, _ in throw CheckFailure(description: "stale queued action must not mutate") })
        await protections.refresh(); let oldContext = protections.contextID
        protections.reset(fetch: { policy }, mutate: { _, _ in throw CheckFailure(description: "stale queued action must not mutate") }); await protections.refresh()
        try expectFalse(await protections.change(.add("/private"), contextID: oldContext))
        try expectEqual(protections.error, nil)
        print("PASS: grouped protected paths, task phase/terminal states, exact scoped approval/tool details, local provenance and stale queued protection actions")
    }
}
