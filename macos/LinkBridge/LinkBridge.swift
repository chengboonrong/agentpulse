// The phone link's Mac side for the Rust app (docs/mobile.md, "Crypto, QR"): keys in the
// Keychain, sealing through AgentPulseLink, the QR image and the direct listener.
// Loaded with dlopen from Contents/Frameworks by crates/agentpulse-app/src/phones/bridge.rs.
//
// C ABI: `aplink_call` takes a JSON request `{"op": ..., ...}` and returns a malloc'd JSON
// response, `{"ok": {...}}` or `{"error": "..."}`, freed with `aplink_free`. Listener events
// go to the handler set with `aplink_set_event_handler`, on any thread; the string is valid
// only during the call. Binary values are unpadded base64url.

import AgentPulseLink
import CoreImage
import CryptoKit
import Foundation
import Security
import SystemConfiguration

private struct BridgeError: Error, CustomStringConvertible {
    let description: String
    init(_ description: String) { self.description = description }
}

private typealias Json = [String: Any]
private typealias EventHandler = @convention(c) (UnsafeMutableRawPointer?, UnsafePointer<CChar>?) -> Void

@_cdecl("aplink_call")
public func aplinkCall(_ request: UnsafePointer<CChar>?) -> UnsafeMutablePointer<CChar>? {
    var response: Json
    do {
        guard let request,
            let req = (try? JSONSerialization.jsonObject(with: Data(String(cString: request).utf8))) as? Json
        else { throw BridgeError("request must be a JSON object") }
        response = ["ok": try LinkBridge.shared.handle(req)]
    } catch {
        response = ["error": "\(error)"]
    }
    let data = (try? JSONSerialization.data(withJSONObject: response)) ?? Data(#"{"error":"encoding"}"#.utf8)
    return strdup(String(decoding: data, as: UTF8.self))
}

@_cdecl("aplink_free")
public func aplinkFree(_ response: UnsafeMutablePointer<CChar>?) {
    free(response)
}

@_cdecl("aplink_set_event_handler")
public func aplinkSetEventHandler(
    _ handler: (@convention(c) (UnsafeMutableRawPointer?, UnsafePointer<CChar>?) -> Void)?,
    _ ctx: UnsafeMutableRawPointer?
) {
    LinkBridge.shared.setHandler(handler, ctx)
}

private final class LinkBridge: @unchecked Sendable {
    static let shared = LinkBridge()
    private static let service = "io.github.chengboonrong.AgentPulse.pairing"
    private static let relayKey = "relay-key"

    // Everything below is guarded by `lock`.
    private let lock = NSLock()
    private var handler: EventHandler?
    private var ctx: UnsafeMutableRawPointer?
    private var listener: LinkListener?
    private var pairings: [LinkPairing] = []
    /// Pairing id to the latest `state` message sent, the listener's first frame.
    private var latest: [Data: Data] = [:]
    /// Counts listener starts. link/ numbers connections per listener from 0, so the
    /// bridge's connection number is `listeners << 32 | link's number`, unique per process.
    private var listeners = 0

    func setHandler(_ handler: EventHandler?, _ ctx: UnsafeMutableRawPointer?) {
        lock.withLock {
            self.handler = handler
            self.ctx = ctx
        }
    }

    func handle(_ req: Json) throws -> Json {
        switch try string(req, "op") {
        case "pair.new": return try pairNew(req)
        case "pair.token": return ["token": try keychain(id(req), "token").base64URLEncodedString()]
        case "hello.open": return try helloOpen(req)
        case "pair.code":
            let id = try id(req)
            return ["code": verificationCode(pairingId: id, macKey: try macKey(id).publicKey, phoneKey: try phoneKey(req))]
        case "seal":
            let id = try id(req)
            let message = try RelayMessage.seal(
                bytes(req, "plaintext"), to: phoneKey(req), from: macKey(id), direction: direction(req), pairingId: id)
            return ["message": message]
        case "open":
            let id = try id(req)
            let plaintext = try RelayMessage.open(
                string(req, "message"), with: macKey(id), from: phoneKey(req), direction: direction(req), pairingId: id)
            return ["plaintext": plaintext.base64URLEncodedString()]
        case "pair.delete":
            let id = try id(req)
            for kind in ["key", "token", "secret"] { try keychainDelete(account(id, kind)) }
            return [:]
        case "relaykey.set":
            switch req["key"] {
            case let key as String: try keychainSet(Self.relayKey, Data(key.utf8))
            case nil, is NSNull: try keychainDelete(Self.relayKey)
            default: throw BridgeError("key must be a string or null")
            }
            return [:]
        case "relaykey.get":
            let key = try keychainRead(Self.relayKey).map { String(decoding: $0, as: UTF8.self) }
            return ["key": key ?? NSNull()]
        case "qr": return try qr(req)
        case "listener.set": return try listenerSet(req)
        case "listener.send": return try listenerSend(req)
        case let op: throw BridgeError("unknown op \(op)")
        }
    }

    // MARK: Pairing

    private func pairNew(_ req: Json) throws -> Json {
        guard let relay = URL(string: try string(req, "relay")) else { throw BridgeError("bad relay URL") }
        let name = (req["macName"] as? String) ?? (SCDynamicStoreCopyComputerName(nil, nil) as String?) ?? "Mac"
        let id = try random(16), secret = try random(32), token = try random(32)
        let key = Curve25519.KeyAgreement.PrivateKey()
        // Validates the relay and the name before anything is stored.
        let link = try PairingLink(relay: relay, id: id, macKey: key.publicKey, secret: secret, macName: name)
        do {
            try keychainSet(account(id, "key"), key.rawRepresentation)
            try keychainSet(account(id, "token"), token)
            try keychainSet(account(id, "secret"), secret)
        } catch {
            for kind in ["key", "token", "secret"] { try? keychainDelete(account(id, kind)) }
            throw error
        }
        return [
            "id": id.base64URLEncodedString(),
            "macKey": key.publicKey.rawRepresentation.base64URLEncodedString(),
            "link": link.string,
            "secretHash": Data(SHA256.hash(data: secret)).base64URLEncodedString(),
            "macTokenHash": Data(SHA256.hash(data: token)).base64URLEncodedString(),
        ]
    }

    private func helloOpen(_ req: Json) throws -> Json {
        let id = try id(req)
        let key = try macKey(id)
        let hello = try RelayMessage.openHello(
            string(req, "message"), with: key, pairingId: id, secret: keychain(id, "secret"))
        let code = verificationCode(pairingId: id, macKey: key.publicKey, phoneKey: hello.phoneKey)
        // The secret's only use was this check; kept until it succeeded, so a retry can open it.
        try? keychainDelete(account(id, "secret"))
        return [
            "phoneKey": hello.phoneKey.rawRepresentation.base64URLEncodedString(), "phoneName": hello.phoneName,
            "code": code,
        ]
    }

    private func macKey(_ id: Data) throws -> Curve25519.KeyAgreement.PrivateKey {
        try Curve25519.KeyAgreement.PrivateKey(rawRepresentation: keychain(id, "key"))
    }

    // MARK: QR

    private func qr(_ req: Json) throws -> Json {
        let text = try string(req, "text")
        let scale = (req["scale"] as? Int) ?? 1
        guard (1...64).contains(scale) else { throw BridgeError("scale must be 1 to 64") }
        guard let filter = CIFilter(name: "CIQRCodeGenerator") else { throw BridgeError("no QR generator") }
        filter.setValue(Data(text.utf8), forKey: "inputMessage")
        filter.setValue("M", forKey: "inputCorrectionLevel")
        guard let image = filter.outputImage else { throw BridgeError("text too long for a QR code") }
        let w = Int(image.extent.width), h = Int(image.extent.height)
        guard w * scale <= 2048, h * scale <= 2048 else { throw BridgeError("QR image over 2048×2048 pixels") }
        var modules = [UInt8](repeating: 0, count: w * h * 4)
        CIContext().render(
            image, toBitmap: &modules, rowBytes: w * 4, bounds: image.extent, format: .RGBA8,
            colorSpace: CGColorSpace(name: CGColorSpace.sRGB))
        // Nearest neighbour. The bitmap's first row is the image's top.
        var out = [UInt8](repeating: 255, count: w * h * 4 * scale * scale)
        for y in 0..<(h * scale) {
            for x in 0..<(w * scale) {
                let dark = modules[(y / scale * w + x / scale) * 4] < 128
                let o = (y * w * scale + x) * 4
                if dark { out[o] = 0; out[o + 1] = 0; out[o + 2] = 0 }
            }
        }
        return ["width": w * scale, "height": h * scale, "rgba": Data(out).base64URLEncodedString()]
    }

    // MARK: Direct listener

    private func listenerSet(_ req: Json) throws -> Json {
        let enabled = req["enabled"] as? Bool ?? false
        let entries = req["pairings"] as? [Json] ?? []
        let wanted: [LinkPairing]
        do {
            wanted = try entries.map { e in
                let id = try id(e)
                return LinkPairing(id: id, macKey: try macKey(id), phoneKey: try phoneKey(e))
            }
        } catch {
            stopListener()
            throw error
        }
        let (old, start): (LinkListener?, Bool) = lock.withLock {
            let removed = !Set(wanted.map(\.id)).isSuperset(of: pairings.map(\.id))
            pairings = wanted
            latest = latest.filter { id, _ in wanted.contains { $0.id == id } }
            let old = listener
            guard enabled, !wanted.isEmpty else {
                listener = nil
                return (old, false)
            }
            // A removed pairing's connections must close, so start afresh.
            if removed, old != nil {
                listener = nil
                return (old, true)
            }
            return (nil, old == nil)
        }
        old?.stop()
        if start {
            do { try startListener() } catch {
                stopListener()
                throw error
            }
        }
        return [:]
    }

    /// After a failed `listener.set`: nothing listens and nothing is kept, so the next
    /// `listener.set` starts afresh.
    private func stopListener() {
        let old: LinkListener? = lock.withLock {
            defer { listener = nil }
            pairings = []
            latest = [:]
            return listener
        }
        old?.stop()
    }

    private func startListener() throws {
        let generation = lock.withLock {
            listeners += 1
            return listeners
        }
        let event: @Sendable (String, LinkConnection) -> Json = { name, c in
            ["event": name, "connection": generation << 32 | c.id, "id": c.pairingId.base64URLEncodedString()]
        }
        // A pairing without a state yet is left out of the handshake, so its phone is
        // dropped without a reply instead of getting an empty first frame. link/'s
        // `latestState` cannot decline; the empty fallback only covers a state removed
        // between the two calls.
        let listener = try LinkListener(
            pairings: { self.lock.withLock { self.pairings.filter { self.latest[$0.id] != nil } } },
            latestState: { id in self.lock.withLock { self.latest[id] } ?? Data() },
            onOpen: { self.emit(event("open", $0)) },
            onMessage: { c, m in
                self.emit(event("message", c).merging(["plaintext": m.base64URLEncodedString()]) { $1 })
            },
            onClose: { self.emit(event("close", $0)) })
        lock.withLock { self.listener = listener }
        Task {
            do { _ = try await listener.start() } catch {
                // A listener stopped or replaced before it was ready is no failure.
                let current = self.lock.withLock {
                    guard self.listener === listener else { return false }
                    self.listener = nil
                    return true
                }
                if current { self.emit(["event": "listener-failed", "error": "\(error)"]) }
            }
        }
    }

    private func listenerSend(_ req: Json) throws -> Json {
        let id = try id(req), plaintext = try bytes(req, "plaintext")
        let isState = (try? JSONSerialization.jsonObject(with: plaintext) as? Json)?["type"] as? String == "state"
        let listener: LinkListener? = lock.withLock {
            if isState { latest[id] = plaintext }
            return self.listener
        }
        listener?.send(plaintext, to: id)
        return [:]
    }

    private func emit(_ event: Json) {
        let (handler, ctx) = lock.withLock { (self.handler, self.ctx) }
        guard let handler, let data = try? JSONSerialization.data(withJSONObject: event) else { return }
        String(decoding: data, as: UTF8.self).withCString { handler(ctx, $0) }
    }

    // MARK: Keychain (generic passwords, the login keychain)

    // Every Keychain failure's text starts with "keychain:", which the Rust side reads as
    // transient (a pairing is kept for another try).

    private func account(_ id: Data, _ kind: String) -> String { "\(id.base64URLEncodedString()).\(kind)" }

    private func query(_ account: String) -> Json {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: Self.service,
            kSecAttrAccount as String: account,
        ]
    }

    /// Updates in place, so a failure keeps the old value.
    private func keychainSet(_ account: String, _ value: Data) throws {
        var status = SecItemUpdate(query(account) as CFDictionary, [kSecValueData as String: value] as CFDictionary)
        if status == errSecItemNotFound {
            var q = query(account)
            q[kSecValueData as String] = value
            status = SecItemAdd(q as CFDictionary, nil)
        }
        guard status == errSecSuccess else { throw BridgeError("keychain: cannot store (\(status))") }
    }

    /// nil when there is no such item.
    private func keychainRead(_ account: String) throws -> Data? {
        var q = query(account)
        q[kSecReturnData as String] = true
        q[kSecMatchLimit as String] = kSecMatchLimitOne
        var result: CFTypeRef?
        let status = SecItemCopyMatching(q as CFDictionary, &result)
        if status == errSecItemNotFound { return nil }
        guard status == errSecSuccess, let data = result as? Data else { throw BridgeError("keychain: \(status)") }
        return data
    }

    private func keychain(_ id: Data, _ kind: String) throws -> Data {
        guard let data = try keychainRead(account(id, kind)) else {
            throw BridgeError("keychain: no \(kind) for this pairing")
        }
        return data
    }

    private func keychainDelete(_ account: String) throws {
        let status = SecItemDelete(query(account) as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else {
            throw BridgeError("keychain: cannot delete (\(status))")
        }
    }
}

// MARK: Request fields

private func string(_ req: Json, _ key: String) throws -> String {
    guard let v = req[key] as? String else { throw BridgeError("\(key) must be a string") }
    return v
}

private func bytes(_ req: Json, _ key: String) throws -> Data {
    guard let v = Data(base64URLEncoded: try string(req, key)) else { throw BridgeError("\(key) must be base64url") }
    return v
}

private func id(_ req: Json) throws -> Data {
    let id = try bytes(req, "id")
    guard id.count == 16 else { throw BridgeError("id must be 16 bytes") }
    return id
}

private func phoneKey(_ req: Json) throws -> Curve25519.KeyAgreement.PublicKey {
    try Curve25519.KeyAgreement.PublicKey(rawRepresentation: bytes(req, "phoneKey"))
}

private func direction(_ req: Json) throws -> Direction {
    guard let d = Direction(rawValue: try string(req, "direction")) else { throw BridgeError("direction must be down or up") }
    return d
}

private func random(_ count: Int) throws -> Data {
    var d = Data(count: count)
    let status = d.withUnsafeMutableBytes { SecRandomCopyBytes(kSecRandomDefault, count, $0.baseAddress!) }
    guard status == errSecSuccess else { throw BridgeError("no randomness (\(status))") }
    return d
}
