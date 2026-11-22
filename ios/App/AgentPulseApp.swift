import SwiftUI
import UIKit

@main
struct AgentPulseApp: App {
    @UIApplicationDelegateAdaptor(PhoneAppDelegate.self) private var appDelegate
    @State private var loaded = Result { try AppModel.make() }
    @Environment(\.scenePhase) private var scenePhase

    var body: some Scene {
        WindowGroup {
            switch loaded {
            case .success(let model):
                MacsView()
                    .environment(model)
                    .onAppear { appDelegate.model = model }
                    .onOpenURL { model.open($0) }
            case .failure(let error):
                ContentUnavailableView(
                    "AgentPulse Can't Start", systemImage: "exclamationmark.triangle",
                    description: Text(error.localizedDescription))
            }
        }
        .onChange(of: scenePhase) { _, phase in
            guard case .success(let model) = loaded else { return }
            switch phase {
            case .active:
                appDelegate.model = model
                model.start()
            case .background: model.stop()
            default: break
            }
        }
    }
}

@MainActor
final class PhoneAppDelegate: NSObject, UIApplicationDelegate {
    weak var model: AppModel?

    func application(_ application: UIApplication, didRegisterForRemoteNotificationsWithDeviceToken deviceToken: Data) {
        model?.receivedPushToken(deviceToken)
    }

    func application(_ application: UIApplication, didFailToRegisterForRemoteNotificationsWithError error: any Error) {
        NSLog("Remote notification registration failed: %@", error.localizedDescription)
        // Still send the choices with a token from earlier, if there is one.
        model?.syncNotifications(register: false)
    }
}
