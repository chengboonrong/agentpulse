import AgentPulsePhone
import SwiftUI

/// The paired Macs: name, last update and agents; an empty state with "Pair a Mac".
/// Compact width (iPhone, narrow iPad windows) pushes a Mac's view; regular width (iPad)
/// shows Macs, the selected Mac and the selected session side by side. Both keep the
/// selected Mac in `model.path`, so it survives a size class change.
struct MacsView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.horizontalSizeClass) private var sizeClass
    /// The Mac and its session; the Macs column opens from the toolbar or in landscape.
    @State private var columns = NavigationSplitViewVisibility.doubleColumn

    var body: some View {
        @Bindable var model = model
        Group {
            if sizeClass == .regular, model.loadError == nil, let first = model.pairings.first {
                // Portrait hides the Macs column, so with none chosen the first one shows.
                let selected = model.path.first ?? first.id
                NavigationSplitView(columnVisibility: $columns) {
                    macs(selection: Binding(
                        get: { selected },
                        set: {
                            model.path = $0.map { [$0] } ?? []
                            model.session = nil
                        }))
                } content: {
                    MacView(id: selected, session: $model.session)
                } detail: {
                    if let key = model.session {
                        SessionView(id: selected, key: key)
                    } else {
                        ContentUnavailableView("No Session Selected", systemImage: "terminal")
                    }
                }
            } else {
                NavigationStack(path: $model.path) {
                    macs(selection: nil)
                        .navigationDestination(for: String.self) { MacView(id: $0) }
                }
            }
        }
        .sheet(item: $model.pairRequest) { PairView(request: $0) }
        .sheet(isPresented: $model.settingsShown) { SettingsView() }
    }

    private func macs(selection: Binding<String?>?) -> some View {
        Group {
            if let error = model.loadError {
                ContentUnavailableView(
                    "Can't Read Your Macs", systemImage: "exclamationmark.triangle",
                    description: Text(error))
            } else if model.pairings.isEmpty {
                ContentUnavailableView {
                    Label("No Macs", systemImage: "laptopcomputer")
                } description: {
                    Text("1. Install AgentPulse on your Mac from agentpulse.snitco.com.\n2. Open it, choose Phones in the sidebar, and click Pair a Phone.\n3. Tap Pair a Mac below and scan the code.")
                        .multilineTextAlignment(.leading)
                } actions: {
                    Button("Pair a Mac") { model.pairRequest = PairRequest() }
                        .buttonStyle(.borderedProminent)
                }
            } else {
                List(model.pairings, selection: selection) { p in
                    NavigationLink(value: p.id) {
                        MacRow(pairing: p, direct: model.direct.contains(p.id), problem: model.problems[p.id])
                    }
                }
            }
        }
        .navigationTitle("Macs")
        .toolbar {
            Button("Settings", systemImage: "gearshape") { model.settingsShown = true }
            if !model.pairings.isEmpty {
                Button("Pair a Mac", systemImage: "plus") { model.pairRequest = PairRequest() }
            }
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
                } else if payload == nil,
                    let check = pairing.lastState.map(PayloadFormat.check),
                    check == .macTooOld || check == .phoneTooOld
                {
                    Text("Needs an update")
                } else if let at = pairing.lastStateAt {
                    if direct {
                        Text("Updated \(at, format: .relative(presentation: .named)) · Direct")
                    } else {
                        Text("Updated \(at, format: .relative(presentation: .named))")
                    }
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
