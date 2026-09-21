import Foundation

public enum PrivacySourceGroup: String, CaseIterable, Sendable {
    case external, attachment, other
    public func title(hasProject: Bool) -> String {
        switch self {
        case .external: return hasProject ? "Outside project" : "Recorded file paths"
        case .attachment: return "Attachments"
        case .other: return "Other sources & context"
        }
    }
}
public enum PrivacySourceLocation: Equatable, Sendable {
    case project(String), external, attachment, other
}
public struct PrivacyRecordedSource: Identifiable, Sendable {
    public let id: String
    public let source: String
    public let kind: String
    public let group: PrivacySourceGroup
    public var requests: Set<String> = []
    public var destinations: Set<String> = []
    public var outcomes: Set<String> = []
    public var state: PrivacyFileState = .none
    public var hasContent = false
    public var untraceable = false
    public var displayPath: String {
        if group == .external, let url = URL(string: source), url.isFileURL { return url.path }
        return source
    }
    public var statusDetail: String {
        if untraceable { return "Included context · individual files not traceable" }
        if !hasContent { return "Name / metadata only · no file body recorded" }
        if state == .entire { return "Complete captured content · accepted" }
        return "Partial content or delivery unconfirmed"
    }
}

/// Classifies ledger labels only. Never resolves symlinks or opens outside paths.
public enum PrivacyRecordedSources {
    private static let relativeFileKinds: Set<String> = ["file", "file text", "file excerpt", "project_instructions", "selection", "diagnostic"]
    public static func location(_ source: PrivacySource, root: String?) -> PrivacySourceLocation {
        if source.kind.hasPrefix("attachment") || source.source.hasPrefix("attachment://") { return .attachment }
        let original = source.source
        guard !original.isEmpty, !original.contains("\0"), !original.hasSuffix("… [label truncated]") else { return .other }
        var path = original
        if original.hasPrefix("file:") {
            // URL parsers can normalize dot segments. Reject them before parsing.
            guard let decoded = original.removingPercentEncoding,
                  !decoded.split(separator: "/").contains(".."),
                  let url = URL(string: original), url.isFileURL,
                  url.host == nil || url.host == "" || url.host == "localhost",
                  url.query == nil, url.fragment == nil else { return .other }
            path = url.path
        } else if original.range(of: "^[a-zA-Z][a-zA-Z0-9+.-]*:", options: .regularExpression) != nil { return .other }
        let parts = path.split(separator: "/").filter { $0 != "." }
        guard !parts.isEmpty, !parts.contains(".."), !path.contains("\0"), !path.contains("\\") else { return .other }
        let normalized = (path.hasPrefix("/") ? "/" : "") + parts.joined(separator: "/")
        if normalized.hasPrefix("/") {
            guard let root else { return .external }
            let prefix = root.hasSuffix("/") ? root : root + "/"
            if normalized.hasPrefix(prefix) { return .project(String(normalized.dropFirst(prefix.count))) }
            return .external
        }
        guard root != nil, relativeFileKinds.contains(source.kind) else { return .other }
        return .project(normalized)
    }
    public static func collect(_ requests: [PrivacyRequest], root: String?) -> [PrivacyRecordedSource] {
        var result: [String: PrivacyRecordedSource] = [:]
        func key(_ source: String, _ kind: String, _ group: PrivacySourceGroup) -> String {
            "\(group.rawValue):\(kind.utf8.count):\(kind)\(source)"
        }
        for request in requests {
            for source in request.sources {
                let group: PrivacySourceGroup
                switch location(source, root: root) {
                case .project: if source.contentBytes > 0 { continue }; group = .other
                case .external: group = .external
                case .attachment: group = .attachment
                case .other: group = .other
                }
                let id = key(source.source, source.kind, group)
                var item = result[id] ?? .init(id: id, source: source.source, kind: source.kind, group: group)
                item.requests.insert(request.id); item.destinations.insert(request.destination); item.outcomes.insert(request.statusLabel)
                if source.contentBytes > 0 {
                    item.hasContent = true
                    let state: PrivacyFileState = request.status == "accepted" && !source.partial ? .entire : .partial
                    if state.rawValue > item.state.rawValue { item.state = state }
                }
                result[id] = item
            }
            for context in request.unattributed {
                let id = "context:" + key(context, "unattributed", .other)
                var item = result[id] ?? .init(id: id, source: context, kind: "unattributed", group: .other)
                item.untraceable = true; item.requests.insert(request.id); item.destinations.insert(request.destination); item.outcomes.insert(request.statusLabel)
                result[id] = item
            }
        }
        return result.values.sorted { ($0.source, $0.kind) < ($1.source, $1.kind) }
    }
    public static func page(_ items: [PrivacyRecordedSource], index: Int, query: String = "", size: Int = 40) -> (items: [PrivacyRecordedSource], index: Int, count: Int, total: Int) {
        let needle = query.trimmingCharacters(in: .whitespacesAndNewlines)
        let matching = items.filter { needle.isEmpty || [$0.source, $0.displayPath, $0.kind].contains(where: { $0.localizedCaseInsensitiveContains(needle) }) || $0.destinations.contains(where: { $0.localizedCaseInsensitiveContains(needle) }) }
        let size = max(1, min(size, 100)), pages = max(1, (matching.count + size - 1) / size)
        let page = max(0, min(index, pages - 1))
        return (Array(matching.dropFirst(page * size).prefix(size)), page, pages, matching.count)
    }
}
