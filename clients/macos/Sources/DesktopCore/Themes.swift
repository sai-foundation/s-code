import Foundation

public enum DesktopTheme: String, CaseIterable, Identifiable {
    case system, light, dark, terminal, midnight, nord
    public var id: String { rawValue }
    public var title: String { rawValue.capitalized }
    public var summary: String {
        switch self {
        case .system: return "Follows your Mac’s appearance"
        case .light: return "Bright surfaces, crisp contrast"
        case .dark: return "Neutral charcoal, soft contrast"
        case .terminal: return "Black with a green accent"
        case .midnight: return "Deep blue with a blue accent"
        case .nord: return "Cool slate with a cyan accent"
        }
    }
}

/// Appearance stays local and is isolated by the saved account connection.
public struct ThemePreferences {
    private let defaults: UserDefaults
    public init(defaults: UserDefaults = .standard) { self.defaults = defaults }
    private func key(_ profileID: String?) -> String { "desktop.theme." + (profileID ?? "welcome") }
    public func load(profileID: String?) -> DesktopTheme {
        defaults.string(forKey: key(profileID)).flatMap(DesktopTheme.init(rawValue:)) ?? .system
    }
    public func save(_ theme: DesktopTheme, profileID: String?) { defaults.set(theme.rawValue, forKey: key(profileID)) }
}
