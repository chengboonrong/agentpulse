import AgentPulsePhone
import SwiftUI
import UserNotifications

struct NotificationsView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        List {
            if model.notificationStatus == .notDetermined {
                Section {
                    Text("Allow notifications to get alerts from your Macs.")
                    Button("Allow Notifications") {
                        Task { await model.requestNotifications() }
                    }
                }
            }
            if let error = model.notificationError {
                Section { Text(error) }
            }
            if model.notificationStatus == .denied {
                Section {
                    Text("Notifications are off for AgentPulse in Settings.")
                    Button("Open Settings") {
                        if let url = URL(string: UIApplication.openNotificationSettingsURLString) {
                            UIApplication.shared.open(url)
                        }
                    }
                }
            }
            Section {
                Toggle("Agent waiting for you", isOn: choice(\.waiting))
                Toggle("Agent finished its turn", isOn: choice(\.finished))
                Toggle("Agent failed", isOn: choice(\.failed))
            } footer: {
                Text("Your Macs send only what's on. Alert text is end-to-end encrypted; the relay and Apple see only that an alert was sent.")
            }
            .disabled(!model.notificationsAllowed)
        }
        .navigationTitle("Notifications")
    }

    private func choice(_ key: WritableKeyPath<NotifyChoices, Bool>) -> Binding<Bool> {
        Binding(get: { model.notificationsAllowed && model.notifyChoices[keyPath: key] }, set: { model.setChoice(key, $0) })
    }
}
