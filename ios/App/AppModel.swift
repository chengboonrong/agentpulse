import AgentPulseLink
import AgentPulsePhone
import Foundation
import Network
import Observation
import WidgetKit

/// A request to show the Pair sheet: from "Pair a Mac" (no link) or an opened link.
struct PairRequest: Identifiable {
    let id = UUID()
    var link: String?
    /// Fixtures start at a given step.
    var step: PairView.Step?
}

/// The paired Macs while the app is open (docs/mobile.md, "iPhone app"): a direct
/// connection to each Mac found on the network, else `GET down` every 10 s.
@Observable @MainActor
final class AppModel {
    static let appGroup = "group.io.github.chengboonrong.AgentPulse"

    let client: PhoneClient
    /// Fixtures: no network.
    let offline: Bool
    var pairings: [Pairing]
    var path: [String] = []
    var pairRequest: PairRequest?
    /// Pairing ids connected directly.
    var direct: Set<String> = []
    /// Pairing id to the last error, such as "This Mac removed the pairing."
    var problems: [String: String] = [:]
    /// The stored pairings couldn't be read.
    var loadError: String?

    @ObservationIgnored private var endpoints: [NWEndpoint] = []
    @ObservationIgnored private var connections: [String: DirectConnection] = [:]
    @ObservationIgnored private var connecting: Set<String> = []
    @ObservationIgnored private var browser: LinkBrowser?
    @ObservationIgnored private var refresh: Task<Void, Never>?

    init(client: PhoneClient, offline: Bool = false) {
        self.client = client
        self.offline = offline
        pairings = []
        if !offline { reload() }
    }

    struct StartError: LocalizedError {
        var errorDescription: String? { "AgentPulse has no access to its App Group storage." }
    }

    static func make() throws -> AppModel {
        #if DEBUG
        if let fixture = UserDefaults.standard.string(forKey: "AgentPulseFixture") { return Fixtures.model(fixture) }
        #endif
        guard let store = PhoneStore.appGroup(appGroup) else { throw StartError() }
        let model = AppModel(client: PhoneClient(store: store))
        #if DEBUG
        // scripts/e2e-ios.sh: the simulator asks before `simctl openurl` opens the app, which
        // would need a tap, so the link comes as a launch argument into the same path.
        if let link = UserDefaults.standard.string(forKey: "AgentPulseE2ELink").flatMap(URL.init) { model.open(link) }
        #endif
        return model
    }

    /// An opened link: pair links show Pair; the widget's agentpulse://open just opens the app.
    func open(_ url: URL) {
        if url.host() == "pair" { pairRequest = PairRequest(link: url.absoluteString) }
    }

    func pairing(_ id: String) -> Pairing? { pairings.first { $0.id == id } }

    /// The app came to the foreground: `seen` to every Mac, then direct or polling.
    func start() {
        guard !offline, refresh == nil else { return }
        reload()
        #if DEBUG
        // scripts/e2e-ios.sh: removes every Mac with no tap, and nothing else.
        if UserDefaults.standard.bool(forKey: "AgentPulseE2ERemove") {
            Task { for p in pairings { await remove(p) } }
            return
        }
        #endif
        for p in pairings { Task { await self.seen(p) } }
        refresh = Task {
            while !Task.isCancelled {
                await self.tick()
                try? await Task.sleep(for: .seconds(10))
            }
        }
        browse()
    }

    func stop() {
        refresh?.cancel()
        refresh = nil
        browser?.stop()
        browser = nil
        for c in connections.values { c.close() }
        connections = [:]
        direct = []
    }

    /// A pairing that just got its first state.
    func added(_ p: Pairing) {
        reload()
        WidgetCenter.shared.reloadAllTimelines()
        browse()
        Task {
            await seen(p)
            await connectDirect(p.id)
        }
    }

    /// Out of the model and off the network first, so nothing sends a newer `seen` after
    /// the `unpair`; deleted here even when the relay can't be reached.
    func remove(_ p: Pairing) async {
        path.removeAll()
        pairings.removeAll { $0.id == p.id }
        connections.removeValue(forKey: p.id)?.close()
        direct.remove(p.id)
        problems[p.id] = nil
        await client.remove(p)
        reload()
        WidgetCenter.shared.reloadAllTimelines()
    }

    func reload() {
        do {
            pairings = try client.store.pairings()
            loadError = nil
        } catch {
            loadError = error.localizedDescription
        }
    }

    // MARK: -

    /// Bonjour only once a Mac is paired, so the Local Network prompt never comes before.
    private func browse() {
        guard browser == nil, refresh != nil, !pairings.isEmpty else { return }
        browser = LinkBrowser { [weak self] endpoints in Task { @MainActor in self?.found(endpoints) } }
        browser?.start()
    }

    private func tick() async {
        for p in pairings where !direct.contains(p.id) {
            await connectDirect(p.id)
            if !direct.contains(p.id) { await fetch(p.id) }
        }
    }

    private func found(_ endpoints: [NWEndpoint]) {
        self.endpoints = endpoints
        Task {
            for p in pairings where !direct.contains(p.id) { await connectDirect(p.id) }
        }
    }

    private func seen(_ p: Pairing) async {
        await report(p.id) { try await self.client.post(.seen, p) }
    }

    private func fetch(_ id: String) async {
        guard let p = pairing(id) else { return }
        await report(id) { self.replace(try await self.client.fetchDown(p)) }
    }

    /// Tries each AgentPulse service found; a Mac that doesn't know the pairing closes.
    private func connectDirect(_ id: String) async {
        guard !connecting.contains(id), connections[id] == nil else { return }
        connecting.insert(id)
        defer { connecting.remove(id) }
        for endpoint in endpoints {
            guard let p = pairing(id), let c = try? await client.connect(p, to: endpoint) else { continue }
            // Stopped or removed meanwhile.
            guard refresh != nil, pairing(id) != nil else { return c.close() }
            connections[id] = c
            direct.insert(id)
            problems[id] = nil
            Task { await listen(id, c) }
            return
        }
    }

    private func listen(_ id: String, _ c: DirectConnection) async {
        do {
            for try await m in c.messages {
                if case .state(let p) = m { replace(p) }
            }
        } catch {
            // Closed: back to the relay on the next tick.
        }
        // Unless stop() or a newer connection already replaced it.
        guard connections[id]?.id == c.id else { return }
        connections[id] = nil
        direct.remove(id)
    }

    private func replace(_ p: Pairing) {
        if let i = pairings.firstIndex(where: { $0.id == p.id }) { pairings[i] = p }
    }

    private func report(_ id: String, _ body: () async throws -> Void) async {
        do {
            try await body()
            problems[id] = nil
        } catch is CancellationError {
        } catch let error as URLError where error.code == .cancelled {
        } catch {
            problems[id] = error.localizedDescription
        }
    }
}
