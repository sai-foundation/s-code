// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "SCodeDesktop",
    platforms: [.macOS(.v14)],
    products: [.executable(name: "SCodeDesktop", targets: ["SCodeDesktop"])],
    targets: [
        .target(name: "DesktopCore"),
        .executableTarget(name: "SCodeDesktop", dependencies: ["DesktopCore"]),
        .executableTarget(name: "DesktopChecks", dependencies: ["DesktopCore"], path: "Tests/DesktopCoreTests")
    ],
    swiftLanguageModes: [.v5]
)
