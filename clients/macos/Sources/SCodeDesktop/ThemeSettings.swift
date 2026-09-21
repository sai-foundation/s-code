import SwiftUI
import AppKit
import DesktopCore

/// Observe application appearance, which follows macOS independently of a
/// window's SwiftUI preferredColorScheme override. Never write system defaults.
@MainActor final class SystemAppearance: ObservableObject {
    static let shared = SystemAppearance(application: .shared)
    @Published private(set) var scheme: ColorScheme
    private var observation: NSKeyValueObservation?
    init(application: NSApplication) {
        scheme = Self.scheme(for: application.effectiveAppearance)
        observation = application.observe(\.effectiveAppearance, options: [.new]) { [weak self] application, _ in
            let appearance = application.effectiveAppearance
            Task { @MainActor [weak self] in self?.scheme = Self.scheme(for: appearance) }
        }
    }
    private static func scheme(for appearance: NSAppearance) -> ColorScheme {
        appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua ? .dark : .light
    }
}

struct ThemePalette {
    let background: Color
    let sidebar: Color
    let surface: Color
    let accent: Color
    let ink: Color

    static func resolve(_ theme: DesktopTheme, system: ColorScheme) -> ThemePalette {
        func rgb(_ hex: UInt32) -> Color {
            Color(red: Double((hex >> 16) & 255) / 255, green: Double((hex >> 8) & 255) / 255, blue: Double(hex & 255) / 255)
        }
        let values: [UInt32]
        switch theme == .system ? (system == .dark ? DesktopTheme.dark : .light) : theme {
        case .system, .light: values = [0xFAFAFA, 0xF0F1F3, 0xFFFFFF, 0xBC481F, 0x202124]
        case .dark: values = [0x1B1D21, 0x15171A, 0x25282D, 0xEE966C, 0xF1F2F4]
        case .terminal: values = [0x090D0B, 0x060907, 0x111A14, 0x66DF91, 0xDDEDE2]
        case .midnight: values = [0x101A2E, 0x0B1324, 0x192741, 0x83B4FF, 0xE7EEFA]
        case .nord: values = [0x2E3440, 0x252B36, 0x3B4252, 0x88C0D0, 0xECEFF4]
        }
        return ThemePalette(background: rgb(values[0]), sidebar: rgb(values[1]), surface: rgb(values[2]), accent: rgb(values[3]), ink: rgb(values[4]))
    }
}
private struct ThemePaletteKey: EnvironmentKey {
    static let defaultValue = ThemePalette.resolve(.system, system: .light)
}
extension EnvironmentValues {
    var themePalette: ThemePalette {
        get { self[ThemePaletteKey.self] }
        set { self[ThemePaletteKey.self] = newValue }
    }
}
struct DesktopAppearance: ViewModifier {
    @ObservedObject private var systemAppearance = SystemAppearance.shared
    let theme: DesktopTheme
    func body(content: Content) -> some View {
        let palette = ThemePalette.resolve(theme, system: systemAppearance.scheme)
        content
            .environment(\.themePalette, palette)
            .preferredColorScheme(theme == .system ? nil : theme == .light ? .light : .dark)
            .accentColor(palette.accent).tint(palette.accent)
            .foregroundStyle(palette.ink)
    }
}

struct SettingsView: View {
    @EnvironmentObject private var store: AppStore
    @ObservedObject private var systemAppearance = SystemAppearance.shared
    var body: some View {
        TabView {
            ConnectionsView().tabItem { Label("Connections", systemImage: "network") }
            ScrollView { ThemeSettingsView() }.frame(height: 640).tabItem { Label("Appearance", systemImage: "paintpalette") }
        }.padding(12).frame(width: 654)
            .background(ThemePalette.resolve(store.theme, system: systemAppearance.scheme).background)
            .modifier(DesktopAppearance(theme: store.theme))
    }
}
struct ThemeSettingsView: View {
    @EnvironmentObject private var store: AppStore
    @ObservedObject private var systemAppearance = SystemAppearance.shared
    @FocusState private var focusedTheme: DesktopTheme?
    var body: some View {
        VStack(alignment: .leading, spacing: 20) {
            HStack {
                VStack(alignment: .leading, spacing: 5) {
                    Text("Appearance").font(.title2.weight(.semibold))
                    Text("Choose a theme for \(store.profile?.name ?? "this Mac’s welcome screen").").font(.callout).foregroundStyle(.secondary)
                }
                Spacer()
                Button("Done") { store.settingsOpen = false }.keyboardShortcut(.cancelAction)
            }
            LazyVGrid(columns: [GridItem(.flexible()), GridItem(.flexible())], spacing: 16) {
                ForEach(DesktopTheme.allCases) { theme in
                    let selected = store.theme == theme
                    Button { store.setTheme(theme) } label: {
                        VStack(alignment: .leading, spacing: 9) {
                            ThemePreview(theme: theme, system: systemAppearance.scheme)
                            HStack {
                                Text(theme.title).font(.headline)
                                Spacer()
                                if selected { Image(systemName: "checkmark.circle.fill").foregroundStyle(Color.accentColor) }
                            }
                            Text(theme.summary).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                        }.padding(10)
                            .overlay(RoundedRectangle(cornerRadius: 12).stroke(selected ? Color.accentColor : Color.primary.opacity(0.12), lineWidth: selected ? 2 : 1))
                            .contentShape(RoundedRectangle(cornerRadius: 12))
                    }.buttonStyle(.plain)
                        .focused($focusedTheme, equals: theme)
                        .overlay(RoundedRectangle(cornerRadius: 15).stroke(focusedTheme == theme ? Color.accentColor : .clear, lineWidth: 3).padding(-3).allowsHitTesting(false))
                        .accessibilityElement(children: .ignore)
                        .accessibilityLabel(theme.title + ". " + theme.summary)
                        .accessibilityValue(selected ? "Selected" : "Not selected")
                        .accessibilityAddTraits(selected ? [.isSelected] : [])
                }
            }
            Text("Saved on this Mac for the current connection. System follows macOS Light and Dark appearance.")
                .font(.caption).foregroundStyle(.secondary)
        }.padding(24)
    }
}

/// Prefer the resources inside a distributed app; SwiftPM's generated bundle
/// accessor also supports running the executable directly from its build folder.
enum ThemePreviewResources {
    static let bundle: Bundle = {
        if let url = Bundle.main.resourceURL?.appendingPathComponent("SCodeDesktop_SCodeDesktop.bundle"),
           let packaged = Bundle(url: url) { return packaged }
        return Bundle.module
    }()
    static func image(for theme: DesktopTheme, system: ColorScheme) -> NSImage? {
        let resolved = theme == .system ? (system == .dark ? DesktopTheme.dark : .light) : theme
        guard let url = bundle.url(forResource: "theme-" + resolved.rawValue, withExtension: "png") else { return nil }
        return NSImage(contentsOf: url)
    }
}

private struct ThemePreview: View {
    let theme: DesktopTheme
    let system: ColorScheme
    var body: some View {
        Group {
            if let image = ThemePreviewResources.image(for: theme, system: system) {
                Image(nsImage: image).resizable().aspectRatio(944.0 / 608.0, contentMode: .fit)
            } else {
                Text("Preview unavailable").font(.caption).frame(maxWidth: .infinity, minHeight: 120)
            }
        }.clipShape(RoundedRectangle(cornerRadius: 7))
            .overlay(RoundedRectangle(cornerRadius: 7).stroke(Color.primary.opacity(0.1)))
            .accessibilityHidden(true)
    }
}
