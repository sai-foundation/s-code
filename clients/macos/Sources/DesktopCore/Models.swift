import Foundation
import Combine

public struct ModelOption: Identifiable, Equatable, Sendable {
    public let id: String
    public let name: String
    public let provider: String
    public let source: String
    public let available: Bool
    public let lockedReason: String?
    public let recommended: Bool
    public init(_ value: JSON) throws {
        id = value["id"].string
        guard !id.isEmpty, id.utf8.count <= 256 else { throw DesktopError.protocolMismatch }
        name = value["display_name"].string.isEmpty ? id : value["display_name"].string
        provider = value["provider"].string
        source = value["source"].string
        available = value["available"].boolean
        lockedReason = value["locked_reason"] == .null ? nil : value["locked_reason"].string
        recommended = value["recommended"].boolean
    }
    public var selectable: Bool { available && lockedReason == nil }
    public func matches(_ search: String, configuredProvider: String) -> Bool {
        search.isEmpty || [id, name, provider == "configured" ? configuredProvider : provider].contains { $0.localizedCaseInsensitiveContains(search) }
    }
}

public struct SessionModelState: Sendable {
    public let current: String
    public let options: [ModelOption]
    public init(session: JSON, catalog: JSON, sessionID: String, scope: Scope) throws {
        guard scope.owns(session), session["id"].string == sessionID, !session["model"].string.isEmpty,
              case .array(let entries) = catalog else { throw DesktopError.protocolMismatch }
        current = session["model"].string
        options = try entries.map(ModelOption.init)
        guard Set(options.map(\.id)).count == options.count else { throw DesktopError.protocolMismatch }
    }
}

extension APIClient {
    public func models(sessionID: String) async throws -> SessionModelState {
        async let session = request("/v1/sessions/\(sessionID)")
        async let catalog = request("/v1/models")
        return try await SessionModelState(session: session, catalog: catalog, sessionID: sessionID, scope: scope)
    }
    public func changeModel(sessionID: String, model: String) async throws {
        let session = try await request("/v1/sessions/\(sessionID)", method: "PATCH", body: .object([
            "scope": scope.json, "model": .string(model)
        ]))
        guard scope.owns(session), session["id"].string == sessionID,
              session["model"].string == model else { throw DesktopError.protocolMismatch }
    }
}

/// Retain pending writes across selection changes; only confirmed session state enables Send.
@MainActor public final class SessionModels: ObservableObject {
    public typealias Fetch = @Sendable () async throws -> SessionModelState
    public typealias Mutate = @Sendable (String) async throws -> Void
    private struct Identity: Equatable, Hashable { let actor: String; let session: String }
    @Published public private(set) var state: SessionModelState?
    @Published public private(set) var loading = false
    @Published public private(set) var error: String?
    @Published private var pending: Set<Identity> = []
    private var identity: Identity?
    private var generation = UUID()
    private var requestID = UUID()
    private var fetch: Fetch?
    private var mutate: Mutate?
    public init() {}
    public var saving: Bool { identity.map { pending.contains($0) } ?? false }
    public var ready: Bool { state != nil && !loading && !saving }
    public func reset(actor: String? = nil, sessionID: String? = nil, fetch: Fetch? = nil, mutate: Mutate? = nil) {
        generation = UUID(); requestID = UUID(); state = nil; error = nil; loading = false
        identity = actor.flatMap { actor in sessionID.map { Identity(actor: actor, session: $0) } }
        self.fetch = fetch; self.mutate = mutate
    }
    public func refresh() async {
        guard let fetch, !saving else { return }
        let generation = generation, request = UUID()
        requestID = request; loading = true
        defer { if request == requestID { loading = false } }
        do {
            let result = try await fetch()
            guard generation == self.generation, request == requestID else { return }
            state = result; error = nil
        } catch {
            guard generation == self.generation, request == requestID else { return }
            state = nil; self.error = "Could not load this conversation's model. " + error.localizedDescription
        }
    }
    public func change(to model: String) async {
        guard let identity, let mutate, ready, let state, state.current != model,
              state.options.contains(where: { $0.id == model && $0.selectable }) else { return }
        requestID = UUID(); pending.insert(identity); error = nil
        var failure: String?
        do { try await mutate(model) }
        catch { failure = "Could not confirm the model change. The saved model has been rechecked. " + error.localizedDescription }
        pending.remove(identity)
        // An old request may finish after leaving and returning to its conversation.
        guard identity == self.identity else { return }
        let generation = generation
        await refresh()
        if generation == self.generation, self.state != nil, let failure { error = failure }
    }
}
