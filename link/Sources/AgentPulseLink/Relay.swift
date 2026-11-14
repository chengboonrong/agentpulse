import CryptoKit
import Foundation

/// Everything that can go wrong in this package. `invalidLink` names the bad query field.
public enum LinkError: Error, Equatable, Sendable {
    case invalidPairingId
    case invalidMessage
    case wrongSecret
    case invalidName
    case invalidLink(String)
    case unknownPairing
    case handshakeFailed
    case frameTooLarge
    case frameFailed
    case sessionFailed
    case closed
    case timeout
}

/// `down` is Mac to phone, `up` is phone to Mac.
public enum Direction: String, Sendable {
    case down, up
}

/// HPKE single-shot sealing for relay messages (docs/mobile.md, "Cryptography").
public enum RelayMessage {
    static let suite = HPKE.Ciphersuite.Curve25519_SHA256_ChachaPoly

    static func info(_ direction: Direction, _ pairingId: Data) throws -> Data {
        guard pairingId.count == 16 else { throw LinkError.invalidPairingId }
        return Data("agentpulse/1 \(direction.rawValue) \(pairingId.base64URLEncodedString())".utf8)
    }

    /// Auth mode: sealed to `recipient`, authenticated by the sender's private key.
    /// Returns base64url (unpadded) of `enc || ciphertext`.
    public static func seal(
        _ plaintext: Data,
        to recipient: Curve25519.KeyAgreement.PublicKey,
        from sender: Curve25519.KeyAgreement.PrivateKey,
        direction: Direction,
        pairingId: Data
    ) throws -> String {
        var s = try HPKE.Sender(
            recipientKey: recipient, ciphersuite: suite, info: info(direction, pairingId),
            authenticatedBy: sender)
        return (try s.encapsulatedKey + s.seal(plaintext)).base64URLEncodedString()
    }

    /// Opens an auth-mode message from `sender`.
    public static func open(
        _ message: String,
        with recipient: Curve25519.KeyAgreement.PrivateKey,
        from sender: Curve25519.KeyAgreement.PublicKey,
        direction: Direction,
        pairingId: Data
    ) throws -> Data {
        let (enc, ct) = try split(message)
        var r = try HPKE.Recipient(
            privateKey: recipient, ciphersuite: suite, info: info(direction, pairingId),
            encapsulatedKey: enc, authenticatedBy: sender)
        return try r.open(ct)
    }

    /// The phone's hello: the only base-mode message, direction `up`.
    public static func sealHello(
        _ hello: Hello, to macKey: Curve25519.KeyAgreement.PublicKey, pairingId: Data
    ) throws -> String {
        var s = try HPKE.Sender(recipientKey: macKey, ciphersuite: suite, info: info(.up, pairingId))
        return (try s.encapsulatedKey + s.seal(JSONEncoder().encode(hello))).base64URLEncodedString()
    }

    /// Opens a hello and checks that it carries the pairing's secret. Seed the pairing's `up`
    /// `SeqGuard` with it (`SeqGuard(last: 0)`), so later `up` messages need seq 1 or more.
    public static func openHello(
        _ message: String, with macKey: Curve25519.KeyAgreement.PrivateKey, pairingId: Data,
        secret: Data
    ) throws -> Hello {
        let (enc, ct) = try split(message)
        var r = try HPKE.Recipient(
            privateKey: macKey, ciphersuite: suite, info: info(.up, pairingId), encapsulatedKey: enc)
        let hello = try JSONDecoder().decode(Hello.self, from: r.open(ct))
        guard constantTimeEqual(hello.secret, secret) else { throw LinkError.wrongSecret }
        return hello
    }

    private static func split(_ message: String) throws -> (Data, Data) {
        // 32-byte enc plus at least the 16-byte tag.
        guard let d = Data(base64URLEncoded: message), d.count >= 48 else { throw LinkError.invalidMessage }
        return (Data(d.prefix(32)), Data(d.dropFirst(32)))
    }
}

/// The hello plaintext. Byte fields travel as base64url strings. Always `up` seq 0.
public struct Hello: Sendable {
    public var seq: UInt64 { 0 }
    public let secret: Data
    public let phoneKey: Curve25519.KeyAgreement.PublicKey
    /// Sanitised: see `PairingLink.macName`.
    public let phoneName: String
    public let sentAtMs: Int64

    public init(secret: Data, phoneKey: Curve25519.KeyAgreement.PublicKey, phoneName: String, sentAtMs: Int64) throws {
        guard secret.count == 32 else { throw LinkError.invalidMessage }
        guard let name = sanitizedName(phoneName) else { throw LinkError.invalidName }
        self.secret = secret
        self.phoneKey = phoneKey
        self.phoneName = name
        self.sentAtMs = sentAtMs
    }
}

extension Hello: Codable {
    enum CodingKeys: String, CodingKey { case seq, secret, phoneKey, phoneName, sentAtMs }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        guard try c.decode(UInt64.self, forKey: .seq) == 0 else {
            throw DecodingError.dataCorruptedError(forKey: .seq, in: c, debugDescription: "the hello is seq 0")
        }
        guard let secret = Data(base64URLEncoded: try c.decode(String.self, forKey: .secret)),
            let key = Data(base64URLEncoded: try c.decode(String.self, forKey: .phoneKey))
        else { throw DecodingError.dataCorruptedError(forKey: .secret, in: c, debugDescription: "not base64url") }
        try self.init(
            secret: secret, phoneKey: .init(rawRepresentation: key),
            phoneName: c.decode(String.self, forKey: .phoneName), sentAtMs: c.decode(Int64.self, forKey: .sentAtMs))
    }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(seq, forKey: .seq)
        try c.encode(secret.base64URLEncodedString(), forKey: .secret)
        try c.encode(phoneKey.rawRepresentation.base64URLEncodedString(), forKey: .phoneKey)
        try c.encode(phoneName, forKey: .phoneName)
        try c.encode(sentAtMs, forKey: .sentAtMs)
    }
}

/// Receiver side of the `seq` rule: accepts only a seq newer than the last accepted one.
/// Seqs count from 0 per pairing and direction; the hello is `up` seq 0, so the phone's
/// later messages start at 1. A fresh guard accepts any seq, 0 included.
/// Persisting `last` per pairing and direction is the caller's job: the Mac keeps its own,
/// the phone one record in its App Group, shared by the app, widgets and Live Activity.
public struct SeqGuard: Sendable {
    public private(set) var last: UInt64?

    public init(last: UInt64? = nil) { self.last = last }

    public mutating func accept(_ seq: UInt64) -> Bool {
        if let last, seq <= last { return false }
        last = seq
        return true
    }
}

/// Equality of two secrets or tags without a timing leak (CryptoKit compares digests in
/// constant time).
func constantTimeEqual(_ a: Data, _ b: Data) -> Bool {
    SHA256.hash(data: a) == SHA256.hash(data: b)
}

extension Data {
    /// base64url without padding (RFC 4648 §5).
    public func base64URLEncodedString() -> String {
        String(
            base64EncodedString().map { $0 == "+" ? "-" : $0 == "/" ? "_" : $0 }.filter { $0 != "=" })
    }

    /// Strict: base64url alphabet only, no padding, canonical encoding.
    public init?(base64URLEncoded s: String) {
        guard s.utf8.allSatisfy({ $0.isB64URL }), s.utf8.count % 4 != 1 else { return nil }
        let std = String(s.map { $0 == "-" ? "+" : $0 == "_" ? "/" : $0 })
        guard let d = Data(base64Encoded: std + String(repeating: "=", count: (4 - s.utf8.count % 4) % 4)),
            d.base64URLEncodedString() == s
        else { return nil }
        self = d
    }
}

extension UInt8 {
    fileprivate var isB64URL: Bool {
        (0x41...0x5A).contains(self) || (0x61...0x7A).contains(self) || (0x30...0x39).contains(self)
            || self == 0x2D || self == 0x5F
    }
}
