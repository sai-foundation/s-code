import DesktopCore

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
        print("PASS: snapshot invalidation during request and draft acknowledgements")
    }
}
