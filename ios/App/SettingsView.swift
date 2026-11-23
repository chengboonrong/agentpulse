import AgentPulsePhone
import SwiftUI

struct SettingsView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        @Bindable var model = model
        NavigationStack {
            List {
                Section("Macs") {
                    if model.pairings.isEmpty {
                        ContentUnavailableView("No Macs", systemImage: "laptopcomputer")
                    }
                    ForEach(model.pairings) { p in
                        NavigationLink(p.macName) { MacSettingsView(pairing: p) }
                    }
                }
                Section {
                    NavigationLink {
                        NotificationsView()
                    } label: {
                        LabeledContent("Notifications", value: notificationSummary)
                    }
                }
                Section("Connection") {
                    Toggle("Connect directly on the same network", isOn: $model.directEnabled)
                }
                Section("About") {
                    LabeledContent("Version", value: Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "—")
                    Link("Privacy Policy", destination: URL(string: "https://agentpulse.snitco.com/privacy.html")!)
                    LabeledContent("Relay", value: relayHosts)
                }
            }
            .navigationTitle("Settings")
            .navigationDestination(isPresented: $model.notificationsShown) { NotificationsView() }
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
        }
    }

    private var notificationSummary: String {
        // Show the effective choices sent to the Macs.
        guard model.notificationsAllowed else { return "Off" }
        let choices = model.notifyChoices
        let names = [(choices.waiting, "Waiting"), (choices.finished, "Finished"), (choices.failed, "Failed")]
            .filter { $0.0 }.map { $0.1 }
        return names.isEmpty ? "Off" : names.joined(separator: ", ")
    }

    private var relayHosts: String {
        let hosts = Set(model.pairings.compactMap { $0.relay.host() }).sorted()
        return hosts.isEmpty ? "None" : hosts.joined(separator: ", ")
    }
}

private struct MacSettingsView: View {
    let pairing: Pairing
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var name: String
    @State private var confirmingRemove = false

    init(pairing: Pairing) {
        self.pairing = pairing
        _name = State(initialValue: pairing.macName)
    }

    var body: some View {
        List {
            Section {
                TextField("Name", text: $name)
                    .onSubmit { save() }
            }
            Section {
                Button("Remove", role: .destructive) { confirmingRemove = true }
            }
        }
        .navigationTitle(model.pairing(pairing.id)?.macName ?? pairing.macName)
        .onDisappear { save() }
        .confirmationDialog("Remove \(model.pairing(pairing.id)?.macName ?? pairing.macName)?", isPresented: $confirmingRemove, titleVisibility: .visible) {
            Button("Remove", role: .destructive) {
                Task {
                    await model.remove(pairing)
                    dismiss()
                }
            }
        } message: {
            Text("This \(UIDevice.current.model) stops showing this Mac, and the Mac stops sending to it.")
        }
    }

    private func save() {
        guard let current = model.pairing(pairing.id), name != current.macName else { return }
        model.rename(current, to: name)
    }
}
