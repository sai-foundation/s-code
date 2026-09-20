import SwiftUI
import DesktopCore

struct PermissionPicker: View {
    @EnvironmentObject private var store: AppStore
    @State private var open = false
    var body: some View {
        Button { open.toggle() } label: {
            HStack(spacing: 5) {
                Image(systemName: store.permissions?.mode.icon ?? "lock.shield")
                Text(store.permissionsSaving ? "Saving…" : store.permissionsLoading ? "Loading permissions…" : store.permissions?.mode.title ?? "Permissions unavailable")
                Image(systemName: "chevron.down").font(.system(size: 8, weight: .semibold))
            }.font(.caption)
        }
        .buttonStyle(.bordered)
        .disabled(!store.connected || store.selectedID == nil)
        .accessibilityLabel("Permissions")
        .accessibilityValue(store.permissions?.mode.title ?? "Unavailable")
        .help("Permissions for this conversation")
        .popover(isPresented: $open, arrowEdge: .top) {
            VStack(alignment: .leading, spacing: 12) {
                Text("Conversation permissions").font(.headline)
                Text("Saved for this conversation. All modes respect the active security policy.")
                    .font(.caption).foregroundStyle(.secondary)
                if let permissions = store.permissions {
                    ForEach(permissions.options) { mode in
                        Button { store.setPermissionMode(mode) } label: {
                            HStack(alignment: .top, spacing: 10) {
                                Image(systemName: mode.icon).frame(width: 18)
                                VStack(alignment: .leading, spacing: 4) {
                                    Text(mode.title).fontWeight(.medium)
                                    Text(permissions.description(mode)).font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                                    if let reason = permissions.lock(mode) { Text(reason).font(.caption).foregroundStyle(.secondary) }
                                }
                                Spacer(minLength: 0)
                                if mode == permissions.mode { Image(systemName: "checkmark").foregroundStyle(.orange) }
                            }.padding(9).frame(maxWidth: .infinity, alignment: .leading)
                                .background(mode == permissions.mode ? Color.orange.opacity(0.10) : .clear, in: RoundedRectangle(cornerRadius: 8))
                        }.buttonStyle(.plain)
                            .disabled(store.turnRunning || store.submitting || store.permissionsLoading || store.permissionsSaving || permissions.lock(mode) != nil)
                            .accessibilityLabel(mode.title)
                    }
                }
                if store.turnRunning { Text("Stop or finish the current task before changing permissions. Pending approvals still need your decision.").font(.caption).foregroundStyle(.secondary) }
                if store.permissionsError != nil || (store.permissions == nil && !store.permissionsLoading && !store.permissionsSaving) {
                    Text(store.permissionsError ?? "Permissions are unavailable. Retry to load this conversation's settings.").font(.caption).foregroundStyle(.red)
                    Button("Retry") { Task { await store.refreshPermissions() } }.disabled(store.permissionsSaving || store.permissionsLoading)
                }
                if store.permissionsLoading || store.permissionsSaving { ProgressView().controlSize(.small) }
            }.padding(18).frame(width: 360)
        }
        .onChange(of: store.selectedID) { _, _ in open = false }
    }
}
