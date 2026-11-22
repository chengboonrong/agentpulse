import CryptoKit
import Foundation
import Testing

@testable import AgentPulsePhone

@Test func renamePairing() throws {
    let dir = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
    defer { try? FileManager.default.removeItem(at: dir) }
    let secrets = MemorySecrets()
    let store = PhoneStore(container: dir, secrets: secrets)
    let key = Curve25519.KeyAgreement.PrivateKey()
    var pairing = Pairing(id: "studio", relay: URL(string: "https://relay.example.com")!,
                          macName: "Studio", macKey: key.publicKey.rawRepresentation)
    pairing.lastState = Data("state".utf8)
    pairing.downSeq = 7
    try store.add(pairing, key: key, token: Data(repeating: 1, count: 32))
    let secret = try secrets.read(pairing.id)

    var expected = pairing
    expected.macName = "Office Mac"
    #expect(try store.rename(pairing.id, to: " \n Office Mac \t") == expected)
    #expect(try store.pairing(pairing.id) == expected)
    let reloaded = PhoneStore(container: dir, secrets: secrets)
    #expect(try reloaded.pairing(pairing.id) == expected)
    #expect(try secrets.read(pairing.id) == secret)

    let file = dir.appending(path: "pairings.json")
    let saved = try Data(contentsOf: file)
    for blank in ["", " \n\t "] {
        #expect(try store.rename(pairing.id, to: blank) == nil)
        #expect(try store.pairing(pairing.id) == expected)
        #expect(try Data(contentsOf: file) == saved)
    }
    #expect(try store.rename("missing", to: "Other") == nil)
    #expect(try store.pairings() == [expected])
    #expect(try Data(contentsOf: file) == saved)
}
