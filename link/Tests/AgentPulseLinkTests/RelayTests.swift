import CryptoKit
import Foundation
import Testing

@testable import AgentPulseLink

typealias PrivateKey = Curve25519.KeyAgreement.PrivateKey
typealias PublicKey = Curve25519.KeyAgreement.PublicKey

func randomId() -> Data { Data((0..<16).map { _ in UInt8.random(in: 0...255) }) }

struct RelayTests {
    let mac = PrivateKey()
    let phone = PrivateKey()
    let id = randomId()
    let text = Data("{\"seq\":1}".utf8)

    @Test func roundTripBothDirections() throws {
        let down = try RelayMessage.seal(text, to: phone.publicKey, from: mac, direction: .down, pairingId: id)
        #expect(try RelayMessage.open(down, with: phone, from: mac.publicKey, direction: .down, pairingId: id) == text)

        let up = try RelayMessage.seal(text, to: mac.publicKey, from: phone, direction: .up, pairingId: id)
        #expect(try RelayMessage.open(up, with: mac, from: phone.publicKey, direction: .up, pairingId: id) == text)

        // base64url, unpadded, of enc (32) || ciphertext (plaintext + 16-byte tag).
        #expect(down.allSatisfy { $0.isLetter || $0.isNumber || $0 == "-" || $0 == "_" })
        #expect(Data(base64URLEncoded: down)?.count == 32 + text.count + 16)
    }

    @Test func wrongContextFails() throws {
        let down = try RelayMessage.seal(text, to: phone.publicKey, from: mac, direction: .down, pairingId: id)
        #expect(throws: (any Error).self) {
            try RelayMessage.open(down, with: phone, from: mac.publicKey, direction: .up, pairingId: id)
        }
        #expect(throws: (any Error).self) {
            try RelayMessage.open(down, with: phone, from: mac.publicKey, direction: .down, pairingId: randomId())
        }
        #expect(throws: (any Error).self) {
            try RelayMessage.open(down, with: phone, from: PrivateKey().publicKey, direction: .down, pairingId: id)
        }
        #expect(throws: (any Error).self) {
            try RelayMessage.open(down, with: PrivateKey(), from: mac.publicKey, direction: .down, pairingId: id)
        }
        #expect(throws: LinkError.invalidMessage) {
            try RelayMessage.open("AAAA", with: phone, from: mac.publicKey, direction: .down, pairingId: id)
        }
        #expect(throws: LinkError.invalidPairingId) {
            try RelayMessage.seal(text, to: phone.publicKey, from: mac, direction: .down, pairingId: Data(count: 15))
        }
    }

    @Test func helloRoundTrip() throws {
        let secret = Data((0..<32).map { _ in UInt8.random(in: 0...255) })
        let hello = try Hello(secret: secret, phoneKey: phone.publicKey, phoneName: "Chris's iPhone", sentAtMs: 1_763_000_000_000)
        let sealed = try RelayMessage.sealHello(hello, to: mac.publicKey, pairingId: id)

        let opened = try RelayMessage.openHello(sealed, with: mac, pairingId: id, secret: secret)
        #expect(opened.seq == 0)
        #expect(opened.secret == secret)
        #expect(opened.phoneKey.rawRepresentation == phone.publicKey.rawRepresentation)
        #expect(opened.phoneName == "Chris's iPhone")
        #expect(opened.sentAtMs == 1_763_000_000_000)

        #expect(throws: LinkError.wrongSecret) {
            try RelayMessage.openHello(sealed, with: mac, pairingId: id, secret: Data(count: 32))
        }
        #expect(throws: (any Error).self) {
            try RelayMessage.openHello(sealed, with: mac, pairingId: randomId(), secret: secret)
        }
        // A base-mode hello is not an auth-mode message.
        #expect(throws: (any Error).self) {
            try RelayMessage.open(sealed, with: mac, from: phone.publicKey, direction: .up, pairingId: id)
        }
    }

    @Test func helloJSON() throws {
        let hello = try Hello(
            secret: Data((0..<32).map { [0xFB, 0xFF, 0xBF][$0 % 3] }), phoneKey: phone.publicKey, phoneName: "p", sentAtMs: 1)
        let encoded = try JSONEncoder().encode(hello)
        let json = try JSONSerialization.jsonObject(with: encoded) as? [String: Any]
        #expect(json?["secret"] as? String == String(repeating: "-_-_", count: 10) + "-_8")
        #expect(json?["seq"] as? Int == 0)

        func decode(_ edit: (inout [String: Any]) -> Void) throws -> Hello {
            var object = try #require(json)
            edit(&object)
            return try JSONDecoder().decode(Hello.self, from: JSONSerialization.data(withJSONObject: object))
        }
        #expect(try decode { _ in }.phoneName == "p")
        // The hello is up seq 0; anything else is refused.
        #expect(throws: DecodingError.self) { try decode { $0["seq"] = 1 } }
        #expect(throws: (any Error).self) { try decode { $0["secret"] = "AAAA" } }
        #expect(throws: (any Error).self) { try decode { $0["phoneKey"] = "AAAA" } }
        #expect(throws: LinkError.invalidName) { try decode { $0["phoneName"] = "\u{202E}\u{7} \n" } }
        #expect(try decode { $0["phoneName"] = "\u{202E}evil\u{7}" }.phoneName == "evil")
    }

    @Test func seqRejectsAnythingNotNewer() {
        var seq = SeqGuard()
        let accepted = [0, 0, 5, 3, 5, 6].map { seq.accept($0) }
        #expect(accepted == [true, false, true, false, false, true])
        #expect(seq.last == 6)

        var restored = SeqGuard(last: 6)
        #expect([6, 7].map { restored.accept($0) } == [false, true])
    }

    @Test func base64URL() {
        let bytes = Data([0xFB, 0xFF, 0xBF, 0x00])
        #expect(bytes.base64URLEncodedString() == "-_-_AA")
        #expect(Data(base64URLEncoded: "-_-_AA") == bytes)
        #expect(Data(base64URLEncoded: "") == Data())
        for bad in ["-_-_AA==", "+/+/AA", "-_-_A", "-_-_AB", "a b", "é"] {
            #expect(Data(base64URLEncoded: bad) == nil, "\(bad)")
        }
    }
}
