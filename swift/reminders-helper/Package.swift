// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "reminders-helper",
    platforms: [.macOS(.v13)],
    targets: [
        .executableTarget(
            name: "reminders-helper",
            path: "Sources"
        )
    ]
)
