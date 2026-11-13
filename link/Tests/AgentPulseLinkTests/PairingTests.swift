import CryptoKit
import Foundation
import Testing

@testable import AgentPulseLink

struct PairingTests {
    static let key = PrivateKey().publicKey
    static let id = Data((0..<16).map { UInt8($0) })
    static let secret = Data(repeating: 7, count: 32)

    /// A link from raw fields, so malformed values can be built.
    static func link(_ overrides: [String: String?] = [:], prefix: String = "agentpulse://pair?") -> String {
        var fields: [(String, String?)] = [
            ("v", "1"), ("r", "https://relay.example.com/ap"), ("id", id.base64URLEncodedString()),
            ("k", key.rawRepresentation.base64URLEncodedString()), ("s", secret.base64URLEncodedString()),
            ("n", "Studio"),
        ]
        fields = fields.map { name, value in (name, overrides.keys.contains(name) ? overrides[name] ?? nil : value) }
        let query = fields.compactMap { name, value in
            value.map { "\(name)=\($0.addingPercentEncoding(withAllowedCharacters: .alphanumerics) ?? "")" }
        }
        return prefix + query.joined(separator: "&")
    }

    @Test func buildAndParse() throws {
        let relay = try #require(URL(string: "https://relay.example.com/ap"))
        let name = "Chris's MacBook Pro & co = 1+1 ü 🐱"
        let built = try PairingLink(relay: relay, id: Self.id, macKey: Self.key, secret: Self.secret, macName: name)
        #expect(built.string.hasPrefix("agentpulse://pair?v=1&r=https%3A%2F%2Frelay.example.com%2Fap&id=AAECAwQFBgcICQoLDA0ODw&k="))

        let parsed = try PairingLink(built.string)
        #expect(parsed.relay == relay)
        #expect(parsed.id == Self.id)
        #expect(parsed.macKey.rawRepresentation == Self.key.rawRepresentation)
        #expect(parsed.secret == Self.secret)
        #expect(parsed.macName == name)
    }

    @Test func acceptsLocalhostRelayAndUnknownFields() throws {
        #expect(try PairingLink(Self.link(["r": "http://localhost:8787"])).relay.port == 8787)
        #expect(try PairingLink(Self.link() + "&future=1").macName == "Studio")
    }

    @Test(arguments: [
        ("v", ["v": "2"]), ("v", ["v": nil]),
        ("r", ["r": nil]), ("r", ["r": "http://relay.example.com"]), ("r", ["r": "ftp://relay.example.com"]),
        ("r", ["r": "https://"]), ("r", ["r": "relay.example.com"]), ("r", ["r": "https://u:p@relay.example.com"]),
        ("r", ["r": "https://relay.example.com/?x=1"]), ("r", ["r": "https://relay.example.com/#x"]),
        ("r", ["r": "http://127.0.0.1:8787"]), ("r", ["r": "not a url"]),
        ("id", ["id": nil]), ("id", ["id": "AAECAwQFBgcICQoLDA0O"]), ("id", ["id": "AAECAwQFBgcICQoLDA0ODxA"]),
        ("id", ["id": "AAECAwQFBgcICQoLDA0ODw=="]), ("id", ["id": "AAECAwQFBgcICQoLDA0OD+"]),
        ("id", ["id": "AAECAwQFBgcICQoLDA0ODx"]),
        ("k", ["k": nil]), ("k", ["k": Data(count: 31).base64URLEncodedString()]),
        ("s", ["s": nil]), ("s", ["s": Data(count: 33).base64URLEncodedString()]),
        ("n", ["n": nil]), ("n", ["n": ""]), ("n", ["n": "  "]),
    ] as [(String, [String: String?])])
    func rejectsMalformedField(field: String, overrides: [String: String?]) {
        #expect(throws: LinkError.invalidLink(field)) { try PairingLink(Self.link(overrides)) }
    }

    @Test(arguments: [
        PairingTests.link(prefix: "https://pair?"), PairingTests.link(prefix: "agentpulse://pairing?"),
        PairingTests.link(prefix: "agentpulse://pair/x?"),
        "agentpulse://pair", "hello", "",
    ])
    func rejectsMalformedURL(string: String) {
        #expect(throws: LinkError.invalidLink("url")) { try PairingLink(string) }
    }

    @Test func rejectsDuplicateField() {
        #expect(throws: LinkError.invalidLink("id")) {
            try PairingLink(Self.link() + "&id=\(Self.id.base64URLEncodedString())")
        }
    }

    @Test func namesAreSanitised() throws {
        #expect(sanitizedName("  Studio\u{0} Mac\n ") == "Studio Mac")
        // Cf: bidirectional marks, U+061C, zero-width characters; Zl and Zp.
        let format = "\u{200E}\u{200F}\u{202A}a\u{202B}\u{202C}\u{202D}\u{202E}\u{2066}\u{2067}\u{2068}\u{2069}b"
        #expect(sanitizedName(format) == "ab")
        #expect(sanitizedName("a\u{061C}b\u{200B}c\u{FEFF}d") == "abcd")
        #expect(sanitizedName("a\u{2028}b\u{2029}c") == "abc")
        #expect(sanitizedName("\u{7F}\u{85}\u{1B}\t\u{200D}") == nil)

        // At most 64 scalars, cut at a Character boundary, trimmed again after the cut.
        #expect(sanitizedName(String(repeating: "🐱", count: 70)) == String(repeating: "🐱", count: 64))
        #expect(sanitizedName(String(repeating: "a", count: 63) + " b") == String(repeating: "a", count: 63))
        // A flag is one Character of two scalars: at 63 + 2 it doesn't fit, and isn't split.
        #expect(sanitizedName(String(repeating: "a", count: 63) + "🇲🇾") == String(repeating: "a", count: 63))
        #expect(sanitizedName(String(repeating: "a", count: 62) + "🇲🇾") == String(repeating: "a", count: 62) + "🇲🇾")
        // A ZWJ emoji at the boundary: the ZWJs (Cf) go, leaving three people of one scalar each.
        let family = sanitizedName(String(repeating: "a", count: 62) + "👩\u{200D}👩\u{200D}👧")
        #expect(family == String(repeating: "a", count: 62) + "👩👩")
        #expect(family?.unicodeScalars.count == 64)
        // A letter with 1,000 combining marks is one Character of 1,001 scalars: dropped whole.
        let stacked = "e" + String(repeating: "\u{0301}", count: 1_000)
        #expect(sanitizedName("ab" + stacked) == "ab")
        #expect(sanitizedName(stacked) == nil)

        #expect(try PairingLink(Self.link(["n": "\u{202E}Studio\u{7}"])).macName == "Studio")
        #expect(throws: LinkError.invalidLink("n")) { try PairingLink(Self.link(["n": "\u{202E}\u{2066} "])) }
    }

    @Test func buildingValidatesToo() throws {
        let relay = try #require(URL(string: "https://relay.example.com"))
        #expect(throws: LinkError.invalidLink("id")) {
            try PairingLink(relay: relay, id: Data(count: 15), macKey: Self.key, secret: Self.secret, macName: "m")
        }
        #expect(throws: LinkError.invalidLink("s")) {
            try PairingLink(relay: relay, id: Self.id, macKey: Self.key, secret: Data(count: 31), macName: "m")
        }
        let http = try #require(URL(string: "http://relay.example.com"))
        #expect(throws: LinkError.invalidLink("r")) {
            try PairingLink(relay: http, id: Self.id, macKey: Self.key, secret: Self.secret, macName: "m")
        }
    }

    /// Expected values computed outside Swift (Python `hashlib` and `cryptography`'s X25519),
    /// then checked here step by step with CryptoKit.
    @Test(arguments: [
        (Data((0..<16).map { UInt8($0) }), "AAECAwQFBgcICQoLDA0ODw", [UInt8]([0x93, 0x05, 0xC9, 0x32]), UInt32(2_466_629_938), "629938"),
        (Data(repeating: 0x11, count: 16), "EREREREREREREREREREREQ", [0x00, 0xC7, 0x48, 0x25], 13_060_133, "060133"),
    ])
    func verificationCodeVector(id: Data, idText: String, first4: [UInt8], number: UInt32, code: String) throws {
        let mac = try PrivateKey(rawRepresentation: Data(repeating: 0x11, count: 32)).publicKey
        let phone = try PrivateKey(rawRepresentation: Data(repeating: 0x22, count: 32)).publicKey
        #expect(mac.rawRepresentation.hex == "7b4e909bbe7ffe44c465a220037d608ee35897d31ef972f07f74892cb0f73f13")
        #expect(phone.rawRepresentation.hex == "0faa684ed28867b97f4a6a2dee5df8ce974e76b7018e3f22a1c4cf2678570f20")
        #expect(id.base64URLEncodedString() == idText)

        let digest = SHA256.hash(data: Data("agentpulse/1 code \(idText)".utf8) + mac.rawRepresentation + phone.rawRepresentation)
        #expect(Array(digest.prefix(4)) == first4)
        #expect(UInt32(first4[0]) << 24 | UInt32(first4[1]) << 16 | UInt32(first4[2]) << 8 | UInt32(first4[3]) == number)
        #expect(String(format: "%06u", number % 1_000_000) == code)

        #expect(verificationCode(pairingId: id, macKey: mac, phoneKey: phone) == code)
        // Order matters: swapping the keys gives another code.
        #expect(verificationCode(pairingId: id, macKey: phone, phoneKey: mac) != code)
    }
}

extension Data {
    var hex: String { map { String(format: "%02x", $0) }.joined() }
}
