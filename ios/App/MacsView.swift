import AgentPulsePhone
import SwiftUI

/// The paired Macs: name, last update and agents; an empty state with "Pair a Mac".
struct MacsView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        @Bindable var model = model
        NavigationStack(path: $model.path) {
            Group {
                if let error = model.loadError {
                    ContentUnavailableView(
                        "Can't Read Your Macs", systemImage: "exclamationmark.triangle",
                        description: Text(error))
                } else if model.pairings.isEmpty {
                    ContentUnavailableView {
                        Label("No Macs", systemImage: "laptopcomputer")
                    } description: {
                        Text("On your Mac, open AgentPulse Settings, then Phones, and choose Pair a Phone.")
                    } actions: {
                        Button("Pair a Mac") { model.pairRequest = PairRequest() }
                            .buttonStyle(.borderedProminent)
                    }
                } else {
                    List(model.pairings) { p in
                        NavigationLink(value: p.id) {
                            MacRow(pairing: p, direct: model.direct.contains(p.id), problem: model.problems[p.id])
                        }
                    }
                }
            }
            .navigationTitle("Macs")
            .toolbar {
                if !model.pairings.isEmpty {
                    Button("Pair a Mac", systemImage: "plus") { model.pairRequest = PairRequest() }
                }
            }
            .navigationDestination(for: String.self) { MacView(id: $0) }
            .sheet(item: $model.pairRequest) { PairView(request: $0) }
        }
    }
}

private struct MacRow: View {
    let pairing: Pairing
    let direct: Bool
    let problem: String?

    var body: some View {
        let payload = pairing.lastState.flatMap(WidgetPayload.decode)
        VStack(alignment: .leading, spacing: 4) {
            Text(pairing.macName)
                .font(.headline)
            if let payload {
                HStack(spacing: 6) {
                    Text(AgentPulseWidgetView.headline(payload.agents))
                    if payload.agents.waiting > 0 {
                        Text("\(payload.agents.waiting) need you")
                            .font(.caption.weight(.bold))
                            .foregroundStyle(.white)
                            .padding(.horizontal, 6)
                            .background(Palette(.light).need, in: .capsule)
                    }
                }
                .font(.subheadline)
            }
            Group {
                if let problem {
                    Text(problem).foregroundStyle(.red)
                } else if let at = pairing.lastStateAt {
                    Text("Updated \(at, format: .relative(presentation: .named))\(direct ? " · Direct" : "")")
                } else {
                    Text("Waiting for your Mac")
                }
            }
            .font(.footnote)
            .foregroundStyle(.secondary)
        }
        .padding(.vertical, 2)
    }
}
