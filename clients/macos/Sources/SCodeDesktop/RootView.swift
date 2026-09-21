import SwiftUI
import AppKit
import DesktopCore

private let accent = Color(red: 0.92, green: 0.40, blue: 0.18)
struct RootView: View {
    @EnvironmentObject var store: AppStore
    @State private var search = ""
    var body: some View {
        NavigationSplitView {
            VStack(alignment: .leading, spacing: 18) {
                HStack(spacing: 10) {
                    Image(systemName: "s.square.fill").font(.system(size: 28, weight: .semibold)).foregroundStyle(accent)
                    VStack(alignment: .leading, spacing: 2) { Text("S-Code").font(.headline); Text("Your ideas, in motion.").font(.caption).foregroundStyle(.secondary) }
                }.padding(.top, 26)
                HStack {
                    Button { store.create() } label: { Label("New chat", systemImage: "plus") }.buttonStyle(.borderedProminent).tint(accent)
                    Button { store.chooseFolder() } label: { Image(systemName: "folder.badge.plus") }.help("Open project · ⌘O").accessibilityLabel("Open project")
                }.disabled(!store.connected || store.submitting)
                TextField("Search conversations", text: $search).textFieldStyle(.roundedBorder).accessibilityLabel("Search conversations")
                Text("CONVERSATIONS").font(.system(size: 10, weight: .semibold)).tracking(1.5).foregroundStyle(.secondary)
                ScrollView {
                    LazyVStack(spacing: 4) {
                        ForEach(store.sessions.filter { search.isEmpty || $0.title.localizedCaseInsensitiveContains(search) }) { session in
                            Button { Task { await store.select(session.id) } } label: {
                                HStack(alignment: .top, spacing: 9) {
                                    Image(systemName: session.mode == "work" ? "folder" : "bubble.left").foregroundStyle(.secondary).frame(width: 16)
                                    VStack(alignment: .leading, spacing: 4) {
                                        Text(session.title).font(.system(size: 13, weight: .medium)).lineLimit(2)
                                        Text(session.mode == "work" ? URL(fileURLWithPath: session.folder).lastPathComponent : "Chat").font(.caption2).foregroundStyle(.secondary).lineLimit(1)
                                    }
                                    Spacer(minLength: 0)
                                    if store.needsAttention.contains(session.id) { Image(systemName: "hand.raised.fill").foregroundStyle(accent).help("Needs your response") }
                                    else if store.running.contains(session.id) { ProgressView().controlSize(.mini) }
                                }.padding(10).frame(maxWidth: .infinity, alignment: .leading)
                                    .background(store.selectedID == session.id ? accent.opacity(0.13) : .clear, in: RoundedRectangle(cornerRadius: 9))
                            }.buttonStyle(.plain)
                        }
                    }
                }
                Spacer(minLength: 0)
                Divider()
                HStack {
                    Circle().fill(store.connected ? Color.green : Color.secondary).frame(width: 6, height: 6)
                    Text(store.status).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                    Spacer()
                    if store.connecting { ProgressView().controlSize(.small) }
                    Button { store.settingsOpen = true } label: { Image(systemName: "slider.horizontal.3") }.buttonStyle(.plain).accessibilityLabel("Connections")
                }
                if let profile = store.profile {
                    Menu {
                        ForEach(store.profiles) { profile in Button(profile.name) { store.connect(profile) } }
                        Divider(); Button("Manage connections…") { store.settingsOpen = true }
                    } label: { Label(profile.name, systemImage: "person.crop.circle").font(.caption) }
                    .disabled(store.connecting)
                }
            }.padding(18)
                .navigationSplitViewColumnWidth(min: 215, ideal: 250, max: 310)
        } detail: {
            VStack(spacing: 0) {
                header
                if let error = store.error {
                    HStack(alignment: .top, spacing: 10) {
                        Image(systemName: "exclamationmark.circle").foregroundStyle(accent)
                        Text(error).font(.callout).textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading)
                        if !store.connected { Button("Reconnect") { if let profile = store.profile { store.connect(profile) } else { store.settingsOpen = true } }.disabled(store.connecting) }
                        Button { store.error = nil } label: { Image(systemName: "xmark") }.buttonStyle(.plain).accessibilityLabel("Dismiss error")
                    }.padding(14).background(accent.opacity(0.08))
                }
                if store.privacyOpen, store.selectedID != nil {
                    GeometryReader { viewport in
                        PrivacyView(history: store.privacy, files: store.privacyFiles, protections: store.protections, protectionEditorOpen: $store.protectionEditorOpen, root: store.selected?.mode == "work" ? store.selected?.folder : nil, connected: store.connected) { store.privacyOpen = false }
                            .id((store.profile?.id ?? "") + ":" + (store.selectedID ?? ""))
                            .frame(width: viewport.size.width, height: viewport.size.height, alignment: .topLeading)
                            .clipped()
                    }
                } else {
                    GeometryReader { viewport in
                    HStack(spacing: 0) {
                        VStack(spacing: 0) {
                            if store.selectedID == nil { emptyState }
                            else { transcript }
                            if store.selectedID != nil { composer }
                        }.frame(maxWidth: .infinity, maxHeight: .infinity)
                        if store.selectedID != nil {
                            Divider()
                            ProtectionSidebar(protections: store.protections, connected: store.connected, width: viewport.size.width < 760 ? 180 : 230) {
                                store.protectionEditorOpen = true; store.privacyOpen = true
                            }
                        }
                    }
                    }
                }
            }.background(Color(nsColor: .textBackgroundColor))
        }
        .tint(accent)
        .sheet(isPresented: $store.settingsOpen) { ConnectionsView().environmentObject(store) }
        .sheet(item: $store.detail) { detail in
            VStack(alignment: .leading, spacing: 16) {
                HStack { Text(detail.title).font(.title2.bold()); Spacer(); Button("Done") { store.detail = nil }.keyboardShortcut(.cancelAction) }
                ScrollView([.vertical, .horizontal]) { Text(detail.text).font(.system(.body, design: .monospaced)).textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading).padding(12) }
                Button("Copy") { NSPasteboard.general.clearContents(); NSPasteboard.general.setString(detail.text, forType: .string) }
            }.padding(24).frame(minWidth: 680, idealWidth: 850, minHeight: 480, idealHeight: 620)
        }
    }
    private var header: some View {
        HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 4) {
                Text(store.selected?.title ?? "Make something great.").font(.system(size: 16, weight: .semibold)).lineLimit(1)
                if let session = store.selected {
                    HStack(spacing: 6) {
                        Text(session.mode == "work" ? "WORK" : "CHAT").font(.system(size: 9, weight: .bold)).tracking(1).padding(.horizontal, 6).padding(.vertical, 3).background(accent.opacity(0.1), in: Capsule())
                        Text(session.mode == "work" ? session.folder : "A conversation without file access").font(.caption).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle)
                    }
                } else { Text("Safe · Speedy · Self-evolving").font(.caption).foregroundStyle(.secondary) }
            }
            Spacer()
            if store.selectedID != nil {
                Button { store.privacyOpen.toggle() } label: { Label("Privacy", systemImage: "hand.raised.square") }
                    .tint(store.privacyOpen ? accent : nil)
                    .disabled(!store.connected && !store.privacyOpen).help(store.privacyOpen ? "Return to conversation" : "Explore files and model request history")
                    .accessibilityValue(store.privacyOpen ? "Open" : "Closed")
            }
            if store.selected?.mode == "work" {
                Button { store.showDiff() } label: { Label("Changes", systemImage: "plus.forwardslash.minus") }.disabled(!store.connected || !(store.protections.policy?.rules.isEmpty ?? true) || store.busyRequests.contains("diff:" + (store.selectedID ?? "")))
                Button { if let folder = store.selected?.folder { NSWorkspace.shared.selectFile(nil, inFileViewerRootedAtPath: folder) } } label: { Image(systemName: "folder") }.help("Reveal project in Finder")
            }
        }.padding(.horizontal, 26).padding(.top, 22).padding(.bottom, 18)
    }
    private var emptyState: some View {
        VStack(spacing: 18) {
            Spacer()
            Image(systemName: "sparkle").font(.system(size: 48, weight: .light)).foregroundStyle(accent)
            Text("A little spark.\nYour next big thing.").font(.system(size: 34, weight: .semibold, design: .rounded)).multilineTextAlignment(.center)
            Text("Ask a question, explore an idea, or open a project\nand build something together.").font(.body).foregroundStyle(.secondary).multilineTextAlignment(.center)
            HStack(spacing: 12) {
                Button { if store.connected { store.create() } else { store.settingsOpen = true } } label: { Label(store.connected ? "Start a chat" : "Connect your model", systemImage: "bubble.left") }.buttonStyle(.borderedProminent).controlSize(.large)
                if store.connected { Button { store.chooseFolder() } label: { Label("Open a project", systemImage: "folder") }.controlSize(.large) }
            }.padding(.top, 8).disabled(store.connecting)
            if store.connecting { ProgressView("Starting your local engine…").controlSize(.small) }
            Spacer()
            Text("Your projects stay on your Mac. Model requests go to your chosen provider.").font(.caption).foregroundStyle(.secondary).padding(.bottom, 28)
        }.frame(maxWidth: .infinity)
    }
    private var transcript: some View {
        GeometryReader { _ in
            ScrollViewReader { proxy in
                ScrollView {
                    VStack(alignment: .leading, spacing: 22) {
                        if store.nextCursor != nil {
                            Button(store.loadingHistory ? "Loading…" : "Load older messages") { store.older() }.disabled(store.loadingHistory || store.turnRunning).frame(maxWidth: .infinity)
                        }
                        if store.transcript.rows.isEmpty { Text("What would you like to work on?").foregroundStyle(.secondary).padding(.top, 30) }
                        ForEach(store.transcript.rows) { row in
                            TranscriptRowView(row: row) { store.showDetails(row) }.equatable().id(row.id)
                        }
                        ForEach(store.approvals, id: \.self) { request in ApprovalCard(request: request).environmentObject(store) }
                        ForEach(store.questions, id: \.self) { request in QuestionCard(request: request).environmentObject(store) }
                        if store.activity.isWorking && store.approvals.isEmpty && store.questions.isEmpty {
                            HStack(spacing: 9) { ProgressView().controlSize(.small); Text("S-Code is working…").font(.callout).foregroundStyle(.secondary) }.padding(.vertical, 4)
                        }
                        if let feedback = store.turnFeedback {
                            Label(feedback, systemImage: "info.circle").font(.callout).foregroundStyle(.secondary).textSelection(.enabled)
                        }
                        Color.clear.frame(height: 1).id("latest")
                    }.padding(.horizontal, 30).padding(.vertical, 20).frame(maxWidth: 980, alignment: .leading).frame(maxWidth: .infinity)
                }.coordinateSpace(name: "transcript")
                    .background(UserScrollObserver { if store.followLatest { store.followLatest = false } })
                    .task(id: store.renderTick) {
                        // Scroll after the layout transaction, never from its update callback.
                        try? await Task.sleep(for: .milliseconds(16))
                        if !Task.isCancelled && store.followLatest { proxy.scrollTo("latest", anchor: .bottom) }
                    }
                    .overlay(alignment: .bottomTrailing) {
                        if !store.followLatest { Button { store.followLatest = true; proxy.scrollTo("latest", anchor: .bottom) } label: { Label("Latest", systemImage: "arrow.down") }.buttonStyle(.bordered).padding(18) }
                    }
            }
        }
    }
    private var composer: some View {
        VStack(spacing: 7) {
            if let waiting = store.activity.waitingLabel {
                HStack {
                    Label(waiting, systemImage: "hand.raised.fill").foregroundStyle(accent)
                    Spacer()
                    Button("Show request") { store.followLatest = true; store.renderTick += 1 }
                }.font(.callout).padding(.bottom, 6)
            }
            VStack(spacing: 4) {
                ZStack(alignment: .topLeading) {
                    if store.draft.isEmpty { Text("Message S-Code…").foregroundStyle(.tertiary).padding(.horizontal, 12).padding(.top, 10).allowsHitTesting(false) }
                    Composer(text: $store.draft) { store.send() }.frame(height: 86)
                }
                HStack {
                    Text(store.selected?.model ?? store.profile?.model ?? "").font(.caption).foregroundStyle(.secondary).lineLimit(1)
                    Spacer()
                    if store.turnRunning && !store.draftIsProtectionCommand { Button { store.stopTurn() } label: { Label("Stop", systemImage: "stop.fill") }.buttonStyle(.bordered) }
                    else { Button { store.send() } label: { Image(systemName: "arrow.up").font(.body.bold()).frame(width: 24, height: 22) }.buttonStyle(.borderedProminent).disabled(!store.connected || !store.permissionsReady || store.submitting || store.draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty).accessibilityLabel("Send message") }
                }.padding(.horizontal, 12).padding(.bottom, 10)
            }.background(Color(nsColor: .controlBackgroundColor), in: RoundedRectangle(cornerRadius: 15))
                .overlay(RoundedRectangle(cornerRadius: 15).stroke(Color.primary.opacity(0.12)))
            HStack { Text("↵ Send · ⇧↵ New line"); Spacer(); if store.usage > 0 { Text("\(store.usage.formatted()) tokens") }; PermissionPicker().environmentObject(store) }.font(.system(size: 10)).foregroundStyle(.tertiary)
        }.padding(.horizontal, 28).padding(.bottom, 18)
    }
}
struct TranscriptRowView: View, Equatable {
    let row: TranscriptRow
    let details: () -> Void
    static func == (lhs: Self, rhs: Self) -> Bool { lhs.row == rhs.row }
    var body: some View {
        if row.isMessage {
            VStack(alignment: .leading, spacing: 9) {
                HStack { Text(row.role == "user" ? "YOU" : "S-CODE").font(.system(size: 10, weight: .bold)).tracking(1.4).foregroundStyle(row.role == "user" ? Color.secondary : accent); Spacer(); Button { NSPasteboard.general.clearContents(); NSPasteboard.general.setString(row.text, forType: .string) } label: { Image(systemName: "doc.on.doc") }.buttonStyle(.plain).foregroundStyle(.tertiary).help("Copy message").accessibilityLabel("Copy message") }
                MessageContent(text: row.text, streaming: row.status == "running" || row.status == "streaming")
            }.padding(16).frame(maxWidth: .infinity, alignment: .leading).background(row.role == "user" ? Color.primary.opacity(0.035) : .clear, in: RoundedRectangle(cornerRadius: 12))
        } else if !["approval", "question", "usage"].contains(row.kind) {
            Button(action: details) {
                HStack(alignment: .top, spacing: 10) {
                    Image(systemName: ["failed", "denied"].contains(row.status) ? "exclamationmark.circle" : "terminal").foregroundStyle(accent)
                    VStack(alignment: .leading, spacing: 4) {
                        Text(row.text).font(.system(.callout, design: .monospaced)).lineLimit(3)
                        Text(row.status.replacingOccurrences(of: "_", with: " ")).font(.caption).foregroundStyle(.secondary)
                    }
                    Spacer(); Image(systemName: "chevron.right").font(.caption).foregroundStyle(.tertiary)
                }.padding(12).background(Color.primary.opacity(0.025), in: RoundedRectangle(cornerRadius: 10))
            }.buttonStyle(.plain)
        }
    }
}
struct MessageContent: View {
    let text: String
    let streaming: Bool
    var body: some View {
        if streaming { Text(text).font(.system(size: 14)).textSelection(.enabled).lineSpacing(5) }
        else {
            let parts = text.components(separatedBy: "```")
            ForEach(Array(parts.enumerated()), id: \.offset) { index, part in
                if index % 2 == 1 {
                    let lines = part.split(separator: "\n", omittingEmptySubsequences: false)
                    let code = lines.dropFirst().joined(separator: "\n")
                    VStack(alignment: .leading, spacing: 8) {
                        HStack { Text(String(lines.first ?? "Code")).font(.caption).foregroundStyle(.secondary); Spacer(); Button("Copy code") { NSPasteboard.general.clearContents(); NSPasteboard.general.setString(code, forType: .string) }.buttonStyle(.plain).font(.caption) }
                        ScrollView(.horizontal) { Text(code).font(.system(size: 12, design: .monospaced)).textSelection(.enabled) }
                    }.padding(12).background(Color.primary.opacity(0.055), in: RoundedRectangle(cornerRadius: 8))
                } else if !part.isEmpty {
                    Text((try? AttributedString(markdown: part, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace))) ?? AttributedString(part)).font(.system(size: 14)).textSelection(.enabled).lineSpacing(5)
                }
            }
        }
    }
}
struct ApprovalCard: View {
    @EnvironmentObject var store: AppStore
    let request: JSON
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Label("Your approval is needed", systemImage: "hand.raised.fill").font(.headline).foregroundStyle(accent)
            Text(request["summary"].string).textSelection(.enabled)
            if !request["target"].string.isEmpty { Text(request["target"].string).font(.system(.caption, design: .monospaced)).textSelection(.enabled) }
            Text("\(request["risk"].string.capitalized) risk · \(request["impact_scope"].string)").font(.caption).foregroundStyle(.secondary)
            Text(request["policy_reason"].string).font(.caption).foregroundStyle(.secondary)
            Text("Open the tool card above to inspect its arguments before approving.").font(.caption).foregroundStyle(.secondary)
            HStack {
                Button("Reject") { store.decide(request, approved: false) }
                Button("Allow once") { store.decide(request, approved: true) }.buttonStyle(.borderedProminent)
            }.disabled(store.busyRequests.contains(request["id"].string))
        }.padding(18).frame(maxWidth: .infinity, alignment: .leading).background(accent.opacity(0.07), in: RoundedRectangle(cornerRadius: 12))
    }
}
struct QuestionCard: View {
    @EnvironmentObject var store: AppStore
    let request: JSON
    @State private var values: [String: String] = [:]
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Label("A quick question", systemImage: "questionmark.bubble").font(.headline)
            ForEach(request["questions"].array, id: \.self) { question in
                Text(question["question"].string)
                ForEach(question["options"].array, id: \.self) { option in
                    Button { values[question["id"].string] = option["label"].string } label: {
                        Label(option["label"].string, systemImage: values[question["id"].string] == option["label"].string ? "checkmark.circle.fill" : "circle")
                    }.help(option["description"].string)
                }
                if request["allow_other"].boolean || question["options"].array.isEmpty {
                    TextField("Your answer", text: Binding(get: { values[question["id"].string] ?? "" }, set: { values[question["id"].string] = $0 })).textFieldStyle(.roundedBorder)
                }
            }
            Button("Continue") { store.answer(request, values: values) }.buttonStyle(.borderedProminent).disabled(store.busyRequests.contains(request["id"].string) || request["questions"].array.contains { (values[$0["id"].string] ?? "").trimmingCharacters(in: .whitespacesAndNewlines).isEmpty })
        }.padding(18).frame(maxWidth: .infinity, alignment: .leading).background(Color.primary.opacity(0.04), in: RoundedRectangle(cornerRadius: 12))
    }
}
