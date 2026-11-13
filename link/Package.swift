// swift-tools-version: 6.4
import PackageDescription

let package = Package(
    name: "AgentPulseLink",
    platforms: [.macOS(.v14), .iOS(.v17)],
    products: [.library(name: "AgentPulseLink", targets: ["AgentPulseLink"])],
    targets: [
        .target(name: "AgentPulseLink"),
        .testTarget(name: "AgentPulseLinkTests", dependencies: ["AgentPulseLink"]),
    ]
)
