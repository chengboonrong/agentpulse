// Widget views, shared by the extension and the preview renderer.

import SwiftUI
import WidgetKit

struct AgentPulseWidgetView: View {
    let state: WidgetState
    let family: WidgetFamily
    /// Forces the single-tone cat; nil follows the widget rendering mode.
    var flatten: Bool? = nil

    @Environment(\.widgetRenderingMode) private var renderingMode

    var body: some View {
        let silhouette = flatten ?? (renderingMode != .fullColor)
        Group {
            switch state {
            case .notRunning:
                sleeping(Text("AgentPulse isn't running"), "Open the app to resume", silhouette)
            case let .stale(since):
                sleeping(Text("No update since \(Text(since, style: .time))"),
                         "Open AgentPulse to refresh", silhouette)
            case let .live(p, asOf) where family == .systemMedium:
                HStack(alignment: .top, spacing: 14) {
                    summary(p, asOf: asOf, silhouette: silhouette)
                        .frame(width: 132)
                    detail(p)
                        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
                }
            case let .live(p, asOf):
                summary(p, asOf: asOf, silhouette: silhouette)
            }
        }
        .containerBackground(.background, for: .widget)
        .widgetURL(URL(string: "agentpulse://open"))
    }

    /// No live data: the sleeping cat (when the placeholder resource exists),
    /// the title, and in the medium size the subtitle.
    @ViewBuilder
    private func sleeping(_ title: Text, _ subtitle: String, _ silhouette: Bool) -> some View {
        let cat = WidgetPayload.placeholder?.cat
        if family == .systemMedium {
            HStack(spacing: 14) {
                if let cat {
                    CatView(cat: cat, silhouette: silhouette)
                        .frame(width: 132)
                }
                VStack(alignment: .leading, spacing: 2) {
                    title
                        .font(.headline)
                    Text(subtitle)
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }
        } else {
            VStack(alignment: .leading, spacing: 2) {
                if let cat {
                    CatView(cat: cat, silhouette: silhouette)
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                } else {
                    Spacer(minLength: 0)
                }
                title
                    .font(.headline)
                    .lineLimit(2)
                    .minimumScaleFactor(0.8)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
        }
    }

    private func summary(_ p: WidgetPayload, asOf: Date?, silhouette: Bool) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            CatView(cat: p.cat, silhouette: silhouette)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            Text(Self.headline(p.agents))
                .font(.headline)
                .lineLimit(1)
            Text("\(p.today.cost) today")
                .font(.subheadline)
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .minimumScaleFactor(0.75)
                .accessibilityLabel(p.today.cost == "—" ? "Cost today unavailable" : "Cost today \(p.today.cost), estimate")
            if let asOf {
                Text("as of \(Text(asOf, style: .time))")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .minimumScaleFactor(0.8)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
    }

    private func detail(_ p: WidgetPayload) -> some View {
        VStack(alignment: .leading, spacing: 7) {
            if p.sessions.isEmpty {
                Text("No live sessions")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            } else {
                ForEach(Array(p.sessions.prefix(3).enumerated()), id: \.offset) { _, s in
                    SessionRow(session: s)
                }
            }
            Spacer(minLength: 0)
            Text("Today \(p.today.tokens) tokens")
                .font(.caption)
                .foregroundStyle(.secondary)
                .lineLimit(1)
        }
        .padding(.top, 2)
    }

    static func headline(_ a: WidgetPayload.Agents) -> String {
        if a.active > 0 { return "\(a.active) active" }
        if a.waiting > 0 { return "\(a.waiting) waiting" }
        if a.idle > 0 { return "\(a.idle) idle" }
        return "No agents"
    }
}

private struct SessionRow: View {
    let session: WidgetPayload.Session

    private var color: Color {
        switch session.status {
        case "Active": .green
        case "Waiting": .orange
        default: .gray
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 1) {
            HStack(spacing: 5) {
                Circle()
                    .fill(color)
                    .frame(width: 7, height: 7)
                Text(session.status)
                    .font(.caption2.weight(.semibold))
                    .foregroundStyle(.secondary)
                Spacer(minLength: 4)
                if let ms = session.startedAtMs {
                    Text(Date(timeIntervalSince1970: Double(ms) / 1000), style: .timer)
                        .font(.caption2.monospacedDigit())
                        .foregroundStyle(.secondary)
                        .multilineTextAlignment(.trailing)
                        .frame(maxWidth: 64, alignment: .trailing)
                }
            }
            Text(session.project)
                .font(.caption.weight(.medium))
                .lineLimit(1)
                .truncationMode(.tail)
        }
    }
}

/// The payload's ASCII sprite at the largest whole-number scale that fits.
/// `silhouette` draws one tone with the eye cut out, for rendering modes
/// that flatten colours.
struct CatView: View {
    let cat: WidgetPayload.Cat
    let silhouette: Bool

    var body: some View {
        Canvas { ctx, size in
            let w = CGFloat(max(cat.width, 1)), h = CGFloat(max(cat.height, 1))
            let scale = max(1, floor(min(size.width / w, size.height / h)))
            let ox = floor((size.width - w * scale) / 2)
            let oy = floor((size.height - h * scale) / 2)
            var paths: [Character: Path] = [:]
            for (y, row) in cat.rows.enumerated() {
                for (x, ch) in row.enumerated() where cat.palette[String(ch)] != nil {
                    paths[ch, default: Path()].addRect(CGRect(
                        x: ox + CGFloat(x) * scale, y: oy + CGFloat(y) * scale,
                        width: scale, height: scale))
                }
            }
            if silhouette {
                var all = Path()
                for (ch, p) in paths where ch != "e" { all.addPath(p) }
                ctx.fill(all, with: .foreground)
            } else {
                for (ch, p) in paths {
                    guard let c = cat.palette[String(ch)], c.count == 4 else { continue }
                    let v = c.map { Double($0) / 255 }
                    ctx.fill(p, with: .color(Color(.sRGB, red: v[0], green: v[1], blue: v[2], opacity: v[3])))
                }
            }
        }
        .accessibilityLabel("Cat, \(cat.state.replacingOccurrences(of: "_", with: " "))")
    }
}
