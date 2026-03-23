// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "contacts-helper",
    platforms: [.macOS(.v13)],
    targets: [
        .executableTarget(
            name: "contacts-helper",
            path: "Sources"
        )
    ]
)
