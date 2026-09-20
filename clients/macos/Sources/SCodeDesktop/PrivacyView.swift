import SwiftUI
import DesktopCore

struct PrivacyView: View {
    @ObservedObject var history: PrivacyHistory
    let connected: Bool
    let close: () -> Void
    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 10) {
                Image(systemName: "hand.raised.square").foregroundStyle(.secondary)
                Text("Privacy history").font(.headline)
                Spacer()
                if history.loading { ProgressView().controlSize(.small).accessibilityLabel("Loading privacy history") }
                Button { Task { await history.refresh() } } label: { Image(systemName: "arrow.clockwise") }
                    .disabled(history.loading || !connected).help("Refresh privacy history").accessibilityLabel("Refresh privacy history")
                Button(action: close) { Image(systemName: "xmark") }.help("Close privacy history").accessibilityLabel("Close privacy history")
            }.buttonStyle(.borderless).padding(18)
            Divider()
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 16) {
                    Text("Model requests for this conversation").font(.subheadline.weight(.medium))
                    Text("Stored locally by your engine. Records show destinations and identified sources, without storing file contents or request bodies in this view.")
                        .font(.caption).foregroundStyle(.secondary)
                    if !connected {
                        Label("Reconnect to load privacy history.", systemImage: "wifi.slash").font(.callout).foregroundStyle(.secondary)
                    }
                    if let error = history.error {
                        VStack(alignment: .leading, spacing: 8) {
                            Label(error, systemImage: "exclamationmark.circle").font(.callout).textSelection(.enabled)
                            Button("Retry") { Task { await history.retry() } }.disabled(history.loading || !connected)
                        }.padding(12).frame(maxWidth: .infinity, alignment: .leading).background(Color.orange.opacity(0.08), in: RoundedRectangle(cornerRadius: 10))
                    }
                    if history.requests.isEmpty && history.hasLoaded {
                        VStack(alignment: .leading, spacing: 8) {
                            Label("No recorded requests", systemImage: "clock").font(.headline)
                            Text("An empty history does not prove that no data left this Mac. Older activity and traffic outside recorded model requests may not appear here.")
                                .font(.callout).foregroundStyle(.secondary)
                        }.padding(.vertical, 12)
                    }
                    if !history.requests.isEmpty {
                        sourceOverview
                        Text("\(history.requests.count.formatted()) loaded requests · newest first").font(.caption).foregroundStyle(.secondary)
                        ForEach(history.requests) { request in PrivacyRequestCard(request: request) }
                    }
                    if history.nextBefore != nil {
                        Button("Load older requests") { Task { await history.older() } }
                            .disabled(history.loading || !connected).frame(maxWidth: .infinity)
                    }
                    Divider()
                    Text("Delivery status does not establish provider retention. File attribution can be incomplete; other context may contain file data. This history is not a complete network audit.")
                        .font(.caption).foregroundStyle(.secondary)
                }.padding(18)
            }
        }.background(.background).accessibilityElement(children: .contain).accessibilityLabel("Privacy history")
    }
    private var sourceOverview: some View {
        let sources = Dictionary(grouping: history.requests.flatMap { request in request.sources.map { ($0.source, request) } }, by: { $0.0 })
        return DisclosureGroup("\(sources.count) identified sources · loaded requests") {
            VStack(alignment: .leading, spacing: 12) {
                if sources.isEmpty { Text("No individually attributed files. Other context may still contain file data.").foregroundStyle(.secondary) }
                ForEach(sources.keys.sorted(), id: \.self) { source in
                    let requests = sources[source, default: []].map { $0.1 }
                    VStack(alignment: .leading, spacing: 4) {
                        Text(source).font(.system(.caption, design: .monospaced)).textSelection(.enabled)
                        Text("\(Set(requests.map(\.id)).count) requests · \(Set(requests.map(\.destination)).sorted().joined(separator: ", "))").foregroundStyle(.secondary).textSelection(.enabled)
                        Text(Set(requests.map(\.statusLabel)).sorted().joined(separator: " · ")).foregroundStyle(.secondary)
                    }
                }
            }.font(.caption).padding(.top, 8).frame(maxWidth: .infinity, alignment: .leading)
        }.font(.caption.weight(.medium))
    }
}

private struct PrivacyRequestCard: View {
    let request: PrivacyRequest
    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(request.title).font(.subheadline.weight(.semibold))
            Text(timestamp).font(.caption).foregroundStyle(.secondary)
            Label(request.statusLabel, systemImage: statusIcon).font(.caption).foregroundStyle(request.status == "accepted" ? Color.secondary : Color.orange)
            VStack(alignment: .leading, spacing: 4) {
                Text(request.model).font(.system(.callout, design: .monospaced))
                Text(request.destination).font(.caption).foregroundStyle(.secondary)
            }.textSelection(.enabled)
            Text("\(request.requestBytes.formatted()) request bytes").font(.caption).foregroundStyle(.secondary)
            DisclosureGroup("Sources and other context") {
                VStack(alignment: .leading, spacing: 12) {
                    if request.sources.isEmpty {
                        Text("No individually attributed files in this request. Other context may still contain file data.").foregroundStyle(.secondary)
                    }
                    ForEach(Array(request.sources.enumerated()), id: \.offset) { _, source in
                        VStack(alignment: .leading, spacing: 4) {
                            Text(source.source).font(.system(.caption, design: .monospaced)).textSelection(.enabled)
                            Text(source.kind.replacingOccurrences(of: "_", with: " ") + " · \(source.contentBytes.formatted()) content bytes" + (source.partial ? " · excerpt / partial context" : "")).foregroundStyle(.secondary)
                        }
                    }
                    if !request.unattributed.isEmpty {
                        Text("Other context included").fontWeight(.medium)
                        Text(request.unattributed.joined(separator: " · ")).foregroundStyle(.secondary).textSelection(.enabled)
                    }
                }.padding(.top, 8).frame(maxWidth: .infinity, alignment: .leading)
            }.font(.caption)
        }.padding(14).frame(maxWidth: .infinity, alignment: .leading)
            .background(Color.primary.opacity(0.035), in: RoundedRectangle(cornerRadius: 12))
    }
    private var statusIcon: String {
        switch request.status {
        case "accepted": return "checkmark.circle"
        case "rejected", "connection_error": return "exclamationmark.circle"
        default: return "clock"
        }
    }
    private var timestamp: String {
        let formatter = ISO8601DateFormatter(); formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        let date = formatter.date(from: request.startedAt) ?? ISO8601DateFormatter().date(from: request.startedAt)
        return date?.formatted(date: .abbreviated, time: .standard) ?? request.startedAt
    }
}
