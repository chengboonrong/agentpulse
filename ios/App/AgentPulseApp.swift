import SwiftUI

@main
struct AgentPulseApp: App {
    @State private var loaded = Result { try AppModel.make() }
    @Environment(\.scenePhase) private var scenePhase

    var body: some Scene {
        WindowGroup {
            switch loaded {
            case .success(let model):
                MacsView()
                    .environment(model)
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
            case .active: model.start()
            case .background: model.stop()
            default: break
            }
        }
    }
}
