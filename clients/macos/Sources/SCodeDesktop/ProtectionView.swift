import SwiftUI
import AppKit
import DesktopCore

private struct ProtectionEffects: View {
    @State private var expanded = false
    var body: some View {
        DisclosureGroup("How protection works", isExpanded: $expanded) {
            VStack(alignment: .leading, spacing: 6) {
                Text("Protected paths are blocked across this account’s tasks. Adding protection starts fresh model context; previously sent content cannot be recalled.")
                Text("Shell, Git, MCP, attachments and undo are restricted while protection is active because they could access protected content. Guarded file tools can still access permitted files.")
                Text("Cached project instructions, skills, memories and editor context are isolated. Unprotect allows future access under normal permissions; it does not restore older model context or erase transmission history.")
            }.font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true).padding(.top, 6)
        }.font(.caption)
    }
}

private struct ProtectionPathView: View {
    let rule: ProtectionRule
    let root: String?
    private var label: ProtectedPathLabel { ProtectedPathLabel(path: rule.path, root: root) }
    var body: some View {
        HStack(alignment: .top, spacing: 7) {
            Image(systemName: rule.kind == "directory" ? "folder.badge.lock" : "doc.badge.ellipsis").foregroundStyle(.secondary)
            VStack(alignment: .leading, spacing: 2) {
                Text(label.name).font(.caption.weight(.medium)).lineLimit(1).truncationMode(.middle)
                Text(label.parent).font(.caption2).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle)
            }
        }.frame(maxWidth: .infinity, alignment: .leading).help(rule.path)
            .accessibilityLabel("Protected: " + rule.path)
            .contextMenu { Button("Copy protected path") { NSPasteboard.general.clearContents(); NSPasteboard.general.setString(rule.path, forType: .string) } }
    }
}

private struct AddProtectionForm: View {
    @ObservedObject var protections: PrivacyProtections
    let connected: Bool
    var compact = false
    @State private var path = ""
    private var busy: Bool { protections.loading || protections.saving || !connected || protections.policy == nil }
    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            TextField("Absolute or project-relative path", text: $path).textFieldStyle(.roundedBorder).accessibilityLabel("File or folder to protect")
                .onSubmit { add() }
            HStack {
                Button("Choose file or folder…") { choose() }.disabled(busy)
                Spacer(minLength: 4)
                Button("Protect") { add() }.buttonStyle(.borderedProminent).disabled(busy || path.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            }
            Text("Applies to every task in this account. External absolute paths are supported.").font(.caption).foregroundStyle(.secondary)
            if compact, let error = protections.error { Text(error).font(.caption).foregroundStyle(.red).textSelection(.enabled) }
            if protections.saving { ProgressView("Saving protection…").controlSize(.small) }
        }
        .onChange(of: protections.contextID) { _, _ in path = "" }
    }
    private func add() {
        let value = path.trimmingCharacters(in: .whitespacesAndNewlines), context = protections.contextID
        guard !busy, !value.isEmpty else { return }
        Task { if await protections.change(.add(value), contextID: context), protections.contextID == context, path == value { path = "" } }
    }
    private func choose() {
        let context = protections.contextID
        let panel = NSOpenPanel(); panel.canChooseFiles = true; panel.canChooseDirectories = true
        panel.allowsMultipleSelection = false; panel.prompt = "Choose path"; panel.message = "Choose a file or folder to protect across this account’s tasks."
        panel.begin { response in
            guard response == .OK, let url = panel.url else { return }
            Task { @MainActor in if protections.contextID == context { path = url.path } }
        }
    }
}

struct ProtectionEditorView: View {
    @ObservedObject var protections: PrivacyProtections
    let connected: Bool
    @Binding var expanded: Bool
    var root: String? = nil
    @State private var page = 0
    private var rules: [ProtectionRule] { (protections.policy?.rules ?? []).sorted { a, b in
        let aProject = ProtectedPathLabel(path: a.path, root: root).inProject, bProject = ProtectedPathLabel(path: b.path, root: root).inProject
        return aProject != bProject ? aProject : a.path.localizedStandardCompare(b.path) == .orderedAscending
    } }
    private var busy: Bool { protections.loading || protections.saving || !connected }
    var body: some View {
        DisclosureGroup(isExpanded: $expanded) {
            VStack(alignment: .leading, spacing: 12) {
                Text("Current protection · all tasks in this account").font(.caption).foregroundStyle(.secondary)
                AddProtectionForm(protections: protections, connected: connected)
                ProtectionEffects()
                if let error = protections.error { Text(error).font(.caption).foregroundStyle(.red).textSelection(.enabled) }
                if protections.loading { ProgressView("Loading protections…").controlSize(.small) }
                if rules.isEmpty && protections.policy != nil { Text(protections.policy?.changedAt == nil ? "No protected paths in this account." : "No active rules. Fresh model context remains in effect.").font(.caption).foregroundStyle(.secondary) }
                if !rules.isEmpty { Text("Unprotect allows future access under normal permissions. Past transmission records remain.").font(.caption).foregroundStyle(.secondary) }
                ForEach([true, false], id: \.self) { inProject in
                    let group = Array(rules.dropFirst(page * 40).prefix(40)).filter { ProtectedPathLabel(path: $0.path, root: root).inProject == inProject }
                    if !group.isEmpty {
                        Text(inProject ? "In this project" : "Outside this project").font(.caption.weight(.semibold)).padding(.top, 4)
                        ForEach(group) { rule in
                            HStack(alignment: .top) {
                                ProtectionPathView(rule: rule, root: root)
                                Button("Unprotect") {
                                    let context = protections.contextID
                                    Task { _ = await protections.change(.remove(rule.id), contextID: context) }
                                }.disabled(busy).accessibilityLabel("Unprotect " + rule.path).help("Allow future access under normal permissions; transmission history remains.")
                            }.padding(.vertical, 3)
                        }
                    }
                }
                if rules.count > 40 {
                    HStack { Button("Previous") { page -= 1 }.disabled(page == 0); Text("Page \(page + 1) of \((rules.count + 39) / 40)").font(.caption); Button("Next") { page += 1 }.disabled((page + 1) * 40 >= rules.count) }
                }
                Button("Refresh protections") { Task { await protections.refresh() } }.disabled(busy)
            }.padding(.top, 10)
        } label: { Label("Protected now · \(protections.policy.map { String($0.rules.count) } ?? "…")", systemImage: "lock.shield") }
        .padding(14).background(Color.primary.opacity(0.025), in: RoundedRectangle(cornerRadius: 9))
        .overlay(RoundedRectangle(cornerRadius: 9).stroke(Color.primary.opacity(0.1)))
        .padding(.horizontal, 24).padding(.vertical, 12)
        .onChange(of: rules.count) { _, count in page = min(page, max(0, (count - 1) / 40)) }
    }
}

struct ProtectionSidebar: View {
    @ObservedObject var protections: PrivacyProtections
    let connected: Bool
    let width: CGFloat
    var root: String? = nil
    var accountName = "Current account"
    let manage: () -> Void
    @State private var expanded = true
    @State private var adding = false
    private var rules: [ProtectionRule] { protections.policy?.rules ?? [] }
    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 12) {
                DisclosureGroup(isExpanded: $expanded) {
                    VStack(alignment: .leading, spacing: 9) {
                        if protections.policy != nil {
                            if rules.isEmpty { Text("No protected paths.").foregroundStyle(.secondary) }
                            ForEach([true, false], id: \.self) { inProject in
                                let group = Array(rules.filter { ProtectedPathLabel(path: $0.path, root: root).inProject == inProject }.prefix(4))
                                if !group.isEmpty {
                                    Text(inProject ? "In this project" : "Outside this project").font(.caption2.weight(.semibold)).foregroundStyle(.secondary)
                                    ForEach(group) { rule in ProtectionPathView(rule: rule, root: root) }
                                }
                            }
                        } else { Text(protections.loading ? "Loading protection policy…" : connected ? "Protection policy unavailable." : "Reconnect to load protections.").foregroundStyle(.secondary) }
                    }.font(.caption).padding(.top, 10)
                } label: { Label("Protected files · \(protections.policy.map { String($0.rules.count) } ?? "…")", systemImage: "lock.shield").font(.caption.weight(.semibold)) }
                Text("\(accountName) · all tasks").font(.caption2).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                Button { adding = true } label: { Label("Add protection", systemImage: "plus") }.font(.caption)
                    .disabled(!connected || protections.policy == nil || protections.loading || protections.saving)
                    .popover(isPresented: $adding) {
                        VStack(alignment: .leading, spacing: 12) {
                            Text("Add protection").font(.headline)
                            AddProtectionForm(protections: protections, connected: connected, compact: true)
                            Button("Done") { adding = false }
                        }.padding(18).frame(width: 370)
                    }
                if let error = protections.error { Text(error).font(.caption2).foregroundStyle(.red) }
                if !rules.isEmpty { ProtectionEffects() }
                Button("View all in Privacy (\(rules.count))", action: manage).font(.caption)
                Text("Protection applies now. Earlier transmission history stays in Privacy.").font(.caption2).foregroundStyle(.secondary)
            }.padding(14)
        }.frame(width: width).background(Color.primary.opacity(0.025)).accessibilityLabel("Protected files")
        .onChange(of: protections.contextID) { _, _ in adding = false }
    }
}
