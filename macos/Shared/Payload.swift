// The widget.json contract (version 1), written by
// crates/agentpulse-app/src/widget_export.rs. Shared by the widget extension
// and the preview renderer.

import Foundation

struct WidgetPayload: Decodable {
    struct Agents: Decodable {
        let active: Int
        let waiting: Int
        let idle: Int
    }

    struct Today: Decodable {
        let tokens: String
        let cost: String
        let costNote: String
        let unpriced: Int
    }

    struct Cat: Decodable {
        let state: String
        let width: Int
        let height: Int
        let rows: [String]
        /// Sprite char to RGBA 0...255.
        let palette: [String: [Int]]
    }

    struct Session: Decodable {
        let project: String
        let status: String
        let model: String
        let startedAtMs: Int64?
        let tokens: String
        let cost: String
    }

    let version: Int
    let generatedAtMs: Int64
    let stopped: Bool
    let agents: Agents
    let today: Today
    let outputRate: String
    let cat: Cat
    let sessions: [Session]

    var generatedAt: Date { Date(timeIntervalSince1970: Double(generatedAtMs) / 1000) }
}

/// What a widget shows.
enum WidgetState {
    /// Data as of `asOf`; nil for the gallery's generic preview.
    case live(WidgetPayload, asOf: Date?)
    /// The app wrote data but nothing newer than `ageLimit`: it may have
    /// crashed, or be alive and out of reload budget, so claim neither.
    case stale(since: Date)
    /// No file, an unreadable or unknown-version file, or the app marked it stopped.
    case notRunning

    /// Must exceed the timeline's 60-minute refresh period.
    static let ageLimit: TimeInterval = 75 * 60

    static func from(data: Data?, now: Date) -> WidgetState {
        guard let p = data.flatMap(WidgetPayload.decode), !p.stopped else { return .notRunning }
        if now.timeIntervalSince(p.generatedAt) > ageLimit { return .stale(since: p.generatedAt) }
        return .live(p, asOf: p.generatedAt)
    }

    /// The gallery's generic preview; text-only "isn't running" without the resource.
    static var preview: WidgetState {
        WidgetPayload.placeholder.map { .live($0, asOf: nil) } ?? .notRunning
    }
}

extension WidgetPayload {
    /// Version 1 only; nil for anything else or anything undecodable.
    static func decode(_ data: Data) -> WidgetPayload? {
        struct Version: Decodable { let version: Int }
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        guard (try? decoder.decode(Version.self, from: data))?.version == 1 else { return nil }
        return try? decoder.decode(WidgetPayload.self, from: data)
    }

    /// `placeholder.json`, generated at build time by `agentpulse --widget-placeholder`
    /// (zero agents, the real sleeping cat) and shipped as a resource. Looked up in
    /// the bundle's resources, then next to the executable (the preview tool).
    static let placeholder: WidgetPayload? = {
        let url = Bundle.main.url(forResource: "placeholder", withExtension: "json")
            ?? Bundle.main.executableURL?.deletingLastPathComponent()
                .appendingPathComponent("placeholder.json")
        return url.flatMap { try? Data(contentsOf: $0) }.flatMap(decode)
    }()
}
