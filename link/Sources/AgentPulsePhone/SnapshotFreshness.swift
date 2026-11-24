import Foundation

/// Foreground phone status; a relay response alone does not mean the Mac is awake.
public enum SnapshotFreshness: Equatable, Sendable {
    case fresh, macQuiet, unreachable

    /// Two five-minute publishing cycles, including slack for an unchanged Mac.
    public static let freshAgeLimit: TimeInterval = 10 * 60

    public static func stateFetchFailedInTransport(_ error: any Error) -> Bool {
        error is URLError
    }

    public static func evaluate(generatedAt: Date, now: Date, failed: Bool, directOpen: Bool = false, removed: Bool = false) -> Self {
        if directOpen { return .fresh }
        // A removed pairing shows its removal error, not "can't reach"; its counts still age.
        if failed && !removed { return .unreachable }
        return now.timeIntervalSince(generatedAt) <= freshAgeLimit ? .fresh : .macQuiet
    }
}
