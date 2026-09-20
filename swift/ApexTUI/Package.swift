// swift-tools-version:6.0
import PackageDescription

let package = Package(
    name: "ApexTUI",
    platforms: [
        .macOS(.v13)
    ],
    products: [
        .executable(name: "apex-tui", targets: ["ApexTUI"])
    ],
    dependencies: [
        .package(url: "https://github.com/migueldeicaza/TermKit.git", branch: "main")
    ],
    targets: [
        .executableTarget(
            name: "ApexTUI",
            dependencies: ["TermKit"],
            swiftSettings: [.swiftLanguageMode(.v5)]
        )
    ]
)
