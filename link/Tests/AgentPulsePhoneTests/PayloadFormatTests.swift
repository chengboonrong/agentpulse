import Foundation
import Testing

@testable import AgentPulsePhone

struct PayloadFormatTests {
    @Test func versions() {
        #expect(PayloadFormat.supported == 1...1)
        #expect(PayloadFormat.check(Data(#"{"version":0}"#.utf8)) == .macTooOld)
        #expect(PayloadFormat.check(Data(#"{"version":1,"sessions":"ignored"}"#.utf8)) == .current)
        #expect(PayloadFormat.check(Data(#"{"version":2}"#.utf8)) == .phoneTooOld)
    }

    @Test(arguments: ["{}", "not JSON", #"{"version":"1"}"#, #"{"version":null}"#, #"{"version":1.5}"#])
    func unreadable(json: String) {
        #expect(PayloadFormat.check(Data(json.utf8)) == .unreadable)
    }
}
