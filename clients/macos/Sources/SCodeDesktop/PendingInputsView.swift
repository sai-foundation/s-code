import SwiftUI
import DesktopCore

struct PendingInputsView: View {
    @EnvironmentObject private var store: AppStore
    var body: some View {
        VStack(alignment: .leading, spacing: 7) {
            if !store.pendingInputs.isEmpty {
                HStack {
                    Label("Saved follow-ups · \(store.pendingInputs.count)", systemImage: "text.badge.plus").font(.caption.weight(.medium))
                    Spacer()
                    Button("Refresh") { Task { await store.refreshInputs() } }.font(.caption)
                }
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 6) {
                        ForEach(store.pendingInputs) { input in
                            HStack(alignment: .top, spacing: 8) {
                                VStack(alignment: .leading, spacing: 3) {
                                    Text(input.status == "processing" ? "Processing" : input.mode == "steer" ? "Steering · pending" : "Queued · pending").font(.caption.weight(.medium))
                                    Text(input.content).font(.caption).lineLimit(2).textSelection(.enabled).help(input.content)
                                }.frame(maxWidth: .infinity, alignment: .leading)
                                Button("Remove") { store.removeInput(input) }
                                    .font(.caption).disabled(!store.connected || input.status != "pending" || store.inputRemoving.contains(input.id))
                                    .accessibilityLabel("Remove queued message: " + input.content)
                                    .help(input.status == "processing" ? "Already processing; stop the task if needed." : "Cancel this saved input before it is consumed")
                            }.padding(7).background(Color.primary.opacity(0.035), in: RoundedRectangle(cornerRadius: 6))
                        }
                    }
                }.frame(height: min(132, CGFloat(store.pendingInputs.count) * 58))
            }
            if store.hasUnconfirmedInput && !store.submitting {
                VStack(alignment: .leading, spacing: 5) {
                    Text("The previous message may already have been accepted. Check pending messages and conversation history before sending it again.").font(.caption).foregroundStyle(.orange)
                    Button("Send again") { store.resendUnconfirmedInput() }.font(.caption).disabled(!store.connected || !store.composerReady || (store.turnRunning ? !store.canQueueInput : !store.pendingInputs.isEmpty))
                }
            }
            if let error = store.inputError {
                HStack(alignment: .top) {
                    Text(error).font(.caption).foregroundStyle(.orange).textSelection(.enabled)
                    Spacer(minLength: 4)
                    Button("Refresh queue") { Task { await store.refreshInputs() } }.font(.caption)
                }
            } else if let notice = store.inputNotice {
                HStack {
                    Label(notice, systemImage: "checkmark.circle").font(.caption).foregroundStyle(.secondary)
                    Spacer()
                    Button { store.inputNotice = nil } label: { Image(systemName: "xmark") }.buttonStyle(.plain).accessibilityLabel("Dismiss input confirmation")
                }
            }
        }
    }
}
