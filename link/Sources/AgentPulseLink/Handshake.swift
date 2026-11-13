import CryptoKit
import Foundation

// The direct connection's handshake and frames (docs/mobile.md, "Direct connection").
// Transport-free: these consume and produce the length-prefixed messages as bytes.

/// The largest frame body (ciphertext and tag) after the 4-byte length.
let maxFrameLength = 65_536
let magic = Data("APL1".utf8)
/// Message 1's body: `"APL1" || ep (32) || tag (16)`.
let helloLength = magic.count + 32 + 16
let replyLength = 32

func lengthPrefix(_ n: Int) -> Data {
    withUnsafeBytes(of: UInt32(n).bigEndian) { Data($0) }
}

func prefixed(_ body: Data) -> Data { lengthPrefix(body.count) + body }

func length(ofPrefix p: Data) -> Int {
    p.prefix(4).reduce(0) { $0 << 8 | Int($1) }
}

private func dh(_ priv: Curve25519.KeyAgreement.PrivateKey, _ pub: Curve25519.KeyAgreement.PublicKey) throws
    -> Data
{
    try priv.sharedSecretFromKeyAgreement(with: pub).withUnsafeBytes { Data($0) }
}

/// The first 16 bytes of HMAC-SHA256(k, ep), with
/// k = HKDF-SHA256(DH(sp, sm), info: "agentpulse/1 link id " + base64url(id)), no salt.
private func idTag(ss: Data, id: Data, ep: Curve25519.KeyAgreement.PublicKey) -> Data {
    let k = HKDF<SHA256>.deriveKey(
        inputKeyMaterial: SymmetricKey(data: ss), info: Data("agentpulse/1 link id \(id.base64URLEncodedString())".utf8),
        outputByteCount: 32)
    return Data(HMAC<SHA256>.authenticationCode(for: ep.rawRepresentation, using: k).prefix(16))
}

/// `ikm` is `ee || es || se || ss`; the Mac sends with `down`, the phone with `up`.
private func session(
    id: Data, sm: Curve25519.KeyAgreement.PublicKey, sp: Curve25519.KeyAgreement.PublicKey,
    ep: Curve25519.KeyAgreement.PublicKey, em: Curve25519.KeyAgreement.PublicKey, ikm: Data, isMac: Bool
) -> LinkSession {
    var h = SHA256()
    for part in [Data("agentpulse/1 link".utf8), id, sm.rawRepresentation, sp.rawRepresentation,
                 ep.rawRepresentation, em.rawRepresentation] {
        h.update(data: part)
    }
    let salt = Data(h.finalize())
    func key(_ info: String) -> SymmetricKey {
        HKDF<SHA256>.deriveKey(
            inputKeyMaterial: SymmetricKey(data: ikm), salt: salt, info: Data(info.utf8), outputByteCount: 32)
    }
    let down = key("down"), up = key("up")
    return LinkSession(sendKey: isMac ? down : up, receiveKey: isMac ? up : down)
}

/// The phone's side. Noncopyable, so one handshake yields at most one session.
public struct LinkClientHandshake: ~Copyable {
    /// Message 1, length prefix included: `"APL1" || ep || tag`. The id itself is never sent.
    public let message: Data
    private let id: Data
    private let phoneKey: Curve25519.KeyAgreement.PrivateKey
    private let macKey: Curve25519.KeyAgreement.PublicKey
    private let ephemeral: Curve25519.KeyAgreement.PrivateKey

    public init(
        pairingId: Data, phoneKey: Curve25519.KeyAgreement.PrivateKey, macKey: Curve25519.KeyAgreement.PublicKey
    ) throws {
        try self.init(pairingId: pairingId, phoneKey: phoneKey, macKey: macKey, ephemeral: .init())
    }

    /// Tests pass a known ephemeral key.
    init(
        pairingId: Data, phoneKey: Curve25519.KeyAgreement.PrivateKey, macKey: Curve25519.KeyAgreement.PublicKey,
        ephemeral: Curve25519.KeyAgreement.PrivateKey
    ) throws {
        guard pairingId.count == 16 else { throw LinkError.invalidPairingId }
        guard let ss = try? dh(phoneKey, macKey) else { throw LinkError.handshakeFailed }
        id = pairingId
        self.ephemeral = ephemeral
        self.phoneKey = phoneKey
        self.macKey = macKey
        message = prefixed(
            magic + ephemeral.publicKey.rawRepresentation + idTag(ss: ss, id: pairingId, ep: ephemeral.publicKey))
    }

    /// Consumes message 2 (length prefix included) and yields the session.
    public consuming func finish(reply: Data) throws -> LinkSession {
        guard reply.count == 4 + replyLength, length(ofPrefix: reply) == replyLength,
            let em = try? Curve25519.KeyAgreement.PublicKey(rawRepresentation: reply.dropFirst(4))
        else { throw LinkError.handshakeFailed }
        do {
            let ikm = try dh(ephemeral, em) + dh(ephemeral, macKey) + dh(phoneKey, em) + dh(phoneKey, macKey)
            return session(
                id: id, sm: macKey, sp: phoneKey.publicKey, ep: ephemeral.publicKey, em: em, ikm: ikm,
                isMac: false)
        } catch {
            throw LinkError.handshakeFailed
        }
    }
}

/// One confirmed pairing as the Mac knows it.
public struct LinkPairing: Sendable {
    public let id: Data
    public let macKey: Curve25519.KeyAgreement.PrivateKey
    public let phoneKey: Curve25519.KeyAgreement.PublicKey

    public init(id: Data, macKey: Curve25519.KeyAgreement.PrivateKey, phoneKey: Curve25519.KeyAgreement.PublicKey) {
        self.id = id
        self.macKey = macKey
        self.phoneKey = phoneKey
    }
}

/// The Mac's side.
public enum LinkServerHandshake {
    /// Consumes message 1 (length prefix included), finding its pairing by the tag. Returns
    /// message 2 and the session, or throws, in which case nothing may be sent back.
    public static func respond(to message: Data, pairings: [LinkPairing]) throws -> (
        pairingId: Data, reply: Data, session: LinkSession
    ) {
        let body = Data(message.dropFirst(4))
        guard message.count == 4 + helloLength, length(ofPrefix: message) == helloLength,
            body.prefix(4) == magic,
            let ep = try? Curve25519.KeyAgreement.PublicKey(rawRepresentation: body.subdata(in: 4..<36))
        else { throw LinkError.handshakeFailed }
        let tag = body.suffix(16)
        // Every pairing is checked, without stopping at the first match.
        var match: LinkPairing?
        for p in pairings {
            guard let ss = try? dh(p.macKey, p.phoneKey) else { continue }
            if constantTimeEqual(idTag(ss: ss, id: p.id, ep: ep), tag), match == nil { match = p }
        }
        guard let p = match else { throw LinkError.unknownPairing }
        let ephemeral = Curve25519.KeyAgreement.PrivateKey()
        do {
            let ikm = try dh(ephemeral, ep) + dh(p.macKey, ep) + dh(ephemeral, p.phoneKey) + dh(p.macKey, p.phoneKey)
            let s = session(
                id: p.id, sm: p.macKey.publicKey, sp: p.phoneKey, ep: ep, em: ephemeral.publicKey, ikm: ikm,
                isMac: true)
            return (p.id, prefixed(ephemeral.publicKey.rawRepresentation), s)
        } catch {
            throw LinkError.handshakeFailed
        }
    }
}

/// Seals and opens frames with strict per-direction counters. Any error makes it unusable.
/// A class, not a struct, so a copy can never reuse a nonce.
public final class LinkSession {
    let sendKey: SymmetricKey
    let receiveKey: SymmetricKey
    private var sent: UInt64 = 0
    private var received: UInt64 = 0
    private var failed = false

    init(sendKey: SymmetricKey, receiveKey: SymmetricKey) {
        self.sendKey = sendKey
        self.receiveKey = receiveKey
    }

    /// Returns the frame, length prefix included.
    public func seal(_ plaintext: Data) throws -> Data {
        try guarded {
            guard plaintext.count + 16 <= maxFrameLength else { throw LinkError.frameTooLarge }
            let n = try nonce(sent)
            let header = lengthPrefix(plaintext.count + 16)
            let box = try ChaChaPoly.seal(plaintext, using: sendKey, nonce: n, authenticating: header)
            sent += 1
            return header + box.ciphertext + box.tag
        }
    }

    /// Opens one frame, length prefix included.
    public func open(_ frame: Data) throws -> Data {
        try guarded {
            let header = Data(frame.prefix(4))
            let body = Data(frame.dropFirst(4))
            guard header.count == 4, length(ofPrefix: header) == body.count else { throw LinkError.frameFailed }
            guard body.count <= maxFrameLength else { throw LinkError.frameTooLarge }
            guard body.count >= 16 else { throw LinkError.frameFailed }
            let box = try ChaChaPoly.SealedBox(
                nonce: nonce(received), ciphertext: body.dropLast(16), tag: body.suffix(16))
            guard let plaintext = try? ChaChaPoly.open(box, using: receiveKey, authenticating: header) else {
                throw LinkError.frameFailed
            }
            received += 1
            return plaintext
        }
    }

    private func nonce(_ counter: UInt64) throws -> ChaChaPoly.Nonce {
        guard counter < .max else { throw LinkError.sessionFailed }
        return try ChaChaPoly.Nonce(data: Data(count: 4) + withUnsafeBytes(of: counter.bigEndian) { Data($0) })
    }

    private func guarded<T>(_ body: () throws -> T) throws -> T {
        guard !failed else { throw LinkError.sessionFailed }
        do {
            return try body()
        } catch {
            failed = true
            throw error
        }
    }
}
