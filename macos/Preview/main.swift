// Renders the widget views to PNG for review, since nobody can add the
// widget to a desktop from a shell.
//
//   widget-preview <widget.json> <out-dir>
//
// Writes {data,stale,not-running,placeholder}-{small,medium}-{light,dark,accented}.png
// at 2x, plus "limit-reset" when the file's Codex limit resets before it turns stale:
// the timeline entry the widget adds at that reset. "placeholder" is the gallery preview from placeholder.json next to
// this executable (scripts/bundle.sh puts it there).
// "data" goes through the same age and version checks as the widget; "stale"
// is the same file seen just past the age limit.

import AppKit
import ImageIO
import SwiftUI
import UniformTypeIdentifiers
import WidgetKit

enum Look: String, CaseIterable { case light, dark, accented }

@MainActor
func render(_ state: WidgetState, family: WidgetFamily, look: Look, to url: URL) throws {
    let size = family == .systemMedium ? CGSize(width: 364, height: 170) : CGSize(width: 170, height: 170)
    let widget = AgentPulseWidgetView(state: state, family: family, flatten: look == .accented ? true : nil)
        .padding(14)
        .frame(width: size.width, height: size.height)
        .environment(\.colorScheme, look == .light ? .light : .dark)
    let page: AnyView = switch look {
    case .light: AnyView(widget.background(Color.white))
    case .dark: AnyView(widget.background(Color(white: 0.16)))
    // Approximates the flattened desktop mode: only opacity survives, in one tint.
    case .accented: AnyView(ZStack {
            Color(red: 0.22, green: 0.24, blue: 0.29)
            Color.white.mask(widget)
        })
    }
    let renderer = ImageRenderer(content: page
        .frame(width: size.width, height: size.height)
        .clipShape(RoundedRectangle(cornerRadius: 20, style: .continuous)))
    renderer.scale = 2
    guard let image = renderer.cgImage,
          let dest = CGImageDestinationCreateWithURL(url as CFURL, UTType.png.identifier as CFString, 1, nil)
    else { throw CocoaError(.fileWriteUnknown) }
    CGImageDestinationAddImage(dest, image, nil)
    guard CGImageDestinationFinalize(dest) else { throw CocoaError(.fileWriteUnknown) }
}

let args = CommandLine.arguments
guard args.count == 3 else {
    FileHandle.standardError.write(Data("usage: widget-preview <widget.json> <out-dir>\n".utf8))
    exit(2)
}
let out = URL(fileURLWithPath: args[2], isDirectory: true)
try FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)
let data = FileManager.default.contents(atPath: args[1])
let decoded = data.flatMap(WidgetPayload.decode)
var states: [(String, WidgetState)] = [
    ("data", WidgetState.from(data: data, now: .now)),
    ("stale", WidgetState.from(data: data, now: data.flatMap(WidgetPayload.decode)
        .map { $0.generatedAt.addingTimeInterval(WidgetState.ageLimit + 1) } ?? .now)),
    ("not-running", WidgetState.from(data: nil, now: .now)),
    ("placeholder", WidgetState.preview),
]
if let p = decoded, let resetAt = p.codexLimit?.resetsAt,
   resetAt < p.generatedAt.addingTimeInterval(WidgetState.ageLimit + 1) {
    states.append(("limit-reset", WidgetState.from(data: data, now: max(resetAt, .now).addingTimeInterval(1))))
}
for (name, state) in states {
    for family in [WidgetFamily.systemSmall, .systemMedium] {
        for look in Look.allCases {
            let f = family == .systemSmall ? "small" : "medium"
            let url = out.appendingPathComponent("\(name)-\(f)-\(look.rawValue).png")
            try render(state, family: family, look: look, to: url)
            print(url.path)
        }
    }
}
