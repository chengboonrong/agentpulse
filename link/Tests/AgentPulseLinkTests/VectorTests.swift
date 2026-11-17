import CryptoKit
import Foundation
import Testing

@testable import AgentPulseLink

/// Messages sealed by one side, opened by the other (docs/mobile.md, "Android app
/// (planned)"): Tests/vectors/swift.json comes from here (`AGENTPULSE_WRITE_VECTORS=1
/// swift test`), kotlin.json from android/link (`./gradlew :link:test -PwriteVectors
/// --rerun`). Both suites open both files.
struct VectorTests {
    struct Vectors: Codable {
        let pairingId, macPrivateKey, phonePrivateKey, secret, phoneName: String
        let sentAtMs: Int64
        let plaintext, down, up, hello, code: String
    }

    static let dir = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
        .appending(path: "vectors")

    @Test func opensSwiftVectors() throws {
        if ProcessInfo.processInfo.environment["AGENTPULSE_WRITE_VECTORS"] == "1" {
            let encoder = JSONEncoder()
            encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
            try (encoder.encode(make()) + Data("\n".utf8)).write(to: Self.dir.appending(path: "swift.json"))
        }
        try check(read("swift.json"))
    }

    @Test func opensKotlinVectors() throws {
        try check(read("kotlin.json"))
    }

    func read(_ name: String) throws -> Vectors {
        try JSONDecoder().decode(Vectors.self, from: Data(contentsOf: Self.dir.appending(path: name)))
    }

    func bytes(_ s: String) throws -> Data {
        try #require(Data(base64URLEncoded: s))
    }

    func check(_ v: Vectors) throws {
        let id = try bytes(v.pairingId)
        let mac = try PrivateKey(rawRepresentation: bytes(v.macPrivateKey))
        let phone = try PrivateKey(rawRepresentation: bytes(v.phonePrivateKey))
        let text = Data(v.plaintext.utf8)

        #expect(try RelayMessage.open(v.down, with: phone, from: mac.publicKey, direction: .down, pairingId: id) == text)
        #expect(try RelayMessage.open(v.up, with: mac, from: phone.publicKey, direction: .up, pairingId: id) == text)

        let hello = try RelayMessage.openHello(v.hello, with: mac, pairingId: id, secret: bytes(v.secret))
        #expect(hello.phoneKey.rawRepresentation == phone.publicKey.rawRepresentation)
        #expect(hello.phoneName == v.phoneName)
        #expect(hello.sentAtMs == v.sentAtMs)

        #expect(verificationCode(pairingId: id, macKey: mac.publicKey, phoneKey: phone.publicKey) == v.code)
    }

    func make() throws -> Vectors {
        let id = randomId()
        let secret = Data((0..<32).map { _ in UInt8.random(in: 0...255) })
        let mac = PrivateKey()
        let phone = PrivateKey()
        let text = #"{"seq":1,"type":"state","payload":{"version":1}}"#
        let name = "Chris's iPhone"
        let sentAtMs: Int64 = 1_763_000_000_000
        let hello = try Hello(secret: secret, phoneKey: phone.publicKey, phoneName: name, sentAtMs: sentAtMs)
        return Vectors(
            pairingId: id.base64URLEncodedString(),
            macPrivateKey: mac.rawRepresentation.base64URLEncodedString(),
            phonePrivateKey: phone.rawRepresentation.base64URLEncodedString(),
            secret: secret.base64URLEncodedString(), phoneName: name, sentAtMs: sentAtMs, plaintext: text,
            down: try RelayMessage.seal(Data(text.utf8), to: phone.publicKey, from: mac, direction: .down, pairingId: id),
            up: try RelayMessage.seal(Data(text.utf8), to: mac.publicKey, from: phone, direction: .up, pairingId: id),
            hello: try RelayMessage.sealHello(hello, to: mac.publicKey, pairingId: id),
            code: verificationCode(pairingId: id, macKey: mac.publicKey, phoneKey: phone.publicKey))
    }
}
