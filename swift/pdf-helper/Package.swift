// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "pdf-helper",
    platforms: [.macOS(.v13)],
    targets: [
        .executableTarget(
            name: "pdf-helper",
            path: "Sources"
        )
    ]
)
