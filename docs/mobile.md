# Mobile app and relay

Status: design, approved direction (owner, 2026-11-13): relay and push from the start.
Nothing here is deployed. The relay runs locally in tests until the owner has a paid
Apple Developer Program membership and deploys it.

## Goal

An iPhone app, its home-screen widgets and a Live Activity (Lock Screen and Dynamic
Island) show what the Mac app shows in its own widget: live sessions, who needs you,
today's tokens and estimated cost, the output rate and the cat. Data comes from one or
more Macs through a relay that sees only ciphertext.

## Rules (owner)

- Off by default. Nothing leaves the Mac until the user pairs a phone in Settings.
- Metadata only: the widget payload (`widget.json` version 1, see
  `crates/agentpulse-app/src/widget_export.rs`). It already holds live sessions only,
  at most five, with the project's folder name and no paths, prompts or code.
- End-to-end encrypted. The relay and the push gateway handle ciphertext, random
  pairing ids, token hashes and Apple push tokens. The one plain secret it sees is the
  one-time pairing secret in the phone's hello, which it needs to check and then forgets;
  without the Mac's public key, which only the QR code carries, it cannot use it.
- Pairing by QR code. Every pairing has its own keys, is listed by device name and last
  seen time on both sides, and can be removed alone.
- Several Macs per phone, several phones per Mac.
- Self-hostable: the relay is one small open-source Worker; its address is a setting.

## Parts

| Part | Where | What |
|---|---|---|
| Link | `link/` (Swift package `AgentPulseLink`, CryptoKit and Network.framework), used by the Mac bridge and the iPhone app | Relay message sealing, the direct connection's handshake, framing, listener and client |
| Crypto, QR | `macos/Bridge/` (Swift, CoreImage), loaded by the Rust app as today | Keys in the Keychain, calls into `link/`, QR image |
| Mac client | `crates/agentpulse-app/src/phones.rs`, Settings "Phones" card | Pairing, publishing, pushes, the paired list |
| Relay + push gateway | `relay/` (TypeScript, Cloudflare Worker + Durable Objects) | Mailboxes and APNs sends |
| iPhone app | `ios/` (SwiftUI, XcodeGen like `macos/`) | Scanning, the Macs, sessions, widgets, Live Activity |

The Mac's crypto lives in Swift so both ends run the same CryptoKit code and the Rust
workspace gains no crypto crates. Like the widget bridge, it is only present in the
bundle (`scripts/bundle.sh`); a plain `cargo run` shows "Phones need the app bundle".

## Cryptography

CryptoKit HPKE, ciphersuite `Curve25519_SHA256_ChachaPoly` (RFC 9180: DHKEM X25519
HKDF-SHA256, HKDF-SHA256, ChaCha20-Poly1305). macOS 14 and iOS 17 have it.

- Keys per pairing on both sides: the Mac makes a key pair when it shows a QR code, the
  phone makes one when it scans. Private keys stay in each device's Keychain (the
  phone's in the App Group's access group, so its widgets and Live Activity can decrypt).
  Removing a pairing deletes its keys.
- Every message is one HPKE single-shot seal: `enc (32 bytes) || ciphertext`, base64url.
- `info` = `"agentpulse/1 " + direction + " " + pairing id`, direction `down` (Mac to
  phone) or `up` (phone to Mac), so a message can't be replayed in the other direction
  or another pairing.
- After pairing, both directions use auth mode (sealed with the sender's private key,
  opened with its public key), so the phone knows its data came from that Mac.
- The phone's hello is the only base-mode message; it proves the QR was seen by
  carrying the pairing secret inside the ciphertext.
- Relay messages have no forward secrecy: whoever later steals a device's private key for
  a pairing can open that pairing's recorded relay messages. That is accepted for this
  metadata; removing a pairing deletes its keys. Direct connections (below) do have it.
- Encodings: the pairing id is its base64url text (22 characters) wherever it is part of a
  string (`info`, the verification code, URLs) and its 16 raw bytes on the direct
  connection's wire and in `h`. Keys and secrets are unpadded base64url in JSON and links.
  HPKE uses no additional data.
- The phone keeps one `seq` record per pairing in its App Group, shared by the app, the
  widgets and the Live Activity, so none of them accepts a state older than one already
  shown. The hello is `up` seq 0; the phone's later messages continue from 1.
- Every plaintext carries `seq`, a counter per pairing and direction; a receiver drops
  anything not newer than the last it accepted.

## Messages

JSON objects with `seq` (a counter per pairing and direction, from 0) and `type`:

| Direction | `type` | Other fields |
|---|---|---|
| down | `state` | `payload`: the widget payload |
| down | `heartbeat` | `sentAtMs`; direct connections only, with the `seq` of the latest `state`, so it never advances a receiver's record |
| up | `seen` | `sentAtMs`; sent when the app opens, every 6 hours from background refresh, and as the first frame of a direct connection |
| up | `unpair` | none; the Mac removes the pairing |

The phone's pairing hello is the one message without `type`; it is described below.

## Pairing

1. The Mac (Settings, Phones, "Pair a Phone") makes a pairing:
   - id: 16 random bytes, base64url;
   - its key pair;
   - a pairing secret, 32 random bytes;
   - its relay token, 32 random bytes.
   It registers the pairing at the relay with the SHA-256 of the secret and of its token,
   then shows a QR code of
   `agentpulse://pair?v=1&r=<relay URL>&id=<id>&k=<Mac public key>&s=<secret>&n=<Mac name>`.
   The code expires after 10 minutes.
2. The phone scans it (or opens the link; in the simulator, pastes it) and shows the Mac's
   name and the relay's host, asking before it goes on, since a link can come from
   anywhere. Names are at most 64 characters after removing control and bidirectional
   formatting characters.
3. On yes, the phone makes its key pair and relay token, and posts a hello sealed to the
   Mac's key: `{seq, secret, phoneKey, phoneName, sentAtMs}`, with the SHA-256 of its
   token. The relay accepts one hello per pairing, checks the secret's hash, stores the
   phone's token hash and forgets the secret's.
4. The Mac opens the hello and checks the secret. Both screens then show the same
   six-digit code: the first 4 bytes, big-endian, of SHA-256 over the UTF-8 text
   `"agentpulse/1 code " + id` followed by the Mac's and the phone's raw 32-byte public
   keys, modulo 1,000,000, with leading zeros. The Mac asks "Pair with <phone name>?
   Check that your phone shows 123 456." Only after the user confirms does it start publishing.
   Declining deletes the pairing. The code stops someone who glimpsed the QR code from
   pairing in the phone's place: their hello carries their own key, so their code
   differs, and the phone shows "Someone else answered this code" when its own hello
   gets 409.

## Relay API

Base URL from Settings. Every request carries `Authorization: Bearer <token>`; the relay
stores only SHA-256 hashes of tokens. One Durable Object per pairing id.

| Request | Token | Does |
|---|---|---|
| `POST /v1/pairings` `{id, macTokenHash, secretHash}` | none | Creates a pairing that expires unless a hello comes within 10 minutes |
| `POST /v1/pairings/{id}/hello` `{secret, phoneTokenHash, message}` | none | One hello per pairing (secret checked) |
| `PUT /v1/pairings/{id}/down` `{seq, message}` | Mac | Replaces the latest state (one slot, at most 16 KB) |
| `GET /v1/pairings/{id}/down` | phone | Latest state; `If-None-Match: <seq>` returns 304 |
| `POST /v1/pairings/{id}/up` `{message}` | phone | Queues a message for the Mac (at most 32 queued) |
| `GET /v1/pairings/{id}/up` | Mac | Returns and deletes queued messages, the hello first |
| `PUT /v1/pairings/{id}/push-tokens` `{activity?, start?, environment}` | phone | Apple push tokens for the Live Activity |
| `POST /v1/pairings/{id}/push` `{event: "update" \| "start" \| "end", message, urgent}` | Mac | Sends a Live Activity push |
| `DELETE /v1/pairings/{id}` | either | Removes the pairing and everything stored for it |

Limits:
- 1 write and 4 reads a second per pairing, side and route, so a Mac's `PUT down` and
  `push` in the same second both pass, and the phone's app and widgets can read together;
- 60 regular and 12 urgent pushes an hour per pairing, counted apart;
- pairings idle for 30 days are removed; "last active" is stored at most hourly, so reads
  cost no storage writes.

For a public relay, the deployer can set `RELAY_KEY` (creating pairings then needs it)
and bind a rate limiter on create and hello, keyed on the client's IPv4 address or IPv6 /64; see `relay/README.md`. Without them
anyone can create pairings, which expire after 10 minutes unanswered.
`GET /v1/health` returns the relay's version.

## Push gateway

Part of the same Worker, holding the APNs key as Worker secrets (`APNS_KEY_P8`,
`APNS_KEY_ID`, `APNS_TEAM_ID`, `APNS_TOPIC`). For a push it signs an ES256 JWT with
WebCrypto and posts to `api.push.apple.com` (or the sandbox for development tokens),
type `liveactivity`, with content state `{"m": "<message>"}`. The Live Activity's views
decrypt `m` when they draw, with the key the App Group shares. Priority 10 only when
`urgent` (an agent starts waiting for you), else 5. A relay without APNs secrets still
relays; its `push` answers 501. A `start` push carries a fixed alert, because Apple
requires one when a push starts a Live Activity (ActivityKit, "Starting and updating
Live Activities with ActivityKit push notifications").

## Direct connection (same network)

When the phone and a paired Mac are on the same network, the app talks to the Mac
directly: faster than the relay, and still working when the relay is down. Away from it,
the relay. Idea borrowed from Orca's direct LAN path; Orca's code is not used.

- Discovery: the Mac advertises Bonjour service `_agentpulse._tcp` with the instance
  name "AgentPulse" and TXT `v=1` only, so the network learns nothing about pairings.
  The phone tries each AgentPulse service it finds; a Mac that doesn't know the
  pairing closes the connection without a reply.
- The Mac listens only while at least one pairing is confirmed and "Direct connection"
  (Settings, Phones, on by default) is on. At most 8 connections across all pairings, counting
  ones still in the handshake, which gets 5 s from accept to the Mac's reply. A device on
  the network can hold those slots; phones then fall back to the relay, which is
  accepted.
- Permissions: `NSLocalNetworkUsageDescription` and `NSBonjourServices`
  (`_agentpulse._tcp`) in both apps' `Info.plist`.

Handshake (TCP, every message length-prefixed: 4-byte big-endian length, then bytes):

1. Phone to Mac: `"APL1"`, a fresh ephemeral X25519 public key `ep` (32 bytes), and a
   16-byte tag: the first 16 bytes of HMAC-SHA256(key: `k`, message: `ep`), where
   `k = HKDF-SHA256(DH(sp, sm), info: "agentpulse/1 link id " + id, 32 bytes)`. The
   pairing id itself never crosses the network, so a device that advertises
   `_agentpulse._tcp` learns nothing that identifies or tracks the phone.
2. The Mac computes the tag for each confirmed pairing and uses the one that matches;
   with none, it closes without a reply. It sends its fresh ephemeral public key `em`
   (32 bytes).
3. Both compute, with the pairing's static keys `sm` (Mac) and `sp` (phone):
   - `ikm = DH(ep, em) || DH(ep, sm) || DH(sp, em) || DH(sp, sm)` (Noise KK's
     ee, es, se, ss);
   - `h = SHA-256("agentpulse/1 link" || id || sm || sp || ep || em)`;
   - `down = HKDF-SHA256(ikm, salt: h, info: "down", 32 bytes)`, and `up` likewise.
   One key per direction, so neither side's frames can be reflected back to it.
4. Frames: 4-byte big-endian length (at most 65,536), then ChaCha20-Poly1305 with that
   direction's key, nonce = 4 zero bytes || an 8-byte big-endian counter starting at 0,
   and the length prefix as associated data. Each side counts its own frames; any frame
   that fails to open, or arrives out of order, closes the connection.
5. The Mac's first frame is the latest state; the phone's first frame is a `seen`
   message (see Messages). A peer without the pairing's private key cannot derive the
   keys, so neither side counts the connection as live until it has opened the other's
   first frame: the phone within 5 s of connecting, the Mac within the same 5 s
   handshake window, after which the slot is closed.

Forward secrecy: the ephemeral keys and both frame keys live only for the connection,
so a private key stolen later does not open recorded direct traffic.

Direct frames carry the message JSON itself: the frame keys already encrypt it, so it
is not HPKE-sealed a second time. Frames are opaque bytes to the link layer. The Mac sends the
state when it changes and a heartbeat every 30 s; the phone closes a connection that is
silent for 90 s and goes back to the relay. The Mac keeps publishing to the relay while
a phone is connected directly, because the phone may leave the network and Live Activity
pushes only go through the relay. Widgets use the relay only.

## Mac client

- Settings, a "Phones" card: relay address (empty until set), "Pair a Phone" (QR sheet
  with a countdown), "Direct connection" (on by default), the paired phones with last
  seen time and whether they are connected directly, and Remove per phone.
- The direct listener (see above), sending the same state messages to connected phones.
- Publishing, per confirmed pairing: the widget payload, sealed, `PUT down` when it
  changed (ignoring `generatedAtMs`) and at most every 10 s. Every 5 minutes it re-sends
  the state with a fresh `generatedAtMs` instead of a heartbeat, because the relay keeps
  one slot and a heartbeat there would replace the state. Reads `up` after every publish
  and every 30 s, and every 2 s while a pairing's QR code is shown.
- The direct listener starts once there is a state to send, so a phone's first frame is
  always a state.
- A relay that sets `RELAY_KEY`: Settings takes the key, kept in the Keychain and sent
  only when creating a pairing.
- Live Activity pushes, when the phone has sent tokens:
  - `start` when the first agent becomes active (iOS 17.2 push-to-start);
  - `update` when the glance changes, at most once a minute unless urgent;
  - `end` when no agent is live for 10 minutes.
  The glance is small, to fit APNs' 4 KB: counts, the project that needs you, today's
  cost, the output rate and the cat sprite.
- Last seen: when the phone's last `up` message arrived (the phone sends one when it
  opens and every 6 hours from background refresh).
- Network use stays in the README and telemetry spec, beside the update check.

## iPhone app (iOS 17+)

- Macs list (each: name, last update, agents), a Mac's view (counts, sessions, today,
  cat), Pair (camera scanner with VisionKit; paste field in the simulator), Remove.
- While open: a direct connection to each paired Mac on the same network, else fetches
  `down` every 10 s; widgets (small, medium) fetch in their timeline
  provider and show "as of".
- The Live Activity: started from the app, or by push when an agent starts; ended by
  push or after 8 hours (the system's limit).
- Reuses `macos/Shared/Payload.swift` and the cat drawing from `WidgetViews.swift`.

## Build order

1. This document, then the relay with its tests (local, `wrangler` and Vitest pinned).
   Then `link/`: relay sealing, the direct handshake, framing, listener and client,
   tested with `swift test` over loopback.
2. Crypto bridge and the Mac client against a locally run relay. Done:
   `scripts/e2e-phones.sh` pairs, publishes and unpairs through it with a simulated phone.
3. iPhone app and widgets, in the simulator against the local relay.
4. Live Activity and push. Needs the paid program and a deployed relay to test for
   real; until then only the request the relay would send is checked.

## Not decided

- Who hosts the default relay. Until the owner deploys one, the address is empty and
  pairing asks for it.
