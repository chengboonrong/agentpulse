import AgentPulseLink
import CryptoKit
import Foundation
import Network

/// What the phone tells the user when the relay says no (relay/src/index.ts's codes).
public enum PhoneError: Error, Equatable, LocalizedError, Sendable {
    /// 401 or 404 for a pairing: the Mac removed it (or the relay dropped it).
    case removed
    /// 409 on the hello.
    case taken
    /// 401 on the hello: the code expired or was never valid.
    case expired
    /// Still 429 after waiting.
    case busy
    case badResponse(Int)

    public var errorDescription: String? {
        switch self {
        case .removed: "This Mac removed the pairing."
        case .taken: "Someone else answered this code."
        case .expired: "This code expired. Show a new one on your Mac."
        case .busy: "The relay is busy. Try again in a moment."
        case .badResponse(let status): "The relay answered HTTP \(status)."
        }
    }
}

/// What a direct connection delivers.
public enum DirectMessage: Equatable, Sendable {
    /// A state newer than the last accepted one, now stored.
    case state(Pairing)
    /// A heartbeat, or the state already shown sent again: the connection is alive.
    case alive(type: String, seq: UInt64)
    /// Not accepted: older than the last accepted state, or of an unknown type.
    case dropped(type: String, seq: UInt64)
}

/// A live direct connection to a paired Mac.
public struct DirectConnection: Identifiable, Sendable {
    public var id: ObjectIdentifier { ObjectIdentifier(client) }
    /// Accepted messages; finishes when the connection closes.
    public let messages: AsyncThrowingStream<DirectMessage, Error>
    let client: LinkClient
    public func close() { client.close() }
}

/// The phone's side of the link (docs/mobile.md): pairing, the relay and the direct
/// connection, persisting through a `PhoneStore`.
public struct PhoneClient: Sendable {
    public let store: PhoneStore
    let session: URLSession

    public init(store: PhoneStore, session: URLSession = URLSession(configuration: .ephemeral)) {
        self.store = store
        self.session = session
    }

    /// Says hello for a scanned or pasted link and stores the pairing. Returns the six-digit
    /// code the Mac shows too.
    public func pair(link string: String, phoneName: String) async throws -> (code: String, pairing: Pairing) {
        let link = try PairingLink(string)
        let key = Curve25519.KeyAgreement.PrivateKey()
        let token = Data((0..<32).map { _ in UInt8.random(in: 0...255) })
        let hello = try Hello(
            secret: link.secret, phoneKey: key.publicKey, phoneName: phoneName, sentAtMs: Self.nowMs())
        let pairing = Pairing(
            id: link.id.base64URLEncodedString(), relay: link.relay, macName: link.macName,
            macKey: link.macKey.rawRepresentation)
        let (status, _) = try await request(
            "POST", pairing, "hello", token: nil,
            body: [
                "secret": link.secret.base64URLEncodedString(),
                "phoneTokenHash": Data(SHA256.hash(data: token)).base64URLEncodedString(),
                "message": try RelayMessage.sealHello(hello, to: link.macKey, pairingId: link.id),
            ])
        switch status {
        case 204: break
        case 409: throw PhoneError.taken
        case 401, 404: throw PhoneError.expired
        default: throw PhoneError.badResponse(status)
        }
        try store.add(pairing, key: key, token: token)
        return (verificationCode(pairingId: link.id, macKey: link.macKey, phoneKey: key.publicKey), pairing)
    }

    /// `GET down`: stores a newer state and returns the pairing as stored (unchanged on
    /// 204, 304 or an old seq).
    public func fetchDown(_ pairing: Pairing) async throws -> Pairing {
        let current = try store.pairing(pairing.id) ?? pairing
        let (_, token) = try store.keys(pairing.id)
        let (status, data) = try await request(
            "GET", pairing, "down", token: token, etag: current.downSeq.map { "\"\($0)\"" })
        switch status {
        case 200: break
        case 204, 304: return current
        case 401, 404: throw PhoneError.removed
        default: throw PhoneError.badResponse(status)
        }
        guard let sealed = (try? JSONSerialization.jsonObject(with: data) as? [String: Any])?["message"] as? String
        else { throw PhoneError.badResponse(status) }
        let plaintext = try RelayMessage.open(
            sealed, with: store.keys(pairing.id).key, from: macKey(pairing), direction: .down,
            pairingId: rawId(pairing))
        return try take(plaintext, pairing.id).1 ?? current
    }

    public enum Up: String, Sendable { case seen, unpair }

    /// `POST up` through the relay.
    public func post(_ type: Up, _ pairing: Pairing) async throws {
        let (key, token) = try store.keys(pairing.id)
        let sealed = try RelayMessage.seal(
            nextUp(type, pairing.id), to: macKey(pairing), from: key, direction: .up, pairingId: rawId(pairing))
        let (status, _) = try await request("POST", pairing, "up", token: token, body: ["message": sealed])
        switch status {
        case 204: return
        case 401, 404: throw PhoneError.removed
        default: throw PhoneError.badResponse(status)
        }
    }

    /// Tells the Mac (`unpair`; the Mac then deletes the pairing at the relay) and deletes the
    /// pairing and its secrets here, always. If the `unpair` can't be posted, deletes the
    /// pairing at the relay itself as a best effort, so the Mac sees it gone.
    public func remove(_ pairing: Pairing) async {
        defer { try? store.delete(pairing.id) }
        do {
            try await post(.unpair, pairing)
        } catch {
            guard let token = try? store.keys(pairing.id).token else { return }
            _ = try? await request("DELETE", pairing, nil, token: token)
        }
    }

    /// Connects directly to a Mac found over Bonjour, `seen` as the first frame. Throws when
    /// that Mac doesn't know the pairing or doesn't answer within 5 s.
    public func connect(_ pairing: Pairing, to endpoint: NWEndpoint) async throws -> DirectConnection {
        let client = try LinkClient(
            endpoint: endpoint, pairingId: rawId(pairing), phoneKey: store.keys(pairing.id).key,
            macKey: macKey(pairing))
        try await client.connect(firstMessage: nextUp(.seen, pairing.id))
        let messages = AsyncThrowingStream<DirectMessage, Error> { out in
            let task = Task {
                do {
                    for try await frame in client.messages {
                        if let m = try take(frame, pairing.id).0 { out.yield(m) }
                    }
                    out.finish()
                } catch {
                    out.finish(throwing: error)
                }
            }
            out.onTermination = { _ in
                task.cancel()
                client.close()
            }
        }
        return DirectConnection(messages: messages, client: client)
    }

    // MARK: -

    /// A `down` plaintext: a newer `state` is stored (`seq` > last); a `heartbeat`, or a
    /// repeated state, only counts with the last accepted seq. Anything else is dropped;
    /// nil when it isn't a message at all.
    func take(_ plaintext: Data, _ id: String) throws -> (DirectMessage?, Pairing?) {
        guard let m = try? JSONSerialization.jsonObject(with: plaintext) as? [String: Any],
            let seq = (m["seq"] as? NSNumber)?.uint64Value, let type = m["type"] as? String
        else { return (nil, nil) }
        let payload = (m["payload"] as? [String: Any]).flatMap { try? JSONSerialization.data(withJSONObject: $0) }
        let updated = try store.update(id) { p -> DirectMessage? in
            if type == "state", let payload {
                var seqGuard = SeqGuard(last: p.downSeq)
                if seqGuard.accept(seq) {
                    p.downSeq = seq
                    p.lastState = payload
                    p.lastStateAt = .now
                    return .state(p)
                }
            }
            guard ["state", "heartbeat"].contains(type), seq == p.downSeq else { return .dropped(type: type, seq: seq) }
            return .alive(type: type, seq: seq)
        }
        guard let (p, message) = updated else { throw PhoneError.removed }
        return (message, p)
    }

    /// The next `up` message, its seq taken from the shared record.
    private func nextUp(_ type: Up, _ id: String) throws -> Data {
        guard let (_, seq) = try store.update(id, { p -> UInt64 in
            p.upSeq += 1
            return p.upSeq
        }) else { throw PhoneError.removed }
        var m: [String: Any] = ["seq": seq, "type": type.rawValue]
        if type == .seen { m["sentAtMs"] = Self.nowMs() }
        return try JSONSerialization.data(withJSONObject: m)
    }

    private func rawId(_ p: Pairing) throws -> Data {
        guard let d = Data(base64URLEncoded: p.id) else { throw LinkError.invalidPairingId }
        return d
    }

    private func macKey(_ p: Pairing) throws -> Curve25519.KeyAgreement.PublicKey {
        try .init(rawRepresentation: p.macKey)
    }

    static func nowMs() -> Int64 { Int64(Date().timeIntervalSince1970 * 1000) }

    /// One relay request; waits out a 429 (Retry-After) up to five times.
    private func request(
        _ method: String, _ pairing: Pairing, _ route: String?, token: Data?, body: [String: Any]? = nil,
        etag: String? = nil
    ) async throws -> (Int, Data) {
        let path = "v1/pairings/\(pairing.id)" + (route.map { "/\($0)" } ?? "")
        var r = URLRequest(url: pairing.relay.appending(path: path), timeoutInterval: 20)
        r.httpMethod = method
        r.cachePolicy = .reloadIgnoringLocalCacheData
        if let token { r.setValue("Bearer \(token.base64URLEncodedString())", forHTTPHeaderField: "Authorization") }
        if let etag { r.setValue(etag, forHTTPHeaderField: "If-None-Match") }
        if let body {
            r.httpBody = try JSONSerialization.data(withJSONObject: body)
            r.setValue("application/json", forHTTPHeaderField: "Content-Type")
        }
        for _ in 0..<5 {
            let (data, response) = try await session.data(for: r)
            guard let http = response as? HTTPURLResponse else { throw PhoneError.badResponse(0) }
            guard http.statusCode == 429 else { return (http.statusCode, data) }
            let wait = Double(http.value(forHTTPHeaderField: "Retry-After") ?? "1") ?? 1
            try await Task.sleep(for: .seconds(min(max(wait, 1), 10)))
        }
        throw PhoneError.busy
    }
}
