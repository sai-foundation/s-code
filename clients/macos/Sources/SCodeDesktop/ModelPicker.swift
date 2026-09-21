import SwiftUI
import DesktopCore

struct ModelPicker: View {
    @EnvironmentObject private var store: AppStore
    @State private var open = false
    @State private var search = ""
    @FocusState private var searchFocused: Bool
    private var current: String { store.models.state?.current ?? store.selected?.model ?? "Model unavailable" }
    private var provider: String { store.profile?.name ?? "Configured provider" }
    private var options: [ModelOption] {
        (store.models.state?.options ?? []).filter { $0.matches(search.trimmingCharacters(in: .whitespacesAndNewlines), configuredProvider: provider) }
    }
    var body: some View {
        Button { open.toggle() } label: {
            HStack(spacing: 5) {
                Image(systemName: "sparkles")
                VStack(alignment: .leading, spacing: 1) {
                    Text(store.models.saving ? "Saving model…" : store.models.state == nil ? (store.models.loading ? "Loading model…" : "Model unavailable") : current).lineLimit(1).truncationMode(.middle)
                    Text(provider).font(.caption2).foregroundStyle(.secondary).lineLimit(1)
                }
                Image(systemName: "chevron.down").font(.system(size: 8, weight: .semibold))
            }.font(.caption)
        }
        .frame(maxWidth: 220, alignment: .leading).fixedSize(horizontal: true, vertical: false)
        .buttonStyle(.bordered)
        .disabled(!store.connected || store.selectedID == nil)
        .accessibilityLabel("Model")
        .accessibilityValue("\(current), \(provider)")
        .help("Choose a model · \(provider)")
        .popover(isPresented: $open, arrowEdge: .top) {
            VStack(alignment: .leading, spacing: 12) {
                Text("Choose a model").font(.headline)
                Text("\(provider) · Saved for this conversation").font(.caption).foregroundStyle(.secondary)
                TextField("Search model or provider", text: $search).textFieldStyle(.roundedBorder).focused($searchFocused)
                    .accessibilityLabel("Search model or provider")
                ScrollView {
                    LazyVStack(spacing: 4) {
                        ForEach(options) { model in
                            Button { store.setModel(model.id) } label: {
                                HStack(alignment: .top, spacing: 10) {
                                    VStack(alignment: .leading, spacing: 4) {
                                        Text(model.name).fontWeight(.medium)
                                        Text("\(model.provider == "configured" ? provider : model.provider) · \(model.recommended ? "Default" : model.source == "session_history" ? "Used in conversations" : "Available model")")
                                            .font(.caption).foregroundStyle(.secondary)
                                        if model.name != model.id { Text(model.id).font(.caption2).foregroundStyle(.secondary) }
                                        if !model.selectable { Text(model.lockedReason ?? "Unavailable from this provider").font(.caption).foregroundStyle(.secondary) }
                                    }.fixedSize(horizontal: false, vertical: true)
                                    Spacer(minLength: 0)
                                    if current == model.id { Image(systemName: "checkmark").foregroundStyle(.orange) }
                                }.padding(9).frame(maxWidth: .infinity, alignment: .leading)
                                    .background(current == model.id ? Color.orange.opacity(0.10) : .clear, in: RoundedRectangle(cornerRadius: 8))
                            }.buttonStyle(.plain)
                                .disabled(!store.configurationIdle || !store.permissionsReady || !store.models.ready || !model.selectable)
                        }
                        if options.isEmpty && !store.models.loading { Text(search.isEmpty ? "No models are available in the server catalog." : "No matching models.").font(.callout).foregroundStyle(.secondary).padding(.vertical, 12) }
                    }
                }.frame(maxHeight: 280)
                Text("The server lists configured and previously used models. Availability follows its security policy.").font(.caption).foregroundStyle(.secondary)
                if !store.configurationIdle { Text("Stop or finish the current task and resolve pending requests before changing models.").font(.caption).foregroundStyle(.secondary) }
                if let error = store.models.error { Text(error).font(.caption).foregroundStyle(.red) }
                HStack {
                    Button("Refresh models") { Task { await store.models.refresh() } }.disabled(store.models.loading || store.models.saving)
                    Spacer()
                    if store.models.loading || store.models.saving { ProgressView().controlSize(.small) }
                }
            }.padding(18).frame(width: 370)
                .onAppear { searchFocused = true }
        }
        .onChange(of: store.selectedID) { _, _ in open = false; search = "" }
        .onChange(of: store.profile?.id) { _, _ in open = false; search = "" }
    }
}
