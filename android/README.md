# Android app

The AgentPulse Android app (Kotlin, Jetpack Compose, minimum SDK 26), the phone side of
[docs/mobile.md](../docs/mobile.md) on Android phones and tablets.

| Path | What |
|---|---|
| `link/` | Kotlin/JVM port of `link/`: the relay crypto, the pairing link, the relay client, the store and the direct connection (handshake, frames, a socket client). Plain JVM, so its tests need no emulator. |
| `app/` | The app: Macs, a Mac, a session (Material 3 adaptive panes), Pair (scan the code or paste the link), secrets under the Android Keystore, Macs on the network found with NSD, the home-screen widget (Glance) and its 15-minute refresh (WorkManager). `src/debug` has the fixtures and e2e hooks; `src/release` a no-op. |
| `e2e/` | UI tests of the phone and tablet layouts on the fixtures ([below](#ui-tests-phone-and-tablet-layouts)). |

## Build and run

Needs JDK 17 and the Android SDK (`ANDROID_HOME`) with platform 37.

```sh
cd android
./gradlew :link:test                  # crypto, link, store, relay client, direct connection
./gradlew :app:assembleDebug          # app/build/outputs/apk/debug/app-debug.apk
adb install -r app/build/outputs/apk/debug/app-debug.apk
adb shell am start -n io.github.chengboonrong.agentpulse/.MainActivity
```

Debug builds allow a relay at `http://localhost` for testing: `adb reverse tcp:8787 tcp:8787`
gives an emulator the Mac's local relay at the same address. Anything else, and everything
in release builds, must be `https`.

## Layout

Material 3's list-detail scaffold, by window width: below 600 dp one pane at a time (the
Macs, a Mac, a session, with back); from 600 dp two (the Macs and the selected Mac, or
the Mac and its selected session); from 1200 dp all three. The selected Mac is the first
one until you choose.

## Fixtures (debug builds only)

`adb shell am start -S -n io.github.chengboonrong.agentpulse/.MainActivity -e fixture <name>`
shows a screen from fixture data without network: `macs`, `mac`, `session`,
`pair-confirm`, `pair-code`, `empty`, the same set as the iPhone app. `adb exec-out
screencap -p > <png>` to capture.

## UI tests (phone and tablet layouts)

`e2e/` is an [e2e](https://github.com/e2e-inc/e2e) project, as `ios/e2e`, on the
fixtures with locators only (no model, no network). It needs Node 22.22.3+ or 24.8+, the
debug APK above, which each test installs, and booted emulators named `Pixel_9a` and
`AgentPulse_Tablet`:

```sh
avdmanager create avd -n AgentPulse_Tablet -d pixel_tablet \
    -k "system-images;android-35;google_apis_playstore_tablet;arm64-v8a"
emulator -avd AgentPulse_Tablet -no-window &
cd android/e2e && npm ci
npx e2e doctor
npm run test:tablet    # portrait: two panes; landscape: three
npm run test:phone     # one pane at a time; Pair: the camera, pasting, a rotation
```

Scanning itself is checked on the JVM, `./gradlew :app:testDebugUnitTest`: a pairing link
drawn as a QR code into a camera-like frame, read back by the decoder the camera feeds.

On the tablet, whose natural orientation is landscape, agent-device's `portrait` is
landscape; `tests/tablet.e2e.ts` names them by what the screen shows. The CLI sends
anonymous usage data; `E2E_TELEMETRY_DISABLED=1` turns it off.

## Google Play

`fastlane/` builds the signed release bundle and talks to Play Console; run it from
`android/` with `bundle install` once, then `bundle exec fastlane <lane>`.

| Lane | What |
|---|---|
| `build` | Signed release AAB, version code = the highest on any Play track + 1. `build first:true`: version code 1, for the very first upload, which Play only takes by hand. |
| `play` | `build`, then upload to `internal` (`track:alpha` for the closed test) as a draft release, rolled out in Play Console. `PLAY_RELEASE_STATUS=completed` once the app has a production release. |
| `listing` | The store listing in `fastlane/metadata/android/en-GB`: text, icon, feature graphic, phone and tablet screenshots. |
| `promote` | `promote version_code:N from:alpha`: to production as a 10% staged rollout (`rollout:`, `to:` to change). |

Secrets live outside the repo and reach fastlane through the environment:

- `PLAY_JSON_KEY_PATH`: the Play service-account JSON key, with release access to this app.
- `ANDROID_KEYSTORE_PATH`, `ANDROID_KEYSTORE_PASSWORD`: the upload key. They default to
  `~/.android-keys/agentpulse-upload.jks` and the password in `agentpulse-upload.pass`
  beside it (alias `upload`). Play App Signing holds the app key; a lost upload key is
  reset through Play Console support.
- `ANDROID_HOME`: the Android SDK, as for any build.

`versionName` is the workspace version from `Cargo.toml`, as on the Mac and iPhone.
The screenshots come from the fixtures above on a 1080×2400 phone (cropped to 1080×2160,
Play's 2:1 limit) and a 2560×1600 tablet, with `adb shell cmd uimode night yes` for dark.
`fastlane/app_access.txt` is the text for Play Console's App access section, which needs
a paired Mac to see anything.

## End to end

```sh
scripts/e2e-android.sh          # android/link as the phone, no emulator
scripts/e2e-android-app.sh      # the app on a booted emulator (ANDROID_SERIAL to pick one)
```

Both run the Mac side of `scripts/e2e-phones.sh` (throwaway keychain, the e2e driver, a
local relay, the direct listener) and require the phone to connect directly.
`e2e-android.sh`'s phone finds the listener with macOS's `dns-sd`, as the JVM has no
DNS-SD. `e2e-android-app.sh` passes the link as the debug-only extra `e2eLink`, answers
the app's "Pair with <Mac>?" with `e2eAutoConfirm`, and, since an emulator's NSD can't see
the host's services, hands the app the driver's listener as `e2eMac` through `adb reverse`.
It screenshots the Mac view (`target/e2e-android-app.png`), then relaunches with
`e2eRemove`, which removes the Mac, and checks the Mac deleted the pairing at the relay.
Finding a Mac with NSD needs a real device on the Mac's network.
