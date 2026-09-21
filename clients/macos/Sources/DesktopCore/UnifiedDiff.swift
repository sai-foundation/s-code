import Foundation

public struct DiffLine: Identifiable, Sendable {
    public enum Kind: Sendable { case context, addition, removal, hunk, metadata }
    public let id: Int
    public let text: String
    public let kind: Kind
    public let oldLine: Int?
    public let newLine: Int?
}
public struct DiffFile: Identifiable, Sendable {
    public let id: Int
    public var title: String
    public var lines: [DiffLine] = []
    public var additions: Int { lines.filter { $0.kind == .addition }.count }
    public var removals: Int { lines.filter { $0.kind == .removal }.count }
}
/// Bounded display-only parsing of server-returned diff bytes. Never opens a path.
public struct UnifiedDiff: Sendable {
    public let files: [DiffFile]
    public let truncated: Bool
    public let raw: String
    public init(_ source: String, serverTruncated: Bool = false, maxBytes: Int = 524288, maxLines: Int = 12000, maxFiles: Int = 200) {
        let bytes = source.utf8.prefix(max(0, maxBytes))
        raw = String(decoding: bytes, as: UTF8.self)
        var truncated = serverTruncated || source.utf8.count > bytes.count
        var files: [DiffFile] = [], oldLine: Int?, newLine: Int?, inHunk = false
        let hunk = try! NSRegularExpression(pattern: "^@@ -(\\d+)(?:,\\d+)? \\+(\\d+)(?:,\\d+)? @@")
        let lines = raw.split(separator: "\n", omittingEmptySubsequences: false)
        for (index, substring) in lines.enumerated() {
            if index >= maxLines { truncated = true; break }
            let line = String(substring)
            if index == lines.count - 1 && line.isEmpty { continue }
            if line.hasPrefix("diff --git ") || files.isEmpty {
                if files.count >= maxFiles { truncated = true; break }
                let title = line.hasPrefix("diff --git ") ? String(line.dropFirst(11)) : "Working changes"
                files.append(DiffFile(id: files.count, title: title)); oldLine = nil; newLine = nil; inHunk = false
            }
            var kind: DiffLine.Kind = .metadata, old: Int?, new: Int?
            if let match = hunk.firstMatch(in: line, range: NSRange(line.startIndex..., in: line)),
               let oldRange = Range(match.range(at: 1), in: line), let newRange = Range(match.range(at: 2), in: line) {
                oldLine = Int(line[oldRange]); newLine = Int(line[newRange]); kind = .hunk; inHunk = true
            } else if inHunk, line.hasPrefix("+") {
                kind = .addition; new = newLine; newLine = newLine.flatMap { $0 < Int.max ? $0 + 1 : nil }
            } else if inHunk, line.hasPrefix("-") {
                kind = .removal; old = oldLine; oldLine = oldLine.flatMap { $0 < Int.max ? $0 + 1 : nil }
            } else if inHunk, line.hasPrefix(" ") {
                kind = .context; old = oldLine; new = newLine
                oldLine = oldLine.flatMap { $0 < Int.max ? $0 + 1 : nil }; newLine = newLine.flatMap { $0 < Int.max ? $0 + 1 : nil }
            } else if !inHunk, line.hasPrefix("+++ "), line != "+++ /dev/null" {
                files[files.count - 1].title = line.hasPrefix("+++ b/") ? String(line.dropFirst(6)) : String(line.dropFirst(4))
            } else if !inHunk, line.hasPrefix("--- a/") { files[files.count - 1].title = String(line.dropFirst(6)) }
            let display = line.count > 4096 ? String(line.prefix(4096)) + " … [line clipped]" : line
            if display != line { truncated = true }
            files[files.count - 1].lines.append(DiffLine(id: index, text: display, kind: kind, oldLine: old, newLine: new))
        }
        self.files = files; self.truncated = truncated
    }
    public var additions: Int { files.reduce(0) { $0 + $1.additions } }
    public var removals: Int { files.reduce(0) { $0 + $1.removals } }
}
