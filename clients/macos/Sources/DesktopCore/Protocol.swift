import Foundation

public enum JSON: Codable, Equatable, Hashable, Sendable {
    case object([String: JSON]), array([JSON]), string(String), number(Double), bool(Bool), null
    public init(from decoder: Decoder) throws {
        let c = try decoder.singleValueContainer()
        if c.decodeNil() { self = .null }
        else if let v = try? c.decode(Bool.self) { self = .bool(v) }
        else if let v = try? c.decode(String.self) { self = .string(v) }
        else if let v = try? c.decode(Double.self) { self = .number(v) }
        else if let v = try? c.decode([JSON].self) { self = .array(v) }
        else { self = .object(try c.decode([String: JSON].self)) }
    }
    public func encode(to encoder: Encoder) throws {
        var c = encoder.singleValueContainer()
        switch self {
        case .null: try c.encodeNil()
        case .bool(let v): try c.encode(v)
        case .string(let v): try c.encode(v)
        case .number(let v): try c.encode(v)
        case .array(let v): try c.encode(v)
        case .object(let v): try c.encode(v)
        }
    }
    public subscript(_ key: String) -> JSON { if case .object(let o) = self { return o[key] ?? .null }; return .null }
    public var string: String { if case .string(let s) = self { return s }; return "" }
    public var array: [JSON] { if case .array(let a) = self { return a }; return [] }
    public var integer: Int { if case .number(let n) = self, n.isFinite, n >= 0, n < Double(Int.max) { return Int(n) }; return 0 }
    public var boolean: Bool { if case .bool(let b) = self { return b }; return false }
    public var pretty: String { guard let d = try? JSONEncoder.pretty.encode(self) else { return "" }; return String(decoding: d, as: UTF8.self) }
    public func replacing(_ key: String, with value: JSON) -> JSON { guard case .object(var o) = self else { return self }; o[key] = value; return .object(o) }
}
extension JSONEncoder {
    static var pretty: JSONEncoder { let e = JSONEncoder(); e.outputFormatting = [.prettyPrinted, .sortedKeys]; return e }
}
public struct Scope: Sendable, Equatable {
    public let actor: String
    public init(profileID: String) { actor = "desktop_" + profileID }
    public var json: JSON { .object(["organization_id": .string("org_local"), "team_id": .string("team_local"), "actor_id": .string(actor)]) }
    public var query: [URLQueryItem] { [URLQueryItem(name: "organization_id", value: "org_local"), URLQueryItem(name: "team_id", value: "team_local"), URLQueryItem(name: "actor_id", value: actor)] }
    public func owns(_ session: JSON) -> Bool { session["scope"]["organization_id"].string == "org_local" && session["scope"]["team_id"].string == "team_local" && session["scope"]["actor_id"].string == actor }
}
public struct Conversation: Identifiable, Equatable, Sendable {
    public let value: JSON
    public init(_ value: JSON) { self.value = value }
    public var id: String { value["id"].string }
    public var title: String { value["title"].string }
    public var mode: String { value["mode"].string }
    public var folder: String { URL(string: value["workspace_uri"].string)?.path ?? "" }
    public var model: String { value["model"].string }
}
public struct TranscriptRow: Identifiable, Equatable, Sendable {
    public var value: JSON
    public init(_ value: JSON) { self.value = value }
    public var id: String { value["id"].string }
    public var kind: String { value["kind"].string }
    public var status: String { value["status"].string }
    public var isMessage: Bool { value["content"]["type"].string == "message" }
    public var role: String { value["content"]["role"].string }
    public var text: String {
        let c = value["content"]
        if c["type"].string == "message" {
            if case .string(let t) = c["content"] { return t }
            return c["content"]["text"].string.isEmpty ? c["content"].pretty : c["content"]["text"].string
        }
        return c["display"].string.isEmpty ? value["summary"].string : c["display"].string
    }
    public mutating func append(_ delta: String) {
        let c = value["content"].replacing("content", with: .string(text + delta))
        value = value.replacing("content", with: c)
    }
}
public struct TranscriptState: Sendable {
    public private(set) var rows: [TranscriptRow] = []
    public private(set) var cursor = 0
    public private(set) var revision = 0
    public private(set) var sessionID = ""
    private var snapshotRevision = 0
    private var persistedIDs: Set<String> = []
    private var activeTurnIDs: Set<String> = []
    private var streamStarts: [String: Int] = [:]
    private var recoveryCursor: Int?
    public var needsReplay: Bool { recoveryCursor != nil }
    public init() {}

    /// Replays an unfinished answer from its first delta. Snapshot projections do
    /// not contain assistant text until completion, so their cursor is not a safe
    /// reconnect position for an unfinished turn. Offset checks deduplicate replay.
    @discardableResult public mutating func prepareForReconnect() -> Int {
        if let start = streamStarts.values.min() { cursor = min(cursor, max(0, start - 1)) }
        if let recoveryCursor { cursor = min(cursor, recoveryCursor) }
        recoveryCursor = nil
        return cursor
    }

    public mutating func load(_ snapshot: JSON, scope: Scope, older: Bool = false) throws {
        let s = snapshot["session"]
        guard scope.owns(s), !s["id"].string.isEmpty, snapshot["protocol_version"].string == "1.0" else { throw DesktopError.protocolMismatch }
        let incoming = snapshot["items"].array
        guard incoming.allSatisfy({ $0["session_id"].string == s["id"].string && !$0["id"].string.isEmpty }) else { throw DesktopError.protocolMismatch }
        if older {
            guard sessionID == s["id"].string else { throw DesktopError.protocolMismatch }
            let ids = Set(rows.map(\.id)); rows = incoming.filter { !ids.contains($0["id"].string) }.map(TranscriptRow.init) + rows
            persistedIDs.formUnion(incoming.map { $0["id"].string })
            return
        }
        let sameSession = sessionID == s["id"].string
        if sameSession && snapshot["snapshot_revision"].integer < revision { return }
        if !sameSession { self = TranscriptState() }
        sessionID = s["id"].string
        activeTurnIDs = Set(snapshot["turns"].array.filter {
            !["completed", "failed", "cancelled"].contains($0["status"].string)
        }.map { $0["id"].string })
        let firstDate = incoming.first?["created_at"].string ?? ""
        let ids = Set(incoming.map { $0["id"].string })
        let streams = rows.filter {
            streamStarts[$0.id] != nil && !ids.contains($0.id)
                && (snapshot["turns"] == .null || activeTurnIDs.contains($0.value["turn_id"].string))
        }
        let prefix = rows.filter {
            streamStarts[$0.id] == nil && !firstDate.isEmpty && !ids.contains($0.id) && $0.value["created_at"].string < firstDate
        }
        rows = prefix + incoming.map(TranscriptRow.init)
        persistedIDs = Set(rows.map(\.id))
        // Keep the synthetic answer in chronological position even when tool or
        // approval snapshots arrive while that answer is still streaming.
        for row in streams {
            let date = row.value["created_at"].string
            let index = rows.firstIndex { !date.isEmpty && $0.value["created_at"].string > date } ?? rows.endIndex
            rows.insert(row, at: index)
        }
        let survivingIDs = Set(streams.map(\.id))
        streamStarts = streamStarts.filter { survivingIDs.contains($0.key) }
        snapshotRevision = snapshot["snapshot_revision"].integer
        revision = snapshotRevision
        if !activeTurnIDs.isEmpty || !streams.isEmpty || recoveryCursor != nil {
            // A cold selection of an active turn starts at zero to recover its
            // journaled prefix; an existing selection keeps its last safe cursor.
            cursor = min(cursor, snapshot["cursor"].integer)
        } else {
            cursor = max(cursor, snapshot["cursor"].integer)
        }
    }
    /// Returns false when a snapshot is needed to repair state. Global event gaps are normal.
    public mutating func apply(_ event: JSON) -> Bool {
        let sequence = event["sequence"].integer
        if sequence <= cursor { return true }
        if event["session_id"].string != sessionID { cursor = sequence; return true }
        let n = event["notification"]
        // Structural events establish a revision barrier immediately, even though
        // their full projection must be obtained from a snapshot.
        revision = max(revision, sequence)
        guard n["type"].string == "agent_message_delta" else {
            if sequence <= snapshotRevision { cursor = sequence; return true }
            return false
        }
        let id = n["item_id"].string, delta = n["delta"].string
        guard !id.isEmpty else { return false }
        if sequence <= snapshotRevision && (persistedIDs.contains(id)
            || (!activeTurnIDs.contains(event["turn_id"].string) && streamStarts[id] == nil)) {
            cursor = sequence
            return true
        }
        let offset = n["byte_offset"]
        if !rows.contains(where: { $0.id == id }) {
            guard offset == .number(0) else {
                recoveryCursor = 0
                return false
            }
            rows.append(TranscriptRow(.object([
                "id": .string(id), "session_id": .string(sessionID), "turn_id": event["turn_id"],
                "kind": .string("agent_message"), "status": .string("streaming"),
                "created_at": event["timestamp"],
                "content": .object(["type": .string("message"), "role": .string("assistant"), "content": .string("")])
            ])))
        }
        guard let index = rows.firstIndex(where: { $0.id == id }), rows[index].isMessage, rows[index].role == "assistant" else { return false }
        if offset != .null {
            guard case .number(let rawOffset) = offset, rawOffset.isFinite, rawOffset >= 0,
                  rawOffset < Double(Int.max), rawOffset.rounded(.towardZero) == rawOffset else { return false }
            let count = rows[index].text.utf8.count, start = offset.integer
            if start < count {
                let bytes = Array(rows[index].text.utf8), added = Array(delta.utf8)
                if start <= count - added.count && Array(bytes[start..<(start + added.count)]) == added {
                    cursor = sequence // Exact replay of a prefix already displayed.
                    return true
                }
            }
            guard start == count else {
                recoveryCursor = min(recoveryCursor ?? cursor, max(0, (streamStarts[id] ?? 1) - 1))
                return false
            }
        }
        streamStarts[id] = streamStarts[id] ?? sequence
        rows[index].append(delta)
        rows[index].value = rows[index].value.replacing("status", with: .string("streaming"))
        cursor = sequence
        return true
    }
}
public enum DesktopError: LocalizedError {
    case invalidEndpoint, protocolMismatch, http(Int), missingDaemon, startup, keychain(Int32), tooLarge
    public var errorDescription: String? {
        switch self {
        case .invalidEndpoint: return "Use an HTTPS endpoint, or HTTP on localhost. Credentials and query parameters do not belong in the URL."
        case .protocolMismatch: return "The local service returned an incompatible response. Reconnect to reload this conversation."
        case .http(let status): return "Request failed (HTTP \(status)). Check your connection and model access, then retry."
        case .missingDaemon: return "The app is missing its bundled engine. Rebuild or reinstall S-Code.app."
        case .startup: return "The local engine did not become ready. Try reconnecting."
        case .keychain(let status): return "Keychain could not save or read this connection (\(status)). Unlock your login keychain and retry."
        case .tooLarge: return "The response exceeded the desktop client's safety limit. Try a smaller request."
        }
    }
}
