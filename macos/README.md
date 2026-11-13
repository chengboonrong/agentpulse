# macOS widgets

Small and medium desktop / Notification Center widgets for AgentPulse (WidgetKit,
macOS 14+), and the pieces that put them in a signed `AgentPulse.app`.

| Path | What |
|---|---|
| `project.yml` | XcodeGen spec: the widget extension, the bridge dylib, the preview tool. The generated `AgentPulseWidgets.xcodeproj` is not committed. |
| `Shared/` | The `widget.json` model (`Payload.swift`) and the SwiftUI views (`WidgetViews.swift`). Used by both the extension and the preview tool. |
| `Widgets/` | The extension: timeline provider, widget, `Info.plist`. |
| `Bridge/` | `libAgentPulseWidgetBridge.dylib`: one C function, `agentpulse_reload_widgets`, that calls `WidgetCenter.shared.reloadAllTimelines()`. The Rust app `dlopen`s it. |
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

The script builds the release binary and the Swift targets, assembles
`target/bundle/AgentPulse.app` (under `$CARGO_TARGET_DIR` when set; the script asks
`cargo metadata` for the target directory), signs it with the certificate whose OU is that team
(selected by hash), and verifies the signature. If `codesign` waits on a keychain
dialog it gives up after two minutes: run the script once in a visible terminal and
click "Always Allow".

To add the widget, open the app once (`open target/bundle/AgentPulse.app`) so macOS
registers the extension, then right-click the desktop > Edit Widgets > AgentPulse.

Previews: `target/xcode/build/Release/widget-preview <widget.json> docs/screenshots/widgets`.

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
and the extension, a group container would be the cleaner route (and the only one the
Mac App Store accepts).

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
