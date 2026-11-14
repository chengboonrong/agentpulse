import SwiftUI
import WidgetKit

struct AgentPulseEntry: TimelineEntry {
    let date: Date
    let state: WidgetState
}

struct Provider: TimelineProvider {
    func placeholder(in context: Context) -> AgentPulseEntry {
        AgentPulseEntry(date: .now, state: .preview)
    }

    func getSnapshot(in context: Context, completion: @escaping (AgentPulseEntry) -> Void) {
        completion(context.isPreview ? placeholder(in: context) : load())
    }

    /// Two entries: the data now, and the same data turning stale once it passes
    /// the age limit, so a crashed app never leaves old numbers looking live.
    func getTimeline(in context: Context, completion: @escaping (Timeline<AgentPulseEntry>) -> Void) {
        let now = Date.now
        let data = readData()
        var entries = [AgentPulseEntry(date: now, state: WidgetState.from(data: data, now: now))]
        if case let .live(p, asOf?, _) = entries[0].state {
            let staleAt = asOf.addingTimeInterval(WidgetState.ageLimit + 1)
            // The limit bar turns "out of date" at its reset, if that comes first.
            if let resetAt = p.codexLimit?.resetsAt, resetAt > now, resetAt < staleAt {
                entries.append(AgentPulseEntry(date: resetAt, state: WidgetState.from(data: data, now: resetAt)))
            }
            entries.append(AgentPulseEntry(date: staleAt, state: WidgetState.from(data: data, now: staleAt)))
        }
        completion(Timeline(entries: entries, policy: .after(now.addingTimeInterval(60 * 60))))
    }

    private func load() -> AgentPulseEntry {
        let now = Date.now
        return AgentPulseEntry(date: now, state: WidgetState.from(data: readData(), now: now))
    }

    /// The file the app writes. The sandbox exception in the extension's
    /// entitlements allows exactly this directory, read-only. In the sandbox
    /// NSHomeDirectory() is the container, so the real home comes from getpwuid.
    private func readData() -> Data? {
        getpwuid(getuid()).flatMap { pw in
            FileManager.default.contents(
                atPath: String(cString: pw.pointee.pw_dir)
                    + "/Library/Application Support/AgentPulse/Widget/widget.json")
        }
    }
}

struct AgentPulseWidget: Widget {
    var body: some WidgetConfiguration {
        StaticConfiguration(kind: "AgentPulse", provider: Provider()) { entry in
            EntryView(entry: entry)
        }
        .configurationDisplayName("AgentPulse")
        .description("Your AI coding agents at a glance.")
        .supportedFamilies([.systemSmall, .systemMedium])
    }
}

private struct EntryView: View {
    let entry: AgentPulseEntry
    @Environment(\.widgetFamily) private var family

    var body: some View {
        AgentPulseWidgetView(state: entry.state, family: family)
    }
}

@main
struct AgentPulseWidgets: WidgetBundle {
    var body: some Widget {
        AgentPulseWidget()
    }
}
