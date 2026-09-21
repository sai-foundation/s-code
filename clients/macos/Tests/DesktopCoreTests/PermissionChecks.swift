import DesktopCore

enum PermissionChecks {
    static func run() throws {
        var pending = PendingPermissionChanges()
        pending.begin(actor: "account-1", session: "a")
        // Selecting B must not forget the outstanding write when returning to A.
        try expectFalse(pending.contains(actor: "account-1", session: "b"))
        try expectFalse(pending.contains(actor: "account-2", session: "a"))
        try expectTrue(pending.contains(actor: "account-1", session: "a"))
        pending.begin(actor: "account-1", session: "b")
        pending.finish(actor: "account-1", session: "b")
        try expectTrue(pending.contains(actor: "account-1", session: "a"))
        pending.finish(actor: "account-1", session: "a")
        try expectFalse(pending.contains(actor: "account-1", session: "a"))
        let catalog = PermissionMode.allCases.map { JSON.object(["mode": .string($0.rawValue), "description": .string($0.title)]) }
        let preferences = JSON.object(["session_id": .string("a"), "permission_mode": .string("manual")])
        let manual = try SessionPermissions(preferences: preferences, catalog: catalog, sessionID: "a")
        try expectEqual(manual.mode, .manual)
        try expectEqual(manual.options, PermissionMode.allCases)
        for mode in PermissionMode.allCases {
            let value = try SessionPermissions(preferences: preferences.replacing("permission_mode", with: .string(mode.rawValue)), catalog: catalog, sessionID: "a")
            try expectEqual(value.mode, mode)
            try expectEqual(value.lock(mode), nil)
        }
        try expectThrows(try SessionPermissions(preferences: preferences, catalog: catalog, sessionID: "b"))
        try expectThrows(try SessionPermissions(preferences: preferences.replacing("permission_mode", with: .string("full_access")), catalog: catalog, sessionID: "a"))
        try expectThrows(try SessionPermissions(preferences: preferences, catalog: [], sessionID: "a"))
        let legacyCatalog = catalog.filter { $0["mode"].string != "full" }
        let legacy = try SessionPermissions(preferences: preferences, catalog: legacyCatalog, sessionID: "a")
        try expectFalse(legacy.options.contains(.full))
        try expectEqual(legacy.lock(.full), "Unavailable")
        try expectThrows(try SessionPermissions(preferences: preferences.replacing("permission_mode", with: .string("full")), catalog: legacyCatalog, sessionID: "a"))
        let locked = try SessionPermissions(preferences: preferences.replacing("locked_reason", with: .string("Managed policy")), catalog: catalog, sessionID: "a")
        try expectEqual(locked.lock(.workspace), "Managed policy")
        let restrictedCatalog = catalog.map { $0["mode"].string == "workspace" ? $0.replacing("locked_reason", with: .string("Unavailable here")) : $0 }
        let restricted = try SessionPermissions(preferences: preferences, catalog: restrictedCatalog, sessionID: "a")
        try expectEqual(restricted.lock(.workspace), "Unavailable here")
        try expectEqual(restricted.lock(.manual), nil)
        print("PASS: permission modes, session identity, unknown-mode rejection and policy locks")
    }
}
