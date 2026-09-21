import Foundation
import Combine
import Darwin

public enum PrivacyFileState: Int, Sendable { case none, partial, entire }
public struct PrivacyFileRow: Identifiable, Sendable {
    public let id: String
    public let name: String
    public let path: String
    public let depth: Int
    public let isDirectory: Bool
    public let isExpanded: Bool
    public let state: PrivacyFileState
    public let requestCount: Int
    public let statusDetail: String
}
public struct PrivacyDirectoryEntry: Sendable {
    public let path: String
    public let isDirectory: Bool
    public let isLink: Bool
}
public struct PrivacyDirectoryPage: Sendable {
    public let entries: [PrivacyDirectoryEntry]
    public let truncated: Bool
}

/// Name-only, bounded local browsing. Descriptor-relative opens never follow
/// symlinks, including when a directory is replaced while a scan is running.
public enum PrivacyDirectoryReader {
    public static func read(root: String, path: String, limit: Int = 3000) throws -> PrivacyDirectoryPage {
        let parts = path.split(separator: "/", omittingEmptySubsequences: false)
        guard !path.hasPrefix("/"), path.isEmpty || parts.allSatisfy({ !$0.isEmpty && $0 != "." && $0 != ".." }), limit > 0 else { throw DesktopError.invalidEndpoint }
        var fd = open(root, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)
        guard fd >= 0 else { throw CocoaError(.fileReadNoPermission) }
        for part in path.isEmpty ? [] : parts {
            let child = openat(fd, String(part), O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)
            close(fd); fd = child
            guard fd >= 0 else { throw CocoaError(.fileReadNoPermission) }
        }
        guard let directory = fdopendir(fd) else { close(fd); throw CocoaError(.fileReadNoPermission) }
        defer { closedir(directory) }
        var entries: [PrivacyDirectoryEntry] = []
        var truncated = false
        while true {
            try Task.checkCancellation()
            errno = 0
            guard let item = readdir(directory) else {
                if errno != 0 { throw CocoaError(.fileReadUnknown) }
                break
            }
            let name = withUnsafePointer(to: &item.pointee.d_name) { pointer in
                pointer.withMemoryRebound(to: CChar.self, capacity: Int(MAXNAMLEN) + 1) { String(cString: $0) }
            }
            if name == "." || name == ".." { continue }
            if entries.count >= limit { truncated = true; break }
            var metadata = stat()
            guard fstatat(dirfd(directory), name, &metadata, AT_SYMLINK_NOFOLLOW) == 0 else { continue }
            let kind = metadata.st_mode & S_IFMT
            entries.append(.init(path: path.isEmpty ? name : path + "/" + name, isDirectory: kind == S_IFDIR, isLink: kind == S_IFLNK))
        }
        entries.sort { a, b in a.isDirectory != b.isDirectory ? a.isDirectory : a.path < b.path }
        return .init(entries: entries, truncated: truncated)
    }
}

public struct PrivacyFileEvidence: Sendable {
    public var state: PrivacyFileState = .none
    public var requests: Set<String> = []
    public var unknownDelivery = false
}
public enum PrivacyFileAttribution {
    /// Attachment display names cannot establish a workspace file's identity.
    public static func relativePath(_ source: PrivacySource, root: String) -> String? {
        guard source.contentBytes > 0,
              case let .project(path) = PrivacyRecordedSources.location(source, root: root) else { return nil }
        return path
    }

    public static func evidence(_ requests: [PrivacyRequest], root: String) -> [String: PrivacyFileEvidence] {
        var result: [String: PrivacyFileEvidence] = [:]
        for request in requests {
            for source in request.sources {
                guard let path = relativePath(source, root: root) else { continue }
                var value = result[path, default: .init()]
                value.requests.insert(request.id)
                if request.status == "accepted" {
                    let state: PrivacyFileState = source.partial ? .partial : .entire
                    if state.rawValue > value.state.rawValue { value.state = state }
                } else {
                    value.unknownDelivery = true
                    if value.state == .none { value.state = .partial }
                }
                result[path] = value
            }
        }
        return result
    }
}

@MainActor public final class PrivacyFileTree: ObservableObject {
    @Published public private(set) var recordedSources: [PrivacyRecordedSource] = []
    @Published public private(set) var sourceContextID = UUID()
    @Published public private(set) var rows: [PrivacyFileRow] = []
    @Published public private(set) var loading = false
    @Published public private(set) var error: String?
    @Published public var query = "" { didSet { visibleLimit = 200; rebuild() } }
    @Published public private(set) var hasMore = false
    @Published public private(set) var rootName: String?
    @Published public private(set) var notice: String?
    @Published public private(set) var coverageIncomplete = true
    private var root: String?
    private var generation = UUID()
    private var directories: [String: PrivacyDirectoryPage] = [:]
    private var expanded: Set<String> = [""]
    private var tasks: [String: Task<Void, Never>] = [:]
    private var failedPath = ""
    private var evidence: [String: PrivacyFileEvidence] = [:]
    private var visibleLimit = 200
    private var historicalDirectories: Set<String> = []
    public init() {}
    public func reset(root: String? = nil) {
        sourceContextID = UUID(); recordedSources = []
        generation = UUID(); tasks.values.forEach { $0.cancel() }; tasks = [:]
        self.root = root; rootName = root.map { URL(fileURLWithPath: $0).lastPathComponent }
        directories = [:]; expanded = [""]; evidence = [:]; error = nil; notice = nil
        query = ""; rows = []; hasMore = false; loading = false; coverageIncomplete = true
        if root != nil { scan("") }
    }
    public func update(requests: [PrivacyRequest], incomplete: Bool) {
        recordedSources = PrivacyRecordedSources.collect(requests, root: root)
        coverageIncomplete = incomplete
        evidence = root.map { PrivacyFileAttribution.evidence(requests, root: $0) } ?? [:]
        rebuild()
    }
    public func toggle(_ id: String) {
        guard rows.contains(where: { $0.id == id && $0.isDirectory }) else { return }
        if expanded.contains(id) { expanded.remove(id) }
        else { expanded.insert(id); if directories[id] == nil && !historicalDirectories.contains(id) { scan(id) } }
        rebuild()
    }
    public func loadMore() { visibleLimit += 200; rebuild() }
    public func retry() { scan(failedPath) }
    public func refresh() {
        generation = UUID(); tasks.values.forEach { $0.cancel() }; tasks = [:]
        directories = [:]; error = nil; notice = nil; loading = false
        // Refresh visible directories only; never recursively crawl a repository.
        for path in expanded.sorted() where !historicalDirectories.contains(path) { scan(path) }
        rebuild()
    }
    private func scan(_ path: String) {
        guard let root, tasks[path] == nil else { return }
        let epoch = generation
        loading = true
        let worker = Task.detached(priority: .utility) { try PrivacyDirectoryReader.read(root: root, path: path) }
        tasks[path] = Task { [weak self] in
            do {
                let page = try await withTaskCancellationHandler { try await worker.value } onCancel: { worker.cancel() }
                guard let self, epoch == self.generation, !Task.isCancelled else { return }
                self.directories[path] = page; self.error = nil
            } catch {
                guard let self, epoch == self.generation, !Task.isCancelled else { return }
                self.failedPath = path; self.error = "Could not list \(path.isEmpty ? "workspace" : path). \(error.localizedDescription)"
            }
            guard let self, epoch == self.generation else { return }
            self.tasks[path] = nil; self.loading = !self.tasks.isEmpty; self.rebuild()
        }
    }
    private func rebuild() {
        var entries: [String: PrivacyDirectoryEntry] = [:]
        for page in directories.values { for entry in page.entries { entries[entry.path] = entry } }
        // Include recorded files even if deleted, hidden in an unscanned folder,
        // or beyond a directory's cap. Do not touch these paths on disk.
        historicalDirectories = []
        for path in evidence.keys {
            let parts = path.split(separator: "/")
            for i in parts.indices {
                let p = parts[...i].joined(separator: "/")
                if i < parts.count - 1, let existing = entries[p], !existing.isDirectory {
                    historicalDirectories.insert(p)
                    entries[p] = .init(path: p, isDirectory: true, isLink: existing.isLink)
                } else if entries[p] == nil {
                    entries[p] = .init(path: p, isDirectory: i < parts.count - 1, isLink: false)
                }
            }
        }
        var children: [String: [PrivacyDirectoryEntry]] = [:]
        for entry in entries.values {
            let parent = entry.path.split(separator: "/").dropLast().joined(separator: "/")
            children[parent, default: []].append(entry)
        }
        for key in Array(children.keys) {
            children[key]?.sort { a, b in a.isDirectory != b.isDirectory ? a.isDirectory : a.path < b.path }
        }
        let needle = query.trimmingCharacters(in: .whitespacesAndNewlines)
        var matching: Set<String> = []
        if !needle.isEmpty {
            for path in entries.keys where path.localizedCaseInsensitiveContains(needle) {
                let parts = path.split(separator: "/")
                for i in parts.indices { matching.insert(parts[...i].joined(separator: "/")) }
            }
        }
        var result: [PrivacyFileRow] = []
        var stack = children["", default: []].reversed().map { ($0, 0) }
        while let (entry, depth) = stack.popLast(), result.count <= visibleLimit {
            if !needle.isEmpty && !matching.contains(entry.path) { continue }
            let item = evidence[entry.path, default: .init()]
            let detail: String
            if historicalDirectories.contains(entry.path) { detail = "Recorded folder · no longer a local directory" }
            else if entry.isDirectory { detail = "Folder" }
            else if entry.isLink { detail = "Symbolic link · not followed" }
            else if item.state == .entire { detail = "Full text recorded · captured version" }
            else if item.state == .partial { detail = item.unknownDelivery ? "Partial or delivery unconfirmed" : "Partial text recorded" }
            else { detail = "No recorded transmission" }
            let open = expanded.contains(entry.path) || !needle.isEmpty
            result.append(.init(id: entry.path, name: String(entry.path.split(separator: "/").last ?? ""), path: entry.path, depth: depth, isDirectory: entry.isDirectory, isExpanded: open, state: item.state, requestCount: item.requests.count, statusDetail: detail))
            if entry.isDirectory && open { stack.append(contentsOf: children[entry.path, default: []].reversed().map { ($0, depth + 1) }) }
        }
        hasMore = result.count > visibleLimit; rows = Array(result.prefix(visibleLimit))
        notice = directories.values.contains(where: \.truncated) ? "A folder has more than 3,000 entries. Its listing is shortened; recorded sources are still included." : nil
    }
}
