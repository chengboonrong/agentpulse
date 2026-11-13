import CryptoKit
import Foundation
import Network

// The direct connection over Network.framework (docs/mobile.md, "Direct connection").

let serviceType = "_agentpulse._tcp"

extension NWConnection {
    fileprivate func receive(exactly n: Int, _ done: @escaping @Sendable (Data?) -> Void) {
        receive(minimumIncompleteLength: n, maximumLength: n) { data, _, _, error in
            done(error == nil && data?.count == n ? data : nil)
        }
    }

    /// One length-prefixed message, prefix included. Nil on EOF, error or a length over `maxLength`.
    fileprivate func receiveMessage(maxLength: Int, _ done: @escaping @Sendable (Data?) -> Void) {
        receive(exactly: 4) { prefix in
            guard let prefix, case let n = length(ofPrefix: prefix), n > 0, n <= maxLength else { return done(nil) }
            self.receive(exactly: n) { body in done(body.map { prefix + $0 }) }
        }
    }
}

/// One phone's direct connection, as the listener's callbacks see it.
public struct LinkConnection: Hashable, Sendable {
    public let id: Int
    public let pairingId: Data
}

/// The Mac's side: accepts phones, runs the handshake, then exchanges frames.
/// Callbacks run on the listener's private queue.
public final class LinkListener: @unchecked Sendable {
    private struct Peer {
        let connection: NWConnection
        var pairingId: Data?
        var session: LinkSession?
        /// The phone's first frame opened, so it holds the pairing's private key.
        var authenticated = false
    }

    static let maxConnections = 8
    static let handshakeTimeout: TimeInterval = 5

    private let queue = DispatchQueue(label: "AgentPulseLink.LinkListener")
    private let listener: NWListener
    private let pairings: @Sendable () -> [LinkPairing]
    private let latestState: @Sendable (Data) -> Data
    private let onOpen: @Sendable (LinkConnection) -> Void
    private let onMessage: @Sendable (LinkConnection, Data) -> Void
    private let onClose: @Sendable (LinkConnection) -> Void
    // State below is only touched on `queue`.
    private var peers: [Int: Peer] = [:]
    private var nextPeer = 0
    private var ready: CheckedContinuation<NWEndpoint.Port, Error>?

    /// - Parameters:
    ///   - host: bind to this address only (tests use 127.0.0.1); nil for all interfaces.
    ///   - advertise: Bonjour `_agentpulse._tcp`, instance "AgentPulse", TXT `v=1`.
    ///   - pairings: every confirmed pairing; called once per handshake.
    ///   - latestState: pairing id to the latest state, sent as the Mac's first frame right
    ///     after message 2. Also called for peers that never authenticate, so it is not a
    ///     sign of a connection; `onOpen` is.
    ///   - onOpen: the phone's first frame opened within 5 s of accept; the connection is
    ///     live. The pairing's older connections are closed first (`onClose` for live
    ///     ones). That frame then arrives through `onMessage`.
    ///   - onMessage: a message from a live connection.
    ///   - onClose: a live connection closed.
    public init(
        host: NWEndpoint.Host? = nil, port: NWEndpoint.Port = .any, advertise: Bool = true,
        pairings: @escaping @Sendable () -> [LinkPairing],
        latestState: @escaping @Sendable (Data) -> Data,
        onOpen: @escaping @Sendable (LinkConnection) -> Void,
        onMessage: @escaping @Sendable (LinkConnection, Data) -> Void,
        onClose: @escaping @Sendable (LinkConnection) -> Void
    ) throws {
        let parameters = NWParameters.tcp
        if let host { parameters.requiredLocalEndpoint = .hostPort(host: host, port: port) }
        listener = try NWListener(using: parameters, on: port)
        if advertise {
            listener.service = NWListener.Service(
                name: "AgentPulse", type: serviceType, domain: nil, txtRecord: NWTXTRecord(["v": "1"]))
        }
        self.pairings = pairings
        self.latestState = latestState
        self.onOpen = onOpen
        self.onMessage = onMessage
        self.onClose = onClose
        listener.stateUpdateHandler = { [weak self] in self?.listenerChanged($0) }
        listener.newConnectionHandler = { [weak self] in self?.accept($0) }
    }

    deinit {
        listener.cancel()
        for peer in peers.values { peer.connection.cancel() }
    }

    /// Starts listening; returns the bound port.
    public func start() async throws -> NWEndpoint.Port {
        try await withCheckedThrowingContinuation { c in
            queue.async {
                self.ready = c
                self.listener.start(queue: self.queue)
            }
        }
    }

    /// Sends a message to every connection of this pairing that has finished message 2.
    /// That includes ones not yet live: only the pairing's phone can open the frame, and the
    /// real phone then doesn't miss a state that changed while its first frame was on the way.
    public func send(_ message: Data, to pairingId: Data) {
        queue.async {
            for (id, peer) in self.peers where peer.pairingId == pairingId {
                guard let session = peer.session, let frame = try? session.seal(message) else {
                    self.drop(id)
                    continue
                }
                peer.connection.send(content: frame, completion: self.dropOnError(id))
            }
        }
    }

    /// Stops listening and closes every connection.
    public func stop() {
        queue.async {
            self.listener.cancel()
            for id in self.peers.keys { self.drop(id) }
        }
    }

    private func listenerChanged(_ state: NWListener.State) {
        switch state {
        case .ready:
            ready?.resume(returning: listener.port ?? .any)
            ready = nil
        case .failed(let error):
            ready?.resume(throwing: error)
            ready = nil
            listener.cancel()
        case .cancelled:
            ready?.resume(throwing: LinkError.closed)
            ready = nil
        default:
            break
        }
    }

    private func accept(_ connection: NWConnection) {
        guard peers.count < Self.maxConnections else { return connection.cancel() }
        let id = nextPeer
        nextPeer += 1
        peers[id] = Peer(connection: connection)
        connection.stateUpdateHandler = { [weak self] state in
            switch state {
            case .failed, .cancelled: self?.drop(id)
            default: break
            }
        }
        connection.start(queue: queue)
        // Message 1, message 2 and the phone's first frame all within the window.
        queue.asyncAfter(deadline: .now() + Self.handshakeTimeout) { [weak self] in
            if let self, let peer = self.peers[id], !peer.authenticated { self.drop(id) }
        }
        connection.receiveMessage(maxLength: helloLength) { [weak self] in self?.handshake(id, $0) }
    }

    private func handshake(_ id: Int, _ message: Data?) {
        guard let peer = peers[id] else { return }
        // No matching pairing or a malformed message: close without a reply.
        guard let message, let r = try? LinkServerHandshake.respond(to: message, pairings: pairings()) else {
            return drop(id)
        }
        peers[id]?.pairingId = r.pairingId
        peers[id]?.session = r.session
        guard let first = try? r.session.seal(latestState(r.pairingId)) else { return drop(id) }
        peer.connection.send(content: r.reply + first, completion: dropOnError(id))
        readFrames(id)
    }

    private func readFrames(_ id: Int) {
        peers[id]?.connection.receiveMessage(maxLength: maxFrameLength) { [weak self] frame in
            guard let self, let peer = self.peers[id], let session = peer.session, let pairingId = peer.pairingId
            else { return }
            guard let frame, let message = try? session.open(frame) else { return self.drop(id) }
            let handle = LinkConnection(id: id, pairingId: pairingId)
            if !peer.authenticated {
                self.peers[id]?.authenticated = true
                // A phone that reconnects replaces its older connections, live or not.
                for (other, p) in self.peers where other < id && p.pairingId == pairingId { self.drop(other) }
                self.onOpen(handle)
            }
            self.onMessage(handle, message)
            self.readFrames(id)
        }
    }

    private func dropOnError(_ id: Int) -> NWConnection.SendCompletion {
        .contentProcessed { [weak self] error in
            if error != nil { self?.drop(id) }
        }
    }

    private func drop(_ id: Int) {
        guard let peer = peers.removeValue(forKey: id) else { return }
        peer.connection.cancel()
        if peer.authenticated, let pairingId = peer.pairingId { onClose(LinkConnection(id: id, pairingId: pairingId)) }
    }
}

/// The phone's side of one direct connection. Sending a heartbeat is the caller's job;
/// the client closes after 90 s without a frame from the Mac.
public final class LinkClient: @unchecked Sendable {
    /// Messages from the Mac, the latest state first. Finishes when the connection closes:
    /// normally after `close()`, with an error otherwise.
    public let messages: AsyncThrowingStream<Data, Error>
    private let stream: AsyncThrowingStream<Data, Error>.Continuation
    private let queue = DispatchQueue(label: "AgentPulseLink.LinkClient")
    private let connection: NWConnection
    private let idleTimeout: TimeInterval
    static let connectTimeout: TimeInterval = 5
    // State below is only touched on `queue`.
    private var handshake: LinkClientHandshake?
    private var session: LinkSession?
    private var connecting: CheckedContinuation<Void, Error>?
    private var firstMessage = Data()
    private var started = false
    private var closed = false
    private var lastHeard = DispatchTime.now()

    public convenience init(
        endpoint: NWEndpoint, pairingId: Data, phoneKey: Curve25519.KeyAgreement.PrivateKey,
        macKey: Curve25519.KeyAgreement.PublicKey
    ) throws {
        try self.init(endpoint: endpoint, pairingId: pairingId, phoneKey: phoneKey, macKey: macKey, idleTimeout: 90)
    }

    init(
        endpoint: NWEndpoint, pairingId: Data, phoneKey: Curve25519.KeyAgreement.PrivateKey,
        macKey: Curve25519.KeyAgreement.PublicKey, idleTimeout: TimeInterval
    ) throws {
        handshake = try LinkClientHandshake(pairingId: pairingId, phoneKey: phoneKey, macKey: macKey)
        connection = NWConnection(to: endpoint, using: .tcp)
        self.idleTimeout = idleTimeout
        (messages, stream) = AsyncThrowingStream.makeStream()
        connection.stateUpdateHandler = { [weak self] in self?.connectionChanged($0) }
    }

    deinit {
        connection.cancel()
        stream.finish()
    }

    /// Connects, runs the handshake and returns once the Mac's first frame has opened, which
    /// proves the Mac holds the pairing's key; that frame is the first item on `messages`.
    /// `firstMessage` (the `seen` message) is sent as the phone's first frame right then,
    /// before returning, so it lands inside the Mac's 5 s window.
    /// Throws `LinkError.timeout` if that takes more than 5 s. Call once.
    public func connect(firstMessage: Data) async throws {
        try await withCheckedThrowingContinuation { (c: CheckedContinuation<Void, Error>) in
            queue.async {
                guard !self.started, !self.closed else { return c.resume(throwing: LinkError.closed) }
                self.started = true
                self.connecting = c
                self.firstMessage = firstMessage
                self.lastHeard = .now()
                self.watchSilence()
                self.watchConnect()
                self.connection.start(queue: self.queue)
            }
        }
    }

    /// Sends one message as a frame. Throws if `connect()` hasn't returned or the connection
    /// is closed. Blocks briefly on the client's private queue to seal in order; callers are
    /// never on that queue (callbacks only reach them through `messages`).
    public func send(_ message: Data) throws {
        dispatchPrecondition(condition: .notOnQueue(queue))
        return try queue.sync {
            guard let session, connecting == nil, !closed else { throw LinkError.closed }
            do {
                let frame = try session.seal(message)
                connection.send(
                    content: frame,
                    completion: .contentProcessed { [weak self] error in
                        if error != nil { self?.fail(LinkError.closed) }
                    })
            } catch {
                fail(error)
                throw error
            }
        }
    }

    public func close() {
        queue.async { self.fail(nil) }
    }

    private func connectionChanged(_ state: NWConnection.State) {
        switch state {
        case .ready:
            guard let message = handshake?.message else { return }
            connection.send(
                content: message,
                completion: .contentProcessed { [weak self] error in
                    if error != nil { self?.fail(LinkError.closed) }
                })
            connection.receiveMessage(maxLength: replyLength) { [weak self] in self?.finishHandshake($0) }
        case .waiting(let error), .failed(let error):
            fail(error)
        case .cancelled:
            fail(LinkError.closed)
        default:
            break
        }
    }

    private func finishHandshake(_ reply: Data?) {
        guard !closed, let handshake = handshake.take() else { return }
        // The Mac closes without a reply when it doesn't know the pairing.
        guard let reply, let session = try? handshake.finish(reply: reply) else {
            return fail(LinkError.handshakeFailed)
        }
        self.session = session
        readFrames()
    }

    private func readFrames() {
        connection.receiveMessage(maxLength: maxFrameLength) { [weak self] frame in
            guard let self, let session = self.session else { return }
            guard let frame else { return self.fail(LinkError.closed) }
            guard let message = try? session.open(frame) else { return self.fail(LinkError.frameFailed) }
            self.lastHeard = .now()
            self.stream.yield(message)
            if let connecting = self.connecting {
                // The Mac's first frame opened: answer at once, then the connection is live.
                do {
                    let frame = try session.seal(self.firstMessage)
                    self.connection.send(
                        content: frame,
                        completion: .contentProcessed { [weak self] error in
                            if error != nil { self?.fail(LinkError.closed) }
                        })
                } catch {
                    return self.fail(error)
                }
                self.connecting = nil
                connecting.resume()
            }
            self.readFrames()
        }
    }

    private func watchConnect() {
        queue.asyncAfter(deadline: .now() + Self.connectTimeout) { [weak self] in
            if self?.connecting != nil { self?.fail(LinkError.timeout) }
        }
    }

    private func watchSilence() {
        queue.asyncAfter(deadline: lastHeard + idleTimeout) { [weak self] in
            guard let self, !self.closed else { return }
            if self.lastHeard + self.idleTimeout <= .now() { self.fail(LinkError.timeout) } else { self.watchSilence() }
        }
    }

    /// Closes for good; `error` nil means the caller closed it.
    private func fail(_ error: Error?) {
        guard !closed else { return }
        closed = true
        session = nil
        handshake = nil
        connection.cancel()
        connecting?.resume(throwing: error ?? LinkError.closed)
        connecting = nil
        stream.finish(throwing: error)
    }
}

/// Finds AgentPulse Macs on the local network (Bonjour `_agentpulse._tcp`, TXT `v=1`).
/// `onChange` runs on the main queue with every matching service currently visible.
public final class LinkBrowser: @unchecked Sendable {
    private let browser = NWBrowser(for: .bonjourWithTXTRecord(type: serviceType, domain: nil), using: .tcp)

    public init(onChange: @escaping @Sendable ([NWEndpoint]) -> Void) {
        browser.browseResultsChangedHandler = { results, _ in
            onChange(
                results.compactMap { result in
                    guard case .bonjour(let txt) = result.metadata, txt["v"] == "1" else { return nil }
                    return result.endpoint
                })
        }
    }

    public func start() { browser.start(queue: .main) }
    public func stop() { browser.cancel() }
}
