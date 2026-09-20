import SwiftUI
import DesktopCore
import Security

struct ConnectionsView: View {
    @EnvironmentObject var store: AppStore
    @State private var editing = Profile()
    @State private var chosenID = "new"
    @State private var preset = "SAI"
    @State private var key = ""
    @State private var models: [String] = []
    @State private var filter = ""
    @State private var discovering = false
    @State private var discoveryEpoch = UUID()
    @State private var message: String?
    private let providers: [(String, String, String)] = [
        ("SAI", "openai_compatible", "https://api.sai.foundation/v1"),
        ("OpenAI", "openai_compatible", "https://api.openai.com/v1"),
        ("Claude", "anthropic", "https://api.anthropic.com/v1"),
        ("Gemini", "gemini", "https://generativelanguage.googleapis.com/v1beta"),
        ("DeepSeek", "openai_compatible", "https://api.deepseek.com/v1"),
        ("OpenRouter", "openai_compatible", "https://openrouter.ai/api/v1"),
        ("Local", "openai_compatible", "http://127.0.0.1:11434/v1"),
        ("Custom", "openai_compatible", "")
    ]
    private var savedCredentialMatches: Bool {
        store.profiles.first(where: { $0.id == editing.id }).map { editing.canReuseCredential(from: $0) } ?? false
    }
    private var filtered: [String] { models.filter { filter.isEmpty || $0.localizedCaseInsensitiveContains(filter) } }
    var body: some View {
        VStack(alignment: .leading, spacing: 20) {
            HStack(alignment: .top) {
                VStack(alignment: .leading, spacing: 6) {
                    Text("Make yourself at home.").font(.system(size: 27, weight: .semibold, design: .rounded))
                    Text("Bring a model. Keep your workflow.").foregroundStyle(.secondary)
                }
                Spacer()
                Button { store.settingsOpen = false } label: { Image(systemName: "xmark.circle.fill").font(.title3).foregroundStyle(.secondary) }.buttonStyle(.plain).keyboardShortcut(.cancelAction).accessibilityLabel("Close connections")
            }
            if !store.profiles.isEmpty {
                Picker("Connection", selection: $chosenID) {
                    Text("New connection").tag("new")
                    ForEach(store.profiles) { p in Text(p.name).tag(p.id) }
                }.onChange(of: chosenID) { _, id in
                    discoveryEpoch = UUID(); discovering = false; models = []; key = ""; message = nil
                    editing = store.profiles.first(where: { $0.id == id }) ?? Profile()
                    preset = providers.first(where: { $0.2 == editing.endpoint })?.0 ?? "Custom"
                }
            }
            HStack(spacing: 12) {
                Label("Connect", systemImage: "1.circle.fill").foregroundStyle(.orange)
                Image(systemName: "chevron.right").foregroundStyle(.tertiary)
                Label("Choose a model", systemImage: "2.circle").foregroundStyle(.secondary)
                Spacer()
                Image(systemName: "lock.shield").foregroundStyle(.secondary)
            }.font(.callout)
            Grid(alignment: .leading, horizontalSpacing: 18, verticalSpacing: 13) {
                GridRow {
                    Text("Provider").foregroundStyle(.secondary)
                    Picker("Provider", selection: $preset) { ForEach(providers, id: \.0) { value in Text(value.0).tag(value.0) } }.labelsHidden()
                        .onChange(of: preset) { _, value in
                            guard value != "Custom", let selected = providers.first(where: { $0.0 == value }) else { return }
                            if selected.2 != editing.endpoint {
                                discoveryEpoch = UUID(); discovering = false; models = []
                                editing.name = value; editing.provider = selected.1; editing.endpoint = selected.2; editing.model = ""; key = ""
                            }
                        }
                }
                GridRow { Text("Name").foregroundStyle(.secondary); TextField("Connection name", text: $editing.name) }
                GridRow { Text("Endpoint").foregroundStyle(.secondary); TextField("https://…/v1", text: $editing.endpoint) }
                if preset == "Custom" {
                    GridRow { Text("API format").foregroundStyle(.secondary); Picker("API format", selection: $editing.provider) { Text("OpenAI compatible").tag("openai_compatible"); Text("Anthropic").tag("anthropic"); Text("Gemini").tag("gemini") }.labelsHidden() }
                }
                GridRow { Text("API key").foregroundStyle(.secondary); SecureField(savedCredentialMatches ? "Leave blank to keep the saved key" : "Paste your key", text: $key) }
            }.textFieldStyle(.roundedBorder)
            HStack {
                Text("Keys are stored in macOS Keychain.").font(.caption).foregroundStyle(.secondary)
                Spacer()
                if preset == "SAI" { Link("Get an API key ↗", destination: URL(string: "https://api.sai.foundation/")!).font(.caption) }
            }
            Divider()
            HStack {
                Text("Model").font(.headline)
                Spacer()
                if discovering { ProgressView().controlSize(.small) }
                Button("Find models") { discover() }.disabled(discovering || editing.endpoint.isEmpty)
            }
            TextField("Exact model ID", text: $editing.model).textFieldStyle(.roundedBorder)
            if !models.isEmpty {
                TextField("Filter \(models.count.formatted()) models…", text: $filter).textFieldStyle(.roundedBorder)
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 2) {
                        ForEach(Array(filtered.prefix(100)), id: \.self) { id in
                            Button { editing.model = id } label: {
                                HStack { Image(systemName: editing.model == id ? "checkmark.circle.fill" : "circle"); Text(id).lineLimit(1); Spacer() }.font(.callout).padding(7).contentShape(Rectangle())
                            }.buttonStyle(.plain)
                        }
                    }
                }.frame(height: 145)
                Text("\(filtered.count.formatted()) matches · showing up to 100. Narrow the filter or enter an exact ID.").font(.caption).foregroundStyle(.secondary)
            } else { Text("Find available models, or enter the exact ID from your provider.").font(.caption).foregroundStyle(.secondary) }
            if let message { Text(message).font(.callout).foregroundStyle(.orange).textSelection(.enabled) }
            Text("Use New connection for a different account. Updating a saved connection keeps its conversations and workspaces.").font(.caption).foregroundStyle(.secondary)
            HStack {
                Spacer()
                Button("Save and connect") {
                    do {
                        let saved = savedCredentialMatches ? (try CredentialStore.read(editing.credentialID) ?? "") : ""
                        let entered = key.trimmingCharacters(in: .whitespacesAndNewlines)
                        if entered.isEmpty && saved.isEmpty && !["localhost", "127.0.0.1", "::1"].contains(URL(string: editing.endpoint)?.host ?? "") { message = "Enter an API key for this connection."; return }
                        try store.saveProfile(editing, key: entered)
                    } catch { message = error.localizedDescription }
                }.buttonStyle(.borderedProminent).controlSize(.large).disabled(editing.model.isEmpty || store.connecting || discovering)
            }
        }.padding(30).frame(width: 570)
        .onAppear {
            if let p = store.profile { editing = p; chosenID = p.id; preset = providers.first(where: { $0.2 == p.endpoint })?.0 ?? "Custom" }
        }
        .onChange(of: editing.endpoint) { _, _ in invalidateDiscovery() }
        .onChange(of: editing.provider) { _, _ in invalidateDiscovery() }
        .onChange(of: key) { _, _ in invalidateDiscovery() }
        .onDisappear { discoveryEpoch = UUID() }
    }
    private func invalidateDiscovery() { discoveryEpoch = UUID(); discovering = false; models = []; message = nil }
    private func discover() {
        let epoch = UUID(); discoveryEpoch = epoch; discovering = true; message = nil
        let endpoint = editing.endpoint, provider = editing.provider, id = editing.credentialID, enteredKey = key
        let mayReuse = savedCredentialMatches
        Task {
            do {
                let credential = enteredKey.isEmpty ? (mayReuse ? (try CredentialStore.read(id) ?? "") : "") : enteredKey.trimmingCharacters(in: .whitespacesAndNewlines)
                if credential.isEmpty && !["localhost", "127.0.0.1", "::1"].contains(URL(string: endpoint)?.host ?? "") { throw DesktopError.keychain(errSecItemNotFound) }
                let found = try await ProviderDiscovery.models(endpoint: endpoint, provider: provider, key: credential)
                guard epoch == discoveryEpoch else { return }
                models = found; filter = ""
                if found.isEmpty { message = "This endpoint returned no models. Check your account access, or enter an exact model ID." }
                else if editing.model.isEmpty { editing.model = found[0] }
            } catch { if epoch == discoveryEpoch { message = error.localizedDescription } }
            if epoch == discoveryEpoch { discovering = false }
        }
    }
}
