import AgentPulsePhone
import SwiftUI

/// One Mac: the cat and counts, live sessions, today; "as of"; Remove.
struct MacView: View {
    let id: String
    @Environment(AppModel.self) private var model
    @Environment(\.colorScheme) private var scheme
    @State private var confirmingRemove = false

    var body: some View {
        if let pairing = model.pairing(id) {
            content(pairing)
        } else {
            ContentUnavailableView("Removed", systemImage: "laptopcomputer.slash")
        }
    }

    private func content(_ pairing: Pairing) -> some View {
        let payload = pairing.lastState.flatMap(WidgetPayload.decode)
        let palette = Palette(scheme)
        return List {
            if let problem = model.problems[id] {
                Section {
                    Label(problem, systemImage: "exclamationmark.triangle")
                        .foregroundStyle(.red)
                }
            }
            if let p = payload {
                Section {
                    CatView(cat: p.cat, silhouette: false)
                        .frame(height: 120)
                        .frame(maxWidth: .infinity)
                        .padding(8)
                        .background(palette.stage, in: .rect(cornerRadius: 12))
                        .listRowInsets(EdgeInsets(top: 8, leading: 8, bottom: 8, trailing: 8))
                    LabeledContent("Needs you", value: p.agents.waiting, format: .number)
                    LabeledContent("Working", value: p.agents.active, format: .number)
                    LabeledContent("Idle", value: p.agents.idle, format: .number)
                } header: {
                    Text(AgentPulseWidgetView.headline(p.agents))
                } footer: {
                    Text("As of \(p.generatedAt, format: .dateTime.hour().minute()) · \(model.direct.contains(id) ? "Connected directly" : "Through the relay")")
                }
                Section("Sessions") {
                    if p.sessions.isEmpty {
                        Text("No live sessions")
                            .foregroundStyle(.secondary)
                    }
                    ForEach(Array(p.sessions.enumerated()), id: \.offset) { _, s in
                        SessionRow(session: s, palette: palette)
                    }
                }
                Section {
                    LabeledContent("Tokens", value: p.today.tokens)
                    LabeledContent("Cost", value: p.today.cost)
                        .accessibilityLabel(p.today.cost == "—" ? "Cost unavailable" : "Cost \(p.today.cost), estimate")
                    LabeledContent("Output rate", value: p.outputRate)
                } header: {
                    Text("Today")
                } footer: {
                    if !p.today.costNote.isEmpty { Text(p.today.costNote) }
                }
            } else {
                Section {
                    Text("Waiting for your Mac to send its first update.")
                        .foregroundStyle(.secondary)
                }
            }
            Section {
                Button("Remove", role: .destructive) { confirmingRemove = true }
            }
        }
        .navigationTitle(pairing.macName)
        .confirmationDialog("Remove \(pairing.macName)?", isPresented: $confirmingRemove, titleVisibility: .visible) {
            Button("Remove", role: .destructive) {
                Task { await model.remove(pairing) }
            }
        } message: {
            Text("This iPhone stops showing this Mac, and the Mac stops sending to it.")
        }
    }
}

private struct SessionRow: View {
    let session: WidgetPayload.Session
    let palette: Palette

    var body: some View {
        let (color, status) = palette.ribbon(session.status)
        VStack(alignment: .leading, spacing: 4) {
            HStack {
                Text(session.project)
                    .font(.headline)
                    .lineLimit(1)
                Spacer()
                if let ms = session.startedAtMs {
                    Text(Date(timeIntervalSince1970: Double(ms) / 1000), style: .timer)
                        .font(.subheadline.monospacedDigit())
                        .foregroundStyle(.secondary)
                }
            }
            HStack(spacing: 6) {
                Text(status)
                    .font(.caption.weight(.bold))
                    .foregroundStyle(.white)
                    .padding(.horizontal, 6)
                    .padding(.vertical, 1)
                    .background(color, in: .capsule)
                Text("\(session.model) · \(session.tokens) tokens · \(session.cost)")
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
        }
        .accessibilityElement(children: .combine)
    }
}
