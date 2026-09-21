import SwiftUI
import AppKit

@main struct SCodeDesktopApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate
    @StateObject private var store = AppStore()
    var body: some Scene {
        Window("S-Code", id: "main") {
            RootView().environmentObject(store)
                .frame(minWidth: 840, minHeight: 560)
                .task { delegate.store = store; store.startSaved() }
        }
        .defaultSize(width: 1120, height: 760)
        .windowStyle(.hiddenTitleBar)
        .commands {
            CommandGroup(replacing: .newItem) {
                Button("New Chat") { store.create() }.keyboardShortcut("n").disabled(!store.connected)
                Button("Open Project…") { store.chooseFolder() }.keyboardShortcut("o").disabled(!store.connected)
            }
            CommandGroup(replacing: .appSettings) {
                Button("Connections…") { store.settingsOpen = true }.keyboardShortcut(",")
            }
            CommandMenu("Conversation") {
                Button("Send Message") { store.send() }.keyboardShortcut(.return, modifiers: .command).disabled(!store.connected || (store.turnRunning && !store.draftIsProtectionCommand) || !store.composerReady || store.submitting)
                Button("Stop Task") { store.stopTurn() }.keyboardShortcut(".", modifiers: .command).disabled(!store.turnRunning)
                Button("Working Changes") { store.showDiff() }.keyboardShortcut("d", modifiers: [.command, .shift]).disabled(store.changesUnavailableReason != nil)
                Button("Refresh") { Task { await store.refreshSnapshot() } }.keyboardShortcut("r").disabled(!store.connected)
            }
        }
    }
}
@MainActor final class AppDelegate: NSObject, NSApplicationDelegate {
    weak var store: AppStore?
    private var quitting = false
    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.regular); NSApp.activate(ignoringOtherApps: true)
    }
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }
    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        if !flag { sender.windows.first?.makeKeyAndOrderFront(nil) }; return true
    }
    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        guard !quitting else { return .terminateLater }
        guard store?.confirmLeavingTasks() != false else { return .terminateCancel }
        quitting = true
        Task { await store?.shutdown(); sender.reply(toApplicationShouldTerminate: true) }
        return .terminateLater
    }
}
