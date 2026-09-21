import SwiftUI
import DesktopCore

struct ProtectionEditorView: View {
    @ObservedObject var protections: PrivacyProtections
    let connected: Bool
    @State private var path = ""
    @Binding var expanded: Bool
    @State private var page = 0
    private var rules: [ProtectionRule] { protections.policy?.rules ?? [] }
    private var busy: Bool { protections.loading || protections.saving || !connected }
    var body: some View {
        DisclosureGroup(isExpanded: $expanded) {
            VStack(alignment: .leading, spacing: 10) {
                Text("Protection blocks file access across this account’s tasks and starts fresh model context. Previously sent content cannot be recalled. The context reset remains after removing a rule. Cached project instructions, skills, memories and editor context are isolated; permitted project files may be read afresh through guarded file tools. While protection is active, shell, Git, MCP, attachments and undo are disabled.")
                    .font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                HStack {
                    TextField("Absolute or project-relative path", text: $path).textFieldStyle(.roundedBorder).accessibilityLabel("File or folder to protect")
                    Button("Protect path") { add() }.disabled(busy || protections.policy == nil || path.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                }
                Text("External absolute paths are supported. You can also send /protect <path> in chat.").font(.caption2).foregroundStyle(.secondary)
                if let error = protections.error { Text(error).font(.caption).foregroundStyle(.red).textSelection(.enabled) }
                if protections.loading || protections.saving { ProgressView(protections.saving ? "Saving protection policy…" : "Loading protections…").controlSize(.small) }
                if rules.isEmpty && protections.policy != nil { Text(protections.policy?.changedAt == nil ? "No protected paths in this account." : "No active rules. Fresh model context remains in effect.").font(.caption).foregroundStyle(.secondary) }
                ForEach(Array(rules.dropFirst(page * 40).prefix(40))) { rule in
                    HStack(alignment: .top) {
                        Text(rule.path).font(.system(.caption, design: .monospaced)).textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading)
                        Button("Remove") { Task { _ = await protections.change(.remove(rule.id)) } }.disabled(busy).accessibilityLabel("Remove protection for \(rule.path)")
                    }.padding(.vertical, 3)
                }
                if rules.count > 40 {
                    HStack { Button("Previous") { page -= 1 }.disabled(page == 0); Text("Page \(page + 1) of \((rules.count + 39) / 40)").font(.caption); Button("Next") { page += 1 }.disabled((page + 1) * 40 >= rules.count) }
                }
                Button("Refresh protections") { Task { await protections.refresh() } }.disabled(busy)
            }.padding(.top, 10)
        } label: { Label("Protected files · \(protections.policy.map { String($0.rules.count) } ?? "…") · current account", systemImage: "lock.shield") }
        .padding(14).background(Color.primary.opacity(0.025), in: RoundedRectangle(cornerRadius: 9))
        .overlay(RoundedRectangle(cornerRadius: 9).stroke(Color.primary.opacity(0.1)))
        .padding(.horizontal, 24).padding(.vertical, 12)
        .onChange(of: rules.count) { _, count in page = min(page, max(0, (count - 1) / 40)) }
    }
    private func add() {
        let value = path.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !value.isEmpty else { return }
        Task { if await protections.change(.add(value)) { path = "" } }
    }
}

struct ProtectionSidebar: View {
    @ObservedObject var protections: PrivacyProtections
    let connected: Bool
    let width: CGFloat
    let manage: () -> Void
    @State private var expanded = true
    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 12) {
                DisclosureGroup(isExpanded: $expanded) {
                    VStack(alignment: .leading, spacing: 9) {
                        if let policy = protections.policy {
                            if policy.rules.isEmpty { Text("No protected paths.").foregroundStyle(.secondary) }
                            ForEach(Array(policy.rules.prefix(8))) { rule in Text(rule.path).font(.system(.caption2, design: .monospaced)).textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading) }
                            if policy.rules.count > 8 { Text("Showing 8 of \(policy.rules.count). Manage in Privacy.").foregroundStyle(.secondary) }
                        } else { Text(protections.loading ? "Loading protection policy…" : connected ? "Protection policy unavailable." : "Reconnect to load protections.").foregroundStyle(.secondary) }
                    }.font(.caption).padding(.top, 10)
                } label: { Label("Protected files · \(protections.policy.map { String($0.rules.count) } ?? "…")", systemImage: "lock.shield").font(.caption.weight(.semibold)) }
                if let error = protections.error { Text(error).font(.caption2).foregroundStyle(.red) }
                if !(protections.policy?.rules.isEmpty ?? true) { Text("Strict protection active. Shell, Git, MCP, attachments and undo disabled.").font(.caption2).foregroundStyle(.secondary) }
                Button("Manage in Privacy", action: manage).font(.caption)
            }.padding(14)
        }.frame(width: width).background(Color.primary.opacity(0.025)).accessibilityLabel("Protected files")
    }
}
