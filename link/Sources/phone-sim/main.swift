// The phone for scripts/e2e-phones.sh, on the app's own phone core (AgentPulsePhone): reads
// the pairing link (`LINK <link>`) from stdin, says hello through the relay, prints
// `CODE <code>`, waits for the Mac's state, sends `seen` through the relay and as the first
// frame of a direct connection found over Bonjour, reads the Mac's first direct frame, then
// sends `unpair`. Secrets stay in memory and the store in a temp directory. Exits 0, or 1
// with the reason on stderr.

import AgentPulseLink
import AgentPulsePhone
import Foundation
import Network

struct Failure: Error, CustomStringConvertible {
    let description: String
    init(_ description: String) { self.description = description }
}

final class Found: @unchecked Sendable {
    private let lock = NSLock()
    private var endpoints: [NWEndpoint] = []
    func set(_ e: [NWEndpoint]) { lock.withLock { endpoints = e } }
    var all: [NWEndpoint] { lock.withLock { endpoints } }
}

func run(_ dir: URL) async throws {
    guard let line = readLine() else { throw Failure("no link on stdin") }
    let phone = PhoneClient(store: PhoneStore(container: dir, secrets: MemorySecrets()))
    let deadline = Date().addingTimeInterval(80)

    // Hello.
    let (code, paired) = try await phone.pair(
        link: line.hasPrefix("LINK ") ? String(line.dropFirst(5)) : line, phoneName: "E2E iPhone")
    print("CODE \(code)")
    fflush(stdout)

    // The Mac's state through the relay, once the user confirmed.
    var pairing = paired
    while true {
        guard Date() < deadline else { throw Failure("no state from the relay") }
        pairing = try await phone.fetchDown(pairing)
        if let seq = pairing.downSeq {
            print("RELAY state seq \(seq)")
            break
        }
        try await Task.sleep(for: .seconds(1))
    }
    try await phone.post(.seen, pairing)

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
            let direct: DirectConnection
            do {
                direct = try await phone.connect(pairing, to: endpoint)
            } catch {
                lastError = error
                continue
            }
            var messages = direct.messages.makeAsyncIterator()
            switch try await messages.next() {
            case .state(let p)?: print("DIRECT state seq \(p.downSeq ?? 0) from \(endpoint)")
            case .alive(let type, let seq)?: print("DIRECT \(type) seq \(seq) from \(endpoint)")
            case .dropped?: throw Failure("direct: frame older than the relay's or of the wrong type")
            case nil: throw Failure("direct: closed before a frame")
            }
            // Stays open a moment, as the app would, so the Mac shows it connected.
            try await Task.sleep(for: .seconds(3))
            direct.close()
            connected = true
            break
        }
        if connected { break }
        // None of them knew this pairing; look again until the deadline.
        guard Date() < deadline else { throw Failure("direct: \(tried) attempts failed; last error: \(lastError.map { "\($0)" } ?? "none")") }
        try await Task.sleep(for: .seconds(1))
    }

    await phone.remove(pairing)
    guard try phone.store.pairings().isEmpty else { throw Failure("the pairing is still stored after remove") }
    print("UNPAIRED")
}

let dir = FileManager.default.temporaryDirectory.appending(path: "phone-sim-\(UUID().uuidString)")
do {
    try await run(dir)
    try? FileManager.default.removeItem(at: dir)
} catch {
    try? FileManager.default.removeItem(at: dir)
    FileHandle.standardError.write(Data("phone-sim: \(error)\n".utf8))
    exit(1)
}
