import AppKit
import Combine
import Security
import DesktopCore
import Foundation

@MainActor final class AppStore: ObservableObject {
    @Published var theme: DesktopTheme = .system
    private let themePreferences = ThemePreferences()
    func setTheme(_ value: DesktopTheme) {
        themePreferences.save(value, profileID: profile?.id)
        theme = value
    }
    @Published var profiles: [Profile] = []
    @Published var profile: Profile?
    @Published var sessions: [Conversation] = []
    @Published var selectedID: String?
    @Published var transcript = TranscriptState()
    @Published var taskProvenance = TaskProvenance()
    @Published var approvals: [JSON] = []
    @Published var questions: [JSON] = []
    @Published var pendingInputs: [TurnInputRecord] = []
    @Published var inputNotice: String?
    @Published var inputError: String?
    @Published var inputRemoving: Set<String> = []
    @Published private var inputAttempts = TurnInputAttempts()
    private var inputReads = TurnInputReadGate()
    @Published var running: Set<String> = []
    @Published var needsAttention: Set<String> = []
    @Published var busyRequests: Set<String> = []
    @Published var draft = ""
    @Published var composerFocusID = UUID()
    @Published var status = "Welcome"
    @Published var connected = false
    @Published var connecting = false
    @Published var submitting = false
    @Published var loadingHistory = false
    @Published var nextCursor: String?
    @Published var error: String?
    let models = SessionModels()
    let protections = PrivacyProtections()
    let privacy = PrivacyHistory()
    let privacyFiles = PrivacyFileTree()
    @Published var protectionEditorOpen = false
    @Published var privacyOpen = false { didSet { configurePrivacy(); if privacyOpen { Task { await protections.refresh() } } else { protectionEditorOpen = false } } }
    @Published var settingsOpen = false
    @Published var detail: Detail?
    @Published var workingDiff: DiffDetail?
    @Published var permissions: SessionPermissions?
    @Published var permissionsLoading = false
    @Published private var permissionChanges = PendingPermissionChanges()
    @Published var permissionsError: String?
    private var modelObservation: AnyCancellable?
    private var protectionObservation: AnyCancellable?
    private var permissionRequestID = UUID()
    @Published var usage = 0
    @Published var turnFeedback: String?
    @Published var activity = SessionActivity()
    @Published var followLatest = true
    @Published var renderTick = 0
    let repository = ProfileRepository()
    let engine = Engine()
    private var api: APIClient?
    private var streamTask: Task<Void, Never>?
    private var flushTask: Task<Void, Never>?
    private var snapshotTask: Task<Void, Never>?
    private var connectTask: Task<Void, Never>?
    private var pendingEvents: [JSON] = []
    private var sessionsRequestID = UUID()
    private var profileEpoch = UUID()
    private var streamEpoch = UUID()
    private var selectionEpoch = UUID()
    private var drafts: [String: String] = [:]
    private var eventCursor = 0
    private var sessionActivity: [String: SessionActivity] = [:]
    private var snapshotRequestID = UUID()
    private var snapshotRefresh = SnapshotRefreshState()
    struct DiffDetail: Identifiable { let id = UUID(); let diff: UnifiedDiff }
    struct Detail: Identifiable { let id = UUID(); let title: String; let text: String }
    var selected: Conversation? { sessions.first { $0.id == selectedID } }
    var permissionsSaving: Bool {
        guard let api, let id = selectedID else { return false }
        return permissionChanges.contains(actor: api.scope.actor, session: id)
    }
    var permissionsReady: Bool { permissions?.sessionID == selectedID && transcript.sessionID == selectedID && permissions != nil && !permissionsLoading && !permissionsSaving }
    var composerReady: Bool { permissionsReady && models.allowsSubmission(localProtectionCommand: draftIsProtectionCommand) }
    var configurationIdle: Bool { connected && !turnRunning && !submitting && approvals.isEmpty && questions.isEmpty && pendingInputs.isEmpty }
    private var inputContext: TurnInputContext? {
        guard let profile, let api else { return nil }
        return TurnInputContext(profile: profile, scope: api.scope)
    }
    var hasUnconfirmedInput: Bool {
        guard let inputContext, let selectedID else { return false }
        return inputAttempts.hasUnconfirmed(context: inputContext, session: selectedID, content: draft.trimmingCharacters(in: .whitespacesAndNewlines))
    }
    var supportsTurnInput: Bool { engine.capabilities.contains("turn.input_queue.v1") }
    var inputTarget: JSON? { selectedID.flatMap { TurnInputTarget.active(turns: activity.turns, sessionID: $0) } }
    var canQueueInput: Bool { connected && supportsTurnInput && inputTarget != nil && composerReady && !submitting }
    var canSteerInput: Bool { canQueueInput && inputTarget.map(TurnInputTarget.canSteer) == true && approvals.isEmpty && questions.isEmpty }
    var canSubmitDraft: Bool {
        connected && composerReady && !submitting && !draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty &&
        (draftIsProtectionCommand || (turnRunning ? canQueueInput : pendingInputs.isEmpty && !hasUnconfirmedInput))
    }
    var draftIsProtectionCommand: Bool { ProtectionCommand.recognizes(draft) }
    var turnRunning: Bool { selectedID.map { running.contains($0) } ?? false }
    init() {
        theme = themePreferences.load(profileID: UserDefaults.standard.string(forKey: "activeProfile"))
        modelObservation = models.objectWillChange.sink { [weak self] _ in self?.objectWillChange.send() }
        protectionObservation = protections.objectWillChange.sink { [weak self] _ in self?.objectWillChange.send() }
        do { profiles = try repository.load() } catch { self.error = error.localizedDescription }
        engine.onExit = { [weak self] in
            guard let self else { return }
            self.connected = false; self.status = "Engine stopped"; self.privacy.reset(); self.privacyFiles.reset(); self.protections.reset(); self.models.reset()
            self.streamTask?.cancel(); self.flushTask?.cancel(); self.flushTask = nil
            self.error = "The local engine stopped unexpectedly. Reconnect to restore your conversations."
        }
    }
    func startSaved() {
        guard !connecting, !connected else { return }
        let id = UserDefaults.standard.string(forKey: "activeProfile")
        if let p = profiles.first(where: { $0.id == id }) ?? profiles.first { connect(p) }
        else { settingsOpen = true }
    }
    func confirmLeavingTasks() -> Bool {
        guard !running.isEmpty else { return true }
        let alert = NSAlert(); alert.messageText = "Stop active tasks?"
        alert.informativeText = "There are \(running.count) conversations still running. Switching connection or quitting stops their local engine."
        alert.addButton(withTitle: "Stop and continue"); alert.addButton(withTitle: "Keep working")
        return alert.runModal() == .alertFirstButtonReturn
    }
    func connect(_ target: Profile) {
        guard !connecting, confirmLeavingTasks() else { return }
        connectTask?.cancel(); streamTask?.cancel(); snapshotTask?.cancel(); flushTask?.cancel()
        snapshotTask = nil; flushTask = nil; snapshotRequestID = UUID(); snapshotRefresh = SnapshotRefreshState()
        profileEpoch = UUID(); selectionEpoch = UUID(); resetInputs(); resetPermissions(); models.reset(); privacy.reset(); privacyFiles.reset(); protections.reset()
        let epoch = profileEpoch
        if let id = selectedID { drafts[id] = draft }
        api = nil; connected = false; connecting = true; profile = target; theme = themePreferences.load(profileID: target.id); status = "Starting engine…"
        selectedID = nil; sessions = []; sessionActivity = [:]; activity = SessionActivity(); running = []; needsAttention = []; approvals = []; questions = []
        transcript = TranscriptState(); taskProvenance = TaskProvenance(); turnFeedback = nil; draft = ""; submitting = false; loadingHistory = false; busyRequests = []; eventCursor = 0; pendingEvents = []; error = nil
        connectTask = Task {
            do {
                let key = try CredentialStore.read(target.credentialID) ?? ""
                guard !key.isEmpty || ["localhost", "127.0.0.1", "::1"].contains(URL(string: target.endpoint)?.host ?? "") else { throw DesktopError.keychain(errSecItemNotFound) }
                let executable = Bundle.main.bundleURL.appendingPathComponent("Contents/Helpers/s-code-daemon")
                let client = try await engine.start(profile: target, key: key, repository: repository, executable: executable)
                guard epoch == profileEpoch, !Task.isCancelled else { return }
                api = client; connected = true; status = "Ready"
                UserDefaults.standard.set(target.id, forKey: "activeProfile")
                try await refreshSessions()
                guard epoch == profileEpoch else { return }
                if let first = sessions.first { await select(first.id) }
                startStream(epoch: epoch)
            } catch { if epoch == profileEpoch, !Task.isCancelled { self.error = error.localizedDescription; status = "Connection needs attention" } }
            if epoch == profileEpoch { connecting = false }
        }
    }
    func saveProfile(_ value: Profile, key: String) throws {
        try value.validate()
        if let previous = profiles.first(where: { $0.id == value.id }), !value.canReuseCredential(from: previous), key.isEmpty { throw DesktopError.keychain(errSecItemNotFound) }
        if !key.isEmpty { try CredentialStore.save(key, id: value.credentialID) }
        var updated = profiles
        if let index = updated.firstIndex(where: { $0.id == value.id }) { updated[index] = value } else { updated.append(value) }
        try repository.save(updated); profiles = updated
        settingsOpen = false; connect(value)
    }
    func refreshSessions() async throws {
        guard let api else { return }; let epoch = profileEpoch, requestID = UUID()
        sessionsRequestID = requestID
        let values = try await api.request("/v1/sessions")
        guard epoch == profileEpoch, requestID == sessionsRequestID else { return }
        sessions = values.array.filter { api.scope.owns($0) && $0["status"].string != "deleted" && $0["status"].string != "archived" }.map(Conversation.init)
    }
    func select(_ id: String) async {
        guard let api, sessions.contains(where: { $0.id == id }) else { return }
        if let previous = selectedID { drafts[previous] = draft }
        streamTask?.cancel(); flushTask?.cancel(); flushTask = nil; pendingEvents = []; streamEpoch = UUID()
        selectionEpoch = UUID(); let epoch = selectionEpoch
        snapshotTask?.cancel(); snapshotTask = nil; snapshotRequestID = UUID(); snapshotRefresh = SnapshotRefreshState(); loadingHistory = false; selectedID = id; draft = drafts[id] ?? ""; transcript = TranscriptState(); taskProvenance = TaskProvenance()
        resetInputs(); resetPermissions(); configureModels(); protectionEditorOpen = false; configureProtections(); configurePrivacy()
        approvals = []; questions = []; turnFeedback = nil; activity = SessionActivity(); nextCursor = nil; usage = 0; followLatest = true
        do {
            let snapshot = try await api.request("/v1/sessions/\(id)/snapshot", query: [.init(name: "limit", value: "100")])
            guard epoch == selectionEpoch else { return }
            try apply(snapshot, scope: api.scope)
            eventCursor = transcript.prepareForReconnect()
            startStream(epoch: profileEpoch)
        } catch { if epoch == selectionEpoch { self.error = error.localizedDescription } }
        if epoch == selectionEpoch { await refreshPermissions() }
    }
    private func apply(_ snapshot: JSON, scope: Scope, older: Bool = false, inputTicket: UUID? = nil) throws {
        guard snapshot["session"]["id"].string == selectedID else { throw DesktopError.protocolMismatch }
        if !older && snapshot["snapshot_revision"].integer < transcript.revision { return }
        try transcript.load(snapshot, scope: scope, older: older)
        nextCursor = snapshot["next_cursor"] == .null ? nil : snapshot["next_cursor"].string
        if !older {
            eventCursor = transcript.cursor
            if transcript.needsReplay {
                eventCursor = transcript.prepareForReconnect()
                startStream(epoch: profileEpoch)
            }
            if inputTicket == nil || inputTicket == inputReads.revision {
                inputReads.invalidateRequests()
                pendingInputs = try TurnInputRecord.list(snapshot["pending_inputs"] == .null ? .array([]) : snapshot["pending_inputs"], scope: scope, sessionID: snapshot["session"]["id"].string)
            }
            approvals = snapshot["pending_requests"].array; questions = snapshot["pending_questions"].array
            usage = snapshot["usage"]["total_tokens"].integer
            let id = snapshot["session"]["id"].string
            sessionActivity[id] = SessionActivity(turns: snapshot["turns"].array, revision: snapshot["snapshot_revision"].integer)
            updateActivity(id)
            if approvals.isEmpty && questions.isEmpty { needsAttention.remove(id) } else { needsAttention.insert(id) }
            if let i = sessions.firstIndex(where: { $0.id == id }) { sessions[i] = Conversation(snapshot["session"]) }
        }
        renderTick += 1
    }
    private func updateActivity(_ id: String) {
        let value = sessionActivity[id] ?? SessionActivity()
        if value.isRunning { running.insert(id) } else { running.remove(id) }
        if value.waitingLabel != nil { needsAttention.insert(id) }
        else { needsAttention.remove(id) }
        if selectedID == id { activity = value; turnFeedback = value.feedback }
    }
    private func configureModels() {
        guard connected, let api, let id = selectedID else { models.reset(); return }
        models.reset(actor: api.scope.actor, sessionID: id,
                     fetch: { try await api.models(sessionID: id) },
                     mutate: { try await api.changeModel(sessionID: id, model: $0) })
        Task { await models.refresh() }
    }
    func setModel(_ model: String) {
        guard configurationIdle, permissionsReady, !models.saving else { return }
        let epoch = selectionEpoch
        Task {
            guard epoch == selectionEpoch, configurationIdle, permissionsReady else { return }
            await models.change(to: model)
        }
    }
    private func configureProtections() {
        guard connected, let api, let id = selectedID else { protections.reset(); return }
        protections.reset(fetch: { try await api.protections(sessionID: id) }, mutate: { change, revision in try await api.changeProtection(sessionID: id, change: change, revision: revision) })
        Task { await protections.refresh() }
    }
    private func configurePrivacy() {
        guard privacyOpen, connected, let api, let id = selectedID else { privacy.reset(); privacyFiles.reset(); return }
        privacyFiles.reset(root: selected?.mode == "work" ? selected?.folder : nil)
        privacy.reset(sessionID: id) { before in try await api.privacy(sessionID: id, before: before) }
        Task { await privacy.refresh() }
    }
    private func resetPermissions() {
        permissionRequestID = UUID(); permissions = nil; permissionsLoading = false
        permissionsError = nil
    }
    func refreshPermissions() async {
        guard let api, let id = selectedID, !permissionsSaving else { return }
        let epoch = selectionEpoch, request = UUID()
        permissionRequestID = request; permissionsLoading = true
        defer { if request == permissionRequestID { permissionsLoading = false } }
        do {
            async let preferences = api.request("/v1/sessions/\(id)/preferences")
            async let catalog = api.request("/v1/permission-profiles")
            let value = try await SessionPermissions(preferences: preferences, catalog: catalog.array, sessionID: id)
            guard epoch == selectionEpoch, request == permissionRequestID else { return }
            permissions = value; permissionsError = nil
        } catch {
            guard epoch == selectionEpoch, request == permissionRequestID else { return }
            permissions = nil; permissionsError = "Could not load permissions. Retry before sending. " + error.localizedDescription
        }
    }
    func setPermissionMode(_ mode: PermissionMode) {
        guard let api, let id = selectedID, let previous = permissions, previous.sessionID == id,
              permissionsReady, configurationIdle, !models.saving, previous.mode != mode,
              previous.lock(mode) == nil else { return }
        if mode == .full {
            let epoch = selectionEpoch
            let alert = NSAlert()
            alert.alertStyle = .warning
            alert.messageText = "Enable Full permission?"
            alert.informativeText = "S-Code can run host commands, access the network, and write outside this project without routine approval. Explicit file protections still apply. External integrations keep their approval gates. This setting is saved only for this conversation."
            alert.addButton(withTitle: "Cancel")
            alert.addButton(withTitle: "Enable Full permission")
            guard alert.runModal() == .alertSecondButtonReturn,
                  epoch == selectionEpoch, self.api?.scope.actor == api.scope.actor,
                  selectedID == id, permissionsReady, configurationIdle, !models.saving,
                  permissions?.lock(mode) == nil else { return }
        }
        let epoch = selectionEpoch, request = UUID()
        permissionRequestID = request; permissionsError = nil
        permissionChanges.begin(actor: api.scope.actor, session: id)
        Task {
            var failure: String?
            do {
                let response = try await api.request("/v1/sessions/\(id)/preferences", method: "PATCH", body: .object([
                    "scope": api.scope.json, "permission_mode": .string(mode.rawValue)
                ]))
                if epoch == selectionEpoch, request == permissionRequestID {
                    permissions = try SessionPermissions(preferences: response, catalog: previous.catalog, sessionID: id)
                }
            } catch {
                failure = "Could not confirm the permission change. The saved setting has been rechecked. " + error.localizedDescription
                if epoch == selectionEpoch, request == permissionRequestID {
                    permissions = nil
                }
            }
            permissionChanges.finish(actor: api.scope.actor, session: id)
            // Reconcile even if the user left this session and returned while
            // PATCH was pending. Send stays gated until this GET finishes.
            if self.api?.scope.actor == api.scope.actor, selectedID == id {
                let currentSelection = selectionEpoch
                await refreshPermissions()
                if currentSelection == selectionEpoch, permissions != nil, let failure { permissionsError = failure }
            }
        }
    }
    func refreshSnapshot() async {
        guard let api, let id = selectedID else { return }
        let epoch = selectionEpoch, inputTicket = inputReads.revision
        do {
            let snapshot = try await api.request("/v1/sessions/\(id)/snapshot", query: [.init(name: "limit", value: "100")])
            guard epoch == selectionEpoch else { return }
            try apply(snapshot, scope: api.scope, inputTicket: inputTicket)
            if permissions == nil { await refreshPermissions() }
        } catch { if epoch == selectionEpoch && !Task.isCancelled { self.error = error.localizedDescription } }
    }
    func older() {
        guard let api, let id = selectedID, let cursor = nextCursor, !loadingHistory, !turnRunning else { return }
        let epoch = selectionEpoch; loadingHistory = true
        Task {
            defer { if epoch == selectionEpoch { loadingHistory = false } }
            do {
                let snapshot = try await api.request("/v1/sessions/\(id)/snapshot", query: [.init(name: "limit", value: "100"), .init(name: "before", value: cursor)])
                guard epoch == selectionEpoch else { return }
                try apply(snapshot, scope: api.scope, older: true)
            } catch { if epoch == selectionEpoch { self.error = error.localizedDescription } }
        }
    }
    func create(workspace: URL? = nil) {
        guard let api, let profile, !submitting else { return }
        let epoch = profileEpoch; submitting = true
        Task {
            defer { if epoch == profileEpoch { submitting = false } }
            do {
                let result = try await api.request("/v1/sessions", method: "POST", body: .object([
                    "scope": api.scope.json, "mode": .string(workspace == nil ? "chat" : "work"),
                    "workspace_uri": .string(workspace?.absoluteString ?? ""), "model": .string(profile.model),
                    "title": .string("New conversation")
                ]))
                guard epoch == profileEpoch, api.scope.owns(result), !result["id"].string.isEmpty else { return }
                try await refreshSessions()
                guard epoch == profileEpoch else { return }
                // A background metadata refresh may supersede the GET above.
                // The validated POST response still establishes this new row.
                if !sessions.contains(where: { $0.id == result["id"].string }) { sessions.insert(Conversation(result), at: 0) }
                await select(result["id"].string)
            } catch { if epoch == profileEpoch { self.error = error.localizedDescription } }
        }
    }
    func chooseFolder() {
        let panel = NSOpenPanel(); panel.canChooseFiles = false; panel.canChooseDirectories = true
        panel.allowsMultipleSelection = false; panel.prompt = "Start Work"; panel.message = "Choose the project S-Code can work in."
        panel.begin { [weak self] response in
            guard response == .OK, let url = panel.url else { return }
            Task { @MainActor in self?.create(workspace: url) }
        }
    }
    func send() {
        if turnRunning && !draftIsProtectionCommand { submitInput(.queue); return }
        guard connected, let api, let id = selectedID, !submitting, (!turnRunning || draftIsProtectionCommand), (pendingInputs.isEmpty || draftIsProtectionCommand), (!hasUnconfirmedInput || draftIsProtectionCommand), composerReady else { return }
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, text.utf8.count <= 128 * 1024 else { return }
        let epoch = selectionEpoch, connectionEpoch = profileEpoch, submittedDraft = draft
        submitting = true; running.insert(id); turnFeedback = nil
        Task {
            defer { if connectionEpoch == profileEpoch { submitting = false } }
            do {
                let response = try await api.request("/v1/sessions/\(id)/turns", method: "POST", body: .object(["scope": api.scope.json, "content": .string(text)]))
                guard connectionEpoch == profileEpoch else { return }
                if ["completed", "failed", "cancelled"].contains(response["status"].string) { running.remove(id) }
                if selectedID == id {
                    draft = DraftSubmission.acknowledged(current: draft, submitted: submittedDraft)
                    drafts[id] = draft
                } else {
                    drafts[id] = DraftSubmission.acknowledged(current: drafts[id] ?? "", submitted: submittedDraft)
                }
                guard epoch == selectionEpoch else { return }
                followLatest = true
                await refreshSnapshot(); try await refreshSessions()
                if ProtectionCommand.recognizes(text) { await protections.refresh() }
            } catch {
                guard connectionEpoch == profileEpoch else { return }
                running.remove(id)
                if epoch == selectionEpoch { self.error = "Could not confirm submission. Refresh before retrying to avoid sending it twice. " + error.localizedDescription; await refreshSnapshot() }
            }
        }
    }
    private func resetInputs() {
        inputReads.invalidate(); workingDiff = nil; pendingInputs = []; inputNotice = nil; inputError = nil; inputRemoving = []
    }
    func refreshInputs() async {
        guard supportsTurnInput, let api, let id = selectedID else { return }
        let epoch = selectionEpoch, request = inputReads.begin()
        do {
            let values = try await api.turnInputs(sessionID: id)
            guard epoch == selectionEpoch, inputReads.accepts(request) else { return }
            pendingInputs = values
            if let context = inputContext {
                for record in values {
                    guard let submitted = inputAttempts.acknowledgedDraft(context: context, session: id, record: record) else { continue }
                    inputAttempts.acknowledge(context: context, session: id, record: record)
                    draft = DraftSubmission.acknowledged(current: draft, submitted: submitted); drafts[id] = draft
                    inputError = nil; inputNotice = "Previously submitted input was found in the saved queue."
                }
            }
            inputReads.invalidate()
        } catch { if epoch == selectionEpoch, inputReads.accepts(request) { inputError = "Could not refresh queued messages. " + error.localizedDescription } }
    }
    func submitInput(_ mode: TurnInputMode) {
        if draftIsProtectionCommand { send(); return }
        guard canQueueInput, mode != .steer || canSteerInput, let api, let context = inputContext, let id = selectedID, let target = inputTarget else { return }
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, text.utf8.count <= 128 * 1024 else { return }
        let attempt = inputAttempts.begin(context: context, session: id, target: target["id"].string, mode: mode, content: text, originalDraft: draft)
        let epoch = selectionEpoch, connectionEpoch = profileEpoch, submittedDraft = draft
        submitting = true; inputReads.invalidate(); inputError = nil; inputNotice = nil
        Task {
            defer { if connectionEpoch == profileEpoch { submitting = false } }
            do {
                let record = try await api.submitTurnInput(sessionID: id, attempt: attempt)
                inputAttempts.acknowledge(context: context, session: id, record: record)
                guard connectionEpoch == profileEpoch else { return }
                if selectedID == id { draft = DraftSubmission.acknowledged(current: draft, submitted: submittedDraft); drafts[id] = draft }
                else { drafts[id] = DraftSubmission.acknowledged(current: drafts[id] ?? "", submitted: submittedDraft) }
                if selectedID == id {
                    inputReads.invalidate()
                    inputNotice = mode == .steer ? "Steering accepted by the server." : "Follow-up saved by the server."
                    await refreshInputs()
                }
            } catch {
                if connectionEpoch == profileEpoch, epoch == selectionEpoch {
                    inputReads.invalidate()
                    inputError = "Could not confirm this message. Your draft is kept. Refresh pending messages and check the conversation before sending again. " + error.localizedDescription
                    await refreshInputs(); await refreshSnapshot()
                }
            }
        }
    }
    func resendUnconfirmedInput() {
        guard connected, composerReady, !submitting, hasUnconfirmedInput,
              turnRunning ? canQueueInput : pendingInputs.isEmpty,
              let context = inputContext, let id = selectedID else { return }
        inputAttempts.discard(context: context, session: id, content: draft.trimmingCharacters(in: .whitespacesAndNewlines))
        inputError = nil; inputNotice = nil
        send()
    }
    func removeInput(_ record: TurnInputRecord) {
        guard connected, let api, let id = selectedID, record.value["session_id"].string == id, record.status == "pending", !inputRemoving.contains(record.id) else { return }
        let epoch = selectionEpoch
        inputRemoving.insert(record.id); inputReads.invalidate(); inputError = nil
        Task {
            defer { if epoch == selectionEpoch { inputRemoving.remove(record.id) } }
            do {
                _ = try await api.cancelTurnInput(sessionID: id, inputID: record.id)
                guard epoch == selectionEpoch else { return }
                inputReads.invalidate(); inputNotice = "Queued message removed."; await refreshInputs()
            } catch {
                if epoch == selectionEpoch {
                    inputReads.invalidate(); inputError = "Could not remove this message; it may already be processing. " + error.localizedDescription
                    await refreshInputs()
                }
            }
        }
    }
    func stopTurn() {
        guard let api, let id = selectedID else { return }; let epoch = selectionEpoch
        let action = "stop:" + id
        guard !busyRequests.contains(action) else { return }
        busyRequests.insert(action)
        Task {
            defer { busyRequests.remove(action) }
            do {
                let snapshot = try await api.request("/v1/sessions/\(id)/snapshot")
                guard api.scope.owns(snapshot["session"]), snapshot["session"]["id"].string == id else { throw DesktopError.protocolMismatch }
                for turn in snapshot["turns"].array where !["completed", "failed", "cancelled"].contains(turn["status"].string) {
                    _ = try await api.request("/v1/turns/\(turn["id"].string)/cancel", method: "POST", body: .object(["scope": api.scope.json]))
                }
                if epoch == selectionEpoch { await refreshSnapshot() }
            } catch { if epoch == selectionEpoch { self.error = error.localizedDescription } }
        }
    }
    func decide(_ request: JSON, approved: Bool) {
        let id = request["id"].string
        guard let api, request["session_id"].string == selectedID, !busyRequests.contains(id) else { return }
        busyRequests.insert(id); let epoch = selectionEpoch
        Task {
            defer { busyRequests.remove(id) }
            do {
                _ = try await api.request("/v1/approvals/\(id)", method: "POST", body: .object(["scope": api.scope.json, "approved": .bool(approved), "approval_scope": .string("once")]))
                if epoch == selectionEpoch { await refreshSnapshot() }
            } catch { if epoch == selectionEpoch { self.error = error.localizedDescription; await refreshSnapshot() } }
        }
    }
    func answer(_ request: JSON, values: [String: String]) {
        let id = request["id"].string
        guard let api, request["session_id"].string == selectedID, !busyRequests.contains(id), request["questions"].array.allSatisfy({ !(values[$0["id"].string] ?? "").trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }) else { return }
        busyRequests.insert(id); let epoch = selectionEpoch
        Task {
            defer { busyRequests.remove(id) }
            do {
                _ = try await api.request("/v1/questions/\(id)", method: "POST", body: .object(["scope": api.scope.json, "answers": .array(values.sorted(by: { $0.key < $1.key }).map { .object(["question_id": .string($0.key), "answer": .string($0.value)]) })]))
                if epoch == selectionEpoch { await refreshSnapshot() }
            } catch { if epoch == selectionEpoch { self.error = error.localizedDescription } }
        }
    }
    func toolDetails(_ id: String, sessionID: String) async throws -> JSON {
        guard connected, let api, selectedID == sessionID, !id.isEmpty else { throw CancellationError() }
        let epoch = selectionEpoch
        let result = try await api.request("/v1/sessions/\(sessionID)/tools/\(id)")
        guard epoch == selectionEpoch, !Task.isCancelled else { throw CancellationError() }
        return try ToolPresentation.validate(result, toolID: id, sessionID: sessionID, scope: api.scope)
    }
    func showTool(_ id: String, title: String) {
        guard let api, let sessionID = selectedID, !id.isEmpty else { return }
        let epoch = selectionEpoch
        Task {
            do {
                let result = try await api.request("/v1/sessions/\(sessionID)/tools/\(id)")
                guard epoch == selectionEpoch else { return }
                detail = Detail(title: title, text: "Arguments\n" + result["request"]["arguments"].pretty + "\n\nResult\n" + result["result"].pretty + (result["error"] == .null ? "" : "\n\nError\n" + result["error"].string))
            } catch { if epoch == selectionEpoch { self.error = error.localizedDescription } }
        }
    }
    func showDetails(_ row: TranscriptRow) {
        let toolID = row.value["content"]["tool_call_id"].string
        if !toolID.isEmpty { showTool(toolID, title: row.text); return }
        let href = row.value["detail"]["href"].string
        guard let api, href.hasPrefix("/v1/artifacts/") else { detail = Detail(title: row.kind.replacingOccurrences(of: "_", with: " ").capitalized, text: row.value["content"].pretty); return }
        let epoch = selectionEpoch
        Task { do { let result = try await api.request(href); if epoch == selectionEpoch { detail = Detail(title: row.text, text: result["content"].string.isEmpty ? result["content"].pretty : result["content"].string) } } catch { if epoch == selectionEpoch { self.error = error.localizedDescription } } }
    }
    var changesUnavailableReason: String? {
        if !connected { return "Reconnect to review changes." }
        if selected?.mode != "work" { return "Open a project to review file changes." }
        if protections.loading || protections.saving { return "Checking file protections before reviewing changes." }
        guard protections.verified, let policy = protections.policy else { return "File protections are unavailable. Open Privacy and retry before reviewing changes." }
        if !policy.rules.isEmpty { return "Changes are unavailable while hard file protection is active because Git can read protected content. Manage protected paths in Privacy." }
        if busyRequests.contains("diff:" + (selectedID ?? "")) { return "Loading working-tree changes…" }
        return nil
    }
    func showDiff() {
        guard changesUnavailableReason == nil else { error = changesUnavailableReason; return }
        guard let api, let id = selectedID, let selected, selected.mode == "work" else { return }
        let epoch = selectionEpoch, action = "diff:" + id
        guard !busyRequests.contains(action) else { return }
        busyRequests.insert(action)
        let workspace = URL(fileURLWithPath: selected.folder)
        Task {
            defer { busyRequests.remove(action) }
            let isRepository = await Task.detached(priority: .userInitiated) { WorkspaceInspection.hasGitRepository(workspace) }.value
            guard epoch == selectionEpoch else { return }
            guard isRepository else {
                detail = Detail(title: "Working changes", text: WorkspaceInspection.noGitMessage)
                return
            }
            do {
                let result = try await api.request("/v1/sessions/\(id)/tools", method: "POST", body: .object(["scope": api.scope.json, "tool": .string("git_diff"), "arguments": .object(["paths": .array([]), "max_bytes": .number(524288)])]))
                guard epoch == selectionEpoch else { return }
                if result["outcome"].string == "completed" {
                    let diff = result["tool_call"]["result"]["unified_diff"].string
                    workingDiff = DiffDetail(diff: UnifiedDiff(diff, serverTruncated: result["tool_call"]["result"]["truncated"].boolean))
                } else {
                    let reason = result["tool_call"]["error"].string
                    error = reason.isEmpty ? "Could not inspect changes. Open the Review changes tool card for details." : "Could not inspect changes: " + reason
                }
                await refreshSnapshot()
            } catch { if epoch == selectionEpoch { self.error = error.localizedDescription } }
        }
    }
    private func startStream(epoch: UUID) {
        streamTask?.cancel(); flushTask?.cancel(); flushTask = nil; pendingEvents = []
        streamEpoch = UUID(); let streamID = streamEpoch
        streamTask = Task { [weak self] in
            var attempts = 0
            while let self, !Task.isCancelled, self.profileEpoch == epoch, self.streamEpoch == streamID, let api = self.api {
                do {
                    if attempts > 0 {
                        self.flushTask?.cancel(); self.flushTask = nil; self.pendingEvents = []
                        self.eventCursor = self.transcript.prepareForReconnect()
                    }
                    self.status = attempts == 0 ? "Ready" : "Reconnecting…"
                    try await api.stream(after: self.eventCursor) { [weak self] batch in
                        await self?.enqueue(batch, epoch: epoch, streamID: streamID)
                    }
                } catch {
                    guard !Task.isCancelled, self.profileEpoch == epoch, self.streamEpoch == streamID else { return }
                    if case DesktopError.http(let code) = error, [401, 403].contains(code) { self.connected = false; self.privacy.reset(); self.privacyFiles.reset(); self.protections.reset(); self.models.reset(); self.error = "The local connection expired. Reconnect to continue."; self.status = "Reconnect required"; return }
                    attempts += 1; self.status = "Reconnecting…"
                    try? await Task.sleep(for: .seconds(min(10, attempts)))
                    self.privacy.invalidate()
                    await self.protections.refresh()
                    await self.refreshSnapshot()
                    await self.refreshPermissions()
                    await self.models.refresh()
                    do { try await self.refreshSessions() } catch { }
                }
            }
        }
    }
    private func enqueue(_ batch: [JSON], epoch: UUID, streamID: UUID) {
        guard epoch == profileEpoch, streamID == streamEpoch else { return }
        pendingEvents.append(contentsOf: batch)
        if pendingEvents.count > 4096 { pendingEvents.removeAll(); privacy.invalidate(); Task { await protections.refresh() }; scheduleSnapshot(); return }
        guard flushTask == nil else { return }
        flushTask = Task {
            try? await Task.sleep(for: .milliseconds(50))
            guard epoch == profileEpoch, streamID == streamEpoch, !Task.isCancelled else { return }
            let events = pendingEvents; pendingEvents.removeAll(keepingCapacity: true); flushTask = nil
            privacy.receive(events)
            if let selectedID { taskProvenance.receive(events, sessionID: selectedID) }
            if events.contains(where: { $0["type"].string == "privacy.protection.updated" }) { Task { await protections.refresh() } }
            var repair = false, permissionsChanged = false
            for event in events {
                let sid = event["session_id"].string, n = event["notification"]
                if sid == selectedID && event["type"].string == "session.preferences.updated" { permissionsChanged = true }
                let sequence = event["sequence"].integer
                var value = sessionActivity[sid] ?? SessionActivity()
                if value.apply(event) {
                    sessionActivity[sid] = value
                    updateActivity(sid)
                }
                if ["approval_requested", "question_requested"].contains(n["type"].string), sequence > value.revision { needsAttention.insert(sid) }
                if !transcript.apply(event) { repair = true }
                else { eventCursor = max(eventCursor, event["sequence"].integer) }
            }
            renderTick += 1; status = "Ready"
            if repair { scheduleSnapshot() }
            if permissionsChanged { await refreshPermissions() }
            if events.contains(where: { $0["session_id"].string == selectedID && $0["type"].string.hasPrefix("turn.input.") }) { await refreshInputs() }
            if events.contains(where: { $0["session_id"].string == selectedID && $0["type"].string == "session.updated" && $0["payload"]["model"] != .null }) {
                await models.refresh()
            }
            if ConversationMetadata.needsRefresh(events) {
                do { try await refreshSessions() } catch {
                    if epoch == profileEpoch { self.error = "Could not refresh conversation names. " + error.localizedDescription }
                }
            }
        }
    }
    private func scheduleSnapshot() {
        snapshotRefresh.invalidate()
        guard snapshotTask == nil else { return }
        let epoch = selectionEpoch, requestID = UUID()
        snapshotRequestID = requestID
        snapshotTask = Task {
            while !Task.isCancelled, epoch == selectionEpoch, snapshotRefresh.needsRefresh {
                try? await Task.sleep(for: .milliseconds(100))
                guard !Task.isCancelled, epoch == selectionEpoch else { break }
                let ticket = snapshotRefresh.ticket
                await refreshSnapshot()
                guard !Task.isCancelled, epoch == selectionEpoch else { break }
                snapshotRefresh.finish(ticket)
            }
            if snapshotRequestID == requestID { snapshotTask = nil }
        }
    }
    func shutdown() async {
        privacy.reset(); privacyFiles.reset(); protections.reset(); models.reset()
        profileEpoch = UUID(); streamTask?.cancel(); flushTask?.cancel(); snapshotTask?.cancel(); connectTask?.cancel()
        await engine.stop()
    }
}
