import CryptoKit
import Foundation
import Security

/// One paired Mac, as the phone keeps it. Its secrets (the phone's private key and relay
/// token) are not here but in the store's `SecretStore`.
public struct Pairing: Codable, Hashable, Identifiable, Sendable {
    /// The pairing id, base64url.
    public let id: String
    public let relay: URL
    public let macName: String
    /// The Mac's raw 32-byte public key.
    public let macKey: Data
    /// The latest accepted state's `payload` (widget.json version 1), as JSON.
    public var lastState: Data?
    /// When that state was accepted.
    public var lastStateAt: Date?
    /// The `down` `SeqGuard` record: the last accepted state's seq.
    public internal(set) var downSeq: UInt64?
    /// The last `up` seq sent; the hello is 0.
    public internal(set) var upSeq: UInt64 = 0

    public init(id: String, relay: URL, macName: String, macKey: Data) {
        self.id = id
        self.relay = relay
        self.macName = macName
        self.macKey = macKey
    }

    /// Fields with a default may be missing, so a record written before they existed loads.
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        relay = try c.decode(URL.self, forKey: .relay)
        macName = try c.decode(String.self, forKey: .macName)
        macKey = try c.decode(Data.self, forKey: .macKey)
        lastState = try c.decodeIfPresent(Data.self, forKey: .lastState)
        lastStateAt = try c.decodeIfPresent(Date.self, forKey: .lastStateAt)
        downSeq = try c.decodeIfPresent(UInt64.self, forKey: .downSeq)
        upSeq = try c.decodeIfPresent(UInt64.self, forKey: .upSeq) ?? 0
    }
}

/// Where the phone's secrets live: the Keychain in the app, memory in tests and phone-sim.
public protocol SecretStore: Sendable {
    func read(_ account: String) throws -> Data?
    func write(_ data: Data, _ account: String) throws
    func delete(_ account: String) throws
}

public struct KeychainError: Error, CustomStringConvertible {
    public let status: OSStatus
    public var description: String { "Keychain error \(status)" }
}

/// Generic passwords in an access group the app and its widgets share.
public struct KeychainSecrets: SecretStore {
    static let service = "io.github.chengboonrong.AgentPulse.phone"
    let accessGroup: String?

    public init(accessGroup: String?) { self.accessGroup = accessGroup }

    private func query(_ account: String) -> [String: Any] {
        var q: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: Self.service,
            kSecAttrAccount as String: account,
            kSecUseDataProtectionKeychain as String: true,
        ]
        if let accessGroup { q[kSecAttrAccessGroup as String] = accessGroup }
        return q
    }

    public func read(_ account: String) throws -> Data? {
        var q = query(account)
        q[kSecReturnData as String] = true
        var result: CFTypeRef?
        let status = SecItemCopyMatching(q as CFDictionary, &result)
        if status == errSecItemNotFound { return nil }
        guard status == errSecSuccess else { throw KeychainError(status: status) }
        return result as? Data
    }

    public func write(_ data: Data, _ account: String) throws {
        try delete(account)
        var q = query(account)
        q[kSecValueData as String] = data
        // The widgets read it in the background.
        q[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        let status = SecItemAdd(q as CFDictionary, nil)
        guard status == errSecSuccess else { throw KeychainError(status: status) }
    }

    public func delete(_ account: String) throws {
        let status = SecItemDelete(query(account) as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else { throw KeychainError(status: status) }
    }
}

/// Secrets in memory only.
public final class MemorySecrets: SecretStore, @unchecked Sendable {
    private let lock = NSLock()
    private var items: [String: Data] = [:]

    public init() {}

    public func read(_ account: String) throws -> Data? { lock.withLock { items[account] } }
    public func write(_ data: Data, _ account: String) throws { lock.withLock { items[account] = data } }
    public func delete(_ account: String) throws { _ = lock.withLock { items.removeValue(forKey: account) } }
}

/// The phone's pairings: `pairings.json` in the App Group container (shared by the app and
/// its widgets, so none of them accepts a state older than one already shown), the
/// secrets in a `SecretStore`.
public struct PhoneStore: Sendable {
    let container: URL
    let secrets: any SecretStore

    public init(container: URL, secrets: any SecretStore) {
        self.container = container
        self.secrets = secrets
    }

    /// The iPhone app's store, shared with its widgets: the App Group's container, and the
    /// Keychain with the App Group as the access group. Nil without the entitlement.
    public static func appGroup(_ group: String) -> PhoneStore? {
        FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: group).map {
            PhoneStore(container: $0, secrets: KeychainSecrets(accessGroup: group))
        }
    }

    private var file: URL { container.appending(path: "pairings.json") }

    /// Throws when the file can't be read or decoded, so that shows instead of no Macs.
    public func pairings() throws -> [Pairing] {
        try locked { try load() }
    }

    public func pairing(_ id: String) throws -> Pairing? {
        try pairings().first { $0.id == id }
    }

    /// The phone's private key and relay token for a pairing.
    func keys(_ id: String) throws -> (key: Curve25519.KeyAgreement.PrivateKey, token: Data) {
        guard let d = try secrets.read(id), d.count == 64 else { throw PhoneError.removed }
        return (try .init(rawRepresentation: d.prefix(32)), Data(d.suffix(32)))
    }

    func add(_ pairing: Pairing, key: Curve25519.KeyAgreement.PrivateKey, token: Data) throws {
        try secrets.write(key.rawRepresentation + token, pairing.id)
        try locked {
            var all = try load()
            all.removeAll { $0.id == pairing.id }
            all.append(pairing)
            try save(all)
        }
    }

    /// Read, change and write one pairing under the lock (only if it changed); nil if it is gone.
    func update<T>(_ id: String, _ change: (inout Pairing) throws -> T) throws -> (Pairing, T)? {
        try locked {
            var all = try load()
            guard let i = all.firstIndex(where: { $0.id == id }) else { return nil }
            let before = all[i]
            let result = try change(&all[i])
            if all[i] != before { try save(all) }
            return (all[i], result)
        }
    }

    /// Deletes a pairing and its secrets.
    public func delete(_ id: String) throws {
        try locked { try save(load().filter { $0.id != id }) }
        try secrets.delete(id)
    }

    private func load() throws -> [Pairing] {
        guard let data = try? Data(contentsOf: file) else { return [] }
        return try JSONDecoder().decode([Pairing].self, from: data)
    }

    private func save(_ pairings: [Pairing]) throws {
        try JSONEncoder().encode(pairings).write(to: file, options: .atomic)
    }

    /// An exclusive `flock` across the app and widget processes.
    private func locked<T>(_ body: () throws -> T) throws -> T {
        try FileManager.default.createDirectory(at: container, withIntermediateDirectories: true)
        let fd = open(container.appending(path: "pairings.lock").path, O_CREAT | O_RDWR, 0o600)
        guard fd >= 0 else { throw CocoaError(.fileWriteUnknown) }
        defer { close(fd) }
        while flock(fd, LOCK_EX) != 0 {
            guard errno == EINTR else { throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO) }
        }
        return try body()
    }
}
