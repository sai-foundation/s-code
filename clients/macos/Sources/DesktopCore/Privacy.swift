import Foundation
import Combine

/// The engine's metadata-only disclosure ledger. No request bodies, file
/// contents, headers or credentials are retained by the desktop client.
public struct PrivacySource: Decodable, Equatable, Sendable {
    public let source: String
    public let kind: String
    public let contentBytes: UInt64
    public let partial: Bool
}
public struct PrivacyRequest: Decodable, Equatable, Identifiable, Sendable {
    public let id: String
    public let sequence: Int
    public let turnId: String
    public let startedAt: String
    public let destination: String
    public let model: String
    public let purpose: String
    public let status: String
    public let requestBytes: UInt64
    public let sources: [PrivacySource]
    public let unattributed: [String]
    public var title: String { purpose == "session_title" ? "Conversation title" : "Agent request" }
    public var statusLabel: String {
        switch status {
        case "accepted": return "Accepted by endpoint"
        case "rejected": return "Rejected by endpoint · data may have been received"
        case "connection_error": return "Connection error · delivery unknown"
        default: return "Request started · delivery not confirmed"
        }
    }
}
public struct PrivacyPage: Decodable, Sendable {
    public let requests: [PrivacyRequest]
    public let nextBefore: Int?
    public init(_ json: JSON) throws {
        let decoder = JSONDecoder(); decoder.keyDecodingStrategy = .convertFromSnakeCase
        self = try decoder.decode(Self.self, from: JSONEncoder().encode(json))
        guard Set(requests.map(\.id)).count == requests.count,
              zip(requests, requests.dropFirst()).allSatisfy({ $0.sequence > $1.sequence }),
              requests.allSatisfy({ !$0.id.isEmpty && $0.sequence > 0 }),
              nextBefore.map({ $0 > 0 }) ?? true else { throw DesktopError.protocolMismatch }
    }
}
extension APIClient {
    public func privacy(sessionID: String, before: Int? = nil) async throws -> PrivacyPage {
        try await PrivacyPage(request("/v1/sessions/\(sessionID)/privacy", query: before.map { [.init(name: "before", value: String($0))] } ?? []))
    }
}

/// Serialized pagination with generation guards, including when a transport
/// ignores cancellation. A refresh re-reads the loaded range atomically so new
/// requests cannot leave a gap or drop an older page the user already opened.
@MainActor public final class PrivacyHistory: ObservableObject {
    public typealias Fetch = @Sendable (Int?) async throws -> PrivacyPage
    @Published public private(set) var requests: [PrivacyRequest] = []
    @Published public private(set) var nextBefore: Int?
    @Published public private(set) var loading = false
    @Published public private(set) var error: String?
    @Published public private(set) var hasLoaded = false
    public private(set) var sessionID: String?
    private var fetch: Fetch?
    private var generation = UUID()
    private var requestTask: Task<Void, Never>?
    private var refreshTask: Task<Void, Never>?
    private var dirty = false
    private var eventSequence = 0
    private var failedOlder = false
    private let refreshDelay: @Sendable () async throws -> Void
    public init(refreshDelay: @escaping @Sendable () async throws -> Void = { try await Task.sleep(for: .seconds(1)) }) {
        self.refreshDelay = refreshDelay
    }

    public func reset(sessionID: String? = nil, fetch: Fetch? = nil) {
        generation = UUID(); requestTask?.cancel(); refreshTask?.cancel()
        requestTask = nil; refreshTask = nil; dirty = false; eventSequence = 0
        self.sessionID = sessionID; self.fetch = fetch
        failedOlder = false
        requests = []; nextBefore = nil; error = nil; loading = false; hasLoaded = false
    }
    public func refresh() async { await load(older: false) }
    public func older() async { await load(older: true) }
    public func retry() async { await load(older: failedOlder) }
    private func load(older: Bool) async {
        guard let fetch, !loading, !older || nextBefore != nil else { return }
        let epoch = generation, cursor = older ? nextBefore : nil
        let oldest = older ? nil : requests.last?.sequence
        if !older { refreshTask?.cancel(); refreshTask = nil; dirty = false }
        loading = true; error = nil; failedOlder = older
        let task = Task { [weak self] in
            do {
                var before = cursor, values: [PrivacyRequest] = [], next: Int?
                repeat {
                    try Task.checkCancellation()
                    let page = try await fetch(before)
                    try Task.checkCancellation()
                    // Strict cursor progress prevents malformed endpoints looping.
                    if let before, page.requests.contains(where: { $0.sequence >= before }) { throw DesktopError.protocolMismatch }
                    if let next = page.nextBefore {
                        guard let last = page.requests.last, next == last.sequence,
                              before.map({ next < $0 }) ?? true else { throw DesktopError.protocolMismatch }
                    }
                    values.append(contentsOf: page.requests); next = page.nextBefore
                    if older || oldest == nil || next == nil || (page.requests.last?.sequence ?? 0) <= oldest! { break }
                    before = next
                } while true
                guard let self, epoch == self.generation else { return }
                var byID = Dictionary(self.requests.map { ($0.id, $0) }, uniquingKeysWith: { _, newer in newer })
                for value in values { byID[value.id] = value }
                self.requests = byID.values.sorted { $0.sequence > $1.sequence }
                // For refresh, the loop reaches at least the previously loaded
                // boundary. Its cursor is therefore also safe for the next page.
                self.nextBefore = next; self.hasLoaded = true
            } catch {
                guard let self, epoch == self.generation, !Task.isCancelled else { return }
                self.error = "Could not load privacy history. " + error.localizedDescription
            }
            guard let self, epoch == self.generation else { return }
            self.loading = false; self.requestTask = nil
            if self.dirty { self.scheduleRefresh() }
        }
        requestTask = task
        await task.value
    }
    public func receive(_ events: [JSON]) {
        guard fetch != nil else { return }
        for event in events where event["session_id"].string == sessionID &&
            ["privacy.request.started", "privacy.request.finished"].contains(event["type"].string) {
            let sequence = event["sequence"].integer
            if sequence > eventSequence { eventSequence = sequence; dirty = true }
        }
        if dirty { scheduleRefresh() }
    }
    /// Also called after reconnect, because the event stream can have a gap.
    public func invalidate() {
        guard fetch != nil else { return }
        dirty = true; scheduleRefresh()
    }
    private func scheduleRefresh() {
        guard refreshTask == nil, !loading else { return }
        let epoch = generation
        refreshTask = Task { [weak self] in
            try? await self?.refreshDelay()
            guard let self, epoch == self.generation, !Task.isCancelled else { return }
            self.refreshTask = nil
            guard !self.loading else { return }
            self.dirty = false
            await self.refresh()
        }
    }
}
