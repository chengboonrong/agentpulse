import CryptoKit
import Foundation
import Network
import Testing

@testable import AgentPulseLink

/// Loopback only, Bonjour off, ephemeral ports.
@Suite(.timeLimit(.minutes(1)))
struct TransportTests {
    let id = randomId()
    let mac = PrivateKey()
    let phone = PrivateKey()
    static let state = Data("state 0".utf8)

    func listener(
        onOpen: @escaping @Sendable (LinkConnection) -> Void = { _ in },
        onMessage: @escaping @Sendable (LinkConnection, Data) -> Void = { _, _ in },
        onClose: @escaping @Sendable (LinkConnection) -> Void = { _ in }
    ) throws -> LinkListener {
        let pairings = [
            LinkPairing(id: randomId(), macKey: PrivateKey(), phoneKey: PrivateKey().publicKey),
            LinkPairing(id: id, macKey: mac, phoneKey: phone.publicKey),
        ]
        return try LinkListener(
            host: "127.0.0.1", advertise: false, pairings: { pairings }, latestState: { _ in Self.state },
            onOpen: onOpen, onMessage: onMessage, onClose: onClose)
    }

    func endpoint(_ port: NWEndpoint.Port) -> NWEndpoint { .hostPort(host: "127.0.0.1", port: port) }

    @Test func loopback() async throws {
        let (opens, opensIn) = AsyncStream<LinkConnection>.makeStream()
        let (fromPhone, fromPhoneIn) = AsyncStream<(LinkConnection, Data)>.makeStream()
        let (closes, closesIn) = AsyncStream<LinkConnection>.makeStream()
        let listener = try listener(
            onOpen: { opensIn.yield($0) }, onMessage: { fromPhoneIn.yield(($0, $1)) }, onClose: { closesIn.yield($0) })
        let port = try await listener.start()
        defer { listener.stop() }

        let client = try LinkClient(endpoint: endpoint(port), pairingId: id, phoneKey: phone, macKey: mac.publicKey)
        try await client.connect(firstMessage: Data("seen".utf8))
        var fromMac = client.messages.makeAsyncIterator()
        #expect(try await fromMac.next() == Self.state)

        // The Mac counts the connection once the phone's first frame (sent by connect) opens.
        let up = ["seen", "a", "b"].map { Data($0.utf8) }
        for message in up.dropFirst() { try client.send(message) }
        var opened = opens.makeAsyncIterator()
        let connection = try #require(await opened.next())
        #expect(connection.pairingId == id)
        var received = fromPhone.makeAsyncIterator()
        for message in up {
            let next = await received.next()
            #expect(next?.0 == connection && next?.1 == message)
        }

        let down = ["state 1", "heartbeat", "state 2"].map { Data($0.utf8) }
        for message in down { listener.send(message, to: id) }
        for message in down { #expect(try await fromMac.next() == message) }

        client.close()
        #expect(try await fromMac.next() == nil)
        var closed = closes.makeAsyncIterator()
        #expect(await closed.next() == connection)
        #expect(throws: LinkError.closed) { try client.send(Data()) }

        // A pairing the Mac doesn't know: closed without a reply.
        let stranger = try LinkClient(endpoint: endpoint(port), pairingId: randomId(), phoneKey: phone, macKey: mac.publicKey)
        await #expect(throws: LinkError.handshakeFailed) { try await stranger.connect(firstMessage: Data()) }
    }

    @Test func clientClosesAfterSilence() async throws {
        let listener = try listener()
        let port = try await listener.start()
        defer { listener.stop() }

        let client = try LinkClient(
            endpoint: endpoint(port), pairingId: id, phoneKey: phone, macKey: mac.publicKey, idleTimeout: 0.3)
        try await client.connect(firstMessage: Data("seen".utf8))
        var fromMac = client.messages.makeAsyncIterator()
        #expect(try await fromMac.next() == Self.state)
        await #expect(throws: LinkError.timeout) { _ = try await fromMac.next() }
    }

    /// The same phone connecting again replaces its live connection.
    @Test func reconnectReplacesTheOldConnection() async throws {
        let (opens, opensIn) = AsyncStream<LinkConnection>.makeStream()
        let (closes, closesIn) = AsyncStream<LinkConnection>.makeStream()
        let listener = try listener(onOpen: { opensIn.yield($0) }, onClose: { closesIn.yield($0) })
        let port = try await listener.start()
        defer { listener.stop() }
        var opened = opens.makeAsyncIterator()
        var closed = closes.makeAsyncIterator()

        let first = try LinkClient(endpoint: endpoint(port), pairingId: id, phoneKey: phone, macKey: mac.publicKey)
        try await first.connect(firstMessage: Data("seen".utf8))
        let old = try #require(await opened.next())

        let second = try LinkClient(endpoint: endpoint(port), pairingId: id, phoneKey: phone, macKey: mac.publicKey)
        try await second.connect(firstMessage: Data("seen".utf8))
        #expect(await closed.next() == old)
        let new = try #require(await opened.next())
        #expect(new != old && new.pairingId == id)

        var oldMessages = first.messages.makeAsyncIterator()
        #expect(try await oldMessages.next() == Self.state)
        await #expect(throws: LinkError.closed) { _ = try await oldMessages.next() }

        listener.send(Data("state 1".utf8), to: id)
        var newMessages = second.messages.makeAsyncIterator()
        #expect(try await newMessages.next() == Self.state)
        #expect(try await newMessages.next() == Data("state 1".utf8))
        second.close()
        #expect(await closed.next() == new)
    }

    /// A fake Mac answers message 2 with random bytes, then stays silent: `connect()` must not
    /// return, and gives up 5 s after it started.
    @Test func clientWaitsForTheMacsFirstFrame() async throws {
        let parameters = NWParameters.tcp
        parameters.requiredLocalEndpoint = .hostPort(host: "127.0.0.1", port: .any)
        let fake = try NWListener(using: parameters, on: .any)
        let queue = DispatchQueue(label: "fake Mac")
        fake.newConnectionHandler = { connection in
            connection.start(queue: queue)
            connection.receive(minimumIncompleteLength: 56, maximumLength: 56) { _, _, _, _ in
                let em = Data((0..<32).map { _ in UInt8.random(in: 0...255) })
                connection.send(content: lengthPrefix(32) + em, completion: .idempotent)
                // Silent from here; the pending receive keeps the connection open.
                connection.receive(minimumIncompleteLength: 1, maximumLength: 1) { _, _, _, _ in }
            }
        }
        let port: NWEndpoint.Port = try await withCheckedThrowingContinuation { c in
            fake.stateUpdateHandler = { state in
                switch state {
                case .ready:
                    fake.stateUpdateHandler = nil
                    c.resume(returning: fake.port ?? .any)
                case .failed(let error):
                    fake.stateUpdateHandler = nil
                    c.resume(throwing: error)
                default: break
                }
            }
            fake.start(queue: queue)
        }
        defer { fake.cancel() }

        let client = try LinkClient(endpoint: endpoint(port), pairingId: id, phoneKey: phone, macKey: mac.publicKey)
        let start = ContinuousClock.now
        await #expect(throws: LinkError.timeout) { try await client.connect(firstMessage: Data()) }
        let waited = ContinuousClock.now - start
        #expect(waited >= .seconds(5) && waited < .seconds(10), "\(waited)")
    }

    /// Someone replays a real phone's message 1. It gets message 2 and the Mac's first frame,
    /// which it can't open, is never reported, and is closed when the 5 s window ends.
    @Test func replayedHandshakeIsNeverReported() async throws {
        let (opens, opensIn) = AsyncStream<LinkConnection>.makeStream()
        let listener = try listener(onOpen: { opensIn.yield($0) })
        let port = try await listener.start()
        defer { listener.stop() }

        let captured = try LinkClientHandshake(pairingId: id, phoneKey: phone, macKey: mac.publicKey).message
        let start = ContinuousClock.now
        let (bytes, closedAt) = await rawExchange(endpoint(port), sending: captured)
        #expect(bytes == 4 + 32 + 4 + Self.state.count + 16)
        #expect(closedAt - start >= .seconds(5) && closedAt - start < .seconds(10), "\(closedAt - start)")

        opensIn.finish()
        var count = 0
        for await _ in opens { count += 1 }
        #expect(count == 0)
    }

    /// Nine idle connections: the ninth is closed at once (cap of 8), the other eight when
    /// the 5 s handshake window ends.
    @Test func connectionCapAndHandshakeTimeout() async throws {
        let listener = try listener()
        let port = try await listener.start()
        defer { listener.stop() }

        let start = ContinuousClock.now
        let closedAfter = await withTaskGroup(of: Duration.self) { group in
            for _ in 0..<9 { group.addTask { [self] in await rawExchange(endpoint(port), sending: Data()).1 - start } }
            return await group.reduce(into: []) { $0.append($1) }.sorted()
        }
        #expect(closedAfter[0] < .seconds(2))
        for t in closedAfter.dropFirst() {
            #expect(t >= .seconds(5) && t < .seconds(10), "\(t)")
        }
    }

    /// A raw TCP connection: sends `sending`, then reads until the server closes it, giving
    /// up after 10 s so a server that never closes fails the test instead of hanging it.
    /// Returns the bytes received and when it closed.
    func rawExchange(_ endpoint: NWEndpoint, sending: Data) async -> (Int, ContinuousClock.Instant) {
        let connection = NWConnection(to: endpoint, using: .tcp)
        defer { connection.cancel() }
        connection.start(queue: .global())
        DispatchQueue.global().asyncAfter(deadline: .now() + 10) { connection.cancel() }
        if !sending.isEmpty { connection.send(content: sending, completion: .idempotent) }
        return await withCheckedContinuation { done in
            @Sendable func read(_ total: Int) {
                connection.receive(minimumIncompleteLength: 1, maximumLength: 65_536) { data, _, isComplete, error in
                    let total = total + (data?.count ?? 0)
                    if isComplete || error != nil { done.resume(returning: (total, .now)) } else { read(total) }
                }
            }
            read(0)
        }
    }
}
