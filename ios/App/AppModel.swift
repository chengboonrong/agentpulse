import AgentPulseLink
import AgentPulsePhone
import Foundation
import Network
import Observation
import WidgetKit
import UIKit
import UserNotifications

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
    /// The session shown beside the Mac at regular width: `WidgetPayload.Session.key`.
    var session: String?
    var pairRequest: PairRequest?
    var settingsShown = false
    var notificationsShown = false
    var notificationStatus: UNAuthorizationStatus = .notDetermined
    var notificationError: String?
    var notificationsAllowed: Bool {
        notificationStatus == .authorized || notificationStatus == .provisional || notificationStatus == .ephemeral
    }
    var pushToken: Data?
    var notifyChoices = UserDefaults.standard.data(forKey: "notifyChoices")
        .flatMap { try? JSONDecoder().decode(NotifyChoices.self, from: $0) } ?? .defaults {
        didSet {
            guard !offline else { return }
            UserDefaults.standard.set(try? JSONEncoder().encode(notifyChoices), forKey: "notifyChoices")
            // A known token is used now; only an unknown one waits for the callback.
            syncNotifications(register: pushToken == nil)
        }
    }

    var directEnabled = UserDefaults.standard.object(forKey: "directConnection") as? Bool ?? true {
        didSet {
            if !offline { UserDefaults.standard.set(directEnabled, forKey: "directConnection") }
            if directEnabled {
                browse()
            } else {
                browser?.stop()
                browser = nil
                endpoints = []
                for c in connections.values { c.close() }
                connections = [:]
                direct = []
            }
        }
    }
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
    @ObservationIgnored private var notificationSync: Task<Void, Never>?
    /// One-Mac syncs after a direct connection; any newer sync cancels them, so a stale
    /// PUT or `notify` never lands last.
    @ObservationIgnored private var pairingSyncs: [String: Task<Void, Never>] = [:]
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
        Task {
            notificationStatus = await UNUserNotificationCenter.current().notificationSettings().authorizationStatus
            syncNotifications()
        }
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
        syncNotifications()
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
        session = nil
        pairings.removeAll { $0.id == p.id }
        connections.removeValue(forKey: p.id)?.close()
        direct.remove(p.id)
        problems[p.id] = nil
        await client.remove(p)
        reload()
        WidgetCenter.shared.reloadAllTimelines()
    }

    /// Updates only this Mac in memory: a `reload()` here could bring back a Mac whose
    /// removal is still reaching the relay.
    func rename(_ p: Pairing, to name: String) {
        do {
            if let renamed = try client.store.rename(p.id, to: name) { replace(renamed) }
            WidgetCenter.shared.reloadAllTimelines()
        } catch {
            problems[p.id] = error.localizedDescription
        }
    }

    func reload() {
        do {
            pairings = try client.store.pairings()
            loadError = nil
        } catch {
            loadError = error.localizedDescription
        }
    }

    func setChoice(_ key: WritableKeyPath<NotifyChoices, Bool>, _ enabled: Bool) {
        notifyChoices[keyPath: key] = enabled
    }

    func requestNotifications() async {
        guard !offline else { return }
        notificationError = nil
        do {
            _ = try await UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound])
        } catch {
            notificationError = error.localizedDescription
        }
        notificationStatus = await UNUserNotificationCenter.current().notificationSettings().authorizationStatus
        syncNotifications()
    }

    func receivedPushToken(_ token: Data) {
        pushToken = token
        syncNotifications(register: false)
    }

    func syncNotifications(register: Bool = true, pairingID: String? = nil) {
        guard !offline else { return }
        let effective = notificationsAllowed ? notifyChoices : .off
        if let pairingID {
            pairingSyncs[pairingID]?.cancel()
        } else {
            notificationSync?.cancel()
            for t in pairingSyncs.values { t.cancel() }
            pairingSyncs = [:]
        }
        if effective.any, register {
            UIApplication.shared.registerForRemoteNotifications()
            return
        }
        let token = pushToken
        guard !effective.any || token != nil else { return }
        let sync = Task {
            for p in pairings where pairingID == nil || p.id == pairingID {
                guard !Task.isCancelled else { return }
                guard pairing(p.id) != nil else { continue }
                await report(p.id) {
                    if effective.any, let token {
                        #if DEBUG
                        let environment = PhoneClient.PushEnvironment.development
                        #else
                        let environment = PhoneClient.PushEnvironment.production
                        #endif
                        try await self.client.putPushToken(token, environment: environment, p)
                    }
                    try Task.checkCancellation()
                    guard self.pairing(p.id) != nil else { return }
                    try await self.client.notify(effective, p)
                }
            }
        }
        if let pairingID { pairingSyncs[pairingID] = sync } else { notificationSync = sync }
    }

    // MARK: -

    /// Bonjour only once a Mac is paired, so the Local Network prompt never comes before.
    private func browse() {
        guard directEnabled, !offline, browser == nil, refresh != nil, !pairings.isEmpty else { return }
        browser = LinkBrowser { [weak self] endpoints in Task { @MainActor in self?.found(endpoints) } }
        browser?.start()
    }

    private func tick() async {
        for p in pairings where !direct.contains(p.id) {
            if directEnabled { await connectDirect(p.id) }
            if !direct.contains(p.id) { await fetch(p.id) }
        }
    }

    private func found(_ endpoints: [NWEndpoint]) {
        guard directEnabled, refresh != nil else { return }
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
        guard directEnabled, !connecting.contains(id), connections[id] == nil else { return }
        connecting.insert(id)
        defer { connecting.remove(id) }
        for endpoint in endpoints {
            guard directEnabled, refresh != nil else { return }
            guard let p = pairing(id), let c = try? await client.connect(p, to: endpoint) else { continue }
            // Stopped or removed meanwhile.
            guard directEnabled, refresh != nil, pairing(id) != nil else { return c.close() }
            connections[id] = c
            direct.insert(id)
            problems[id] = nil
            // The relay notify must have a newer seq than the direct connection's seen.
            syncNotifications(register: false, pairingID: id)
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
