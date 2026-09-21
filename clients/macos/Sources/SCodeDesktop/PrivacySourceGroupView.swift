import SwiftUI
import DesktopCore

struct PrivacySourceGroupView: View {
    let group: PrivacySourceGroup
    let sources: [PrivacyRecordedSource]
    let hasProject: Bool
    let query: String
    @State private var expanded = false
    @State private var page = 0
    private var visible: (items: [PrivacyRecordedSource], index: Int, count: Int, total: Int) {
        PrivacyRecordedSources.page(sources, index: page, query: query)
    }
    private var disclosure: Binding<Bool> {
        Binding(get: { expanded || !query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }, set: { expanded = $0 })
    }
    private var eventCount: Int { Set(sources.flatMap { $0.requests }).count }
    var body: some View {
        if !sources.isEmpty {
            DisclosureGroup(isExpanded: disclosure) {
                VStack(alignment: .leading, spacing: 10) {
                    Text(explanation).font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                    if visible.total == 0 { Text("No matching recorded sources.").font(.caption).foregroundStyle(.secondary) }
                    ForEach(visible.items) { source in PrivacyRecordedSourceView(source: source) }
                    if visible.count > 1 {
                        HStack {
                            Button("Previous sources") { page = max(0, visible.index - 1) }.disabled(visible.index == 0)
                            Spacer()
                            Text("Page \(visible.index + 1) of \(visible.count) · \(visible.total) labels").foregroundStyle(.secondary)
                            Spacer()
                            Button("Next sources") { page = visible.index + 1 }.disabled(visible.index + 1 == visible.count)
                        }.font(.caption)
                    }
                }.padding(.leading, 16).padding(.top, 10).padding(.bottom, 6)
            } label: {
                HStack(spacing: 12) {
                    Label(group.title(hasProject: hasProject), systemImage: icon).font(.system(size: 12, weight: .semibold))
                    Spacer(minLength: 4)
                    Text("\(sources.count) labels · \(eventCount) \(eventCount == 1 ? "event" : "events")").font(.caption).foregroundStyle(.secondary)
                }
            }
            .padding(.horizontal, 24).padding(.vertical, 12)
            .onChange(of: query) { _, _ in page = 0 }
            Divider()
        }
    }
    private var icon: String {
        switch group { case .external: return "folder.badge.questionmark"; case .attachment: return "paperclip"; case .other: return "text.alignleft" }
    }
    private var explanation: String {
        switch group {
        case .external: return "Recorded paths only. These folders are not scanned. Expand a label to see the full path and receiving endpoints."
        case .attachment: return "Attachment names do not establish a local path. The same name may represent different files; counts summarize recorded labels."
        case .other: return "Remote sources, unresolved paths and other included context. Individual file locations cannot always be established."
        }
    }
}
private struct PrivacyRecordedSourceView: View {
    let source: PrivacyRecordedSource
    @State private var expanded = false
    private var color: Color { source.state == .entire ? .red : source.state == .partial ? .orange : .secondary }
    private var displayName: String {
        guard source.group == .external else { return source.source }
        return source.displayPath.split(separator: "/").last.map(String.init) ?? source.source
    }
    private var endpointSummary: String {
        let shown = source.destinations.sorted().prefix(20).joined(separator: " · ")
        let more = source.destinations.count > 20 ? " · Showing 20 of \(source.destinations.count); see Event record for every request destination." : ""
        return "Endpoints: " + shown + more
    }
    var body: some View {
        DisclosureGroup(isExpanded: $expanded) {
            if expanded {
                VStack(alignment: .leading, spacing: 7) {
                    Text(source.displayPath).font(.system(.caption, design: .monospaced)).textSelection(.enabled)
                    if source.displayPath != source.source { Text("Recorded label: " + source.source).textSelection(.enabled) }
                    Text(source.kind.replacingOccurrences(of: "_", with: " ") + " · " + source.statusDetail)
                    Text("\(source.requests.count) recorded events")
                    Text(endpointSummary).textSelection(.enabled)
                    Text(source.outcomes.sorted().joined(separator: " · "))
                }.font(.caption).foregroundStyle(.secondary).frame(maxWidth: .infinity, alignment: .leading).padding(.vertical, 6)
            }
        } label: {
            HStack(alignment: .top, spacing: 8) {
                Image(systemName: source.hasContent ? "doc.fill" : "info.circle").foregroundStyle(color)
                VStack(alignment: .leading, spacing: 3) {
                    Text(displayName).font(.system(size: 12)).lineLimit(1).truncationMode(.middle)
                    Text(source.group == .external ? source.displayPath : source.statusDetail)
                        .font(.system(size: 10)).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle)
                }.frame(maxWidth: .infinity, alignment: .leading)
                Text("\(source.requests.count)").font(.caption.monospacedDigit()).foregroundStyle(.secondary)
            }.help(source.source + "\n" + source.statusDetail)
        }.padding(.vertical, 4)
    }
}
