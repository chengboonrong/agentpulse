# macOS widgets

Small and medium desktop / Notification Center widgets for AgentPulse (WidgetKit,
macOS 14+), and the pieces that put them in a signed `AgentPulse.app`.

| Path | What |
|---|---|
| `project.yml` | XcodeGen spec: the widget extension, the three bridge dylibs, the preview tool. The generated `AgentPulseWidgets.xcodeproj` is not committed. |
| `Shared/` | The `widget.json` model (`Payload.swift`) and the SwiftUI views (`WidgetViews.swift`). Used by both the extension and the preview tool. |
| `Widgets/` | The extension: timeline provider, widget, `Info.plist`. |
| `Bridge/` | `libAgentPulseWidgetBridge.dylib`: one C function, `agentpulse_reload_widgets`, that calls `WidgetCenter.shared.reloadAllTimelines()`. The Rust app `dlopen`s it. |
| `LinkBridge/` | `libAgentPulseLinkBridge.dylib`: the phone link's Mac side (Keychain, sealing through `link/`, the QR image, the direct listener) behind `aplink_call`, `aplink_free` and `aplink_set_event_handler`. The Rust app `dlopen`s it (`crates/agentpulse-app/src/phones/`). |
| `UpdateBridge/` | `libAgentPulseUpdateBridge.dylib`: one C function, `agentpulse_install_update`, that hands the update to Sparkle 2.10.1 (Swift package, MIT). The Rust app `dlopen`s it when the user clicks Install Update (`crates/agentpulse-app/src/updates.rs`). |
| `AppIcon.icon` | Icon Composer document for the Mac and iPhone app icon; `scripts/bundle.sh` compiles it with `actool`. |
| `Preview/` | `widget-preview <widget.json> <out-dir>`: renders the same views to PNG (data, no update since, not running, gallery placeholder; small and medium; light, dark, and a flattened "accented" approximation) for review without a desktop. |

The gallery placeholder and the cat in "isn't running" come from `placeholder.json`,
which `scripts/bundle.sh` generates with `agentpulse --widget-placeholder` (the
exporter's payload for no agents) and ships in the extension's resources and beside
the preview tool, so the Swift side carries no copy of the sprite. Without it the
widget shows "AgentPulse isn't running" as text only.

## Build

Set your Apple Developer team id once (the `OU` of your "Apple Development"
certificate; `security find-identity -v -p codesigning` lists them):

```sh
echo 'AGENTPULSE_TEAM_ID=XXXXXXXXXX' > macos/signing.local   # untracked
# or: export AGENTPULSE_TEAM_ID=XXXXXXXXXX
scripts/bundle.sh
```

`scripts/bundle.sh --dmg` also writes `AgentPulse-<version>.dmg`, signed with the same
development certificate and not notarized. `scripts/bundle.sh --release` signs with the
team's Developer ID Application certificate and a secure timestamp, then notarizes and
staples the DMG so it opens on any Mac. It needs that certificate (Xcode > Settings >
Accounts > Manage Certificates; a paid membership) and notarytool credentials. Either
use an App Store Connect API key (Developer role or higher), set in the environment or
`macos/signing.local`:

```sh
AGENTPULSE_NOTARY_KEY_ID=XXXXXXXXXX
AGENTPULSE_NOTARY_ISSUER=xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx
```

with the key file at `~/.appstoreconnect/private_keys/AuthKey_<key id>.p8` (or
`AGENTPULSE_NOTARY_KEY` in the environment). Without those two settings it uses a notarytool
keychain profile instead:

```sh
xcrun notarytool store-credentials agentpulse-notary --apple-id <your Apple ID> --team-id XXXXXXXXXX
```

(another name: `AGENTPULSE_NOTARY_PROFILE` in the environment or `macos/signing.local`).
Every build uses the hardened runtime.

The script builds the release binary and the Swift targets, assembles
`target/bundle/AgentPulse.app` (under `$CARGO_TARGET_DIR` when set; the script asks
`cargo metadata` for the target directory), signs it with the certificate whose OU is that team
(selected by hash), and verifies the signature. If `codesign` waits on a keychain
dialog it gives up after two minutes: run the script once in a visible terminal and
click "Always Allow".

To add the widget, open the app once (`open target/bundle/AgentPulse.app`) so macOS
registers the extension, then right-click the desktop > Edit Widgets > AgentPulse.

Previews: `target/xcode/build/Release/widget-preview <widget.json> docs/screenshots/widgets`.

Phone pairing secrets (each pairing's private key, relay token and one-time secret, and
the relay's `RELAY_KEY`) are generic passwords in the login keychain, service
`io.github.chengboonrong.AgentPulse.pairing`, with the default access control: the app
that created an item may read it. A rebuilt development-signed app can count as another
app, so macOS may ask to allow access to them; "Always Allow" keeps working until the
next rebuild.

## DMG window

The DMG opens on a 660×420 window: the app on a peach stage at (170, 190), Applications
on a periwinkle stage at (490, 190), 128 px icons, a paw-print trail and "Drag AgentPulse
to Applications". It is only the first-run installer; updates come from the zip through
Sparkle, so nothing here can break them. All of it lives in `macos/dmg/`:

| File | What | Made by |
|---|---|---|
| `background.png`, `background@2x.png` | The art, 1x and 2x. Text is drawn with the system rounded face, so no font ships; no version number, so it survives releases | `swift macos/dmg/render-background.swift` |
| `DS_Store` | Window size, icon size and positions, and the background picture. Copied to the volume root as `.DS_Store` on every build | `macos/dmg/make-ds-store.sh` (drives Finder; macOS asks once for Automation permission) |
| `render-background.swift`, `make-ds-store.sh` | The two generators | |

`bundle.sh` joins the two PNGs into `.background/background.tiff` with `tiffutil
-cathidpicheck`, so Finder picks the right scale per display, stages it with `DS_Store`, and
checks both reached the image before it signs. The build itself never opens Finder.

Regenerate after changing the art, the window size, the icon positions or an item name
(`AgentPulse.app`, `Applications`): edit both generators so their numbers agree, run the
Swift script if the art changed, then `make-ds-store.sh`, and commit the results. Finder
rewrites `.DS_Store` whenever a window is opened, so never edit the staging folder or a
mounted image by hand. Finder finds the background through an alias that records the volume's
name, so the volume is always called `AgentPulse`, with no version; the file name
`AgentPulse-<version>.dmg` carries the version. Never set a
custom icon on the `Applications` symlink: it would land on `/Applications` itself.

## Updates

AgentPulse finds a new release with its own GitHub check (see the README). Install Update
in the window, or the menu's Update Available item, starts Sparkle, embedded as `Contents/Frameworks/Sparkle.framework`
without its XPC services (the app is not sandboxed). Sparkle fetches
`https://github.com/chengboonrong/agentpulse/releases/latest/download/appcast.xml`, shows
its own alert, and on the user's click downloads the zip, checks the feed's and the zip's
EdDSA signatures against the installed copy's `SUPublicEDKey` and the new app's code
signature, replaces the app and relaunches it. `Info.plist` turns off Sparkle's scheduled
checks, automatic installs and system profiling. `releases/latest` skips pre-releases, so a
release must be published as a full release to reach Sparkle; for a pre-release AgentPulse
opens its GitHub page instead.

The update signing key is an EdDSA pair. The private half lives only in the login keychain
of the Mac that cuts releases; the public half is `UPDATE_PUBLIC_KEY` in
`scripts/bundle.sh`. Make it once, with the tools the build downloads (under
`$CARGO_TARGET_DIR` instead of `target/` when that is set):

```sh
target/xcode/packages/artifacts/sparkle/Sparkle/bin/generate_keys      # prints the public key
target/xcode/packages/artifacts/sparkle/Sparkle/bin/generate_keys -x /Volumes/<offline drive>/agentpulse-update.key
```

Paste the printed public key into `UPDATE_PUBLIC_KEY` and keep the exported private key on
offline media only (`generate_keys -f <file>` puts it back into a keychain). Never lose or
change the key: installed copies check every feed and zip against the public key they
shipped with, so a release signed with another key is refused and its users must update
by hand.

Release checklist:

1. Bump `version` in `[workspace.package]` in `Cargo.toml`.
2. `scripts/bundle.sh --release`. It stops before notarizing unless the login keychain
   holds the key matching `UPDATE_PUBLIC_KEY`, then notarizes and staples the DMG, staples the app,
   zips it to `AgentPulse-<version>.zip`, signs the zip with `sign_update`, writes a
   one-item `appcast.xml` pointing at the zip on the `v<version>` release, and signs
   the feed.
3. Publish `v<version>` as a full release (not a pre-release) with the DMG, the zip and
   `appcast.xml` from `target/bundle/`; its notes are what the notice shows:
   `gh release create v<version> --title "AgentPulse v<version>" --notes-file <notes.md> target/bundle/AgentPulse-<version>.{dmg,zip} target/bundle/appcast.xml`.
4. Smoke-test from the previous version: open it, Check Now, Install Update, and confirm it
   relaunches as the new version with the menu-bar cat running.

## How data reaches the widget

The app (only when run from inside the bundle) writes
`~/Library/Application Support/AgentPulse/Widget/widget.json`; the sandboxed
extension reads it through a read-only `temporary-exception` entitlement for exactly
that directory. That path is fixed and ignores `AGENTPULSE_DATA_DIR`.

Why not an app group container: on macOS 27 an unsandboxed app signed without a
provisioning profile is denied writing to the group container, with or without the
`application-groups` entitlement and with no prompt (TCC
`kTCCServiceSystemPolicyAppDataDetailed`, "does not allow prompting; recording
denied"). With a provisioning profile that authorises an app group for both the app
and the extension, a group container would be the cleaner route. It is the only one the
Mac App Store accepts, but AgentPulse does not target the App Store: the store requires
the app sandbox, and a sandboxed app cannot see other processes' CPU or memory.

## Refresh, and why the widget can lag the app

The widget is a snapshot, not a live view; it always says "as of HH:mm".

- The app rewrites `widget.json` at most every 10 s, and only when something changed
  (or every 5 minutes, so a quiet but running app never looks stopped).
- It asks WidgetKit to reload once at launch, then at most every 15 minutes and only
  when a glance would notice: agent counts, the listed sessions or their status, or
  the cat; today's cost alone needs 30 minutes since the last request. Quitting asks
  once more.
- The widget's own timeline refreshes every 60 minutes.

Worst case for an 8-hour day: 32 app requests (launch, then one per 15 minutes),
1 at quit, and 8 timeline refreshes, about 41 reloads, inside WidgetKit's daily
budget (roughly 40 to 70 for a background app). WidgetKit may still defer or drop
requests, so the widget can lag the window by up to 15 minutes or more. Session
timers count up live without reloads.

What the widget shows when the data is not live:

- "AgentPulse isn't running": the file is marked stopped (the app quit, or its
  collector thread panicked), missing, unreadable, or of an unknown version.
- "No update since HH:mm" (medium adds "Open AgentPulse to refresh"): the file is
  not marked stopped but is more than 75 minutes old. The app may have crashed, or be
  running while WidgetKit holds back reloads, so the widget claims neither. The
  timeline schedules this entry, so it appears without a reload.

## Privacy contract of `widget.json`

Mode 0600 in a 0700 directory, written atomically. It contains only: counts of
active / waiting / idle agents; today's tokens and estimated cost as display strings;
the output rate; the cat's sprite rows and palette; and for at most 4 live sessions
the project folder's last path component, status, model name, start time, tokens and
estimated cost. No full paths, session ids, prompts, responses or file contents.

## Known limits

- The stopped mark is written from an `atexit` hook, which also calls
  `WidgetCenter.reloadAllTimelines()` during process exit. No problem has been seen,
  but it is not proven safe in every exit path.
- With no widget placed, chronod logs "FOREIGN KEY constraint failed" around reload
  requests. The cause is unexplained.
