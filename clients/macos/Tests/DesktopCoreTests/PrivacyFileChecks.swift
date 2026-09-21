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
        // Every non-project source remains discoverable without scanning its directory.
        let externalPath = root.path + "-other/private.swift"
        let outside = try [
            request(30, path: externalPath), request(31, path: externalPath, status: "connection_error"),
            request(32, path: "photo.png", kind: "attachment"),
            request(33, path: "photo.png", kind: "attachment name only", bytes: 0),
            request(34, path: "../unresolved.swift"),
            request(35, path: "https://example.test", kind: "team_knowledge"),
            request(36, path: "control-plane", kind: "system"),
            request(37, path: "file:///tmp/repo/%2e%2e/outside.swift"),
            request(38, path: "file://remote/tmp/remote.swift"),
            request(39, path: "src/zero.swift", bytes: 0),
            request(40, path: "/tmp/long… [label truncated]")
        ]
        let labels = PrivacyRecordedSources.collect(outside, root: root.path)
        let external = labels.filter { $0.group == .external }
        try expectEqual(external.count, 1)
        try expectEqual(external[0].requests.count, 2)
        try expectEqual(external[0].state, .entire)
        try expectEqual(external[0].outcomes.count, 2)
        try expectEqual(labels.filter { $0.group == .attachment }.count, 2)
        try expectTrue(labels.contains { $0.kind == "attachment name only" && !$0.hasContent && $0.state == .none })
        try expectTrue(labels.contains { $0.source == "control-plane" && $0.group == .other })
        try expectTrue(labels.contains { $0.source.contains("%2e%2e") && $0.group == .other })
        try expectTrue(labels.contains { $0.source == "../unresolved.swift" && $0.group == .other })
        try expectTrue(PrivacyFileAttribution.evidence(outside, root: root.path).isEmpty)
        let noProject = PrivacyRecordedSources.collect(try [request(41, path: root.path + "/src/main.swift"), request(42, path: "relative.swift")], root: nil)
        try expectTrue(noProject.contains { $0.group == .external })
        try expectTrue(noProject.contains { $0.group == .other })
        var allIDs: Set<String> = []
        let manySources = try (100..<205).map { try request($0, path: "/other/dir-\($0)/same.swift") }
        let grouped = PrivacyRecordedSources.collect(manySources, root: root.path).filter { $0.group == .external }
        for index in 0..<3 {
            let page = PrivacyRecordedSources.page(grouped, index: index)
            try expectTrue(page.items.count <= 40)
            page.items.forEach { allIDs.insert($0.id) }
        }
        try expectEqual(allIDs.count, 105)
        try expectEqual(PrivacyRecordedSources.page(grouped, index: 999).index, 2)
        try expectEqual(PrivacyRecordedSources.page(grouped, index: 0, query: "dir-204").total, 1)
        let tree = PrivacyFileTree()
        tree.reset(root: root.path)
        while tree.loading { try await Task.sleep(for: .milliseconds(5)) }
        tree.update(requests: requests + outside, incomplete: true)
        try expectTrue(tree.recordedSources.contains { $0.group == .external })
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
        try expectTrue(tree.recordedSources.isEmpty)
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
