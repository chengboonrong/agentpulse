import CryptoKit
import Foundation

/// The QR code's link: `agentpulse://pair?v=1&r=…&id=…&k=…&s=…&n=…` (docs/mobile.md, "Pairing").
public struct PairingLink: Sendable {
    public let relay: URL
    public let id: Data
    public let macKey: Curve25519.KeyAgreement.PublicKey
    public let secret: Data
    /// Sanitised (see `sanitizedName`): no control, format or separator characters, at
    /// most 64 Unicode scalars, never empty.
    public let macName: String

    public init(
        relay: URL, id: Data, macKey: Curve25519.KeyAgreement.PublicKey, secret: Data, macName: String
    ) throws {
        guard let c = URLComponents(url: relay, resolvingAgainstBaseURL: false),
            let host = c.host, !host.isEmpty,
            c.scheme == "https" || (c.scheme == "http" && host == "localhost"),
            c.user == nil, c.password == nil, c.query == nil, c.fragment == nil
        else { throw LinkError.invalidLink("r") }
        guard id.count == 16 else { throw LinkError.invalidLink("id") }
        guard secret.count == 32 else { throw LinkError.invalidLink("s") }
        guard let name = sanitizedName(macName) else { throw LinkError.invalidLink("n") }
        self.relay = relay
        self.id = id
        self.macKey = macKey
        self.secret = secret
        self.macName = name
    }

    /// Parses a scanned or pasted link, validating every field.
    public init(_ string: String) throws {
        guard let c = URLComponents(string: string), c.scheme?.lowercased() == "agentpulse",
            c.host == "pair", c.path.isEmpty, let items = c.queryItems
        else { throw LinkError.invalidLink("url") }
        func one(_ name: String) throws -> String {
            let values = items.filter { $0.name == name }
            guard values.count == 1, let v = values[0].value else { throw LinkError.invalidLink(name) }
            return v
        }
        func bytes(_ name: String, _ count: Int) throws -> Data {
            guard let d = Data(base64URLEncoded: try one(name)), d.count == count else {
                throw LinkError.invalidLink(name)
            }
            return d
        }
        guard try one("v") == "1" else { throw LinkError.invalidLink("v") }
        guard let relay = URL(string: try one("r")) else { throw LinkError.invalidLink("r") }
        try self.init(
            relay: relay, id: bytes("id", 16), macKey: .init(rawRepresentation: bytes("k", 32)),
            secret: bytes("s", 32), macName: one("n"))
    }

    /// The link as a string, every value percent-encoded outside RFC 3986's unreserved set.
    public var string: String {
        let unreserved = CharacterSet(
            charactersIn: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~")
        let fields = [
            ("v", "1"), ("r", relay.absoluteString), ("id", id.base64URLEncodedString()),
            ("k", macKey.rawRepresentation.base64URLEncodedString()),
            ("s", secret.base64URLEncodedString()), ("n", macName),
        ]
        let query = fields.map { name, value in
            // Only fails for invalid UTF-16, which a Swift String can't hold.
            "\(name)=\(value.addingPercentEncoding(withAllowedCharacters: unreserved) ?? "")"
        }
        return "agentpulse://pair?" + query.joined(separator: "&")
    }
}

/// A device name fit to show: Unicode control (Cc), format (Cf: bidirectional marks,
/// U+061C, zero-width characters), line (Zl) and paragraph (Zp) separators removed,
/// trimmed, cut at a Character boundary to at most 64 Unicode scalars, so stacked
/// combining marks can't exceed it; nil if nothing is left.
func sanitizedName(_ name: String) -> String? {
    let removed: Set<Unicode.GeneralCategory> = [.control, .format, .lineSeparator, .paragraphSeparator]
    let kept = name.unicodeScalars.filter { !removed.contains($0.properties.generalCategory) }
    var capped = ""
    var scalars = 0
    for c in String(String.UnicodeScalarView(kept)).trimmingCharacters(in: .whitespacesAndNewlines) {
        scalars += c.unicodeScalars.count
        guard scalars <= 64 else { break }
        capped.append(c)
    }
    capped = capped.trimmingCharacters(in: .whitespacesAndNewlines)
    return capped.isEmpty ? nil : capped
}

/// The six-digit code both screens show: the first 4 bytes, big-endian, of
/// SHA-256("agentpulse/1 code " + base64url(id) || Mac key || phone key), mod 1,000,000.
public func verificationCode(
    pairingId: Data, macKey: Curve25519.KeyAgreement.PublicKey,
    phoneKey: Curve25519.KeyAgreement.PublicKey
) -> String {
    var h = SHA256()
    h.update(data: Data("agentpulse/1 code \(pairingId.base64URLEncodedString())".utf8))
    h.update(data: macKey.rawRepresentation)
    h.update(data: phoneKey.rawRepresentation)
    let n = h.finalize().prefix(4).reduce(UInt32(0)) { $0 << 8 | UInt32($1) }
    return String(format: "%06u", n % 1_000_000)
}
