import Foundation

public enum EndpointPolicy {
    public static func validate(_ text: String, loopbackOnly: Bool = false) throws -> URL {
        guard let c = URLComponents(string: text), let url = c.url, let host = c.host?.lowercased(),
              !host.isEmpty, c.user == nil, c.password == nil, c.query == nil, c.fragment == nil else { throw DesktopError.invalidEndpoint }
        let local = ["localhost", "127.0.0.1", "::1", "[::1]"].contains(host)
        guard (!loopbackOnly || local), c.scheme == "https" || (c.scheme == "http" && local) else { throw DesktopError.invalidEndpoint }
        return url
    }
}
final class NoRedirect: NSObject, URLSessionTaskDelegate, @unchecked Sendable {
    func urlSession(_ session: URLSession, task: URLSessionTask, willPerformHTTPRedirection response: HTTPURLResponse, newRequest request: URLRequest, completionHandler: @escaping (URLRequest?) -> Void) { completionHandler(nil) }
}
public final class APIClient: @unchecked Sendable {
    public let base: URL
    public let scope: Scope
    private let token: String
    private let session: URLSession
    public init(base: URL, token: String, scope: Scope) throws {
        self.base = try EndpointPolicy.validate(base.absoluteString, loopbackOnly: true)
        self.scope = scope; self.token = token
        let config = URLSessionConfiguration.ephemeral
        config.timeoutIntervalForRequest = 30
        config.timeoutIntervalForResource = 86400
        config.httpCookieStorage = nil
        config.urlCache = nil
        session = URLSession(configuration: config, delegate: NoRedirect(), delegateQueue: nil)
    }
    deinit { session.invalidateAndCancel() }
    public func makeRequest(_ path: String, method: String = "GET", body: JSON? = nil, query: [URLQueryItem] = [], scoped: Bool = true) throws -> URLRequest {
        guard path.hasPrefix("/v1/"), !path.contains(".."), !path.contains("?"), !path.contains("#") else { throw DesktopError.invalidEndpoint }
        var components = URLComponents(url: base, resolvingAgainstBaseURL: false)!
        components.path = path
        components.queryItems = (scoped ? scope.query : []) + query
        var r = URLRequest(url: components.url!)
        r.httpMethod = method
        r.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        r.setValue("1", forHTTPHeaderField: "X-S-Code-CSRF")
        if let body { r.httpBody = try JSONEncoder().encode(body); r.setValue("application/json", forHTTPHeaderField: "Content-Type") }
        return r
    }
    public func request(_ path: String, method: String = "GET", body: JSON? = nil, query: [URLQueryItem] = [], scoped: Bool = true) async throws -> JSON {
        let request = try makeRequest(path, method: method, body: body, query: query, scoped: scoped)
        let (bytes, response) = try await session.bytes(for: request)
        guard let response = response as? HTTPURLResponse, (200..<300).contains(response.statusCode) else { throw DesktopError.http((response as? HTTPURLResponse)?.statusCode ?? 0) }
        let data = try await boundedData(bytes, limit: 16 * 1024 * 1024)
        if data.isEmpty { return .null }
        return try JSONDecoder().decode(JSON.self, from: data)
    }
    public func stream(after: Int, receive: @escaping @Sendable ([JSON]) async throws -> Void) async throws {
        var request = try makeRequest("/v1/events", query: [.init(name: "after", value: String(after))])
        request.timeoutInterval = 90
        let (bytes, response) = try await session.bytes(for: request)
        guard (response as? HTTPURLResponse)?.statusCode == 200 else { throw DesktopError.http((response as? HTTPURLResponse)?.statusCode ?? 0) }
        var parser = SSEParser()
        // Flush complete frames immediately. UI updates are coalesced independently on a timer.
        for try await byte in bytes {
            try Task.checkCancellation()
            if let event = try parser.append(byte) { try await receive([event]) }
        }
        throw URLError(.networkConnectionLost)
    }
}
public struct SSEParser {
    private var line = Data()
    private var data = Data()
    private var previousCR = false
    public init() {}
    public mutating func append(_ byte: UInt8) throws -> JSON? {
        if byte == 10 && previousCR { previousCR = false; return nil }
        previousCR = byte == 13
        if byte == 10 || byte == 13 {
            let current = line; line.removeAll(keepingCapacity: true)
            if current.isEmpty {
                guard !data.isEmpty else { return nil }
                defer { data.removeAll(keepingCapacity: true) }
                return try JSONDecoder().decode(JSON.self, from: data)
            }
            if current.starts(with: Data("data:".utf8)) {
                var piece = current.dropFirst(5)
                if piece.first == 32 { piece = piece.dropFirst() }
                if !data.isEmpty { data.append(10) }; data.append(contentsOf: piece)
            }
        } else { line.append(byte) }
        guard line.count + data.count <= 2 * 1024 * 1024 else { throw DesktopError.tooLarge }
        return nil
    }
}
public enum ProviderDiscovery {
    public static func models(endpoint: String, provider: String, key: String) async throws -> [String] {
        let base = try EndpointPolicy.validate(endpoint)
        var request = URLRequest(url: base.appendingPathComponent("models"))
        request.timeoutInterval = 20
        switch provider {
        case "anthropic": request.setValue(key, forHTTPHeaderField: "x-api-key"); request.setValue("2023-06-01", forHTTPHeaderField: "anthropic-version")
        case "gemini": request.setValue(key, forHTTPHeaderField: "x-goog-api-key")
        default: if !key.isEmpty { request.setValue("Bearer \(key)", forHTTPHeaderField: "Authorization") }
        }
        let config = URLSessionConfiguration.ephemeral; config.httpCookieStorage = nil; config.urlCache = nil
        let session = URLSession(configuration: config, delegate: NoRedirect(), delegateQueue: nil)
        defer { session.invalidateAndCancel() }
        let (bytes, response) = try await session.bytes(for: request)
        guard let response = response as? HTTPURLResponse, (200..<300).contains(response.statusCode) else { throw DesktopError.http((response as? HTTPURLResponse)?.statusCode ?? 0) }
        let data = try await boundedData(bytes, limit: 8 * 1024 * 1024)
        let result = try JSONDecoder().decode(JSON.self, from: data)
        let models = provider == "gemini" ? result["models"].array.map { $0["name"].string.replacingOccurrences(of: "models/", with: "") } : result["data"].array.map { $0["id"].string }
        return Array(Set(models.filter { !$0.isEmpty && $0.utf8.count <= 256 })).sorted()
    }
}

private func boundedData(_ bytes: URLSession.AsyncBytes, limit: Int) async throws -> Data {
    var data = Data()
    for try await byte in bytes {
        try Task.checkCancellation()
        guard data.count < limit else { throw DesktopError.tooLarge }
        data.append(byte)
    }
    return data
}
