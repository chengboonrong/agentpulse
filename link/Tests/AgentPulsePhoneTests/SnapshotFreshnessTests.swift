import Foundation
import Testing
@testable import AgentPulsePhone

struct SnapshotFreshnessTests {
    private let now = Date(timeIntervalSince1970: 1_800_000_000)

    @Test func freshAndBoundary() {
        #expect(SnapshotFreshness.evaluate(generatedAt: now.addingTimeInterval(-60), now: now, failed: false) == .fresh)
        #expect(SnapshotFreshness.evaluate(generatedAt: now.addingTimeInterval(-600), now: now, failed: false) == .fresh)
    }

    @Test func quietAfterTenMinutes() {
        #expect(SnapshotFreshness.evaluate(generatedAt: now.addingTimeInterval(-600.001), now: now, failed: false) == .macQuiet)
    }

    @Test func failureOverridesAge() {
        #expect(SnapshotFreshness.evaluate(generatedAt: now, now: now, failed: true) == .unreachable)
        #expect(SnapshotFreshness.evaluate(generatedAt: now.addingTimeInterval(-3600), now: now, failed: true) == .unreachable)
    }

    @Test func recovery() {
        #expect(SnapshotFreshness.evaluate(generatedAt: now.addingTimeInterval(-3600), now: now, failed: true) == .unreachable)
        #expect(SnapshotFreshness.evaluate(generatedAt: now.addingTimeInterval(-3600), now: now, failed: false) == .macQuiet)
        #expect(SnapshotFreshness.evaluate(generatedAt: now, now: now, failed: false) == .fresh)
    }
    @Test func connectionAndRemovalOverrideTransportFailure() {
        let old = now.addingTimeInterval(-3600)
        #expect(SnapshotFreshness.evaluate(generatedAt: old, now: now, failed: true, directOpen: true) == .fresh)
        #expect(SnapshotFreshness.evaluate(generatedAt: old, now: now, failed: true, removed: true) == .macQuiet)
        // A relay 5xx is an HTTP response, not a transport failure.
        #expect(SnapshotFreshness.evaluate(generatedAt: old, now: now, failed: false) == .macQuiet)
    }
    @Test func onlyTransportErrorsFailTheStateFetch() {
        #expect(SnapshotFreshness.stateFetchFailedInTransport(URLError(.notConnectedToInternet)))
        for error in [PhoneError.badResponse(500), .badResponse(429), .removed] {
            #expect(!SnapshotFreshness.stateFetchFailedInTransport(error))
        }
    }
}
