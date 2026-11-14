#if DEBUG
import AgentPulseLink
import AgentPulsePhone
import CryptoKit
import Foundation

/// `-AgentPulseFixture <macs|mac|pair-confirm|pair-code|empty>`: each screen from fixture
/// data, without network, for screenshots. Debug builds only.
@MainActor
enum Fixtures {
    static func model(_ name: String) -> AppModel {
        let dir = FileManager.default.temporaryDirectory.appending(path: "fixtures")
        let model = AppModel(client: PhoneClient(store: PhoneStore(container: dir, secrets: MemorySecrets())), offline: true)
        let studio = pairing("Studio", minutesAgo: 1, payload: busy)
        switch name {
        case "macs":
            model.pairings = [
                studio, pairing("MacBook Pro", minutesAgo: 42, payload: quiet), pairing("Mac mini", minutesAgo: nil),
            ]
            model.direct = [studio.id]
        case "mac":
            model.pairings = [studio]
            model.direct = [studio.id]
            model.path = [studio.id]
        case "pair-confirm":
            model.pairRequest = PairRequest(step: .confirm(link))
        case "pair-code":
            model.pairRequest = PairRequest(step: .code("042137", pairing("Studio", minutesAgo: nil)))
        default:
            break
        }
        return model
    }

    static let link = try! PairingLink(
        relay: URL(string: "https://relay.example.com")!, id: Data(repeating: 1, count: 16),
        macKey: Curve25519.KeyAgreement.PrivateKey().publicKey, secret: Data(repeating: 2, count: 32),
        macName: "Studio")

    static func pairing(_ name: String, minutesAgo: Double?, payload: (Int64) -> String = { _ in "" }) -> Pairing {
        var p = Pairing(
            id: name, relay: URL(string: "https://relay.example.com")!, macName: name,
            macKey: Data(repeating: 3, count: 32))
        if let minutesAgo {
            let at = Date.now.addingTimeInterval(-minutesAgo * 60)
            p.lastState = Data(payload(Int64(at.timeIntervalSince1970 * 1000)).utf8)
            p.lastStateAt = at
        }
        return p
    }

    static func busy(_ ms: Int64) -> String {
        payload(ms, agents: (2, 1, 1), sessions: """
            {"project":"agentpulse","status":"Waiting","model":"opus","started_at_ms":\(ms - 1_520_000),"tokens":"1.2M","cost":"$4.10"},
            {"project":"kl-rush","status":"Active","model":"gpt-5","started_at_ms":\(ms - 610_000),"tokens":"380k","cost":"$1.25"},
            {"project":"relay","status":"Active","model":"sonnet","started_at_ms":\(ms - 95_000),"tokens":"41k","cost":"$0.12"},
            {"project":"notes","status":"Idle","model":"haiku","started_at_ms":\(ms - 3_400_000),"tokens":"9k","cost":"$0.01"}
            """)
    }

    static func quiet(_ ms: Int64) -> String { payload(ms, agents: (0, 0, 0), sessions: "") }

    static func payload(_ ms: Int64, agents: (Int, Int, Int), sessions: String) -> String {
        """
        {"version":1,"generated_at_ms":\(ms),"stopped":false,
        "agents":{"active":\(agents.0),"waiting":\(agents.1),"idle":\(agents.2)},
        "today":{"tokens":"2.1M","cost":"$6.40","cost_note":"Estimated from list prices.","unpriced":0},
        "output_rate":"38 tok/s",
        "cat":{"state":"sleeping","width":32,"height":16,"rows":[
        "................................","................................","................................",
        "................................","................................","......................o....o....",
        ".............o..oooooobo..obo...","............osoosbbsbbbboobbo...","...........oobsbbsbbsbbbbbbbbo..",
        "..........obbbbbbbbbbbbbbeebbo..","..........obbbbbbbbbbbbbbbwwpo..","..........obbbbbbbbbbbbbbbboo...",
        "..........obbbwwwbbbbwwwbbbo....","...........osssbbwwwwbsssoo.....","............ooosssssssooo.......",
        "...............ooooooo.........."],
        "palette":{"b":[232,140,56,255],"e":[30,24,20,255],"o":[150,78,30,255],"p":[240,120,140,255],"s":[176,88,28,255],"w":[250,222,176,255]}},
        "sessions":[\(sessions)]}
        """
    }
}
#endif
