// Widget views, shared by the extension and the preview renderer.

import SwiftUI
import WidgetKit

struct AgentPulseWidgetView: View {
    let state: WidgetState
    let family: WidgetFamily
    /// Forces the single-tone cat; nil follows the widget rendering mode.
    var flatten: Bool? = nil

    @Environment(\.widgetRenderingMode) private var renderingMode
    @Environment(\.colorScheme) private var scheme

    var body: some View {
        let silhouette = flatten ?? (renderingMode != .fullColor)
        let palette = Palette(scheme)
        Group {
            switch state {
            case .notRunning:
                sleeping(Text("AgentPulse isn't running"), "Open the app to resume", silhouette)
            case let .stale(since):
                sleeping(Text("No update since \(Text(since, style: .time))"),
                         "Open AgentPulse to refresh", silhouette)
            case let .live(p, asOf, now) where family == .systemMedium:
                HStack(alignment: .top, spacing: 14) {
                    summary(p, asOf: asOf, silhouette: silhouette)
                        .frame(width: 132)
                    detail(p, now: now, silhouette: silhouette)
                        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
                }
            case let .live(p, asOf, _):
                summary(p, asOf: asOf, silhouette: silhouette)
            }
        }
        .fontDesign(.rounded)
        .monospacedDigit()
        .environment(\.widgetPalette, palette)
        .containerBackground(palette.ground, for: .widget)
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
                    Stage(cat: cat, silhouette: silhouette)
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
                    Stage(cat: cat, silhouette: silhouette)
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
            Stage(cat: p.cat, silhouette: silhouette)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .padding(.bottom, 4)
            HStack(spacing: 6) {
                Text(Self.headline(p.agents))
                    .font(.headline.weight(.heavy))
                    .lineLimit(1)
                    .minimumScaleFactor(0.8)
                if let (status, n) = Self.lead(p.agents) {
                    Ribbon(status: status, count: n, silhouette: silhouette)
                }
            }
            Text("\(p.today.cost) today")
                .font(.subheadline)
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .minimumScaleFactor(0.75)
                .accessibilityLabel(p.today.cost == "—" ? "Cost today unavailable" : "Cost today \(p.today.cost), estimate")
            if let asOf {
                // Advances on its own between reloads.
                Text("Updated \(Text(asOf, style: .relative)) ago")
                    // The widget's words are English; the system locale would mix languages.
                    .environment(\.locale, Locale(identifier: "en_US"))
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .minimumScaleFactor(0.8)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
    }

    private func detail(_ p: WidgetPayload, now: Date, silhouette: Bool) -> some View {
        VStack(alignment: .leading, spacing: 7) {
            if p.sessions.isEmpty {
                Text("No live sessions")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            } else {
                ForEach(Array(p.sessions.prefix(3).enumerated()), id: \.offset) { _, s in
                    SessionRow(session: s, silhouette: silhouette)
                }
            }
            Spacer(minLength: 0)
            if let limit = p.codexLimit {
                LimitBar(limit: limit, outOfDate: limit.outOfDate(at: now), old: limit.old(at: now))
            }
            Text("Today \(p.today.tokens) tokens")
                .font(.caption)
                .foregroundStyle(.secondary)
                .lineLimit(1)
        }
        .padding(.top, 2)
    }

    static func headline(_ a: WidgetPayload.Agents) -> String {
        switch a.active + a.waiting + a.idle {
        case 0: "No agents"
        case 1: "1 agent"
        case let n: "\(n) agents"
        }
    }

    /// The ribbon beside the headline: who needs you leads, as in the app's roster.
    static func lead(_ a: WidgetPayload.Agents) -> (String, Int)? {
        if a.waiting > 0 { return ("Waiting", a.waiting) }
        if a.active > 0 { return ("Active", a.active) }
        if a.idle > 0 { return ("Idle", a.idle) }
        return nil
    }
}

/// The app's colours: cream ground, lilac stage, one ribbon colour per state.
struct Palette {
    let ground, stage, need, work, idle, codex, warn, full: Color

    init(_ scheme: ColorScheme) {
        let dark = scheme == .dark
        ground = Color(hex: dark ? 0x1F1823 : 0xFFF6E8)
        stage = Color(hex: dark ? 0x30263A : 0xF1EAF3)
        need = Color(hex: 0xD92D6A)
        work = Color(hex: 0x2F6FE0)
        idle = Color(hex: 0x6E737B)
        // The app's Codex series, waiting and failed colours.
        codex = Color(hex: dark ? 0xA3AEF0 : 0x5F6FCB)
        warn = Color(hex: dark ? 0xE59A55 : 0xB9520A)
        full = Color(hex: dark ? 0xF0857C : 0xB4423E)
    }

    /// A limit bar's fill, warning as the window fills, as in the app.
    func limit(_ percent: Double) -> Color {
        percent >= 90 ? full : percent >= 75 ? warn : codex
    }

    func ribbon(_ status: String) -> (Color, String) {
        switch status {
        case "Waiting": (need, "needs you")
        case "Active": (work, "working")
        default: (idle, status.lowercased())
        }
    }
}

private struct PaletteKey: EnvironmentKey {
    static let defaultValue = Palette(.light)
}

extension EnvironmentValues {
    var widgetPalette: Palette {
        get { self[PaletteKey.self] }
        set { self[PaletteKey.self] = newValue }
    }
}

private extension Color {
    init(hex: UInt32) {
        self.init(.sRGB, red: Double(hex >> 16 & 0xFF) / 255,
                  green: Double(hex >> 8 & 0xFF) / 255, blue: Double(hex & 0xFF) / 255)
    }
}

/// The cat on its rounded stage. Flattened modes drop the stage so the
/// silhouette keeps its shape.
private struct Stage: View {
    let cat: WidgetPayload.Cat
    let silhouette: Bool
    @Environment(\.widgetPalette) private var palette

    var body: some View {
        CatView(cat: cat, silhouette: silhouette)
            .padding(6)
            .background {
                if !silhouette {
                    RoundedRectangle(cornerRadius: 12, style: .continuous).fill(palette.stage)
                }
            }
    }
}

/// A state's ribbon: white on the state colour, or an outline when flattened.
private struct Ribbon: View {
    let status: String
    var count: Int? = nil
    let silhouette: Bool
    @Environment(\.widgetPalette) private var palette

    var body: some View {
        let (fill, label) = palette.ribbon(status)
        Text(count.map { $0 > 1 && status == "Waiting" ? "\($0) need you" : "\($0) \(label)" } ?? label)
            .font(.caption2.weight(.bold))
            .lineLimit(1)
            .fixedSize()
            .padding(.horizontal, 6)
            .padding(.vertical, 1)
            .foregroundStyle(silhouette ? AnyShapeStyle(.primary) : AnyShapeStyle(Color.white))
            .background {
                if silhouette {
                    Capsule().strokeBorder(.primary, lineWidth: 1)
                } else {
                    Capsule().fill(fill)
                }
            }
    }
}

/// Codex's plan limit: its name, a thin bar and the share used; text alone once out of date.
private struct LimitBar: View {
    let limit: WidgetPayload.Limit
    let outOfDate: Bool
    /// The reading is over an hour old.
    let old: Bool
    @Environment(\.widgetPalette) private var palette

    var body: some View {
        VStack(alignment: .leading, spacing: 1) {
            HStack(spacing: 5) {
                Text(outOfDate ? "\(limit.label) limit out of date" : limit.label)
                    .font(.caption2.weight(.bold))
                    .lineLimit(1)
                if !outOfDate {
                    let used = Double(limit.percent) / 100
                    Capsule()
                        .fill(.secondary.opacity(0.35))
                        .overlay(alignment: .leading) {
                            GeometryReader { g in
                                Capsule().fill(palette.limit(limit.usedPercent))
                                    .frame(width: g.size.width * used)
                            }
                        }
                        .frame(height: 4)
                    Text("\(limit.percent)%")
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                }
            }
            if old {
                Text("as of \(Text(limit.asOf, style: .time))")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .environment(\.locale, Locale(identifier: "en_US"))
            }
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(outOfDate ? "\(limit.label) limit out of date"
            : "\(limit.label) limit \(limit.percent) percent used")
    }
}

private struct SessionRow: View {
    let session: WidgetPayload.Session
    let silhouette: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 1) {
            HStack(spacing: 5) {
                Ribbon(status: session.status, silhouette: silhouette)
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
                .font(.caption.weight(.heavy))
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
