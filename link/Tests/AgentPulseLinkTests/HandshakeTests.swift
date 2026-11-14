import CryptoKit
import Foundation
import Testing

@testable import AgentPulseLink

struct HandshakeTests {
    let id = randomId()
    let mac = PrivateKey()
    let phone = PrivateKey()

    var pairing: LinkPairing { LinkPairing(id: id, macKey: mac, phoneKey: phone.publicKey) }

    /// Runs both sides; the Mac also knows two other pairings.
    func connect() throws -> (phone: LinkSession, mac: LinkSession) {
        let client = try LinkClientHandshake(pairingId: id, phoneKey: phone, macKey: mac.publicKey)
        #expect(client.message.count == 4 + 52)
        #expect(client.message.prefix(8) == Data([0, 0, 0, 52]) + Data("APL1".utf8))
        let others = [LinkPairing(id: randomId(), macKey: PrivateKey(), phoneKey: PrivateKey().publicKey),
                      LinkPairing(id: randomId(), macKey: mac, phoneKey: phone.publicKey)]
        let server = try LinkServerHandshake.respond(to: client.message, pairings: [others[0], pairing, others[1]])
        #expect(server.pairingId == id)
        #expect(server.reply.count == 4 + 32)
        return (try client.finish(reply: server.reply), server.session)
    }

    func raw(_ key: SymmetricKey) -> Data { key.withUnsafeBytes { Data($0) } }

    @Test func bothSidesDeriveMatchingKeys() throws {
        let (p, m) = try connect()
        #expect(raw(p.sendKey) == raw(m.receiveKey))
        #expect(raw(p.receiveKey) == raw(m.sendKey))
        #expect(raw(p.sendKey) != raw(p.receiveKey))
        // Fresh ephemeral keys every time.
        let (p2, _) = try connect()
        #expect(raw(p.sendKey) != raw(p2.sendKey))
    }

    /// The key schedule spelled out from the spec, using only the phone's private keys.
    @Test func keysFollowTheSpec() throws {
        let ep = PrivateKey()
        let client = try LinkClientHandshake(pairingId: id, phoneKey: phone, macKey: mac.publicKey, ephemeral: ep)
        func dh(_ a: PrivateKey, _ b: PublicKey) throws -> Data {
            try a.sharedSecretFromKeyAgreement(with: b).withUnsafeBytes { Data($0) }
        }
        // Message 1: "APL1" || ep || tag; the id appears nowhere, in bytes or as text.
        let k = HKDF<SHA256>.deriveKey(
            inputKeyMaterial: SymmetricKey(data: try dh(phone, mac.publicKey)),
            info: Data("agentpulse/1 link id \(id.base64URLEncodedString())".utf8), outputByteCount: 32)
        let tag = Data(HMAC<SHA256>.authenticationCode(for: ep.publicKey.rawRepresentation, using: k)).prefix(16)
        #expect(client.message == Data([0, 0, 0, 52]) + Data("APL1".utf8) + ep.publicKey.rawRepresentation + tag)
        #expect(client.message.range(of: id) == nil)
        #expect(client.message.range(of: Data(id.base64URLEncodedString().utf8)) == nil)

        let server = try LinkServerHandshake.respond(to: client.message, pairings: [pairing])
        let em = try PublicKey(rawRepresentation: server.reply.dropFirst(4))
        let session = try client.finish(reply: server.reply)

        let ikm = try dh(ep, em) + dh(ep, mac.publicKey) + dh(phone, em) + dh(phone, mac.publicKey)
        let h = Data(SHA256.hash(data: Data("agentpulse/1 link".utf8) + id + mac.publicKey.rawRepresentation
            + phone.publicKey.rawRepresentation + ep.publicKey.rawRepresentation + em.rawRepresentation))
        func key(_ info: String) -> Data {
            raw(HKDF<SHA256>.deriveKey(inputKeyMaterial: SymmetricKey(data: ikm), salt: h, info: Data(info.utf8), outputByteCount: 32))
        }
        #expect(raw(session.receiveKey) == key("down"))
        #expect(raw(session.sendKey) == key("up"))
        #expect(raw(server.session.sendKey) == key("down"))
    }

    @Test func framesRoundTrip() throws {
        let (p, m) = try connect()
        for text in ["state", "", "heartbeat"] {
            #expect(try p.open(m.seal(Data(text.utf8))) == Data(text.utf8))
            #expect(try m.open(p.seal(Data(text.utf8))) == Data(text.utf8))
        }
        // The wire format, opened by hand: the Mac's fourth frame uses counter 3.
        let frame = try m.seal(Data("abc".utf8))
        let header = frame.prefix(4)
        #expect(header == Data([0, 0, 0, 3 + 16]))
        let box = try ChaChaPoly.SealedBox(
            nonce: ChaChaPoly.Nonce(data: Data([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 3])),
            ciphertext: frame.dropFirst(4).dropLast(16), tag: frame.suffix(16))
        #expect(try ChaChaPoly.open(box, using: p.receiveKey, authenticating: header) == Data("abc".utf8))
    }

    @Test func tamperingFailsAndKillsTheSession() throws {
        let (p, m) = try connect()
        var frame = try m.seal(Data("state".utf8))
        frame[frame.count - 1] ^= 1
        #expect(throws: LinkError.frameFailed) { try p.open(frame) }
        // Unusable after one failure, even for an intact frame.
        #expect(throws: LinkError.sessionFailed) { try p.open(m.seal(Data("next".utf8))) }
        #expect(throws: LinkError.sessionFailed) { try p.seal(Data()) }

        let (p2, m2) = try connect()
        var ciphertext = try m2.seal(Data("state".utf8))
        ciphertext[4] ^= 1
        #expect(throws: LinkError.frameFailed) { try p2.open(ciphertext) }

        let (p3, m3) = try connect()
        let truncated = try m3.seal(Data("state".utf8)).dropLast()
        #expect(throws: LinkError.frameFailed) { try p3.open(Data(truncated)) }
    }

    @Test func replayFails() throws {
        let (p, m) = try connect()
        let first = try m.seal(Data("a".utf8))
        _ = try p.open(first)
        #expect(throws: LinkError.frameFailed) { try p.open(first) }
    }

    @Test func reorderFails() throws {
        let (p, m) = try connect()
        _ = try m.seal(Data("a".utf8))
        let second = try m.seal(Data("b".utf8))
        #expect(throws: LinkError.frameFailed) { try p.open(second) }
    }

    @Test func reflectedFrameFails() throws {
        let (p, _) = try connect()
        let own = try p.seal(Data("up".utf8))
        #expect(throws: LinkError.frameFailed) { try p.open(own) }
    }

    @Test func frameSizeLimit() throws {
        let (p, m) = try connect()
        let biggest = Data(count: maxFrameLength - 16)
        let frame = try m.seal(biggest)
        #expect(frame.count == 4 + 65_536)
        #expect(try p.open(frame) == biggest)

        #expect(throws: LinkError.frameTooLarge) { try m.seal(Data(count: maxFrameLength - 15)) }
        #expect(throws: LinkError.sessionFailed) { try m.seal(Data()) }

        let (p2, _) = try connect()
        #expect(throws: LinkError.frameTooLarge) { try p2.open(lengthPrefix(65_537) + Data(count: 65_537)) }
    }

    @Test func wrongStaticKeyGetsNoReply() throws {
        let impostorPhone = try LinkClientHandshake(pairingId: id, phoneKey: PrivateKey(), macKey: mac.publicKey)
        #expect(throws: LinkError.unknownPairing) {
            try LinkServerHandshake.respond(to: impostorPhone.message, pairings: [pairing])
        }
        let client = try LinkClientHandshake(pairingId: id, phoneKey: phone, macKey: mac.publicKey)
        let impostorMac = LinkPairing(id: id, macKey: PrivateKey(), phoneKey: phone.publicKey)
        #expect(throws: LinkError.unknownPairing) {
            try LinkServerHandshake.respond(to: client.message, pairings: [impostorMac])
        }
    }

    /// A replayed message 1 passes the tag, but without `ep`'s private key the replayer's
    /// session can't be used: the real phone's frames don't open on it.
    @Test func replayedMessageOneYieldsAUselessSession() throws {
        let client = try LinkClientHandshake(pairingId: id, phoneKey: phone, macKey: mac.publicKey)
        let message = client.message
        let real = try LinkServerHandshake.respond(to: message, pairings: [pairing])
        let replayed = try LinkServerHandshake.respond(to: message, pairings: [pairing])
        #expect(real.reply != replayed.reply)
        let p = try client.finish(reply: real.reply)
        let seen = try p.seal(Data("seen".utf8))
        #expect(throws: LinkError.frameFailed) { try replayed.session.open(seen) }
        #expect(try real.session.open(seen) == Data("seen".utf8))
    }

    @Test func pairingsAreToldApart() throws {
        let other = LinkPairing(id: randomId(), macKey: PrivateKey(), phoneKey: PrivateKey().publicKey)
        // Same keys, another id: only the id in the tag's key separates them.
        let sameKeys = LinkPairing(id: randomId(), macKey: mac, phoneKey: phone.publicKey)
        let all = [other, pairing, sameKeys]
        for (target, phoneKey) in [(pairing, phone), (sameKeys, phone)] {
            let client = try LinkClientHandshake(pairingId: target.id, phoneKey: phoneKey, macKey: target.macKey.publicKey)
            let server = try LinkServerHandshake.respond(to: client.message, pairings: all)
            #expect(server.pairingId == target.id)
            let p = try client.finish(reply: server.reply)
            #expect(try p.open(server.session.seal(Data("state".utf8))) == Data("state".utf8))
        }
    }

    @Test func unknownPairingGetsNoReply() throws {
        // Right keys, an id the Mac doesn't have.
        let client = try LinkClientHandshake(pairingId: randomId(), phoneKey: phone, macKey: mac.publicKey)
        #expect(throws: LinkError.unknownPairing) {
            try LinkServerHandshake.respond(to: client.message, pairings: [pairing])
        }
    }

    @Test func malformedHandshakeFails() throws {
        let client = try LinkClientHandshake(pairingId: id, phoneKey: phone, macKey: mac.publicKey)
        var badMagic = client.message
        badMagic[4] = UInt8(ascii: "X")
        for message in [badMagic, client.message.dropLast(), client.message + Data([0])] {
            #expect(throws: LinkError.handshakeFailed) {
                try LinkServerHandshake.respond(to: Data(message), pairings: [pairing])
            }
        }
        #expect(throws: LinkError.handshakeFailed) {
            try LinkClientHandshake(pairingId: id, phoneKey: phone, macKey: mac.publicKey)
                .finish(reply: lengthPrefix(31) + Data(count: 31))
        }
        #expect(throws: LinkError.invalidPairingId) {
            _ = try LinkClientHandshake(pairingId: Data(count: 8), phoneKey: phone, macKey: mac.publicKey)
        }
    }
}
