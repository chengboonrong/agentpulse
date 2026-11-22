# iPhone and iPad: Settings page

Date: 2026-11-22. Issue #64. The owner chose "#66 then #64" and said to make the calls.

## What goes on the page

- **Macs.** One row per paired Mac, opening a small detail screen:
  - **Name.** Rename the Mac on this phone. The new name shows in the Macs list, on the Mac's
    page and in its notices. It is stored in the pairing record (`Pairing.macName`) and never
    sent to the Mac or the relay. A blank or whitespace-only name is ignored, and a name is
    trimmed before saving.
  - **Remove.** The same confirmed `AppModel.remove` the Mac's page already uses. The Mac's
    page keeps its own Remove button.
- **Connection.** "Connect directly on the same network" switch, default on.
  - When off, the app doesn't browse with Bonjour, closes any direct connections, and uses
    only the relay. With no browsing there is also no Local Network prompt.
  - It is stored in the app's `UserDefaults` (key `directConnection`). Only the app connects;
    widgets read stored state and don't need it.
  - Turning it on starts browsing at once if the app is in the foreground.
- **About.**
  - The app's version (`CFBundleShortVersionString`).
  - A link to the privacy policy (`https://agentpulse.snitco.com/privacy.html`).
  - The relay in use: the host of each distinct relay across paired Macs, or "None" with no
    Macs.

## Left out

- **Live Activity on/off.** The app has no Live Activity yet. It is build step 4 in
  `docs/mobile.md` and waits on the paid program and a deployed relay. The switch comes with
  it.

## Where it opens

- A gear button (`gearshape`, labelled "Settings") in the Macs list toolbar. That list is the
  root on iPhone and the sidebar column on iPad, so one button covers both.
- It is shown even with no Macs, since About and the connection switch still apply.
- The page is a sheet with its own `NavigationStack` and a Done button.

## Acceptance

- `swift test --package-path link`: renaming a pairing saves through the store, survives
  reloading the store from disk, ignores blank names and trims whitespace. Renaming a missing
  id is a no-op.
- No new field is sent: rename touches only `pairings.json` on the phone. The direct switch
  only changes what the app does locally.
- A `settings` screenshot fixture opens the page with two Macs, checked on the iPhone and
  iPad simulators by hand.
- The iOS app builds, and the existing Rust, Swift and Android checks still pass.
