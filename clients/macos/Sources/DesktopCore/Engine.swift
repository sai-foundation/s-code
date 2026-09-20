import Foundation
import Security

@MainActor public final class Engine {
    private var process: Process?
    private var lifetime: Pipe?
    private var log: FileHandle?
    public var onExit: (() -> Void)?
    public private(set) var startupMilliseconds: Double = 0
    public init() {}
    public var isRunning: Bool { process?.isRunning == true }
    public func start(profile: Profile, key: String, repository: ProfileRepository, executable: URL) async throws -> APIClient {
        await stop()
        try profile.validate()
        guard FileManager.default.isExecutableFile(atPath: executable.path) else { throw DesktopError.missingDaemon }
        let started = Date()
        let profileRoot = repository.directory(profile)
        let home = profileRoot.appendingPathComponent("Engine")
        let runtime = home.appendingPathComponent("run")
        let state = home.appendingPathComponent("state")
        // Managed projects are siblings of private engine data, never ancestors.
        let workspaces = profileRoot.appendingPathComponent("Workspaces")
        for path in [repository.root, profileRoot, home, runtime, state, workspaces] { try ProfileRepository.privateDirectory(path) }
        let token = UUID().uuidString + UUID().uuidString
        let p = Process(), pipe = Pipe()
        p.executableURL = executable; p.currentDirectoryURL = workspaces
        let userHome = FileManager.default.homeDirectoryForCurrentUser.path
        p.environment = [
            "HOME": userHome, "USER": NSUserName(), "LANG": "en_US.UTF-8",
            "PATH": "\(userHome)/.local/bin:\(userHome)/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
            "TMPDIR": NSTemporaryDirectory(),
            "S_CODE_HOME": home.path, "S_CODE_STATE_DIR": state.path, "S_CODE_RUNTIME_DIR": runtime.path,
            "S_CODE_WORKSPACES_DIR": workspaces.path,
            "S_CODE_DATABASE_URL": "sqlite://" + state.appendingPathComponent("desktop.sqlite").path,
            "S_CODE_DAEMON_LISTEN": "127.0.0.1:0", "S_CODE_TOKEN": token,
            "S_CODE_MODEL_PROVIDER": profile.provider, "S_CODE_MODEL_BASE_URL": profile.endpoint,
            "S_CODE_MODEL_CREDENTIAL_HANDLE": "S_CODE_DESKTOP_PROVIDER_KEY", "S_CODE_DESKTOP_PROVIDER_KEY": key,
            "S_CODE_MODEL": profile.model, "S_CODE_DESKTOP_LIFETIME": "stdin"
        ]
        if key.isEmpty { p.environment?.removeValue(forKey: "S_CODE_MODEL_CREDENTIAL_HANDLE") }
        let logURL = state.appendingPathComponent("engine.log")
        // Keep diagnostic logs private and bounded across launches; never append indefinitely.
        if FileManager.default.fileExists(atPath: logURL.path) {
            let v = try logURL.resourceValues(forKeys: [.isSymbolicLinkKey]); guard v.isSymbolicLink != true else { throw DesktopError.startup }
        }
        FileManager.default.createFile(atPath: logURL.path, contents: nil, attributes: [.posixPermissions: 0o600])
        let handle = try FileHandle(forWritingTo: logURL)
        p.standardOutput = handle; p.standardError = handle; p.standardInput = pipe
        p.terminationHandler = { [weak self] child in
            Task { @MainActor in
                guard let self, self.process === child else { return }
                self.process = nil; self.lifetime = nil; try? self.log?.close(); self.log = nil
                self.onExit?()
            }
        }
        process = p; lifetime = pipe; log = handle
        do {
            try p.run()
            // Close the parent's read descriptor; only the child reads. App death closes the writer.
            try? pipe.fileHandleForReading.close()
            for _ in 0..<200 {
                try Task.checkCancellation()
                guard p.isRunning else { throw DesktopError.startup }
                let connection = runtime.appendingPathComponent("daemon.json")
                if let data = try? Data(contentsOf: connection), data.count < 16384,
                   let value = try? JSONDecoder().decode(JSON.self, from: data),
                   value["pid"].integer == Int(p.processIdentifier), value["token"].string == token {
                    let base = try EndpointPolicy.validate(value["daemon_url"].string, loopbackOnly: true)
                    let client = try APIClient(base: base, token: token, scope: Scope(profileID: profile.id))
                    _ = try await client.request("/v1/health", scoped: false)
                    let capabilities = try await client.request("/v1/capabilities", scoped: false)
                    guard capabilities["protocol_version"].string == "1.0" else { throw DesktopError.protocolMismatch }
                    startupMilliseconds = Date().timeIntervalSince(started) * 1000
                    return client
                }
                try await Task.sleep(for: .milliseconds(50))
            }
            throw DesktopError.startup
        } catch { await stop(); throw error }
    }
    public func stop() async {
        guard let p = process else { return }
        process = nil // Intentional exits do not show a crash banner.
        try? lifetime?.fileHandleForWriting.close(); lifetime = nil
        if p.isRunning { p.terminate() }
        for _ in 0..<160 {
            if !p.isRunning { break }
            try? await Task.sleep(for: .milliseconds(50))
        }
        if p.isRunning { kill(p.processIdentifier, SIGKILL) }
        try? log?.close(); log = nil
    }
}
