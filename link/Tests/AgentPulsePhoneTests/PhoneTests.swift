import AgentPulseLink
import CryptoKit
import Foundation
import Network
import Testing

@testable import AgentPulsePhone

typealias PrivateKey = Curve25519.KeyAgreement.PrivateKey

/// A relay in memory with relay/src/index.ts's routes and status codes, one per host so
/// tests can run in parallel.
final class FakeRelay: URLProtocol, @unchecked Sendable {
    struct Slot {
        var secretHash: Data?
        var phoneTokenHash: Data?
        var down: (seq: UInt64, message: String)?
        var up: [String] = []
        /// `POST up` answers 500.
        var failUp = false
    }

    nonisolated(unsafe) private static var relays: [String: FakeRelay.State] = [:]
    private static let lock = NSLock()

    final class State: @unchecked Sendable {
        let lock = NSLock()
        var pairings: [String: Slot] = [:]
        var requests: [URLRequest] = []
    }

    static func make() -> (URL, State) {
        let host = "relay-\(UUID().uuidString.lowercased()).example"
        let state = State()
        lock.withLock { relays[host] = state }
        return (URL(string: "https://\(host)")!, state)
    }

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
    override func stopLoading() {}

    override func startLoading() {
        let r = request
        let state = Self.lock.withLock { Self.relays[r.url?.host() ?? ""] }
        let (status, body, headers) = state.map { s in s.lock.withLock { s.handle(r) } } ?? (404, nil, [:])
        let response = HTTPURLResponse(url: r.url!, statusCode: status, httpVersion: "HTTP/1.1", headerFields: headers)!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        if let body { client?.urlProtocol(self, didLoad: try! JSONSerialization.data(withJSONObject: body)) }
        client?.urlProtocolDidFinishLoading(self)
    }
}

extension FakeRelay.State {
    func handle(_ r: URLRequest) -> (Int, [String: Any]?, [String: String]) {
        requests.append(r)
        let parts = r.url!.path.split(separator: "/").map(String.init)  // v1 pairings id [route]
        guard parts.count == 3 || parts.count == 4 else { return (404, nil, [:]) }
        let (id, route) = (parts[2], "\(r.httpMethod!) \(parts.count == 4 ? parts[3] : "")")
        let body = r.httpBodyStream.map(Self.read).flatMap { try? JSONSerialization.jsonObject(with: $0) as? [String: Any] } ?? [:]
        guard var slot = pairings[id] else { return (401, ["error": "unauthorized"], [:]) }
        defer { if pairings[id] != nil { pairings[id] = slot } }
        if route == "POST hello" {
            if slot.phoneTokenHash != nil { return (409, ["error": "pairing already has its hello"], [:]) }
            guard let s = (body["secret"] as? String).flatMap(Data.init(base64URLEncoded:)),
                Data(SHA256.hash(data: s)) == slot.secretHash
            else { return (401, ["error": "unauthorized"], [:]) }
            slot.secretHash = nil
            slot.phoneTokenHash = (body["phoneTokenHash"] as? String).flatMap(Data.init(base64URLEncoded:))
            slot.up = [body["message"] as! String]
            return (204, nil, [:])
        }
        let token = (r.value(forHTTPHeaderField: "Authorization") ?? "").replacingOccurrences(of: "Bearer ", with: "")
        guard let t = Data(base64URLEncoded: token), Data(SHA256.hash(data: t)) == slot.phoneTokenHash else {
            return (401, ["error": "unauthorized"], [:])
        }
        switch route {
        case "GET down":
            guard let down = slot.down else { return (204, nil, [:]) }
            let etag = ["etag": "\"\(down.seq)\""]
            if r.value(forHTTPHeaderField: "If-None-Match")?.replacingOccurrences(of: "\"", with: "") == "\(down.seq)" {
                return (304, nil, etag)
            }
            return (200, ["seq": down.seq, "message": down.message], etag)
        case "POST up":
            if slot.failUp { return (500, nil, [:]) }
            slot.up.append(body["message"] as! String)
            return (204, nil, [:])
        case "DELETE ":
            pairings[id] = nil
            return (204, nil, [:])
        default:
            return (404, nil, [:])
        }
    }

    static func read(_ stream: InputStream) -> Data {
        stream.open()
        defer { stream.close() }
        var data = Data()
        var buffer = [UInt8](repeating: 0, count: 4096)
        while stream.hasBytesAvailable {
            let n = stream.read(&buffer, maxLength: buffer.count)
            guard n > 0 else { break }
            data.append(buffer, count: n)
        }
        return data
    }
}

/// The Mac's side, by hand: makes the pairing and link, opens the hello, seals states.
struct Mac {
    let id = Data((0..<16).map { _ in UInt8.random(in: 0...255) })
    let key = PrivateKey()
    let secret = Data((0..<32).map { _ in UInt8.random(in: 0...255) })
    let relay: URL
    let state: FakeRelay.State
    var idText: String { id.base64URLEncodedString() }

    init() {
        (relay, state) = FakeRelay.make()
        state.lock.withLock { state.pairings[idText] = .init(secretHash: Data(SHA256.hash(data: secret))) }
    }

    var link: String {
        get throws { try PairingLink(relay: relay, id: id, macKey: key.publicKey, secret: secret, macName: "Studio").string }
    }

    func hello() throws -> Hello {
        let m = try #require(state.lock.withLock { state.pairings[idText]?.up.first })
        return try RelayMessage.openHello(m, with: key, pairingId: id, secret: secret)
    }

    /// `up` plaintexts after the hello.
    func up(_ phone: Curve25519.KeyAgreement.PublicKey) throws -> [[String: Any]] {
        try state.lock.withLock { state.pairings[idText]!.up }.dropFirst().map {
            let p = try RelayMessage.open($0, with: key, from: phone, direction: .up, pairingId: id)
            return try JSONSerialization.jsonObject(with: p) as! [String: Any]
        }
    }

    func message(_ seq: UInt64, type: String = "state", active: Int = 0) throws -> Data {
        try JSONSerialization.data(withJSONObject: [
            "seq": seq, "type": type, "payload": ["version": 1, "agents": ["active": active]],
        ])
    }

    func put(_ seq: UInt64, active: Int = 0, for phone: Curve25519.KeyAgreement.PublicKey) throws {
        let sealed = try RelayMessage.seal(
            message(seq, active: active), to: phone, from: key, direction: .down, pairingId: id)
        state.lock.withLock { state.pairings[idText]!.down = (seq, sealed) }
    }
}

func tempDir() -> URL {
    FileManager.default.temporaryDirectory.appending(path: "phone-tests-\(UUID().uuidString)")
}

func session() -> URLSession {
    let c = URLSessionConfiguration.ephemeral
    c.protocolClasses = [FakeRelay.self]
    return URLSession(configuration: c)
}

func active(_ p: Pairing) throws -> Int? {
    let state = try JSONSerialization.jsonObject(with: try #require(p.lastState)) as? [String: Any]
    return (state?["agents"] as? [String: Any])?["active"] as? Int
}

struct PhoneTests {
    let mac = Mac()
    let secrets = MemorySecrets()
    let dir = tempDir()
    var phone: PhoneClient { PhoneClient(store: PhoneStore(container: dir, secrets: secrets), session: session()) }

    func paired() async throws -> (Pairing, Curve25519.KeyAgreement.PublicKey) {
        let (code, pairing) = try await phone.pair(link: try mac.link, phoneName: "Test iPhone")
        let hello = try mac.hello()
        #expect(hello.phoneName == "Test iPhone")
        #expect(code == verificationCode(pairingId: mac.id, macKey: mac.key.publicKey, phoneKey: hello.phoneKey))
        return (pairing, hello.phoneKey)
    }

    @Test func pairingHappyPath() async throws {
        let (pairing, phoneKey) = try await paired()
        #expect(pairing.id == mac.idText && pairing.macName == "Studio" && pairing.relay == mac.relay)
        #expect(try phone.store.pairings() == [pairing])
        #expect(try secrets.read(pairing.id)?.count == 64)

        // Nothing until the Mac confirms.
        #expect(try await phone.fetchDown(pairing).lastState == nil)
        try mac.put(0, active: 2, for: phoneKey)
        let got = try await phone.fetchDown(pairing)
        #expect(got.downSeq == 0 && got.lastStateAt != nil)
        #expect(try active(got) == 2)
        #expect(try phone.store.pairing(pairing.id) == got)

        try await phone.post(.seen, got)
        let up = try mac.up(phoneKey)
        #expect(up.count == 1 && up[0]["type"] as? String == "seen" && up[0]["seq"] as? Int == 1)
        #expect(up[0]["sentAtMs"] is Int)
    }

    @Test func secondHelloIsTaken() async throws {
        _ = try await paired()
        await #expect(throws: PhoneError.taken) {
            try await PhoneClient(store: PhoneStore(container: tempDir(), secrets: MemorySecrets()), session: session())
                .pair(link: try mac.link, phoneName: "Someone else")
        }
        #expect(PhoneError.taken.localizedDescription == "Someone else answered this code.")
    }

    @Test func expiredOrUnknownHello() async throws {
        mac.state.lock.withLock { mac.state.pairings = [:] }
        await #expect(throws: PhoneError.expired) { try await phone.pair(link: try mac.link, phoneName: "iPhone") }
        #expect(try phone.store.pairings().isEmpty)
    }

    @Test func etagGives304() async throws {
        let (pairing, phoneKey) = try await paired()
        try mac.put(3, for: phoneKey)
        let first = try await phone.fetchDown(pairing)
        let again = try await phone.fetchDown(first)
        #expect(again == first)
        let last = try #require(mac.state.lock.withLock { mac.state.requests.last })
        #expect(last.value(forHTTPHeaderField: "If-None-Match") == "\"3\"")
    }

    @Test func olderSeqIsRejected() async throws {
        let (pairing, phoneKey) = try await paired()
        try mac.put(5, active: 5, for: phoneKey)
        let five = try await phone.fetchDown(pairing)
        // The relay slot going back (another process could also have stored a newer one).
        try mac.put(4, active: 4, for: phoneKey)
        let after = try await phone.fetchDown(five)
        #expect(try after.downSeq == 5 && active(after) == 5)
        try mac.put(6, active: 6, for: phoneKey)
        #expect(try active(await phone.fetchDown(after)) == 6)
    }

    @Test func heartbeatSeqRule() async throws {
        let (pairing, phoneKey) = try await paired()
        try mac.put(2, for: phoneKey)
        _ = try await phone.fetchDown(pairing)
        let take = { (seq: UInt64, type: String) in try phone.take(try mac.message(seq, type: type), pairing.id).0 }
        #expect(try take(2, "heartbeat") == .alive(type: "heartbeat", seq: 2))
        #expect(try take(1, "heartbeat") == .dropped(type: "heartbeat", seq: 1))
        #expect(try take(3, "heartbeat") == .dropped(type: "heartbeat", seq: 3))
        #expect(try take(2, "state") == .alive(type: "state", seq: 2))
        #expect(try take(1, "state") == .dropped(type: "state", seq: 1))
        #expect(try take(2, "push") == .dropped(type: "push", seq: 2))
        // Untouched by any of those: no rewrite of the shared file.
        let before = try FileManager.default.attributesOfItem(atPath: dir.appending(path: "pairings.json").path)[.modificationDate] as? Date
        #expect(try take(2, "heartbeat") == .alive(type: "heartbeat", seq: 2))
        #expect(try FileManager.default.attributesOfItem(atPath: dir.appending(path: "pairings.json").path)[.modificationDate] as? Date == before)
        guard case .state(let p)? = try take(3, "state") else { Issue.record("seq 3 not accepted"); return }
        #expect(p.downSeq == 3)
        #expect(try take(3, "heartbeat") == .alive(type: "heartbeat", seq: 3))
    }

    @Test func removeSendsUnpairAndDeletesSecrets() async throws {
        let (pairing, phoneKey) = try await paired()
        try await phone.post(.seen, pairing)
        await phone.remove(pairing)
        let up = try mac.up(phoneKey)
        #expect(up.map { $0["type"] as? String } == ["seen", "unpair"])
        #expect(up.map { $0["seq"] as? Int } == [1, 2])
        #expect(!mac.state.lock.withLock { mac.state.requests.contains { $0.httpMethod == "DELETE" } })
        #expect(try phone.store.pairings().isEmpty)
        #expect(try secrets.read(pairing.id) == nil)
    }

    @Test func macRemovedThePairing() async throws {
        let (pairing, _) = try await paired()
        mac.state.lock.withLock { mac.state.pairings = [:] }
        await #expect(throws: PhoneError.removed) { try await phone.fetchDown(pairing) }
        await #expect(throws: PhoneError.removed) { try await phone.post(.seen, pairing) }
        #expect(PhoneError.removed.localizedDescription == "This Mac removed the pairing.")
        // Removing still works and cleans up.
        await phone.remove(pairing)
        #expect(try phone.store.pairings().isEmpty && secrets.read(pairing.id) == nil)
    }

    @Test func failedUnpairDeletesAtTheRelay() async throws {
        let (pairing, _) = try await paired()
        mac.state.lock.withLock { mac.state.pairings[mac.idText]!.failUp = true }
        await phone.remove(pairing)
        let deleted = mac.state.lock.withLock { mac.state.pairings[mac.idText] == nil }
        #expect(deleted)
        let last = try #require(mac.state.lock.withLock { mac.state.requests.last })
        #expect(last.httpMethod == "DELETE" && last.url?.path == "/v1/pairings/\(mac.idText)")
        #expect(try phone.store.pairings().isEmpty && secrets.read(pairing.id) == nil)
    }

    /// Over loopback against the real listener: `seen` first, then the state and heartbeats.
    @Test(.timeLimit(.minutes(1))) func direct() async throws {
        let (pairing, phoneKey) = try await paired()
        try mac.put(1, for: phoneKey)
        _ = try await phone.fetchDown(pairing)
        let (seen, seenIn) = AsyncStream<Data>.makeStream()
        let m = mac
        let listener = try LinkListener(
            host: "127.0.0.1", advertise: false,
            pairings: { [LinkPairing(id: m.id, macKey: m.key, phoneKey: phoneKey)] },
            latestState: { _ in try! m.message(1) }, onOpen: { _ in }, onMessage: { seenIn.yield($1) },
            onClose: { _ in })
        let port = try await listener.start()
        defer { listener.stop() }

        let direct = try await phone.connect(pairing, to: .hostPort(host: "127.0.0.1", port: port))
        var up = seen.makeAsyncIterator()
        let first = try JSONSerialization.jsonObject(with: try #require(await up.next())) as? [String: Any]
        #expect(first?["type"] as? String == "seen" && first?["seq"] as? Int == 1)
        var messages = direct.messages.makeAsyncIterator()
        #expect(try await messages.next() == .alive(type: "state", seq: 1))
        listener.send(try mac.message(2, active: 3), to: mac.id)
        listener.send(try mac.message(1, type: "heartbeat"), to: mac.id)
        listener.send(try mac.message(2, type: "heartbeat"), to: mac.id)
        guard case .state(let p)? = try await messages.next() else { Issue.record("no state"); return }
        #expect(try p.downSeq == 2 && active(p) == 3)
        #expect(try await messages.next() == .dropped(type: "heartbeat", seq: 1))
        #expect(try await messages.next() == .alive(type: "heartbeat", seq: 2))
        direct.close()
        #expect(try await messages.next() == nil)
    }

    @Test func storeRoundTrip() throws {
        let store = PhoneStore(container: dir, secrets: secrets)
        var a = Pairing(id: "a", relay: URL(string: "https://r.example")!, macName: "A", macKey: Data(repeating: 1, count: 32))
        let b = Pairing(id: "b", relay: URL(string: "http://localhost:8787")!, macName: "B", macKey: Data(repeating: 2, count: 32))
        try store.add(a, key: PrivateKey(), token: Data(repeating: 3, count: 32))
        try store.add(b, key: PrivateKey(), token: Data(repeating: 4, count: 32))
        _ = try store.update("a") { p in
            p.lastState = Data("{}".utf8)
            p.lastStateAt = Date(timeIntervalSince1970: 1000)
            p.downSeq = 7
            p.upSeq = 2
        }
        a.lastState = Data("{}".utf8)
        a.lastStateAt = Date(timeIntervalSince1970: 1000)
        a.downSeq = 7
        a.upSeq = 2
        // A second store on the same container, as the widget would open it.
        let other = PhoneStore(container: dir, secrets: secrets)
        #expect(try other.pairings() == [a, b])
        #expect(try other.keys("b").token == Data(repeating: 4, count: 32))
        try other.delete("a")
        #expect(try store.pairings() == [b])
        #expect(try secrets.read("a") == nil)
        #expect(try store.update("a") { _ in } == nil)

        // An older record without the defaulted fields loads; a broken file is an error.
        let old = #"[{"id":"c","relay":"https://r.example","macName":"C","macKey":"AQ=="}]"#
        try Data(old.utf8).write(to: dir.appending(path: "pairings.json"))
        #expect(try store.pairings().first?.upSeq == 0)
        try Data("{".utf8).write(to: dir.appending(path: "pairings.json"))
        #expect(throws: (any Error).self) { try store.pairings() }
    }
}
