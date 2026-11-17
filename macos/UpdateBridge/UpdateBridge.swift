// Hands an update to Sparkle for the Rust app: it fetches the appcast, asks, downloads,
// verifies (the EdDSA signature, then the new app's code signature) and installs. Loaded
// with dlopen from Contents/Frameworks by crates/agentpulse-app/src/updates.rs when the
// user clicks Install; until then Sparkle is not started and makes no request.

import AppKit
import Sparkle

@MainActor private var controller: SPUStandardUpdaterController?

/// Main thread only.
@_cdecl("agentpulse_install_update")
public func agentpulseInstallUpdate() {
    MainActor.assumeIsolated {
        if controller == nil {
            // Sparkle's session would otherwise keep GitHub's cookies and send them back.
            HTTPCookieStorage.shared.cookieAcceptPolicy = .never
            let c = SPUStandardUpdaterController(
                startingUpdater: false, updaterDelegate: nil, userDriverDelegate: nil)
            let version = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String
            c.updater.userAgentString = "AgentPulse/\(version ?? "0")"
            c.updater.httpHeaders = ["Accept-Language": "en"]
            do {
                try c.updater.start()
            } catch {
                // Not kept, so the next click tries again instead of doing nothing.
                NSAlert(error: error).runModal()
                return
            }
            controller = c
        }
        controller?.checkForUpdates(nil)
    }
}
