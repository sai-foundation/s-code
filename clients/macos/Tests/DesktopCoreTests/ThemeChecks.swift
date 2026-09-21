import Foundation
import DesktopCore

enum ThemeChecks {
    static func run() throws {
        let suite = "s-code-theme-checks-" + UUID().uuidString
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let preferences = ThemePreferences(defaults: defaults)
        try expectEqual(DesktopTheme.allCases.map(\.rawValue), ["system", "light", "dark", "terminal", "midnight", "nord"])
        try expectEqual(preferences.load(profileID: "a"), .system)
        preferences.save(.terminal, profileID: "a")
        preferences.save(.midnight, profileID: "b")
        preferences.save(.light, profileID: nil)
        let reloaded = ThemePreferences(defaults: defaults)
        try expectEqual(reloaded.load(profileID: "a"), .terminal)
        try expectEqual(reloaded.load(profileID: "b"), .midnight)
        try expectEqual(reloaded.load(profileID: nil), .light)
        defaults.set("future-theme", forKey: "desktop.theme.a")
        try expectEqual(reloaded.load(profileID: "a"), .system)
        print("PASS: theme catalog, persistence, account isolation and unknown-theme fallback")
    }
}
