# iPhone and iPad app

The AgentPulse iPhone and iPad app and its home-screen widgets (SwiftUI, iOS 17+), the phone side of
[docs/mobile.md](../docs/mobile.md). Local builds are for the simulator, with no team;
TestFlight builds come from `fastlane beta` (below).

| Path | What |
|---|---|
| `project.yml` | XcodeGen spec. The generated `AgentPulsePhone.xcodeproj` is not committed. |
| `App/` | The app: Macs list, a Mac's view, a session's view (iPad), Pair (VisionKit scanner, or a paste field where there is no camera), the `agentpulse://pair` link, DEBUG fixtures. |
| `e2e/` | UI tests of the iPad and iPhone layouts on the fixtures ([below](#ui-tests-ipad-and-iphone-layouts)). |
| `Widget/` | Small and medium widgets for the first paired Mac: `GET down` every 15 minutes through the relay, else the last stored state, always "as of". |

Both use `macos/Shared/` (the widget payload and views) and `link/`'s `AgentPulsePhone`
(pairing, the relay, the direct connection, the store in the App Group
`group.io.github.chengboonrong.AgentPulse` with the secrets in the Keychain under that group).

## Build and run in the simulator

```sh
xcodegen generate --spec ios/project.yml --project ios
UDID=$(xcrun simctl create "AgentPulse Test" com.apple.CoreSimulator.SimDeviceType.iPhone-18-Pro com.apple.CoreSimulator.SimRuntime.iOS-27-0)
xcrun simctl boot $UDID
xcodebuild -project ios/AgentPulsePhone.xcodeproj -scheme AgentPulse \
    -destination "platform=iOS Simulator,id=$UDID" -derivedDataPath target/ios build
xcrun simctl install $UDID target/ios/Build/Products/Debug-iphonesimulator/AgentPulse.app
xcrun simctl launch $UDID io.github.chengboonrong.AgentPulse.phone
```

For an iPad, create the simulator from an iPad device type instead (`xcrun simctl list
devicetypes | grep iPad`), for example
`com.apple.CoreSimulator.SimDeviceType.iPad-Pro-11-inch-M5-16GB`, or pick an existing one by
name: `-destination "platform=iOS Simulator,name=iPad Pro 11-inch (M5)"`. One build runs on
both.

The simulator has no camera, so Pair shows a paste field. A relay at `http://localhost` is
allowed for testing; anything else must be `https`.

## iPad layout

At compact width (iPhone, and an iPad app in Slide Over or a narrow Split View) the app is
the iPhone flow: Macs, then a Mac pushed on top. At regular width (iPad full screen, wide
Split View, Stage Manager) it is a three-column `NavigationSplitView`: the Macs, the
selected Mac (the first one until you choose), and the session selected in that Mac's
list. In portrait the Macs column opens from the sidebar button. The selected Mac stays
selected across size changes. iPhone runs in portrait and landscape; iPad in all four
orientations, which multitasking requires. The widgets are the same small and medium
views on both.

## Fixtures (Debug builds only)

`xcrun simctl launch $UDID io.github.chengboonrong.AgentPulse.phone -AgentPulseFixture <name>`
shows a screen from fixture data without network: `macs`, `mac`, `session` (a selected
session, shown in its column on iPad), `pair-confirm`, `pair-code`, `empty`. `xcrun simctl ui $UDID appearance dark` for dark mode,
`xcrun simctl io $UDID screenshot <png>` to capture.

## UI tests (iPad and iPhone layouts)

`e2e/` is an [e2e](https://github.com/e2e-inc/e2e) project that checks the layouts on
the fixtures, with locators only (no model, no network). It needs Node 22.22.3+ or 24.8+,
and the Debug build above in `target/ios`, which each test installs. The targets name the
`iPad Pro 11-inch (M5)` and `iPhone 18 Pro` simulators; change `device` in
`e2e/e2e.config.ts` to one from `xcrun simctl list devices`.

```sh
cd ios/e2e && npm ci
npx e2e doctor
npm run test:ipad      # portrait: Mac + session column; landscape: all three columns
npm run test:iphone    # compact: the Macs list pushes a Mac
npm run test:ipad -- --video   # MP4 per test in .e2e/results/<test>/attempt-1/video/
```

The CLI sends anonymous usage data; `E2E_TELEMETRY_DISABLED=1` turns it off.

## End to end

```sh
AGENTPULSE_E2E_PORT=8797 scripts/e2e-ios.sh
```

Runs the Mac side of `scripts/e2e-phones.sh` (throwaway keychain, the e2e driver, a local
relay), builds and installs the Debug app in a new simulator, pairs through the relay,
waits until the Mac sees the app's `seen` and its direct connection, screenshots the Mac
view (`target/e2e-ios.png`), then relaunches the app with the DEBUG-only
`-AgentPulseE2ERemove YES` to remove the Mac and checks that the Mac deleted the pairing at
the relay. The simulator is deleted at the end. The link goes in as the DEBUG-only
launch argument `-AgentPulseE2ELink`, because `simctl openurl` makes the simulator ask
"Open in AgentPulse?", and `-AgentPulseE2EAutoConfirm YES` skips the app's own question;
all three are compiled out of Release builds.

## TestFlight

```sh
export APP_STORE_CONNECT_API_KEY_KEY_ID=<key id>
export APP_STORE_CONNECT_API_KEY_ISSUER_ID=<issuer id>
export APP_STORE_CONNECT_API_KEY_KEY_FILEPATH=~/.appstoreconnect/private_keys/AuthKey_<key id>.p8
cd ios && fastlane beta
```

`fastlane/Fastfile` archives the Release app with automatic signing for the team in
`macos/signing.local` (or `AGENTPULSE_TEAM_ID`), using the workspace version from
`Cargo.toml` and the latest TestFlight build number for that version + 1, writes the `.ipa` to `target/ios/`
and uploads it. The API key needs the App Manager or Admin role. One-time setup in the
developer portal and App Store Connect: the App Group above registered on the team and
ticked on both App IDs (`io.github.chengboonrong.AgentPulse.phone` and `.phone.Widgets`),
and an App Store Connect app record for the first bundle id. The app declares
`ITSAppUsesNonExemptEncryption = NO`: it only uses the system's CryptoKit.
