import AppKit
import Combine
import Security
import DesktopCore
import Foundation

@MainActor final class AppStore: ObservableObject {
    @Published var profiles: [Profile] = []
    @Published var profile: Profile?
    @Published var sessions: [Conversation] = []
    @Published var selectedID: String?
    @Published var transcript = TranscriptState()
    @Published var approvals: [JSON] = []
    @Published var questions: [JSON] = []
    @Published var running: Set<String> = []
    @Published var needsAttention: Set<String> = []
    @Published var busyRequests: Set<String> = []
    @Published var draft = ""
    @Published var status = "Welcome"
    @Published var connected = false
    @Published var connecting = false
    @Published var submitting = false
    @Published var loadingHistory = false
    @Published var nextCursor: String?
    @Published var error: String?
    @Published var settingsOpen = false
    @Published var detail: Detail?
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
    private var profileEpoch = UUID()
    private var streamEpoch = UUID()
    private var selectionEpoch = UUID()
    private var drafts: [String: String] = [:]
    private var eventCursor = 0
    private var sessionActivity: [String: SessionActivity] = [:]
    private var snapshotRequestID = UUID()
    private var snapshotRefresh = SnapshotRefreshState()
    struct Detail: Identifiable { let id = UUID(); let title: String; let text: String }
    var selected: Conversation? { sessions.first { $0.id == selectedID } }
    var turnRunning: Bool { selectedID.map { running.contains($0) } ?? false }
    init() {
        do { profiles = try repository.load() } catch { self.error = error.localizedDescription }
        engine.onExit = { [weak self] in
            guard let self else { return }
            self.connected = false; self.status = "Engine stopped"
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
        profileEpoch = UUID(); selectionEpoch = UUID()
        let epoch = profileEpoch
        if let id = selectedID { drafts[id] = draft }
        api = nil; connected = false; connecting = true; profile = target; status = "Starting engine…"
        selectedID = nil; sessions = []; sessionActivity = [:]; activity = SessionActivity(); running = []; needsAttention = []; approvals = []; questions = []
        transcript = TranscriptState(); turnFeedback = nil; draft = ""; submitting = false; loadingHistory = false; busyRequests = []; eventCursor = 0; pendingEvents = []; error = nil
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
        guard let api else { return }; let epoch = profileEpoch
        let values = try await api.request("/v1/sessions")
        guard epoch == profileEpoch else { return }
        sessions = values.array.filter { api.scope.owns($0) && $0["status"].string != "deleted" && $0["status"].string != "archived" }.map(Conversation.init)
    }
    func select(_ id: String) async {
        guard let api, sessions.contains(where: { $0.id == id }) else { return }
        if let previous = selectedID { drafts[previous] = draft }
        streamTask?.cancel(); flushTask?.cancel(); flushTask = nil; pendingEvents = []; streamEpoch = UUID()
        selectionEpoch = UUID(); let epoch = selectionEpoch
        snapshotTask?.cancel(); snapshotTask = nil; snapshotRequestID = UUID(); snapshotRefresh = SnapshotRefreshState(); loadingHistory = false; selectedID = id; draft = drafts[id] ?? ""; transcript = TranscriptState()
        approvals = []; questions = []; turnFeedback = nil; activity = SessionActivity(); nextCursor = nil; usage = 0; followLatest = true
        do {
            let snapshot = try await api.request("/v1/sessions/\(id)/snapshot", query: [.init(name: "limit", value: "100")])
            guard epoch == selectionEpoch else { return }
            try apply(snapshot, scope: api.scope)
            eventCursor = transcript.prepareForReconnect()
            startStream(epoch: profileEpoch)
        } catch { if epoch == selectionEpoch { self.error = error.localizedDescription } }
    }
    private func apply(_ snapshot: JSON, scope: Scope, older: Bool = false) throws {
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
    func refreshSnapshot() async {
        guard let api, let id = selectedID else { return }
        let epoch = selectionEpoch
        do {
            let snapshot = try await api.request("/v1/sessions/\(id)/snapshot", query: [.init(name: "limit", value: "100")])
            guard epoch == selectionEpoch else { return }
            try apply(snapshot, scope: api.scope)
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
                    "title": .string(workspace?.lastPathComponent ?? "New conversation")
                ]))
                guard epoch == profileEpoch, api.scope.owns(result), !result["id"].string.isEmpty else { return }
                try await refreshSessions(); await select(result["id"].string)
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
        guard let api, let id = selectedID, !submitting, !turnRunning else { return }
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, text.utf8.count <= 128 * 1024 else { return }
        let epoch = selectionEpoch, connectionEpoch = profileEpoch, submittedDraft = draft
        submitting = true; running.insert(id); turnFeedback = nil
        Task {
            defer { if connectionEpoch == profileEpoch { submitting = false } }
            do {
                _ = try await api.request("/v1/sessions/\(id)/turns", method: "POST", body: .object(["scope": api.scope.json, "content": .string(text)]))
                guard connectionEpoch == profileEpoch else { return }
                if selectedID == id {
                    draft = DraftSubmission.acknowledged(current: draft, submitted: submittedDraft)
                    drafts[id] = draft
                } else {
                    drafts[id] = DraftSubmission.acknowledged(current: drafts[id] ?? "", submitted: submittedDraft)
                }
                guard epoch == selectionEpoch else { return }
                followLatest = true
                await refreshSnapshot(); try await refreshSessions()
            } catch {
                guard connectionEpoch == profileEpoch else { return }
                running.remove(id)
                if epoch == selectionEpoch { self.error = "Could not confirm submission. Refresh before retrying to avoid sending it twice. " + error.localizedDescription; await refreshSnapshot() }
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
    func showDiff() {
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
                    detail = Detail(title: "Working changes", text: diff.isEmpty ? "No working-tree changes." : diff)
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
                    if case DesktopError.http(let code) = error, [401, 403].contains(code) { self.connected = false; self.error = "The local connection expired. Reconnect to continue."; self.status = "Reconnect required"; return }
                    attempts += 1; self.status = "Reconnecting…"
                    try? await Task.sleep(for: .seconds(min(10, attempts)))
                    await self.refreshSnapshot()
                    do { try await self.refreshSessions() } catch { }
                }
            }
        }
    }
    private func enqueue(_ batch: [JSON], epoch: UUID, streamID: UUID) {
        guard epoch == profileEpoch, streamID == streamEpoch else { return }
        pendingEvents.append(contentsOf: batch)
        if pendingEvents.count > 4096 { pendingEvents.removeAll(); scheduleSnapshot(); return }
        guard flushTask == nil else { return }
        flushTask = Task {
            try? await Task.sleep(for: .milliseconds(50))
            guard epoch == profileEpoch, streamID == streamEpoch, !Task.isCancelled else { return }
            let events = pendingEvents; pendingEvents.removeAll(keepingCapacity: true); flushTask = nil
            var repair = false
            for event in events {
                let sid = event["session_id"].string, n = event["notification"]
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
        profileEpoch = UUID(); streamTask?.cancel(); flushTask?.cancel(); snapshotTask?.cancel(); connectTask?.cancel()
        await engine.stop()
    }
}
