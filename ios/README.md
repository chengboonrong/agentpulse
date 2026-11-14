# iPhone app

The AgentPulse iPhone app and its home-screen widgets (SwiftUI, iOS 17+), the phone side of
[docs/mobile.md](../docs/mobile.md). Simulator only for now: builds are signed to run
locally, with no team.

| Path | What |
|---|---|
| `project.yml` | XcodeGen spec. The generated `AgentPulsePhone.xcodeproj` is not committed. |
| `App/` | The app: Macs list, a Mac's view, Pair (VisionKit scanner, or a paste field where there is no camera), the `agentpulse://pair` link, DEBUG fixtures. |
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

The simulator has no camera, so Pair shows a paste field. A relay at `http://localhost` is
allowed for testing; anything else must be `https`.

## Fixtures (Debug builds only)

`xcrun simctl launch $UDID io.github.chengboonrong.AgentPulse.phone -AgentPulseFixture <name>`
shows a screen from fixture data without network: `macs`, `mac`, `pair-confirm`,
`pair-code`, `empty`. `xcrun simctl ui $UDID appearance dark` for dark mode,
`xcrun simctl io $UDID screenshot <png>` to capture.

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
