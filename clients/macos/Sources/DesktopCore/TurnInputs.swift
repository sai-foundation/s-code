import Foundation

public enum TurnInputMode: String, Sendable { case queue, steer }
public struct TurnInputRecord: Identifiable, Equatable, Sendable {
    public let value: JSON
    public var id: String { value["id"].string }
    public var content: String { value["content"].string }
    public var mode: String { value["mode"].string }
    public var status: String { value["status"].string }
    public var key: String { value["idempotency_key"].string }
    public var isPending: Bool { status == "pending" || status == "processing" }
    public init(_ value: JSON, scope: Scope, sessionID: String) throws {
        guard scope.owns(value), value["session_id"].string == sessionID,
              !value["id"].string.isEmpty, !value["target_turn_id"].string.isEmpty,
              !value["idempotency_key"].string.isEmpty, case .string = value["content"],
              TurnInputMode(rawValue: value["mode"].string) != nil,
              ["pending", "processing", "consumed", "cancelled"].contains(value["status"].string) else { throw DesktopError.protocolMismatch }
        self.value = value
    }
    public static func list(_ json: JSON, scope: Scope, sessionID: String) throws -> [Self] {
        guard case .array(let values) = json else { throw DesktopError.protocolMismatch }
        let result = try values.map { try Self($0, scope: scope, sessionID: sessionID) }
        guard Set(result.map(\.id)).count == result.count else { throw DesktopError.protocolMismatch }
        return result.filter(\.isPending)
    }
}

public struct TurnInputAttempt: Equatable, Sendable {
    public let target: String
    public let mode: TurnInputMode
    public let content: String
    public let originalDraft: String
    public let key: String
    public func body(scope: Scope) -> JSON { .object(["scope": scope.json, "target_turn_id": .string(target), "mode": .string(mode.rawValue), "content": .string(content), "idempotency_key": .string(key)]) }
    public func accepts(_ record: TurnInputRecord) -> Bool {
        record.key == key && record.content == content && record.mode == mode.rawValue && record.value["target_turn_id"].string == target
    }
}

public struct TurnInputContext: Hashable, Sendable {
    public let profileID: String
    public let provider: String
    public let endpoint: String
    public let scope: JSON
    public init(profileID: String, provider: String, endpoint: String, scope: JSON) {
        self.profileID = profileID; self.provider = provider; self.endpoint = endpoint; self.scope = scope
    }
    public init(profile: Profile, scope: Scope) {
        self.init(profileID: profile.id, provider: profile.provider, endpoint: profile.endpoint, scope: scope.json)
    }
}

/// Failed or uncertain POSTs retain their idempotency key, including across selection changes.
public struct TurnInputAttempts {
    private struct Key: Hashable { let context: TurnInputContext; let session: String; let mode: String; let content: String }
    private var attempts: [Key: TurnInputAttempt] = [:]
    public init() {}
    public mutating func begin(context: TurnInputContext, session: String, target: String, mode: TurnInputMode, content: String, originalDraft: String? = nil) -> TurnInputAttempt {
        let identity = Key(context: context, session: session, mode: mode.rawValue, content: content)
        if let attempt = attempts[identity] { return attempt }
        let attempt = TurnInputAttempt(target: target, mode: mode, content: content, originalDraft: originalDraft ?? content, key: "desktop-" + UUID().uuidString.lowercased())
        attempts[identity] = attempt
        return attempt
    }
    public func acknowledgedDraft(context: TurnInputContext, session: String, record: TurnInputRecord) -> String? {
        let key = Key(context: context, session: session, mode: record.mode, content: record.content)
        guard let attempt = attempts[key], attempt.accepts(record) else { return nil }
        return attempt.originalDraft
    }
    @discardableResult public mutating func acknowledge(context: TurnInputContext, session: String, record: TurnInputRecord) -> Bool {
        let key = Key(context: context, session: session, mode: record.mode, content: record.content)
        guard attempts[key]?.accepts(record) == true else { return false }
        attempts.removeValue(forKey: key); return true
    }
    public func hasUnconfirmed(context: TurnInputContext, session: String, content: String) -> Bool {
        attempts.keys.contains { $0.context == context && $0.session == session && $0.content == content }
    }
    public mutating func discard(context: TurnInputContext, session: String, content: String) {
        attempts = attempts.filter { !($0.key.context == context && $0.key.session == session && $0.key.content == content) }
    }
}

public enum TurnInputTarget {
    /// Manual tool turns and event-only placeholders cannot become composer targets.
    public static func active(turns: [JSON], sessionID: String) -> JSON? {
        let active = turns.filter {
            $0["session_id"].string == sessionID && !$0["id"].string.isEmpty &&
            !["completed", "failed", "cancelled"].contains($0["status"].string) &&
            $0["checkpoint"]["execution_mode"].string != "manual_tool"
        }
        return active.count == 1 ? active[0] : nil
    }
    public static func canSteer(_ turn: JSON) -> Bool { ["preparing_context", "calling_model", "running_tool"].contains(turn["status"].string) }
}

extension APIClient {
    public func turnInputs(sessionID: String) async throws -> [TurnInputRecord] {
        try await TurnInputRecord.list(request("/v1/sessions/\(sessionID)/inputs"), scope: scope, sessionID: sessionID)
    }
    public func submitTurnInput(sessionID: String, attempt: TurnInputAttempt) async throws -> TurnInputRecord {
        let response = try await request("/v1/sessions/\(sessionID)/inputs", method: "POST", body: attempt.body(scope: scope))
        let record = try TurnInputRecord(response, scope: scope, sessionID: sessionID)
        guard attempt.accepts(record) else { throw DesktopError.protocolMismatch }
        return record
    }
    public func cancelTurnInput(sessionID: String, inputID: String) async throws -> TurnInputRecord {
        let response = try await request("/v1/turn-inputs/\(inputID)", method: "DELETE", body: .object(["scope": scope.json]))
        let record = try TurnInputRecord(response, scope: scope, sessionID: sessionID)
        guard record.id == inputID, record.status == "cancelled" else { throw DesktopError.protocolMismatch }
        return record
    }
}

/// Reads started before a queue mutation or selection change cannot restore stale items.
public struct TurnInputReadGate {
    public struct Ticket { fileprivate let generation: UUID; fileprivate let request: UUID }
    public private(set) var revision = UUID()
    private var latest = UUID()
    public init() {}
    public mutating func invalidate() { revision = UUID(); latest = UUID() }
    public mutating func invalidateRequests() { latest = UUID() }
    public mutating func begin() -> Ticket { latest = UUID(); return Ticket(generation: revision, request: latest) }
    public func accepts(_ ticket: Ticket) -> Bool { ticket.generation == revision && ticket.request == latest }
}
