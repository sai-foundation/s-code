import Foundation

/// An in-flight write belongs to its account/session, not to the selected view.
public struct PendingPermissionChanges {
    private struct Key: Hashable { let actor: String; let session: String }
    private var pending: Set<Key> = []
    public init() {}
    public mutating func begin(actor: String, session: String) { pending.insert(Key(actor: actor, session: session)) }
    public mutating func finish(actor: String, session: String) { pending.remove(Key(actor: actor, session: session)) }
    public func contains(actor: String, session: String) -> Bool { pending.contains(Key(actor: actor, session: session)) }
}

public enum PermissionMode: String, CaseIterable, Identifiable {
    case manual, acceptEdits = "accept_edits", workspace, plan
    public var id: String { rawValue }
    public var title: String {
        switch self {
        case .manual: return "Manual"
        case .acceptEdits: return "Accept edits"
        case .workspace: return "Workspace"
        case .plan: return "Plan"
        }
    }
    public var icon: String {
        switch self {
        case .manual: return "hand.raised"
        case .acceptEdits: return "pencil.and.outline"
        case .workspace: return "folder.badge.gearshape"
        case .plan: return "list.clipboard"
        }
    }
}

/// Display server-confirmed preferences only; unknown modes fail closed.
public struct SessionPermissions {
    public let sessionID: String
    public let mode: PermissionMode
    public let lockedReason: String?
    public let catalog: [JSON]
    public var options: [PermissionMode] {
        PermissionMode.allCases.filter { mode in catalog.contains { $0["mode"].string == mode.rawValue } }
    }
    public init(preferences: JSON, catalog: [JSON], sessionID: String) throws {
        guard preferences["session_id"].string == sessionID,
              let mode = PermissionMode(rawValue: preferences["permission_mode"].string),
              catalog.contains(where: { $0["mode"].string == mode.rawValue }) else { throw DesktopError.protocolMismatch }
        self.sessionID = sessionID; self.mode = mode; self.catalog = catalog
        self.lockedReason = preferences["locked_reason"] == .null ? nil : preferences["locked_reason"].string
    }
    public func description(_ mode: PermissionMode) -> String {
        catalog.first { $0["mode"].string == mode.rawValue }?["description"].string ?? ""
    }
    public func lock(_ mode: PermissionMode) -> String? {
        guard lockedReason == nil else { return lockedReason }
        guard let option = catalog.first(where: { $0["mode"].string == mode.rawValue }) else { return "Unavailable" }
        return option["locked_reason"] == .null ? nil : option["locked_reason"].string
    }
}
