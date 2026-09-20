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
        case "failed" where turn["error_code"].string.hasPrefix("manual tool"):
            return "A manually requested tool failed. Open its tool card for details."
        case "failed": return "This task failed. Check the connection, model access and tool details before trying again."
        case "cancelled": return "Task stopped. You can send another message when ready."
        default: return nil
        }
    }
}

/// A session can have a model turn and independent manual tool turns. A manual
/// failure cannot terminate or override a model turn waiting for the user.
public struct SessionActivity {
    public private(set) var turns: [JSON]
    public private(set) var revision: Int
    public init(turns: [JSON] = [], revision: Int = 0) {
        // Snapshots list turns by start time. Feedback follows the latest state
        // transition, including a model finishing after a manual tool failure.
        let fractional = ISO8601DateFormatter()
        fractional.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        let whole = ISO8601DateFormatter()
        func date(_ turn: JSON) -> Date {
            let value = turn["updated_at"].string
            return fractional.date(from: value) ?? whole.date(from: value) ?? .distantPast
        }
        var dated: [(index: Int, turn: JSON, date: Date)] = []
        for (index, turn) in turns.enumerated() { dated.append((index, turn, date(turn))) }
        dated.sort { left, right in
            if left.date == right.date { return left.index < right.index }
            return left.date < right.date
        }
        self.turns = dated.map { $0.turn }
        self.revision = revision
    }
    @discardableResult public mutating func apply(_ event: JSON) -> Bool {
        let n = event["notification"], id = event["turn_id"].string
        guard n["type"].string == "turn_status_changed", !id.isEmpty,
              event["sequence"].integer > revision else { return false }
        var turn = turns.first { $0["id"].string == id } ?? .object(["id": .string(id)])
        turn = turn.replacing("status", with: n["status"])
            .replacing("error_code", with: n["error_code"])
            .replacing("updated_at", with: event["timestamp"])
        turns.removeAll { $0["id"].string == id }
        turns.append(turn)
        revision = event["sequence"].integer
        return true
    }
    public var active: [JSON] { turns.filter { !["completed", "failed", "cancelled"].contains($0["status"].string) } }
    public var isRunning: Bool { !active.isEmpty }
    public var isWorking: Bool { active.contains { !["awaiting_approval", "awaiting_input"].contains($0["status"].string) } }
    public var waitingLabel: String? {
        if active.contains(where: { $0["status"].string == "awaiting_approval" }) { return "Waiting for your approval" }
        if active.contains(where: { $0["status"].string == "awaiting_input" }) { return "Waiting for your answer" }
        return nil
    }
    public var feedback: String? { isRunning ? nil : TurnFeedback.message(turns.last ?? .null) }
}
