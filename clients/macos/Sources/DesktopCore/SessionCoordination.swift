import Foundation

/// Tracks invalidations arriving while a snapshot request is suspended.
public struct SnapshotRefreshState {
    private var requested = 0
    private var completed = 0
    public init() {}
    public mutating func invalidate() { requested += 1 }
    public var needsRefresh: Bool { requested != completed }
    public var ticket: Int { requested }
    public mutating func finish(_ ticket: Int) { completed = max(completed, ticket) }
}

public enum DraftSubmission {
    /// An acknowledgement consumes only the exact draft that was submitted.
    public static func acknowledged(current: String, submitted: String) -> String {
        current == submitted ? "" : current
    }
}

public enum TurnFeedback {
    public static func message(_ turn: JSON) -> String? {
        switch turn["status"].string {
        case "failed": return "This task failed. Check the connection, model access and tool details before trying again."
        case "cancelled": return "Task stopped. You can send another message when ready."
        default: return nil
        }
    }
}
