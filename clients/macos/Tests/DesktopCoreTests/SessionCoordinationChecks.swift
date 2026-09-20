import DesktopCore
import Foundation

enum SessionCoordinationChecks {
    static func run() throws {
        let original = Profile(model: "a")
        var changed = original
        changed.endpoint = "https://example.invalid/v1"
        try expectNotEqual(original.credentialID, changed.credentialID)
        changed = original; changed.provider = "anthropic"
        try expectNotEqual(original.credentialID, changed.credentialID)
        changed = original; changed.model = "b"
        try expectEqual(original.credentialID, changed.credentialID)
        var refresh = SnapshotRefreshState()
        refresh.invalidate()
        let delayedSnapshot = refresh.ticket
        refresh.invalidate() // Completion arrives while the snapshot request is in flight.
        refresh.finish(delayedSnapshot)
        try expectTrue(refresh.needsRefresh)
        refresh.finish(refresh.ticket)
        try expectFalse(refresh.needsRefresh)
        try expectEqual(DraftSubmission.acknowledged(current: "sent", submitted: "sent"), "")
        try expectEqual(DraftSubmission.acknowledged(current: "new draft", submitted: "sent"), "new draft")
        var drafts = ["a": "sent", "b": "another conversation"]
        drafts["a"] = DraftSubmission.acknowledged(current: drafts["a"]!, submitted: "sent")
        try expectEqual(drafts["a"], "")
        try expectEqual(drafts["b"], "another conversation")
        try expectTrue(TurnFeedback.message(.object(["status": .string("failed"), "error_code": .string("upstream secret")]))!.contains("failed"))
        try expectFalse(TurnFeedback.message(.object(["status": .string("failed"), "error_code": .string("upstream secret")]))!.contains("secret"))
        try expectTrue(TurnFeedback.message(.object(["status": .string("cancelled")]))!.contains("stopped"))
        try expectEqual(TurnFeedback.message(.object(["status": .string("completed")])), nil)
        let waiting = JSON.object(["id": .string("model"), "status": .string("awaiting_approval")])
        let manualFailure = JSON.object(["id": .string("diff"), "status": .string("failed"), "error_code": .string("manual tool did not complete")])
        let mixed = SessionActivity(turns: [waiting, manualFailure, manualFailure])
        try expectTrue(mixed.isRunning)
        try expectFalse(mixed.isWorking)
        try expectEqual(mixed.waitingLabel, "Waiting for your approval")
        try expectEqual(mixed.feedback, nil)
        let question = SessionActivity(turns: [.object(["status": .string("awaiting_input")]), manualFailure])
        try expectEqual(question.waitingLabel, "Waiting for your answer")
        try expectFalse(question.isWorking)
        try expectTrue(SessionActivity(turns: [.object(["status": .string("calling_model")]), manualFailure]).isWorking)
        try expectTrue(SessionActivity(turns: [manualFailure]).feedback!.contains("manually requested tool"))
        let datedManual = manualFailure.replacing("updated_at", with: .string("2026-09-20T10:00:01.500Z"))
        for status in ["completed", "failed", "cancelled"] {
            let finishedModel = waiting.replacing("status", with: .string(status))
                .replacing("updated_at", with: .string("2026-09-20T10:00:02Z"))
            // Snapshot order is start order, not completion order.
            let finished = SessionActivity(turns: [finishedModel, datedManual])
            try expectEqual(finished.feedback, TurnFeedback.message(finishedModel))
        }
        var replay = SessionActivity(turns: [waiting, datedManual], revision: 20)
        func statusEvent(_ id: String, _ status: String, _ sequence: Int) -> JSON {
            .object(["turn_id": .string(id), "sequence": .number(Double(sequence)),
                     "timestamp": .string("2026-09-20T10:00:02Z"),
                     "notification": .object(["type": .string("turn_status_changed"), "status": .string(status)])])
        }
        try expectFalse(replay.apply(statusEvent("model", "calling_model", 19)))
        try expectFalse(replay.apply(statusEvent("model", "calling_model", 20)))
        try expectEqual(replay.waitingLabel, "Waiting for your approval")
        try expectTrue(replay.apply(statusEvent("diff", "failed", 21)))
        try expectTrue(replay.isRunning)
        try expectEqual(replay.feedback, nil)
        try expectTrue(replay.apply(statusEvent("model", "completed", 22)))
        try expectFalse(replay.isRunning)
        try expectEqual(replay.feedback, nil)
        try expectFalse(replay.apply(statusEvent("diff", "failed", 21)))
        try expectEqual(replay.feedback, nil)
        let temp = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: temp) }
        let project = temp.appendingPathComponent("new-project")
        try FileManager.default.createDirectory(at: project, withIntermediateDirectories: true)
        try expectFalse(WorkspaceInspection.hasGitRepository(project))
        try Data("gitdir: ../repository/worktrees/test\n".utf8).write(to: project.appendingPathComponent(".git"))
        try expectTrue(WorkspaceInspection.hasGitRepository(project))
        let nested = project.appendingPathComponent("src")
        try FileManager.default.createDirectory(at: nested, withIntermediateDirectories: true)
        try expectFalse(WorkspaceInspection.hasGitRepository(nested))
        let linked = temp.appendingPathComponent("linked-project")
        try FileManager.default.createSymbolicLink(at: linked, withDestinationURL: project)
        try expectTrue(WorkspaceInspection.hasGitRepository(linked))
        try expectFalse(WorkspaceInspection.hasGitRepository(URL(string: "https://example.invalid")!))
        print("PASS: snapshot/draft coordination, concurrent manual turns, waiting states and Git preflight")
    }
}
