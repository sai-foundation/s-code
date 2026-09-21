import SwiftUI
import AppKit
import DesktopCore

private enum PrivacyTab: String, CaseIterable {
    case files = "Files"
    case events = "Event record"
}
private let privacyAccent = Color(red: 0.92, green: 0.40, blue: 0.18)

/// Occupies the conversation's detail region, leaving navigation available.
struct PrivacyView: View {
    @ObservedObject var history: PrivacyHistory
    @ObservedObject var files: PrivacyFileTree
    @ObservedObject var protections: PrivacyProtections
    @Binding var protectionEditorOpen: Bool
    let root: String?
    let connected: Bool
    let close: () -> Void
    @State private var tab: PrivacyTab = .files
    @State private var eventSearch = ""
    @State private var selectedFile: String?

    var body: some View {
        VStack(spacing: 0) {
            heading
            Divider()
            if !connected {
                Label("Reconnect to update privacy history.", systemImage: "wifi.slash")
                    .font(.caption).foregroundStyle(.secondary).frame(maxWidth: .infinity, alignment: .leading).padding(.horizontal, 24).padding(.vertical, 8)
            }
            if let error = history.error {
                errorNotice(error) { Task { await history.retry() } }
            }
            if tab == .files { fileBrowser } else { eventBrowser }
            Divider()
            footer
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Color(nsColor: .textBackgroundColor))
        .accessibilityElement(children: .contain).accessibilityLabel("Privacy explorer")
        .onChange(of: history.requests, initial: true) { _, _ in updateFileCoverage() }
        .onChange(of: history.nextBefore) { _, _ in updateFileCoverage() }
        .onChange(of: history.hasLoaded) { _, _ in updateFileCoverage() }
    }

    private func updateFileCoverage() {
        files.update(requests: history.requests, incomplete: !history.hasLoaded || history.nextBefore != nil)
    }

    private var heading: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack(alignment: .center, spacing: 12) {
                Image(systemName: "hand.raised.square").font(.system(size: 24, weight: .light)).foregroundStyle(privacyAccent)
                VStack(alignment: .leading, spacing: 3) {
                    Text("Privacy").font(.system(size: 23, weight: .semibold))
                    Text("Project files, external sources and model requests").font(.caption).foregroundStyle(.secondary)
                }
                Spacer(minLength: 8)
                if history.loading || files.loading { ProgressView().controlSize(.small).accessibilityLabel("Updating privacy history") }
                Button {
                    files.refresh()
                    Task { await history.refresh() }
                } label: { Image(systemName: "arrow.clockwise") }
                    .disabled(history.loading || files.loading || !connected).help("Refresh files and privacy history").accessibilityLabel("Refresh files and privacy history")
                Button(action: close) { Label("Back", systemImage: "arrow.uturn.backward") }
                    .help("Return to conversation").accessibilityLabel("Back to conversation")
            }
            Picker("Privacy view", selection: $tab) {
                ForEach(PrivacyTab.allCases, id: \.self) { tab in
                    Label(tab.rawValue, systemImage: tab == .files ? "folder" : "clock.arrow.circlepath").tag(tab)
                }
            }.pickerStyle(.segmented).frame(maxWidth: 340)
        }.padding(.horizontal, 24).padding(.top, 14).padding(.bottom, 18)
    }

    private var fileBrowser: some View {
        VStack(spacing: 0) {
            VStack(alignment: .leading, spacing: 10) {
                HStack(spacing: 12) {
                    Label("Files & sources", systemImage: "folder").font(.subheadline.weight(.semibold)).lineLimit(1).truncationMode(.middle)
                    Spacer(minLength: 8)
                    searchField("Search files, paths and sources", text: $files.query).frame(maxWidth: 260)
                }
                ViewThatFits(in: .horizontal) {
                    HStack(spacing: 16) { legend }
                    VStack(alignment: .leading, spacing: 6) { legend }
                }
                Text("Past transmission describes captured file versions. Protected now is a separate current restriction; it cannot recall content already sent. Search covers opened folders and loaded source labels.")
                    .font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            }.padding(.horizontal, 24).padding(.vertical, 14)
            if let error = files.error { errorNotice(error) { files.retry() } }
            ScrollView {
                LazyVStack(spacing: 0) {
                    ProtectionEditorView(protections: protections, connected: connected, expanded: $protectionEditorOpen, root: root)
                    ForEach(PrivacySourceGroup.allCases, id: \.self) { group in
                        PrivacySourceGroupView(group: group, sources: files.recordedSources.filter { $0.group == group }, hasProject: files.rootName != nil, query: files.query)
                    }.id(files.sourceContextID)
                    HStack {
                        Label(files.rootName.map { "Project · " + $0 } ?? "Project files", systemImage: "folder")
                            .font(.system(size: 12, weight: .semibold))
                        Spacer()
                    }.padding(.horizontal, 24).padding(.vertical, 12)
                    HStack {
                        Text("NAME").frame(maxWidth: .infinity, alignment: .leading)
                        Text("PAST TRANSMISSION").frame(width: 140, alignment: .leading)
                        Text("EVENTS").frame(width: 52, alignment: .trailing)
                    }.font(.system(size: 9, weight: .semibold)).tracking(0.8).foregroundStyle(.secondary)
                        .padding(.horizontal, 24).padding(.vertical, 9).background(Color.primary.opacity(0.035))
                    Divider()
                    if files.rows.isEmpty && !files.loading {
                        fileEmptyState.padding(.vertical, 40).padding(.horizontal, 24)
                    }
                    ForEach(files.rows) { row in
                        PrivacyFileRowView(row: row, selected: selectedFile == row.id, protected: protections.policy?.protects(path: row.path, root: root) ?? false) {
                            if row.isDirectory { files.toggle(row.id) }
                            else { selectedFile = selectedFile == row.id ? nil : row.id }
                        }
                    }
                    if files.hasMore {
                        Button("Show more files") { files.loadMore() }
                            .font(.callout).padding(.vertical, 16).disabled(files.loading)
                    }
                    if let notice = files.notice {
                        Label(notice, systemImage: "info.circle").font(.caption).foregroundStyle(.secondary)
                            .frame(maxWidth: .infinity, alignment: .leading).padding(18)
                    }
                }.padding(.vertical, 4)
            }
            if let selected = files.rows.first(where: { $0.id == selectedFile }) {
                Divider()
                VStack(alignment: .leading, spacing: 4) {
                    Text(selected.path).font(.system(.caption, design: .monospaced)).textSelection(.enabled)
                    Text(selected.statusDetail).font(.caption).foregroundStyle(.secondary)
                }.frame(maxWidth: .infinity, alignment: .leading).padding(.horizontal, 24).padding(.vertical, 10)
            }
        }
    }

    @ViewBuilder private var fileEmptyState: some View {
        if files.rootName == nil {
            emptyState(icon: "folder", title: "No project folder", detail: "This conversation has no project directory. Recorded file paths, attachments and other context appear in the groups above.")
        } else if !files.query.isEmpty {
            emptyState(icon: "magnifyingglass", title: "No matching loaded files", detail: "Search checks opened folders and recorded paths. Clear the search and expand another folder to include its files.")
        } else {
            emptyState(icon: "folder", title: "No files to show", detail: "Folders load when you expand them. Refresh to check for local changes.")
        }
    }

    @ViewBuilder private var legend: some View {
        legendItem(.red, "Entire file included")
        legendItem(.orange, "Partial or delivery unknown")
        legendItem(.white, "No recorded transmission")
    }
    private func legendItem(_ color: Color, _ title: String) -> some View {
        HStack(spacing: 5) { statusDot(color); Text(title).font(.system(size: 10)) }.foregroundStyle(.secondary)
    }

    private var eventBrowser: some View {
        VStack(spacing: 0) {
            HStack(spacing: 12) {
                Text("\(history.requests.count.formatted()) loaded events · newest first").font(.caption).foregroundStyle(.secondary)
                Spacer(minLength: 8)
                searchField("Search loaded events", text: $eventSearch).frame(maxWidth: 280)
            }.padding(.horizontal, 24).padding(.vertical, 14)
            Divider()
            ScrollView {
                LazyVStack(spacing: 0) {
                    if history.requests.isEmpty && history.hasLoaded {
                        emptyState(icon: "clock", title: "No recorded requests", detail: "An empty history does not prove that no data left this Mac. Older activity and traffic outside recorded model requests may not appear here.")
                            .padding(32)
                    } else if matchingEvents.isEmpty && !eventSearch.isEmpty {
                        emptyState(icon: "magnifyingglass", title: "No matching loaded events", detail: "Try a model, destination or file path, or load earlier records.").padding(32)
                    }
                    ForEach(matchingEvents) { request in PrivacyEventRow(request: request) }
                }
            }
        }
    }
    private var matchingEvents: [PrivacyRequest] {
        let query = eventSearch.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !query.isEmpty else { return history.requests }
        return history.requests.filter { request in
            [request.title, request.model, request.destination, request.statusLabel, request.startedAt].contains { $0.localizedCaseInsensitiveContains(query) } ||
            request.sources.contains { $0.source.localizedCaseInsensitiveContains(query) } ||
            request.unattributed.contains { $0.localizedCaseInsensitiveContains(query) }
        }
    }
    private var footer: some View {
        VStack(alignment: .leading, spacing: 8) {
            if !history.hasLoaded {
                Label(history.loading ? "Loading recorded coverage…" : "Recorded coverage is not available yet.", systemImage: "clock")
                    .font(.caption).foregroundStyle(.secondary)
            } else if history.nextBefore != nil || files.coverageIncomplete {
                HStack(spacing: 10) {
                    Label("Earlier records are not loaded.", systemImage: "clock.arrow.circlepath").font(.caption).foregroundStyle(.secondary)
                    Spacer(minLength: 0)
                    Button("Load earlier records") { Task { await history.older() } }
                        .font(.caption).disabled(history.loading || !connected || history.nextBefore == nil)
                }
            }
            Text("No recorded transmission is not proof that a file stayed local. Attribution may be incomplete; endpoint acceptance does not establish provider retention.")
                .font(.system(size: 10)).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
        }.padding(.horizontal, 24).padding(.vertical, 12)
    }
    private func searchField(_ placeholder: String, text: Binding<String>) -> some View {
        HStack(spacing: 6) {
            Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
            TextField(placeholder, text: text).textFieldStyle(.plain).accessibilityLabel(placeholder)
            if !text.wrappedValue.isEmpty {
                Button { text.wrappedValue = "" } label: { Image(systemName: "xmark.circle.fill").foregroundStyle(.secondary) }
                    .buttonStyle(.plain).accessibilityLabel("Clear search")
            }
        }.font(.caption).padding(.horizontal, 9).padding(.vertical, 7)
            .background(Color.primary.opacity(0.045), in: RoundedRectangle(cornerRadius: 7))
    }
    private func errorNotice(_ text: String, retry: @escaping () -> Void) -> some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: "exclamationmark.circle")
            Text(text).textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading)
            Button("Retry", action: retry).disabled(history.loading || !connected)
        }.font(.caption).padding(.horizontal, 24).padding(.vertical, 10).background(Color.orange.opacity(0.08))
    }
    private func emptyState(icon: String, title: String, detail: String) -> some View {
        VStack(spacing: 10) {
            Image(systemName: icon).font(.system(size: 25, weight: .light)).foregroundStyle(.secondary)
            Text(title).font(.headline)
            Text(detail).font(.callout).foregroundStyle(.secondary).multilineTextAlignment(.center).frame(maxWidth: 420)
        }.frame(maxWidth: .infinity)
    }
}

private struct PrivacyFileRowView: View {
    let row: PrivacyFileRow
    let selected: Bool
    let protected: Bool
    let action: () -> Void
    var body: some View {
        Button(action: action) {
            HStack(spacing: 7) {
                HStack(spacing: 7) {
                    Image(systemName: row.isExpanded ? "chevron.down" : "chevron.right").opacity(row.isDirectory ? 1 : 0)
                        .font(.system(size: 9, weight: .semibold)).foregroundStyle(.secondary).frame(width: 10)
                    Image(systemName: row.isDirectory ? (row.isExpanded ? "folder.fill" : "folder") : "doc.text")
                        .font(.system(size: 12)).foregroundStyle(row.isDirectory || row.state == .none ? Color.secondary : fileColor).frame(width: 15)
                    Text(row.name).font(.system(size: 12)).lineLimit(1).truncationMode(.middle)
                        .foregroundStyle(row.isDirectory || row.state == .none ? Color.primary : fileColor)
                    if protected { Label("Protected now", systemImage: "lock.fill").font(.system(size: 9)).foregroundStyle(.green) }
                }.padding(.leading, CGFloat(min(row.depth, 16)) * 14).frame(maxWidth: .infinity, alignment: .leading)
                HStack(spacing: 6) {
                    if !row.isDirectory { statusDot(fileColor) }
                    Text(row.isDirectory ? "Folder" : status).font(.system(size: 10)).foregroundStyle(.secondary).lineLimit(1)
                }.frame(width: 140, alignment: .leading)
                Text(row.requestCount == 0 ? "—" : row.requestCount.formatted()).font(.system(size: 11, design: .monospaced)).foregroundStyle(.secondary)
                    .frame(width: 52, alignment: .trailing)
            }.padding(.horizontal, 24).frame(height: 29).contentShape(Rectangle())
                .background(selected ? privacyAccent.opacity(0.10) : Color.clear)
        }.buttonStyle(.plain).help(row.path + "\n" + row.statusDetail)
            .accessibilityLabel(row.name + (row.isDirectory ? ", folder" : ", file"))
            .accessibilityValue(accessibilityDescription)
            .contextMenu {
                Button("Copy path") { NSPasteboard.general.clearContents(); NSPasteboard.general.setString(row.path, forType: .string) }
            }
    }
    private var accessibilityDescription: String {
        var parts = [row.statusDetail, "\(row.requestCount) events"]
        if protected { parts.append("Protected now; historical transmission is unchanged") }
        if row.isDirectory { parts.append(row.isExpanded ? "expanded" : "collapsed") }
        return parts.joined(separator: "; ")
    }
    private var fileColor: Color {
        switch row.state { case .entire: return .red; case .partial: return .orange; case .none: return .white }
    }
    private var status: String {
        switch row.state {
        case .entire: return row.isDirectory ? "Contains entire files" : "Entire file"
        case .partial: return row.isDirectory ? "Contains partial / unknown" : "Partial / unknown"
        case .none: return "No record"
        }
    }
}

private func statusDot(_ color: Color) -> some View {
    Circle().fill(color).overlay(Circle().stroke(Color.primary.opacity(0.22), lineWidth: 0.6)).frame(width: 7, height: 7).accessibilityHidden(true)
}

private struct PrivacyEventRow: View {
    let request: PrivacyRequest
    @State private var expanded = false
    @State private var sourceLimit = 40
    var body: some View {
        VStack(spacing: 0) {
            DisclosureGroup(isExpanded: $expanded) {
                if expanded { details.padding(.leading, 18).padding(.top, 10).padding(.bottom, 8) }
            } label: {
                VStack(alignment: .leading, spacing: 5) {
                    HStack(spacing: 10) {
                        Text(request.title).font(.system(size: 12, weight: .semibold))
                        Spacer(minLength: 0)
                        Text(timestamp).font(.system(size: 10)).foregroundStyle(.secondary)
                    }
                    HStack(spacing: 8) {
                        Text(request.destination).font(.system(size: 11, design: .monospaced)).lineLimit(1).truncationMode(.middle)
                        Spacer(minLength: 0)
                        Text("\(request.sources.count) sources · \(request.requestBytes.formatted()) B").font(.system(size: 10)).foregroundStyle(.secondary)
                    }
                    Label(request.statusLabel, systemImage: statusIcon).font(.system(size: 10))
                        .foregroundStyle(request.status == "accepted" ? Color.secondary : .orange)
                }.padding(.vertical, 10)
            }.padding(.horizontal, 24)
            Divider().padding(.leading, 24)
        }
    }
    private var details: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(alignment: .top, spacing: 24) {
                detailValue("MODEL", request.model)
                detailValue("REQUEST SIZE", "\(request.requestBytes.formatted()) bytes")
            }
            detailValue("DESTINATION", request.destination)
            VStack(alignment: .leading, spacing: 6) {
                Text("INCLUDED SOURCES").font(.system(size: 9, weight: .semibold)).tracking(0.7).foregroundStyle(.secondary)
                if request.sources.isEmpty {
                    Text("No individually attributed files. Other context may still contain file data.").font(.caption).foregroundStyle(.secondary)
                }
                ForEach(Array(request.sources.prefix(sourceLimit).enumerated()), id: \.offset) { _, source in
                    HStack(alignment: .top, spacing: 8) {
                        Image(systemName: "doc.text").foregroundStyle(.secondary)
                        VStack(alignment: .leading, spacing: 3) {
                            Text(source.source).font(.system(.caption, design: .monospaced)).textSelection(.enabled)
                            Text(source.kind.replacingOccurrences(of: "_", with: " ") + " · \(source.contentBytes.formatted()) content bytes" + (source.partial ? " · excerpt / partial context" : ""))
                                .font(.system(size: 10)).foregroundStyle(.secondary)
                        }
                    }.font(.caption).padding(.vertical, 3)
                }
                if request.sources.count > sourceLimit {
                    Button("Show more sources (\(request.sources.count - sourceLimit) remaining)") { sourceLimit += 40 }.font(.caption)
                }
            }
            if !request.unattributed.isEmpty {
                detailValue("OTHER CONTEXT INCLUDED", request.unattributed.joined(separator: " · "))
            }
            Text("Records contain attribution metadata, not the request body or file contents.").font(.system(size: 10)).foregroundStyle(.secondary)
        }.frame(maxWidth: .infinity, alignment: .leading)
    }
    private func detailValue(_ label: String, _ value: String) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(label).font(.system(size: 9, weight: .semibold)).tracking(0.7).foregroundStyle(.secondary)
            Text(value).font(.caption).textSelection(.enabled)
        }
    }
    private var statusIcon: String {
        switch request.status { case "accepted": return "checkmark.circle"; case "rejected", "connection_error": return "exclamationmark.circle"; default: return "clock" }
    }
    private var timestamp: String {
        let date = PrivacyDateFormat.fractional.date(from: request.startedAt) ?? PrivacyDateFormat.standard.date(from: request.startedAt)
        return date?.formatted(date: .abbreviated, time: .standard) ?? request.startedAt
    }
}
private enum PrivacyDateFormat {
    static let fractional: ISO8601DateFormatter = {
        let formatter = ISO8601DateFormatter(); formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]; return formatter
    }()
    static let standard = ISO8601DateFormatter()
}
