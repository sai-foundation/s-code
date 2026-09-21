import Foundation
import DesktopCore

@MainActor enum PrivacyFileChecks {
    static func run() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("privacy-tree-" + UUID().uuidString)
        try FileManager.default.createDirectory(at: root.appendingPathComponent("src"), withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        try Data("private body must not be read by tree".utf8).write(to: root.appendingPathComponent("src/main.swift"))
        try Data().write(to: root.appendingPathComponent("untouched.swift"))
        try FileManager.default.createSymbolicLink(at: root.appendingPathComponent("outside"), withDestinationURL: root.deletingLastPathComponent())
        let page = try PrivacyDirectoryReader.read(root: root.path, path: "")
        try expectEqual(page.entries.count, 3)
        try expectTrue(page.entries.contains { $0.path == "outside" && $0.isLink && !$0.isDirectory })
        try expectThrows(try PrivacyDirectoryReader.read(root: root.path, path: "outside"))
        try expectThrows(try PrivacyDirectoryReader.read(root: root.path, path: "../"))
        try expectTrue(try PrivacyDirectoryReader.read(root: root.path, path: "", limit: 2).truncated)
        func request(_ id: Int, path: String, kind: String = "file text", partial: Bool = false, status: String = "accepted", bytes: Int = 12) throws -> PrivacyRequest {
            var json = PrivacyChecks.request(id, status: status)
            json = json.replacing("sources", with: .array([.object(["source": .string(path), "kind": .string(kind), "partial": .bool(partial), "content_bytes": .number(Double(bytes))])]))
            return try PrivacyPage(.object(["requests": .array([json]), "next_before": .null])).requests[0]
        }
        let requests = try [
            request(1, path: "src/main.swift"),
            request(2, path: "src/main.swift", partial: true),
            request(3, path: "deleted/old.swift", partial: true),
            request(4, path: "unknown.swift", status: "connection_error"),
            request(5, path: "untouched.swift", kind: "attachment"),
            request(6, path: "../escape.swift"),
            request(7, path: root.path + "-other/secret.swift"),
            request(8, path: "untouched.swift", bytes: 0),
            request(9, path: root.appendingPathComponent("src/main.swift").absoluteString)
        ]
        let evidence = PrivacyFileAttribution.evidence(requests, root: root.path)
        try expectEqual(evidence["src/main.swift"]?.state, .entire)
        try expectEqual(evidence["src/main.swift"]?.requests.count, 3)
        try expectEqual(evidence["deleted/old.swift"]?.state, .partial)
        try expectTrue(evidence["unknown.swift"]?.unknownDelivery == true)
        try expectEqual(evidence.count, 3)
        let tree = PrivacyFileTree()
        tree.reset(root: root.path)
        while tree.loading { try await Task.sleep(for: .milliseconds(5)) }
        tree.update(requests: requests, incomplete: true)
        try expectTrue(tree.coverageIncomplete)
        try expectTrue(tree.rows.contains { $0.path == "src" && $0.isDirectory })
        try expectTrue(tree.rows.contains { $0.path == "untouched.swift" && $0.state == .none })
        tree.toggle("src")
        while tree.loading { try await Task.sleep(for: .milliseconds(5)) }
        try expectTrue(tree.rows.contains { $0.path == "src/main.swift" && $0.state == .entire })
        tree.query = "old.swift"
        try expectEqual(tree.rows.map(\.path), ["deleted", "deleted/old.swift"])
        // Historic descendants remain discoverable after their parent changes type.
        try Data().write(to: root.appendingPathComponent("former-folder"))
        tree.refresh()
        while tree.loading { try await Task.sleep(for: .milliseconds(5)) }
        let deep = Array(repeating: "level", count: 40).joined(separator: "/") + "/deep.swift"
        tree.update(requests: try [request(20, path: "former-folder/old.swift"), request(21, path: deep)], incomplete: false)
        tree.query = "old.swift"
        try expectTrue(tree.rows.contains { $0.path == "former-folder/old.swift" })
        tree.query = "deep.swift"
        try expectTrue(tree.rows.contains { $0.path == deep })
        tree.reset(root: root.path); tree.reset()
        try await Task.sleep(for: .milliseconds(30))
        try expectTrue(tree.rows.isEmpty)
        try expectEqual(tree.rootName, nil)
        // Large flat folders cannot flood the native view; show-more is explicit.
        let many = root.appendingPathComponent("many")
        try FileManager.default.createDirectory(at: many, withIntermediateDirectories: true)
        for i in 0..<225 { try Data().write(to: many.appendingPathComponent("file-\(i).txt")) }
        tree.reset(root: many.path)
        while tree.loading { try await Task.sleep(for: .milliseconds(5)) }
        try expectEqual(tree.rows.count, 200); try expectTrue(tree.hasMore)
        tree.loadMore(); try expectEqual(tree.rows.count, 225); try expectFalse(tree.hasMore)
        print("PASS: privacy file tree bounded lazy listing, symlink boundary, path attribution, coverage states, missing files, filters, pagination and session reset")
    }
}
