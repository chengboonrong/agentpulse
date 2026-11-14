// swift-tools-version: 6.4
import PackageDescription

let package = Package(
    name: "AgentPulseLink",
    platforms: [.macOS(.v14), .iOS(.v17)],
    products: [
        .library(name: "AgentPulseLink", targets: ["AgentPulseLink"]),
        .library(name: "AgentPulsePhone", targets: ["AgentPulsePhone"]),
    ],
    targets: [
        .target(name: "AgentPulseLink"),
        // The phone's side, shared by the iPhone app, its widgets and phone-sim.
        .target(name: "AgentPulsePhone", dependencies: ["AgentPulseLink"]),
        .testTarget(name: "AgentPulseLinkTests", dependencies: ["AgentPulseLink"]),
        .testTarget(name: "AgentPulsePhoneTests", dependencies: ["AgentPulsePhone"]),
        // scripts/e2e-phones.sh's phone.
        .executableTarget(name: "phone-sim", dependencies: ["AgentPulsePhone"]),
    ]
)
