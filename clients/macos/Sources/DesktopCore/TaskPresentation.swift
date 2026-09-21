import Foundation

/// Display-only paths. Canonical identity and enforcement remain on the daemon.
public struct ProtectedPathLabel: Equatable {
    public let name: String
    public let parent: String
    public let inProject: Bool
    public init(path: String, root: String?) {
        let path = (path as NSString).standardizingPath
        let root = root.map { ($0 as NSString).standardizingPath }
        inProject = root.map { path == $0 || path.hasPrefix($0 == "/" ? "/" : $0 + "/") } ?? false
        name = URL(fileURLWithPath: path).lastPathComponent.isEmpty ? "/" : URL(fileURLWithPath: path).lastPathComponent
        let shown: String
        if inProject, let root { shown = path == root ? "." : String(path.dropFirst(root == "/" ? 1 : root.count + 1)) }
        else { shown = path }
        let directory = (shown as NSString).deletingLastPathComponent
        parent = shown == "." ? "Project root" : directory.isEmpty ? "Project root" : directory
    }
}

/// These associations come from authenticated events, never message text or tool-name guesses.
public struct TaskProvenance {
    public private(set) var localMessages: Set<String> = []
    public init() {}
    public mutating func receive(_ events: [JSON], sessionID: String) {
        for event in events where event["session_id"].string == sessionID {
            if ["model.delta", "turn.completed"].contains(event["type"].string), event["payload"]["local"].boolean,
               !event["payload"]["item_id"].string.isEmpty {
                localMessages.insert(event["payload"]["item_id"].string)
            }
        }
    }
}

public enum ToolPresentation {
    public static func approvalToolID(_ request: JSON, rows: [TranscriptRow], sessionID: String) -> String? {
        guard request["session_id"].string == sessionID else { return nil }
        // Pending approvals are separate from paginated transcript items. New
        // snapshots carry the exact association even when their row is unloaded.
        if request["tool_call_id"] != .null {
            let id = request["tool_call_id"].string
            return validToolID(id) ? id : nil
        }
        guard let item = rows.first(where: { $0.kind == "approval" && $0.id == request["item_id"].string && $0.value["session_id"].string == sessionID }) else { return nil }
        let prefix = "/v1/sessions/" + sessionID + "/tools/"
        let href = item.value["detail"]["href"].string
        guard href.hasPrefix(prefix) else { return nil }
        let id = String(href.dropFirst(prefix.count))
        return validToolID(id) ? id : nil
    }
    private static func validToolID(_ id: String) -> Bool {
        !id.isEmpty && id.utf8.allSatisfy { (65...90).contains($0) || (97...122).contains($0) || (48...57).contains($0) || $0 == 45 || $0 == 95 }
    }
    public static func failure(row: TranscriptRow, detail: JSON?) -> String? {
        guard ["failed", "denied", "cancelled"].contains(row.status) else { return nil }
        if let detail, !detail["error"].string.isEmpty { return detail["error"].string }
        if row.status == "cancelled" { return "Tool stopped before completion." }
        if row.status == "denied" {
            let reason = row.value["content"]["policy_reason"].string
            return reason.isEmpty ? "Rejected by policy or user." : reason
        }
        return detail == nil ? "Loading failure reason…" : "The tool failed without a recorded error message. Inspect its arguments and result below."
    }
    public static func validate(_ detail: JSON, toolID: String, sessionID: String, scope: Scope) throws -> JSON {
        guard detail["request"]["id"].string == toolID,
              detail["request"]["session_id"].string == sessionID,
              scope.owns(detail["request"]) else { throw DesktopError.protocolMismatch }
        return detail
    }
}
