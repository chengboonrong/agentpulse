# AgentPulseLink

The Swift package both ends of the phone link share: the Mac bridge (`macos/Bridge/`)
and the iPhone app (`ios/`). It holds the protocol and nothing else; keys come in as
CryptoKit values or raw bytes, and storing them (Keychain) is the caller's job.

- `RelayMessage`: HPKE sealing of relay messages, the phone's hello, and `SeqGuard`
  for the `seq` rule.
- `PairingLink` and `verificationCode`: the QR code's link and the six-digit code.
- `LinkClientHandshake`, `LinkServerHandshake`, `LinkSession`: the direct connection's
  handshake and frames, as pure functions over bytes.
- `LinkListener` (Mac), `LinkClient` and `LinkBrowser` (phone): the direct connection
  over Network.framework.

The protocol is specified in [docs/mobile.md](../docs/mobile.md) ("Cryptography",
"Pairing" and "Direct connection"). CryptoKit, Foundation and Network only; macOS 14,
iOS 17.

## Test

```sh
cd link && swift test
```

The transport tests run over 127.0.0.1 with Bonjour off, so they need no network
permission. One of them waits out the 5 s handshake timeout.
