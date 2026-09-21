import Foundation
import Combine

public struct ProtectionRule: Decodable, Equatable, Identifiable, Sendable {
    public let id: String
    public let path: String
    public let canonicalPath: String?
    public let kind: String
    public let createdAt: String
}
public struct ProtectionPolicy: Decodable, Equatable, Sendable {
    public let revision: Int
    public let changedAt: String?
    public let rules: [ProtectionRule]
    public init(_ json: JSON) throws {
        let decoder = JSONDecoder(); decoder.keyDecodingStrategy = .convertFromSnakeCase
        self = try decoder.decode(Self.self, from: JSONEncoder().encode(json))
        guard revision >= 0, Set(rules.map(\.id)).count == rules.count else { throw DesktopError.protocolMismatch }
    }
    /// Display hint only; the daemon also checks canonical identity and aliases.
    public func protects(path: String, root: String?) -> Bool {
        let absolute: String
        if path.hasPrefix("/") { absolute = path }
        else if let root { absolute = (root.hasSuffix("/") ? root : root + "/") + path }
        else { return false }
        return rules.contains { rule in
            [rule.path, rule.canonicalPath].compactMap { $0 }.contains { candidate in
                !candidate.isEmpty && (absolute == candidate || absolute.hasPrefix(candidate.hasSuffix("/") ? candidate : candidate + "/"))
            }
        }
    }
}
/// Exact daemon-local command grammar; ordinary prose stays ordinary prose.
public enum ProtectionCommand {
    public static func recognizes(_ content: String) -> Bool {
        let text = content.trimmingCharacters(in: .whitespacesAndNewlines)
        for prefix in ["/protect", "protect file", "保护文件", "保护目录"] where text.hasPrefix(prefix) {
            let suffix = text.dropFirst(prefix.count)
            if suffix.isEmpty || suffix.first?.isWhitespace == true || prefix.hasPrefix("保") { return true }
        }
        return false
    }
}
public enum ProtectionChange: Sendable { case add(String), remove(String) }
extension APIClient {
    public func protections(sessionID: String) async throws -> ProtectionPolicy {
        try await ProtectionPolicy(request("/v1/sessions/\(sessionID)/privacy/protections"))
    }
    public func changeProtection(sessionID: String, change: ProtectionChange, revision: Int) async throws -> ProtectionPolicy {
        let base = "/v1/sessions/\(sessionID)/privacy/protections"
        switch change {
        case .add(let path):
            return try await ProtectionPolicy(request(base, method: "POST", body: .object(["scope": scope.json, "path": .string(path), "expected_revision": .number(Double(revision))])))
        case .remove(let id):
            return try await ProtectionPolicy(request(base + "/" + id, method: "DELETE", query: [.init(name: "expected_revision", value: String(revision))]))
        }
    }
}

@MainActor public final class PrivacyProtections: ObservableObject {
    public typealias Fetch = @Sendable () async throws -> ProtectionPolicy
    public typealias Mutate = @Sendable (ProtectionChange, Int) async throws -> ProtectionPolicy
    @Published public private(set) var policy: ProtectionPolicy?
    @Published public private(set) var loading = false
    @Published public private(set) var saving = false
    @Published public private(set) var error: String?
    private var generation = UUID()
    private var task: Task<Bool, Never>?
    private var dirty = false
    private var fetch: Fetch?
    private var mutate: Mutate?
    public init() {}
    public var verified: Bool { policy != nil && !loading && !saving && error == nil }
    public func reset(fetch: Fetch? = nil, mutate: Mutate? = nil) {
        generation = UUID(); task?.cancel(); task = nil
        self.fetch = fetch; self.mutate = mutate
        policy = nil; loading = false; saving = false; dirty = false; error = nil
    }
    public func refresh() async {
        guard fetch != nil else { return }
        if loading || saving { dirty = true; return }
        _ = await load(nil)
    }
    public func change(_ change: ProtectionChange) async -> Bool {
        guard policy != nil, mutate != nil, !loading, !saving else { return false }
        return await load(change)
    }
    private func load(_ change: ProtectionChange?) async -> Bool {
        guard let fetch else { return false }
        let epoch = generation, revision = policy?.revision ?? 0, mutate = mutate
        dirty = false; error = nil; loading = change == nil; saving = change != nil
        let operation = Task { [weak self] () -> Bool in
            var success = false
            do {
                let result: ProtectionPolicy
                if let change, let mutate { result = try await mutate(change, revision) }
                else { result = try await fetch() }
                guard let self, epoch == self.generation, !Task.isCancelled else { return false }
                self.policy = result; success = true
            } catch {
                guard let self, epoch == self.generation, !Task.isCancelled else { return false }
                self.error = change == nil ? "Could not load protections. \(error.localizedDescription)" : "Protection change failed. Review the current rules and retry. \(error.localizedDescription)"
                if change != nil {
                    if let result = try? await fetch(), epoch == self.generation, !Task.isCancelled { self.policy = result }
                }
            }
            guard let self, epoch == self.generation else { return false }
            self.loading = false; self.saving = false; self.task = nil
            if self.dirty { Task { await self.refresh() } }
            return success
        }
        task = operation
        return await operation.value
    }
}
