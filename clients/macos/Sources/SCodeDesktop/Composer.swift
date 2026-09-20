import AppKit
import SwiftUI

struct Composer: NSViewRepresentable {
    @Binding var text: String
    var send: () -> Void
    func makeCoordinator() -> Coordinator { Coordinator(self) }
    func makeNSView(context: Context) -> NSScrollView {
        let scroll = NSScrollView(); scroll.hasVerticalScroller = true; scroll.drawsBackground = false
        let view = ComposeTextView(); view.isRichText = false; view.drawsBackground = false
        view.font = .systemFont(ofSize: 15); view.textColor = .labelColor
        view.isAutomaticQuoteSubstitutionEnabled = false; view.isAutomaticDashSubstitutionEnabled = false
        view.isAutomaticTextReplacementEnabled = false; view.allowsUndo = true
        view.textContainerInset = NSSize(width: 6, height: 10)
        view.autoresizingMask = [.width]; view.isVerticallyResizable = true; view.isHorizontallyResizable = false
        view.textContainer?.widthTracksTextView = true
        view.textContainer?.containerSize = NSSize(width: 0, height: CGFloat.greatestFiniteMagnitude)
        view.delegate = context.coordinator; view.onSend = send
        view.setAccessibilityLabel("Message S-Code")
        scroll.documentView = view
        return scroll
    }
    func updateNSView(_ scroll: NSScrollView, context: Context) {
        context.coordinator.parent = self
        guard let view = scroll.documentView as? ComposeTextView else { return }
        view.onSend = send
        if view.string != text { view.string = text }
    }
    final class Coordinator: NSObject, NSTextViewDelegate {
        var parent: Composer
        init(_ parent: Composer) { self.parent = parent }
        func textDidChange(_ notification: Notification) {
            guard let view = notification.object as? NSTextView else { return }
            parent.text = view.string
        }
    }
}
final class ComposeTextView: NSTextView {
    var onSend: (() -> Void)?
    override func keyDown(with event: NSEvent) {
        if event.keyCode == 36 && !event.modifierFlags.contains(.shift) && !hasMarkedText() { onSend?() }
        else { super.keyDown(with: event) }
    }
}

// Track user scroll intent separately from geometry changes caused by new tokens.
struct UserScrollObserver: NSViewRepresentable {
    let onScroll: () -> Void
    func makeNSView(context: Context) -> ScrollIntentView { let view = ScrollIntentView(); view.onScroll = onScroll; return view }
    func updateNSView(_ view: ScrollIntentView, context: Context) { view.onScroll = onScroll }
}
final class ScrollIntentView: NSView {
    var onScroll: (() -> Void)?
    private var monitor: Any?
    override func viewDidMoveToWindow() {
        if let monitor { NSEvent.removeMonitor(monitor); self.monitor = nil }
        guard window != nil else { return }
        monitor = NSEvent.addLocalMonitorForEvents(matching: [.scrollWheel, .leftMouseDown, .leftMouseDragged]) { [weak self] event in
            guard let self, event.window === self.window else { return event }
            let point = self.convert(event.locationInWindow, from: nil)
            if self.bounds.contains(point), event.type == .scrollWheel || point.x > self.bounds.width - 20 { self.onScroll?() }
            return event
        }
    }
    override func hitTest(_ point: NSPoint) -> NSView? { nil }
    deinit { if let monitor { NSEvent.removeMonitor(monitor) } }
}
