# Phone: notice when a paired Mac is out of date

Date: 2026-11-22. Issue #66. The owner chose "#66 then #64" and said to make the calls.

## Problem

The phone decodes only payload format version 1. Anything else decodes as nothing, so the
Mac's page and its row in the Macs list go blank with no reason. Every Mac sends version 1
today, so this hasn't happened yet. It will the first time the format changes, in both
directions: an old Mac with a new phone, or a new Mac with a phone that hasn't updated.
Separately, the phone never learns that its Mac is behind on releases.

## Decisions

- **Format stays version 1.** Both new fields are optional, so older phones and the Mac's own
  widget ignore them (Swift `Decodable` skips unknown keys; Android sets `ignoreUnknownKeys`).
- **`app_version`** (string, always sent): the Mac app's version, `CARGO_PKG_VERSION`.
- **`update`** (string, only when there is one): the newer release this Mac's own update check
  found this run (`Hub::available`). This covers the issue's optional half without the phone
  looking up releases itself. The Mac already checks; the phone only repeats what it found.
  The hub tells the phone link through a new message, and the link stamps the value into
  every payload it sends. A change of `update` counts as a change, so it goes out within the
  usual 10 s.
- **The check sits in `AgentPulsePhone`** (link package, tested in CI by `swift test`):
  `PayloadFormat.supported = 1...1` and `PayloadFormat.check(Data) -> .current | .macTooOld |
  .phoneTooOld | .unreadable`, reading only `version`.
- **What the phone shows.**
  - On the Mac's page, in place of the empty page:
    - Mac too old: "Update AgentPulse on <Mac> to see its sessions."
    - Phone too old: "Update this app to see <Mac>'s sessions."
  - In the Macs list row, a short "Needs an update" in the same two cases.
  - When `update` is present, the Mac's page shows one line above the sessions:
    "AgentPulse <version> is available for <Mac>."
  - An unreadable payload keeps today's behaviour.
- **Android is not changed here.** The issue is iOS-only, and Android ignores the new fields.
  The same notice on Android is a follow-up.

## Acceptance

- `swift test --package-path link`: version 0 → Mac too old; 1 → current; 2 → phone too old;
  no version or not JSON → unreadable.
- Rust:
  - the widget payload carries `app_version` and leaves out `update`;
  - after the hub reports an update, the phone link's next sent payload carries it;
  - a payload published after that keeps it;
  - `update` back to none removes it.
- iOS screenshot fixtures `mac-old-format`, `mac-new-format` and `mac-update` render the
  notices. Checked on the simulator by hand.
- `cargo test --workspace`, clippy `-D warnings`, `cargo fmt --check`, and the iOS app builds.
