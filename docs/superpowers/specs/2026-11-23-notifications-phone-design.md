# Notifications, phase 2: the phone

Date: 2026-11-23. Issue #65. Phase 1 (#79) built the relay and the Mac side; the protocol is
in `docs/mobile.md` and `2026-11-23-notifications-design.md`. This phase makes the iPhone and
iPad app receive and show the alerts, and adds the page that chooses them.

## Decisions

- **One set of choices, for all Macs.** It is stored in the app's `UserDefaults`
  (`notifyChoices`). The defaults are:
  - waiting on;
  - failed on;
  - finished off, because it fires after every turn and would be noisy by default.
- **What the Macs are told.** The phone sends `{"seq","type":"notify","waiting","finished","failed"}`
  through `up` to every paired Mac:
  - The choices sent are the effective ones: with iOS permission denied or not yet asked,
    all three are false, so the Macs stop spending pushes.
  - It sends them when the app comes to the foreground, when a choice changes, and when a
    Mac is paired, and after a direct connection opens. The direct-open sync targets
    only that pairing through the relay so its `notify` seq is above the direct `seen`.
    Pairings removed mid-sync are skipped.
  - Sending again on each foreground is what turns alerts back on after a 409, 410 or
    501 turned them off on a Mac.
- **The push token.**
  - When any effective choice is on, the app calls `registerForRemoteNotifications()` on
    each foreground. Registration sends nothing until the token callback, which iOS
    delivers on every register, avoiding duplicate foreground syncs. All-off choices
    sync immediately without registration. Direct-open sync uses the known token.
  - When the token arrives, for each pairing it first sends `PUT push-tokens`
    `{"alert": <hex>, "environment": …}`, then `notify`. The order matters: the PUT replaces
    all of that pairing's tokens, and a `notify` sent before the token exists would get a
    409 and turn alerts off.
  - `environment` is `development` in Debug builds and `production` in Release
    (TestFlight and the App Store). Export rewrites `aps-environment`; the
    `development` entitlement is intended. A Release build run from Xcode must not
    be used to test alerts because development signing supplies a sandbox token.
- **Permission.**
  - With status "not determined", a top section says "Allow notifications to get alerts
    from your Macs." and offers "Allow Notifications". It requests `[.alert, .sound]`,
    then refreshes status and syncs. Errors stay in `notificationError` on this page,
    not in each Mac's problems.
  - Switches show effective choices and are disabled unless status is authorized,
    provisional or ephemeral. The Settings summary is "Off" otherwise. Switches only
    store choices; changing one never requests permission.
  - When permission is denied, the page says so, disables the switches, and links to the
    app's notification settings (`UIApplication.openNotificationSettingsURLString`).
- **Notification Service Extension** (`AgentPulseNotifications`, a new app extension):
  - It uses the same App Group and Keychain access group as the widgets.
  - It tries `m` against each paired Mac's key, with direction `alert`; the alert carries no
    pairing id.
  - A newer `seq` is recorded in that pairing's `alertSeq`, and the alert text is replaced.
  - When the seq isn't newer, nothing opens, or anything throws, it shows the fixed text the
    relay sent.
  - An `NSLock` guards the handler and original content; delivery takes and clears
    the handler under the lock, then calls it outside the lock, including on expiry.
  - The extension's 30 s limit is never close: the work is one Keychain read and one open
    per Mac.
- **Alert text** (Mac name as subtitle when more than one Mac is paired):
  - one event:
    - waiting: title "<agent> is waiting", body "<project> needs your answer."
    - finished: title "<agent> finished", body "<project> is ready for your next prompt."
    - failed: title "<agent> failed", body "<project> stopped with an error."
  - several events: the title is "<n> agents need you" when any is waiting, otherwise
    "<n> agents updated", where `n = events.count + (more ?? 0)`. The body has one line per event ("<project>: waiting" /
    "finished" / "failed"), then "and <more> more" when `more` is set.
  - `threadIdentifier` is the pairing id, so alerts group by Mac.
- **Where the page lives.** It is a "Notifications" row in an untitled Settings section, above "Connection",
  showing a summary ("Waiting, Failed", or "Off").

## Out of scope

- Per-Mac choices.
- Android.
- The Live Activity (its `activity` and `start` tokens). It will need to send the `alert`
  token in the same PUT, which `docs/mobile.md` already says.

## Acceptance

- **`swift test --package-path link`:**
  - `notify` sends the right plaintext through `up`;
  - `PUT push-tokens` sends lowercase hex and the environment, and maps 401/404 to removed;
  - an alert sealed by a test Mac opens to the text above;
  - a replayed or older seq gives nil;
  - an alert for an unknown pairing gives nil;
  - two Macs are tried in turn;
  - `alertSeq` persists and old `pairings.json` files still load;
  - the text format covers one event, several events, `more`, and the Mac-name subtitle.
- **The iOS app and extension build for the simulator.** Fixtures `notifications`,
  `notifications-denied`, and `notifications-undetermined` show the page, which a screenshot checks.
- **`docs/mobile.md`** describes the phone's side.
- **The real-device test is phase 3:** an APNs key in the relay's secrets, the Push
  Notifications capability, and the extension's App ID with the App Group.
