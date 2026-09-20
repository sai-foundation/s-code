import Foundation
import Security
import CryptoKit

public struct Profile: Codable, Identifiable, Equatable, Sendable {
    public var id: String
    public var name: String
    public var provider: String
    public var endpoint: String
    public var model: String
    public init(id: String = UUID().uuidString.lowercased(), name: String = "SAI", provider: String = "openai_compatible", endpoint: String = "https://api.sai.foundation/v1", model: String = "") {
        self.id = id; self.name = name; self.provider = provider; self.endpoint = endpoint; self.model = model
    }
    /// The keychain account is bound to the provider identity even if a metadata
    /// write fails after saving a new credential. A key cannot cross endpoints.
    public var credentialID: String {
        let data = try! JSONEncoder().encode([id, provider, endpoint])
        return SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
    }
    public func canReuseCredential(from saved: Profile) -> Bool {
        id == saved.id && endpoint == saved.endpoint && provider == saved.provider
    }
    public func validate() throws {
        guard UUID(uuidString: id) != nil, !name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
              !model.isEmpty, model.utf8.count <= 256, !model.contains(where: { $0.isWhitespace || $0.isNewline }),
              ["openai_compatible", "anthropic", "gemini"].contains(provider) else { throw DesktopError.protocolMismatch }
        _ = try EndpointPolicy.validate(endpoint)
    }
}
public enum CredentialStore {
    private static let service = "foundation.sai.s-code.desktop.providers"
    private static func query(_ id: String) -> [String: Any] { [kSecClass as String: kSecClassGenericPassword, kSecAttrService as String: service, kSecAttrAccount as String: id] }
    public static func read(_ id: String) throws -> String? {
        var q = query(id); q[kSecReturnData as String] = true; q[kSecMatchLimit as String] = kSecMatchLimitOne
        var result: CFTypeRef?
        let status = SecItemCopyMatching(q as CFDictionary, &result)
        if status == errSecItemNotFound { return nil }
        guard status == errSecSuccess, let data = result as? Data else { throw DesktopError.keychain(status) }
        return String(data: data, encoding: .utf8)
    }
    public static func save(_ key: String, id: String) throws {
        let attributes: [String: Any] = [kSecValueData as String: Data(key.utf8)]
        let status = SecItemUpdate(query(id) as CFDictionary, attributes as CFDictionary)
        if status == errSecItemNotFound {
            var q = query(id); q.merge(attributes) { _, new in new }; q[kSecAttrAccessible as String] = kSecAttrAccessibleWhenUnlockedThisDeviceOnly
            let added = SecItemAdd(q as CFDictionary, nil)
            guard added == errSecSuccess else { throw DesktopError.keychain(added) }
        } else if status != errSecSuccess { throw DesktopError.keychain(status) }
    }
}
public struct ProfileRepository {
    public let root: URL
    public init(root: URL? = nil) {
        self.root = root ?? FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0].appendingPathComponent("SAI/S-Code")
    }
    public func load() throws -> [Profile] {
        let path = root.appendingPathComponent("profiles.json")
        guard FileManager.default.fileExists(atPath: path.path) else { return [] }
        let data = try Data(contentsOf: path)
        guard data.count < 1024 * 1024 else { throw DesktopError.tooLarge }
        let profiles = try JSONDecoder().decode([Profile].self, from: data)
        try profiles.forEach { try $0.validate() }; return profiles
    }
    public func save(_ profiles: [Profile]) throws {
        try profiles.forEach { try $0.validate() }
        try Self.privateDirectory(root)
        try JSONEncoder().encode(profiles).write(to: root.appendingPathComponent("profiles.json"), options: [.atomic, .completeFileProtection])
        try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: root.appendingPathComponent("profiles.json").path)
    }
    public func directory(_ profile: Profile) -> URL { root.appendingPathComponent("Profiles").appendingPathComponent(profile.id) }
    public static func privateDirectory(_ path: URL) throws {
        if FileManager.default.fileExists(atPath: path.path) {
            let values = try path.resourceValues(forKeys: [.isSymbolicLinkKey, .isDirectoryKey])
            guard values.isDirectory == true, values.isSymbolicLink != true else { throw DesktopError.startup }
        } else { try FileManager.default.createDirectory(at: path, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700]) }
        try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: path.path)
    }
}
