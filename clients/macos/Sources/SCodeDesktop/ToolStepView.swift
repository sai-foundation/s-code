import SwiftUI
import AppKit
import DesktopCore

struct ToolStepView: View {
    @EnvironmentObject private var store: AppStore
    let row: TranscriptRow
    @State private var expanded = false
    @State private var detail: JSON?
    @State private var failure: String?
    @State private var retry = 0
    private var failed: Bool { ["failed", "denied", "cancelled"].contains(row.status) }
    private var context: String { (store.profile?.id ?? "") + ":" + row.value["session_id"].string + ":" + row.id + ":" + row.status }
    var body: some View {
        VStack(alignment: .leading, spacing: 7) {
            DisclosureGroup(isExpanded: $expanded) {
                if expanded {
                    if let detail { ToolDetailContent(detail: detail) }
                    else if let failure { retryNotice(failure) }
                    else { Text("Loading tool details…").font(.caption).foregroundStyle(.secondary) }
                }
            } label: {
                HStack(spacing: 8) {
                    Image(systemName: icon).foregroundStyle(failed ? Color.orange : Color.secondary)
                    Text(row.text).font(.callout).lineLimit(expanded ? nil : 1).truncationMode(.middle)
                    Spacer(minLength: 0)
                    Text(statusLabel).font(.caption).foregroundStyle(.secondary)
                }.help(row.text)
            }
            if failed {
                Text(failure ?? ToolPresentation.failure(row: row, detail: detail) ?? "")
                    .font(.caption).foregroundStyle(.orange).textSelection(.enabled).lineLimit(expanded ? nil : 4).fixedSize(horizontal: false, vertical: true)
                if failure != nil && !expanded { Button("Retry details") { retry += 1 }.font(.caption) }
            }
        }.padding(.horizontal, 10).padding(.vertical, row.status == "completed" && !expanded ? 5 : 10)
            .background(Color.primary.opacity(0.025), in: RoundedRectangle(cornerRadius: 8))
            .task(id: context + ":\(expanded):\(retry)") {
                guard expanded || failed else { return }
                await load()
            }
            .onChange(of: context) { _, _ in detail = nil; failure = nil }
    }
    private var icon: String {
        switch row.status {
        case "completed": return "checkmark.circle"
        case "failed", "denied": return "exclamationmark.circle"
        case "cancelled": return "stop.circle"
        case "awaiting_approval": return "hand.raised"
        default: return "terminal"
        }
    }
    private var statusLabel: String {
        switch row.status { case "completed": return "Done"; case "cancelled": return "Stopped"; case "awaiting_approval": return "Needs approval"; default: return row.status.replacingOccurrences(of: "_", with: " ").capitalized }
    }
    private func load() async {
        failure = nil
        do { detail = try await store.toolDetails(row.value["content"]["tool_call_id"].string, sessionID: row.value["session_id"].string) }
        catch { if !Task.isCancelled && !(error is CancellationError) { failure = "Could not load tool details. " + error.localizedDescription } }
    }
    private func retryNotice(_ failure: String) -> some View {
        VStack(alignment: .leading) { Text(failure).font(.caption).foregroundStyle(.secondary); Button("Retry details") { retry += 1 }.font(.caption) }
    }
}

struct ApprovalArguments: View {
    @EnvironmentObject private var store: AppStore
    let toolID: String
    let sessionID: String
    @State private var expanded = false
    @State private var detail: JSON?
    @State private var failure: String?
    @State private var retry = 0
    var body: some View {
        DisclosureGroup("Inspect arguments", isExpanded: $expanded) {
            if let detail { ToolDetailContent(detail: detail, argumentsOnly: true) }
            else if let failure {
                Text(failure).font(.caption).foregroundStyle(.secondary)
                Button("Retry details") { retry += 1 }.font(.caption)
            } else { Text("Loading proposed arguments…").font(.caption).foregroundStyle(.secondary) }
        }.font(.callout)
            .task(id: "\(store.profile?.id ?? ""):\(sessionID):\(toolID):\(expanded):\(retry)") {
                guard expanded else { return }
                detail = nil; failure = nil
                do { detail = try await store.toolDetails(toolID, sessionID: sessionID) }
                catch { if !Task.isCancelled && !(error is CancellationError) { failure = "Could not load proposed arguments. " + error.localizedDescription } }
            }
    }
}

private struct ToolDetailContent: View {
    let detail: JSON
    var argumentsOnly = false
    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            value("Arguments", detail["request"]["arguments"].pretty)
            if !argumentsOnly {
                if detail["result"] != .null { value("Result", detail["result"].pretty) }
                if !detail["error"].string.isEmpty { value("Error", detail["error"].string) }
            }
            Text("Exact recorded details, with secrets filtered by the server.").font(.caption2).foregroundStyle(.secondary)
        }.padding(.top, 8)
    }
    private func value(_ title: String, _ text: String) -> some View {
        VStack(alignment: .leading, spacing: 5) {
            HStack { Text(title).font(.caption.weight(.semibold)); Spacer(); Button("Copy") { NSPasteboard.general.clearContents(); NSPasteboard.general.setString(text, forType: .string) }.font(.caption).accessibilityLabel("Copy " + title.lowercased()) }
            ScrollView([.vertical, .horizontal]) { Text(text).font(.system(.caption, design: .monospaced)).textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading).padding(8) }
                .frame(height: min(220, CGFloat(text.split(separator: "\n", omittingEmptySubsequences: false).count) * 16 + 20)).background(Color.primary.opacity(0.035), in: RoundedRectangle(cornerRadius: 6))
        }
    }
}
