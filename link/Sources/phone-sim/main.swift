// The phone for scripts/e2e-phones.sh: reads the pairing link (`LINK <link>`) from stdin,
// says hello through the relay, prints `CODE <code>`, waits for the Mac's state, sends
// `seen` through the relay and as the first frame of a direct connection found over
// Bonjour, reads the Mac's first direct frame, then sends `unpair`. Exits 0, or 1 with
// the reason on stderr.

import AgentPulseLink
import CryptoKit
import Foundation
import Network

struct Failure: Error, CustomStringConvertible {
    let description: String
    init(_ description: String) { self.description = description }
}

func nowMs() -> Int64 { Int64(Date().timeIntervalSince1970 * 1000) }

let session = URLSession(configuration: .ephemeral)

/// One relay request; waits out a 429.
func request(_ method: String, _ url: URL, token: Data?, body: [String: Any]? = nil) async throws -> (Int, Data) {
    for _ in 0..<10 {
        var r = URLRequest(url: url, timeoutInterval: 20)
        r.httpMethod = method
        if let token { r.setValue("Bearer \(token.base64URLEncodedString())", forHTTPHeaderField: "Authorization") }
        if let body {
            r.httpBody = try JSONSerialization.data(withJSONObject: body)
            r.setValue("application/json", forHTTPHeaderField: "Content-Type")
        }
        let (data, response) = try await session.data(for: r)
        guard let http = response as? HTTPURLResponse else { throw Failure("\(method) \(url.path): not HTTP") }
        guard http.statusCode == 429 else { return (http.statusCode, data) }
        try await Task.sleep(for: .seconds(Double(http.value(forHTTPHeaderField: "Retry-After") ?? "1") ?? 1))
    }
    throw Failure("\(method) \(url.path): still 429")
}

func object(_ data: Data) throws -> [String: Any] {
    guard let o = try JSONSerialization.jsonObject(with: data) as? [String: Any] else { throw Failure("not a JSON object") }
    return o
}

final class Found: @unchecked Sendable {
    private let lock = NSLock()
    private var endpoints: [NWEndpoint] = []
    func set(_ e: [NWEndpoint]) { lock.withLock { endpoints = e } }
    var all: [NWEndpoint] { lock.withLock { endpoints } }
}

func run() async throws {
    guard let line = readLine() else { throw Failure("no link on stdin") }
    let link = try PairingLink(line.hasPrefix("LINK ") ? String(line.dropFirst(5)) : line)
    let phoneKey = Curve25519.KeyAgreement.PrivateKey()
    let token = Data((0..<32).map { _ in UInt8.random(in: 0...255) })
    let pairing = link.relay.appending(path: "v1/pairings/\(link.id.base64URLEncodedString())")
    let deadline = Date().addingTimeInterval(80)

    // Hello.
    let hello = try Hello(secret: link.secret, phoneKey: phoneKey.publicKey, phoneName: "E2E iPhone", sentAtMs: nowMs())
    let (helloStatus, _) = try await request(
        "POST", pairing.appending(path: "hello"), token: nil,
        body: [
            "secret": link.secret.base64URLEncodedString(),
            "phoneTokenHash": Data(SHA256.hash(data: token)).base64URLEncodedString(),
            "message": try RelayMessage.sealHello(hello, to: link.macKey, pairingId: link.id),
        ])
    guard helloStatus == 204 else { throw Failure("hello: HTTP \(helloStatus)") }
    print("CODE \(verificationCode(pairingId: link.id, macKey: link.macKey, phoneKey: phoneKey.publicKey))")
    fflush(stdout)

    // The Mac's state through the relay, once the user confirmed.
    var down = SeqGuard()
    var relaySeq: UInt64 = 0
    while true {
        guard Date() < deadline else { throw Failure("no state from the relay") }
        let (status, data) = try await request("GET", pairing.appending(path: "down"), token: token)
        if status == 200 {
            let sealed = try object(data)["message"] as? String ?? ""
            let plaintext = try RelayMessage.open(sealed, with: phoneKey, from: link.macKey, direction: .down, pairingId: link.id)
            let m = try object(plaintext)
            guard let seq = (m["seq"] as? NSNumber)?.uint64Value, down.accept(seq), m["type"] as? String == "state" else {
                throw Failure("relay state: bad seq or type")
            }
            relaySeq = seq
            print("RELAY state seq \(seq)")
            break
        }
        guard status == 204 else { throw Failure("GET down: HTTP \(status)") }
        try await Task.sleep(for: .seconds(1))
    }

    var up: UInt64 = 0
    func message(_ type: String) throws -> Data {
        up += 1
        return try JSONSerialization.data(withJSONObject: ["seq": up, "type": type, "sentAtMs": nowMs()])
    }
    func post(_ type: String) async throws {
        let sealed = try RelayMessage.seal(message(type), to: link.macKey, from: phoneKey, direction: .up, pairingId: link.id)
        let (status, _) = try await request("POST", pairing.appending(path: "up"), token: token, body: ["message": sealed])
        guard status == 204 else { throw Failure("POST up \(type): HTTP \(status)") }
    }
    try await post("seen")

    // Directly, over Bonjour.
    let found = Found()
    let browser = LinkBrowser { found.set($0) }
    browser.start()
    defer { browser.stop() }
    var tried = 0
    var lastError: Error?
    while true {
        let endpoints = found.all
        if endpoints.isEmpty {
            guard Date() < deadline else {
                throw Failure("no _agentpulse._tcp service found over Bonjour\(lastError.map { "; last error: \($0)" } ?? "")")
            }
            try await Task.sleep(for: .milliseconds(200))
            continue
        }
        var connected = false
        for endpoint in endpoints {
            tried += 1
            let client = try LinkClient(endpoint: endpoint, pairingId: link.id, phoneKey: phoneKey, macKey: link.macKey)
            do {
                try await client.connect(firstMessage: message("seen"))
            } catch {
                lastError = error
                continue
            }
            var frames = client.messages.makeAsyncIterator()
            guard let frame = try await frames.next() else { throw Failure("direct: closed before a frame") }
            let m = try object(frame)
            let type = m["type"] as? String ?? ""
            guard let seq = (m["seq"] as? NSNumber)?.uint64Value, seq >= relaySeq, ["state", "heartbeat"].contains(type) else {
                throw Failure("direct: frame older than the relay's or of the wrong type")
            }
            print("DIRECT \(type) seq \(seq) from \(endpoint)")
            // Stays open a moment, as the app would, so the Mac shows it connected.
            try await Task.sleep(for: .seconds(3))
            client.close()
            connected = true
            break
        }
        if connected { break }
        // None of them knew this pairing; look again until the deadline.
        guard Date() < deadline else { throw Failure("direct: \(tried) attempts failed; last error: \(lastError.map { "\($0)" } ?? "none")") }
        try await Task.sleep(for: .seconds(1))
    }

    try await post("unpair")
    print("UNPAIRED")
}

do {
    try await run()
} catch {
    FileHandle.standardError.write(Data("phone-sim: \(error)\n".utf8))
    exit(1)
}
