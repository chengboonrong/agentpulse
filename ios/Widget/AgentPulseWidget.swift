import AgentPulsePhone
import SwiftUI
import WidgetKit

struct AgentPulseEntry: TimelineEntry {
    let date: Date
    /// Nil when no Mac is paired.
    let state: WidgetState?
    /// Paired, but no state from the Mac yet.
    var waitingFor: String?
}

/// The first paired Mac: `GET down` in the timeline, falling back to the stored last state
/// (shared with the app through the App Group). The relay only; never direct.
struct Provider: TimelineProvider {
    static let refresh: TimeInterval = 15 * 60

    func placeholder(in context: Context) -> AgentPulseEntry {
        AgentPulseEntry(date: .now, state: .preview)
    }

    func getSnapshot(in context: Context, completion: @escaping (AgentPulseEntry) -> Void) {
        guard !context.isPreview else { return completion(placeholder(in: context)) }
        let now = Date.now
        completion(Self.entry((try? Self.client?.store.pairings())?.first, now))
    }

    /// The data now, and the same data turning stale once it passes the age limit.
    func getTimeline(in context: Context, completion: @escaping (Timeline<AgentPulseEntry>) -> Void) {
        // WidgetKit's completion isn't Sendable; it is called once, from this task.
        nonisolated(unsafe) let completion = completion
        Task {
            let now = Date.now
            var pairing = (try? Self.client?.store.pairings())?.first
            if let p = pairing, let client = Self.client { pairing = (try? await client.fetchDown(p)) ?? p }
            var entries = [Self.entry(pairing, now)]
            if let pairing, case .live(_, let asOf?) = entries[0].state {
                let staleAt = asOf.addingTimeInterval(WidgetState.ageLimit + 1)
                entries.append(Self.entry(pairing, staleAt))
            }
            completion(Timeline(entries: entries, policy: .after(now.addingTimeInterval(Self.refresh))))
        }
    }

    static let client = PhoneStore.appGroup("group.io.github.chengboonrong.AgentPulse").map { PhoneClient(store: $0) }

    static func entry(_ p: Pairing?, _ date: Date) -> AgentPulseEntry {
        guard let p, p.lastState != nil else { return AgentPulseEntry(date: date, state: nil, waitingFor: p?.macName) }
        return AgentPulseEntry(date: date, state: WidgetState.from(data: p.lastState, now: date))
    }
}

struct AgentPulseWidget: Widget {
    var body: some WidgetConfiguration {
        StaticConfiguration(kind: "AgentPulse", provider: Provider()) { entry in
            EntryView(entry: entry)
        }
        .configurationDisplayName("AgentPulse")
        .description("Your Mac's AI coding agents at a glance.")
        .supportedFamilies([.systemSmall, .systemMedium])
    }
}

private struct EntryView: View {
    let entry: AgentPulseEntry
    @Environment(\.widgetFamily) private var family

    var body: some View {
        if let state = entry.state {
            AgentPulseWidgetView(state: state, family: family)
        } else {
            Text(entry.waitingFor.map { "Waiting for \($0)" } ?? "Pair a Mac in AgentPulse")
                .font(.headline)
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .bottomLeading)
                .containerBackground(.background, for: .widget)
        }
    }
}

@main
struct AgentPulseWidgets: WidgetBundle {
    var body: some Widget {
        AgentPulseWidget()
    }
}
