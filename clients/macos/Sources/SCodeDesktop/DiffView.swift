import SwiftUI
import AppKit
import DesktopCore

struct DiffView: View {
    let diff: UnifiedDiff
    let close: () -> Void
    @State private var selectedID: Int?
    @State private var lineLimit = 500
    private var selected: DiffFile? { diff.files.first { $0.id == selectedID } ?? diff.files.first }
    var body: some View {
        VStack(spacing: 0) {
            HStack {
                VStack(alignment: .leading, spacing: 4) {
                    Text("Working changes").font(.title2.bold())
                    Text("\(diff.files.count) files · +\(diff.additions) additions · −\(diff.removals) removals\(diff.truncated ? " · partial counts" : "")").font(.caption).foregroundStyle(.secondary)
                }
                Spacer()
                Button("Copy received diff") { NSPasteboard.general.clearContents(); NSPasteboard.general.setString(diff.raw, forType: .string) }.disabled(diff.raw.isEmpty)
                Button("Done", action: close).keyboardShortcut(.cancelAction)
            }.padding(20)
            if diff.truncated {
                Label("Partial diff: the server or display limit was reached. Counts and files shown may be incomplete.", systemImage: "exclamationmark.triangle")
                    .font(.caption).foregroundStyle(.orange).frame(maxWidth: .infinity, alignment: .leading).padding(.horizontal, 20).padding(.bottom, 12)
            }
            Divider()
            if diff.files.isEmpty {
                ContentUnavailableView(diff.truncated ? "No complete diff available" : "No working-tree changes", systemImage: diff.truncated ? "exclamationmark.triangle" : "checkmark.circle", description: Text(diff.truncated ? "The returned diff was incomplete. This does not establish that the workspace is unchanged." : "The server returned an empty diff."))
            } else {
                HSplitView {
                    List(diff.files, selection: $selectedID) { file in
                        VStack(alignment: .leading, spacing: 4) {
                            Text(file.title).lineLimit(2).truncationMode(.middle)
                            Text("+\(file.additions)  −\(file.removals)").font(.caption).foregroundStyle(.secondary)
                        }.padding(.vertical, 4).tag(file.id).help(file.title)
                    }.frame(minWidth: 160, idealWidth: 220, maxWidth: 300).accessibilityLabel("Changed files")
                    if let selected {
                        VStack(alignment: .leading, spacing: 0) {
                            Text(selected.title).font(.headline).textSelection(.enabled).padding(12)
                            Divider()
                            ScrollView([.vertical, .horizontal]) {
                                LazyVStack(alignment: .leading, spacing: 0) {
                                    ForEach(Array(selected.lines.prefix(lineLimit))) { line in
                                        HStack(alignment: .top, spacing: 0) {
                                            Text(line.oldLine.map(String.init) ?? "").foregroundStyle(.secondary).frame(width: 48, alignment: .trailing).padding(.trailing, 8)
                                            Text(line.newLine.map(String.init) ?? "").foregroundStyle(.secondary).frame(width: 48, alignment: .trailing).padding(.trailing, 8)
                                            Text(line.text).textSelection(.enabled).padding(.horizontal, 8).frame(maxWidth: .infinity, alignment: .leading)
                                        }.font(.system(size: 12, design: .monospaced)).padding(.vertical, 2)
                                            .background(background(line.kind))
                                    }
                                    if selected.lines.count > lineLimit { Button("Show next \(min(500, selected.lines.count - lineLimit)) lines") { lineLimit += 500 }.padding(12) }
                                }.frame(minWidth: 480, alignment: .leading)
                            }.accessibilityLabel("Diff lines with old and new line numbers")
                        }.frame(minWidth: 400, maxWidth: .infinity, maxHeight: .infinity)
                    }
                }
            }
        }.frame(minWidth: 680, idealWidth: 980, minHeight: 480, idealHeight: 680)
            .onAppear { selectedID = diff.files.first?.id }
            .onChange(of: selectedID) { _, _ in lineLimit = 500 }
    }
    private func background(_ kind: DiffLine.Kind) -> Color {
        switch kind { case .addition: return .green.opacity(0.10); case .removal: return .red.opacity(0.10); case .hunk: return .blue.opacity(0.08); default: return .clear }
    }
}
